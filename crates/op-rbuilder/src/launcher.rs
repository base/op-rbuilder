use eyre::Result;
use rdkafka::ClientConfig;
use rdkafka::producer::FutureProducer;
use reth_optimism_rpc::OpEthApiBuilder;
use tips_audit::{
    BundleEvent, KafkaBundleEventPublisher, connect_audit_to_publisher,
};
use tips_core::kafka::load_kafka_config_from_file;
use tokio::sync::mpsc;

use crate::{
    args::*,
    builders::{BuilderConfig, BuilderMode, FlashblocksBuilder, PayloadBuilder, StandardBuilder},
    bundles::{BackrunBundleStore, BaseBundlesApiExtServer, BundlesApiExt},
    metrics::{VERSION, record_flag_gauge_metrics},
    monitor_tx_pool::monitor_tx_pool,
    primitives::reth::engine_api_builder::OpEngineApiBuilder,
    resource_metering::{BaseApiExtServer, ResourceMeteringExt},
    revert_protection::{EthApiExtServer, RevertProtectionExt},
    tx::FBPooledTransaction,
};
use core::fmt::Debug;
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
use std::{marker::PhantomData, sync::Arc};

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

    #[cfg(not(feature = "telemetry"))]
    let cli_app = cli.configure();

    #[cfg(feature = "telemetry")]
    let mut cli_app = cli.configure();
    #[cfg(feature = "telemetry")]
    {
        use crate::primitives::telemetry::setup_telemetry_layer;
        let telemetry_layer = setup_telemetry_layer(&telemetry_args)?;
        cli_app.access_tracing_layers()?.add_layer(telemetry_layer);
    }

    match mode {
        BuilderMode::Standard => {
            tracing::info!("Starting OP builder in standard mode");
            let launcher = BuilderLauncher::<StandardBuilder>::new();
            cli_app.run(launcher)?;
        }
        BuilderMode::Flashblocks => {
            tracing::info!("Starting OP builder in flashblocks mode");
            let launcher: BuilderLauncher<FlashblocksBuilder> =
                BuilderLauncher::<FlashblocksBuilder>::new();
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
        let (audit_tx, audit_rx) = mpsc::unbounded_channel::<BundleEvent>();

        if let Some(ref kafka_properties_file) = builder_args.audit_kafka_properties {
            let kafka_config = load_kafka_config_from_file(kafka_properties_file)
                .expect("Failed to load Kafka config from properties file");
            let audit_client_config = ClientConfig::from_iter(kafka_config);
            let audit_producer: FutureProducer = audit_client_config
                .create()
                .expect("Failed to create Kafka producer");
            let audit_publisher = KafkaBundleEventPublisher::new(
                audit_producer,
                builder_args.audit_kafka_topic.clone(),
            );
            connect_audit_to_publisher(audit_rx, audit_publisher);
            tracing::info!(
                topic = %builder_args.audit_kafka_topic,
                "Backrun bundle audit events enabled (Kafka)"
            );
        }

        let backrun_bundle_store =
            BackrunBundleStore::with_audit(builder_args.backrun_bundle_buffer_size, audit_tx);

        let mut builder_config = BuilderConfig::<B::Config>::try_from(builder_args.clone())
            .expect("Failed to convert rollup args to builder config");

        // Replace the default backrun bundle store with the one that has audit
        builder_config.backrun_bundle_store = backrun_bundle_store.clone();

        record_flag_gauge_metrics(&builder_args);

        let da_config = builder_config.da_config.clone();
        let gas_limit_config = builder_config.gas_limit_config.clone();
        let rollup_args = builder_args.rollup_args;
        let op_node = OpNode::new(rollup_args.clone());
        let reverted_cache = Cache::builder().max_capacity(100).build();
        let reverted_cache_copy = reverted_cache.clone();
        let resource_metering = builder_config.resource_metering.clone();

        let mut addons: OpAddOns<
            _,
            OpEthApiBuilder,
            OpEngineValidatorBuilder,
            OpEngineApiBuilder<OpEngineValidatorBuilder>,
        > = OpAddOnsBuilder::default()
            .with_sequencer(rollup_args.sequencer.clone())
            .with_enable_tx_conditional(rollup_args.enable_tx_conditional)
            .with_da_config(da_config)
            .with_gas_limit_config(gas_limit_config)
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
                    .payload(B::new_service(builder_config)?),
            )
            .with_add_ons(addons)
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

                let resource_metering_ext = ResourceMeteringExt::new(resource_metering);
                let bundles_ext = BundlesApiExt::new(backrun_bundle_store);

                ctx.modules
                    .add_or_replace_configured(resource_metering_ext.into_rpc())?;
                ctx.modules
                    .add_or_replace_configured(bundles_ext.into_rpc())?;

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
