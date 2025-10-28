use eyre::{eyre, Result};
use reth_optimism_rpc::OpEthApiBuilder;

use crate::{
    args::*,
    builders::{BuilderConfig, BuilderMode, FlashblocksBuilder, PayloadBuilder, StandardBuilder},
    metrics::{VERSION, record_flag_gauge_metrics},
    monitor_tx_pool::monitor_tx_pool,
    primitives::reth::engine_api_builder::OpEngineApiBuilder,
    revert_protection::{EthApiExtServer, RevertProtectionExt},
    tx::FBPooledTransaction,
};
use core::fmt::Debug;
use std::marker::PhantomData;
use std::sync::Arc;
use moka::future::Cache;
use reth::builder::{NodeBuilder, WithLaunchContext};
use reth_cli_commands::launcher::Launcher;
use reth_db::mdbx::DatabaseEnv;
use reth_optimism_chainspec::OpChainSpec;
use reth_optimism_cli::chainspec::OpChainSpecParser;
use reth_optimism_node::{
    OpNode,
    node::{OpAddOns, OpAddOnsBuilder, OpEngineValidatorBuilder, OpPoolBuilder},
};
use reth_transaction_pool::TransactionPool;
use futures_util::TryStreamExt;
use rdkafka::ClientConfig;
use rdkafka::producer::FutureProducer;
use reth_exex::ExExEvent;
use tips_audit::{connect_audit_to_publisher, BundleEvent, KafkaBundleEventPublisher};
use tips_bundle_pool::{BundleStore, InMemoryBundlePool, KafkaBundleSource};
use tips_bundle_pool::source::BundleSource;
use tips_core::BundleWithMetadata;
use tokio::sync::mpsc;
use tracing::{error, warn};

pub fn launch() -> Result<()> {
    let cli = Cli::parsed();
    let mode = cli.builder_mode();

    #[cfg(feature = "telemetry")]
    let telemetry_args = match &cli.command {
        reth_optimism_cli::commands::Commands::Node(node_command) => {
            node_command.ext.telemetry.clone()
        }
        _ => Default::default(),
    };

    let mut cli_app = cli.configure();

    #[cfg(feature = "telemetry")]
    {
        use crate::primitives::telemetry::setup_telemetry_layer;
        let telemetry_layer = setup_telemetry_layer(&telemetry_args)?;
        cli_app.access_tracing_layers()?.add_layer(telemetry_layer);
    }

    cli_app.init_tracing()?;
    match mode {
        BuilderMode::Standard => {
            tracing::info!("Starting OP builder in standard mode");
            let launcher = BuilderLauncher::<StandardBuilder>::new();
            cli_app.run(launcher)?;
        }
        BuilderMode::Flashblocks => {
            tracing::info!("Starting OP builder in flashblocks mode");
            let launcher = BuilderLauncher::<FlashblocksBuilder>::new();
            cli_app.run(launcher)?;
        }
    }
    Ok(())
}

pub struct BuilderLauncher<B> {
    _builder: PhantomData<B>,
}

impl<B> BuilderLauncher<B>
where
    B: PayloadBuilder,
{
    pub fn new() -> Self {
        Self {
            _builder: PhantomData,
        }
    }
}

impl<B> Default for BuilderLauncher<B>
where
    B: PayloadBuilder,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<B> Launcher<OpChainSpecParser, OpRbuilderArgs> for BuilderLauncher<B>
