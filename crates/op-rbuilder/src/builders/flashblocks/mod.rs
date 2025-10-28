use tips_bundle_pool::InMemoryBundlePool;
use super::BuilderConfig;
use crate::traits::{NodeBounds, PoolBounds};
use config::FlashblocksConfig;
use service::FlashblocksServiceBuilder;

mod builder_tx;
mod config;
mod payload;
mod service;
mod wspub;
mod best_bundles;

/// Block building strategy that progressively builds chunks of a block and makes them available
/// through a websocket update, then merges them into a full block every chain block time.
pub struct FlashblocksBuilder;

impl super::PayloadBuilder for FlashblocksBuilder {
    type Config = FlashblocksConfig;

    type ServiceBuilder<Node, Pool>
        = FlashblocksServiceBuilder
    where
        Node: NodeBounds,
        Pool: PoolBounds;

    fn new_service<Node, Pool>(
        config: BuilderConfig<Self::Config>,
        bundle_store: InMemoryBundlePool,
    ) -> eyre::Result<Self::ServiceBuilder<Node, Pool>>
    where
        Node: NodeBounds,
        Pool: PoolBounds,
    {
        Ok(FlashblocksServiceBuilder{
            config,
            bundle_store,
        })
    }
}
