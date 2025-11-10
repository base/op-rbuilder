use std::fmt::Debug;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use alloy_primitives::TxHash;
use dashmap::try_result::TryResult;
use jsonrpsee::{
    core::{async_trait, RpcResult},
    proc_macros::rpc,
};
use tips_core::MeterBundleResponse;
use crate::metrics::OpRBuilderMetrics;

struct Data {
    enabled: AtomicBool,
    by_tx_hash: dashmap::DashMap<TxHash, MeterBundleResponse>,
}

#[derive(Clone)]
pub struct ResourceMetering {
    data: Arc<Data>,
    metrics: OpRBuilderMetrics,
}

impl Debug for ResourceMetering {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResourceMetering")
         .field("enabled", &self.data.enabled)
         .field("by_tx_hash", &self.data.by_tx_hash.len())
         .finish()
    }
}

impl ResourceMetering {
    pub(crate) fn insert(&self, tx: TxHash, metering_info: MeterBundleResponse) {
        self.data.by_tx_hash.insert(tx, metering_info);
    }

    pub (crate) fn clear(&self) {
        self.data.by_tx_hash.clear();
    }

    pub(crate) fn set_enabled(&self, enabled: bool) {
        self.data.enabled.store(enabled, Ordering::Relaxed);
    }

    pub(crate) fn get(&self, tx: &TxHash) -> Option<MeterBundleResponse> {
        if !self.data.enabled.load(Ordering::Relaxed) {
            return None;
        }

        match self.data.by_tx_hash.try_get(tx) {
            TryResult::Present(result) => {
                self.metrics.metering_known_transaction.increment(1);
                Some(result.clone())
            }
            TryResult::Absent => {
                self.metrics.metering_unknown_transaction.increment(1);
                None
            }
            TryResult::Locked => {
                self.metrics.metering_locked_transaction.increment(1);
                None
            }
        }
    }
}

impl Default for ResourceMetering {
    fn default() -> Self {
        Self::new(false)
    }
}

impl ResourceMetering {
    pub fn new(enabled: bool) -> Self {
        Self {
            data: Arc::new(Data{
                by_tx_hash: dashmap::DashMap::new(),
                enabled: AtomicBool::new(enabled),
            }),
            metrics: OpRBuilderMetrics::default(),
        }
    }
}

// Namespace overrides for ingesting resource metering
#[cfg_attr(not(test), rpc(server, namespace = "base"))]
#[cfg_attr(test, rpc(server, client, namespace = "base"))]
pub trait BaseApiExt {
    #[method(name = "setMeteringInformation")]
    async fn set_metering_information(&self, tx_hash: TxHash, meter: MeterBundleResponse) -> RpcResult<()>;

    #[method(name = "setMeteringEnabled")]
    async fn set_metering_enabled(&self, enabled: bool) -> RpcResult<()>;

    #[method(name = "clearMeteringInformation")]
    async fn clear_metering_information(&self) -> RpcResult<()>;

}

pub(crate) struct ResourceMeteringExt {
    metering_info: ResourceMetering,
}

impl ResourceMeteringExt {
    pub(crate) fn new(metering_info: ResourceMetering) -> Self {
        Self {
            metering_info,
        }
    }
}

#[async_trait]
impl BaseApiExtServer for ResourceMeteringExt {
    async fn set_metering_information(&self, tx_hash: TxHash, metering: MeterBundleResponse) -> RpcResult<()> {
        self.metering_info.insert(tx_hash, metering);
        Ok(())
    }

    async fn set_metering_enabled(&self, enabled: bool) -> RpcResult<()> {
        self.metering_info.set_enabled(enabled);
        Ok(())
    }

    async fn clear_metering_information(&self) -> RpcResult<()> {
        self.metering_info.clear();
        Ok(())
    }
}