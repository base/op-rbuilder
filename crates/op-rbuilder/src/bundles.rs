use alloy_consensus::transaction::Recovered;
use alloy_primitives::TxHash;
use concurrent_queue::ConcurrentQueue;
use jsonrpsee::{
    core::{RpcResult, async_trait},
    proc_macros::rpc,
};
use op_alloy_consensus::OpTxEnvelope;
use std::{fmt::Debug, sync::Arc};
use tips_core::Bundle;
use tips_core::types::ParsedBundle;
use tracing::{debug, warn};

// ============================================================================
// BackrunBundleStore - stores backrun transactions keyed by target tx hash
// ============================================================================

struct BackrunData {
    /// Map: target_tx_hash -> Vec<Vec<Recovered<OpTxEnvelope>>>
    /// Key is txs[0].hash(), value is list of backrun tx lists (txs[1..])
    by_target_tx: dashmap::DashMap<TxHash, Vec<Vec<Recovered<OpTxEnvelope>>>>,
    /// LRU queue for eviction (stores target tx hashes)
    lru: ConcurrentQueue<TxHash>,
}

#[derive(Clone)]
pub struct BackrunBundleStore {
    data: Arc<BackrunData>,
}

impl Debug for BackrunBundleStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BackrunBundleStore")
            .field("by_target_tx_count", &self.data.by_target_tx.len())
            .finish()
    }
}

impl BackrunBundleStore {
    pub fn new(buffer_size: usize) -> Self {
        Self {
            data: Arc::new(BackrunData {
                by_target_tx: dashmap::DashMap::new(),
                lru: ConcurrentQueue::bounded(buffer_size),
            }),
        }
    }

    /// Insert a backrun bundle. Extracts target tx (txs[0]) and stores backrun txs (txs[1..])
    pub fn insert(&self, bundle: ParsedBundle) -> Result<(), String> {
        if bundle.txs.is_empty() {
            return Err("Bundle has no transactions".to_string());
        }

        if bundle.txs.len() < 2 {
            return Err("Bundle must have at least 2 transactions (target + backrun)".to_string());
        }

        // Target tx is txs[0]
        let target_tx_hash = bundle.txs[0].tx_hash();

        // Backrun txs are txs[1..]
        let backrun_txs: Vec<Recovered<OpTxEnvelope>> = bundle.txs[1..].to_vec();

        // Handle LRU eviction
        if self.data.lru.is_full() {
            if let Ok(evicted_hash) = self.data.lru.pop() {
                self.data.by_target_tx.remove(&evicted_hash);
                warn!(
                    target: "backrun_bundles",
                    evicted_target = ?evicted_hash,
                    "Evicted old backrun bundle"
                );
            }
        }

        // Add target to LRU queue
        let _ = self.data.lru.push(target_tx_hash);

        // Store backrun txs
        self.data
            .by_target_tx
            .entry(target_tx_hash)
            .or_insert_with(Vec::new)
            .push(backrun_txs.clone());

        warn!(
            target: "backrun_bundles",
            target_tx = ?target_tx_hash,
            backrun_tx_count = backrun_txs.len(),
            "Stored backrun bundle"
        );

        Ok(())
    }

    /// Get all backrun bundles for a target transaction
    pub fn get(&self, target_tx_hash: &TxHash) -> Option<Vec<Vec<Recovered<OpTxEnvelope>>>> {
        self.data
            .by_target_tx
            .get(target_tx_hash)
            .map(|entry| entry.clone())
    }

    /// Remove backrun bundles for a target (after execution or expiry)
    pub fn remove(&self, target_tx_hash: &TxHash) {
        if let Some((_, bundles)) = self.data.by_target_tx.remove(target_tx_hash) {
            debug!(
                target: "backrun_bundles",
                target_tx = ?target_tx_hash,
                bundle_count = bundles.len(),
                "Removed backrun bundles"
            );
        }
    }

    /// Clear all backrun bundles
    pub fn clear(&self) {
        self.data.by_target_tx.clear();
        debug!(target: "backrun_bundles", "Cleared all backrun bundles");
    }

    /// Get count of target transactions with backrun bundles
    pub fn len(&self) -> usize {
        self.data.by_target_tx.len()
    }

    /// Check if store is empty
    pub fn is_empty(&self) -> bool {
        self.data.by_target_tx.is_empty()
    }
}

impl Default for BackrunBundleStore {
    fn default() -> Self {
        Self::new(10_000)
    }
}

// ============================================================================
// RPC API for receiving backrun bundles
// ============================================================================

#[cfg_attr(not(test), rpc(server, namespace = "base"))]
#[cfg_attr(test, rpc(server, client, namespace = "base"))]
pub trait BaseBundlesApiExt {
    #[method(name = "sendBackrunBundle")]
    async fn send_backrun_bundle(&self, bundle: Bundle) -> RpcResult<()>;
}

pub(crate) struct BundlesApiExt {
    bundle_store: BackrunBundleStore,
}

impl BundlesApiExt {
    pub(crate) fn new(bundle_store: BackrunBundleStore) -> Self {
        Self { bundle_store }
    }
}

#[async_trait]
impl BaseBundlesApiExtServer for BundlesApiExt {
    async fn send_backrun_bundle(&self, bundle: Bundle) -> RpcResult<()> {
        warn!(target: "backrun_bundles", "Received backrun bundle");
        // Parse and validate bundle (convert Bundle -> ParsedBundle)
        let parsed_bundle = ParsedBundle::try_from(bundle).map_err(|e| {
            warn!(target: "backrun_bundles", error = %e, "Failed to parse bundle");
            jsonrpsee::types::ErrorObject::owned(
                jsonrpsee::types::error::INVALID_PARAMS_CODE,
                format!("Failed to parse bundle: {}", e),
                None::<()>,
            )
        })?;

        warn!(target: "backrun_bundles", "Parsed bundle");

        // Store in BackrunBundleStore keyed by target_tx_hash (txs[0])
        self.bundle_store.insert(parsed_bundle).map_err(|e| {
            warn!(target: "backrun_bundles", error = %e, "Failed to store bundle");
            jsonrpsee::types::ErrorObject::owned(
                jsonrpsee::types::error::INTERNAL_ERROR_CODE,
                format!("Failed to store bundle: {}", e),
                None::<()>,
            )
        })?;

        warn!(target: "backrun_bundles", "Stored bundle");

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_backrun_bundle_store_basic() {
        let store = BackrunBundleStore::new(100);
        assert!(store.is_empty());
        assert_eq!(store.len(), 0);
    }

    #[test]
    fn test_backrun_bundle_store_clear() {
        let store = BackrunBundleStore::new(100);
        store.clear();
        assert!(store.is_empty());
    }
}