where
    B: PayloadBuilder,
    BuilderConfig<B::Config>: TryFrom<OpRbuilderArgs>,
    <BuilderConfig<B::Config> as TryFrom<OpRbuilderArgs>>::Error: Debug,
{
    async fn entrypoint(
        self,
        builder: WithLaunchContext<NodeBuilder<Arc<DatabaseEnv>, OpChainSpec>>,
        builder_args: OpRbuilderArgs,
    ) -> Result<()> {
        let builder_config = BuilderConfig::<B::Config>::try_from(builder_args.clone())
            .expect("Failed to convert rollup args to builder config");

        record_flag_gauge_metrics(&builder_args);

        let da_config = builder_config.da_config.clone();
        let rollup_args = builder_args.rollup_args;
        let op_node = OpNode::new(rollup_args.clone());

        let reverted_cache = Cache::builder().max_capacity(100).build();
        let reverted_cache_copy = reverted_cache.clone();

        let bundle_support = true;
        let bundle_pool = self.setup_bundle_store()?;
        let mut bundle_pool_copy = bundle_pool.clone();

        let mut addons: OpAddOns<
            _,
            OpEthApiBuilder,
            OpEngineValidatorBuilder,
            OpEngineApiBuilder<OpEngineValidatorBuilder>,
        > = OpAddOnsBuilder::default()
            .with_sequencer(rollup_args.sequencer.clone())
            .with_enable_tx_conditional(rollup_args.enable_tx_conditional)
            .with_da_config(da_config)
            .build();
        if cfg!(feature = "custom-engine-api") {
            let engine_builder: OpEngineApiBuilder<OpEngineValidatorBuilder> =
                OpEngineApiBuilder::default();
            addons = addons.with_engine_api(engine_builder);
        }
        let handle = builder
            .with_types::<OpNode>()
            .with_components(
                op_node
                    .components()
                    .pool(
                        OpPoolBuilder::<FBPooledTransaction>::default()
                            .with_enable_tx_conditional(
                                // Revert protection uses the same internal pool logic as conditional transactions
                                // to garbage collect transactions out of the bundle range.
                                rollup_args.enable_tx_conditional
                                    || builder_args.enable_revert_protection,
                            )
                            .with_supervisor(
                                rollup_args.supervisor_http.clone(),
                                rollup_args.supervisor_safety_level,
                            ),
                    )
                    .payload(B::new_service(builder_config, bundle_pool)?),
            )
            .with_add_ons(addons)
            .install_exex_if(bundle_support, "bundle-pool-tracking", {
                move |mut ctx| async move {
                    Ok(async move {
                        while let Some(note) = ctx.notifications.try_next().await? {
                            if let Some(committed) = note.committed_chain() {
                                for b in committed.blocks_iter() {
                                    bundle_pool_copy.on_new_block(
                                        b.number,
                                        b.hash());
                                }
                                let _ = ctx.events.send(ExExEvent::FinishedHeight(
                                    committed.tip().num_hash(),
                                ));
                            }
                        }
                        Ok(())
                    })
                }
            })
            .extend_rpc_modules(move |ctx| {
                if builder_args.enable_revert_protection {
                    tracing::info!("Revert protection enabled");

                    let pool = ctx.pool().clone();
                    let provider = ctx.provider().clone();
                    let revert_protection_ext = RevertProtectionExt::new(
                        pool,
                        provider,
                        ctx.registry.eth_api().clone(),
                        reverted_cache,
                    );

                    ctx.modules
                        .add_or_replace_configured(revert_protection_ext.into_rpc())?;
                }
                Ok(())
            })
            .on_node_started(move |ctx| {
                VERSION.register_version_metrics();
                if builder_args.log_pool_transactions {
                    tracing::info!("Logging pool transactions");
                    let listener = ctx.pool.all_transactions_event_listener();
                    let task = monitor_tx_pool(listener, reverted_cache_copy);
                    ctx.task_executor.spawn_critical("txlogging", task);
                }
                Ok(())
            })
            .launch()
            .await?;

        handle.node_exit_future.await?;
        Ok(())
    }
}

impl<B> BuilderLauncher<B> {

    fn setup_bundle_store(&self) -> Result<InMemoryBundlePool> {
        let (bundle_tx, mut bundle_rx) = mpsc::unbounded_channel::<BundleWithMetadata>();
        let (audit_tx, audit_rx) = mpsc::unbounded_channel::<BundleEvent>();

        // TODO: Load from file
        let kafka_producer = ClientConfig::new()
            .set("bootstrap.servers", "localhost:9092")
            .set("message.timeout.ms", "5000")
            .create::<FutureProducer>()
            .expect("Failed to create Kafka FutureProducer");

        let audit_publisher = KafkaBundleEventPublisher::new(
            kafka_producer,
            "tips-audit".to_string(),
        );
        connect_audit_to_publisher(audit_rx, audit_publisher);

        // TODO: Load from file
        let mut bundle_source_config = ClientConfig::new();
        bundle_source_config
            .set("group.id", "op-rbuilder-1")
            .set("bootstrap.servers", "localhost:9092")
            .set("session.timeout.ms", "6000")
            .set("enable.auto.commit", "true")
            .set("auto.offset.reset", "earliest");

        let bundle_source = Arc::new(KafkaBundleSource::new(
            bundle_source_config,
            "tips-ingress".to_string(),
            bundle_tx
        ).map_err(|e| {
            eyre!(e.to_string())
        })?);

        tokio::spawn(async move {
            if let Err(e) = bundle_source.run().await {
                error!(error = %e, "Bundle source failed");
            }
        });

        let bundle_pool = InMemoryBundlePool::new(audit_tx, "op-rbuilder-1".to_string());
        let mut bundle_pool_copy = bundle_pool.clone();

        tokio::spawn(async move {
            while let Some(bundle) = bundle_rx.recv().await {
                warn!(message = "adding bundle", uuid=%bundle.uuid());
                bundle_pool_copy.add_bundle(bundle);
                warn!(message = "added bundle");
            }
        });

        Ok(bundle_pool)
    }
}