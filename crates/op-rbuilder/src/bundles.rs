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
use tracing::{debug, info, warn};

struct BackrunData {
    /// Key is the hash of the target tx, value is list of backrun raw txs
    by_target_tx: dashmap::DashMap<TxHash, Vec<Vec<Recovered<OpTxEnvelope>>>>,
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

    pub fn insert(&self, bundle: ParsedBundle) -> Result<(), String> {
        if bundle.txs.len() < 2 {
            return Err("Bundle must have at least 2 transactions (target + backrun)".to_string());
        }

        // Target tx is txs[0], backrun txs are txs[1..]
        let target_tx_hash = bundle.txs[0].tx_hash();
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

        info!(
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

    /// Get count of target transactions with backrun bundles
    pub fn len(&self) -> usize {
        self.data.by_target_tx.len()
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
        // Parse and validate bundle (convert Bundle -> ParsedBundle)
        let parsed_bundle = ParsedBundle::try_from(bundle).map_err(|e| {
            warn!(target: "backrun_bundles", error = %e, "Failed to parse bundle");
            jsonrpsee::types::ErrorObject::owned(
                jsonrpsee::types::error::INVALID_PARAMS_CODE,
                format!("Failed to parse bundle: {}", e),
                None::<()>,
            )
        })?;

        // Store in BackrunBundleStore keyed by target_tx_hash (txs[0])
        self.bundle_store.insert(parsed_bundle).map_err(|e| {
            warn!(target: "backrun_bundles", error = %e, "Failed to store bundle");
            jsonrpsee::types::ErrorObject::owned(
                jsonrpsee::types::error::INTERNAL_ERROR_CODE,
                format!("Failed to store bundle: {}", e),
                None::<()>,
            )
        })?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::SignableTransaction;
    use alloy_primitives::Bytes;
    use alloy_primitives::{Address, TxHash, U256};
    use alloy_provider::network::TxSignerSync;
    use alloy_provider::network::eip2718::Encodable2718;
    use alloy_signer_local::PrivateKeySigner;
    use op_alloy_consensus::OpTxEnvelope;
    use op_alloy_rpc_types::OpTransactionRequest;

    fn create_transaction(from: PrivateKeySigner, nonce: u64, to: Address) -> OpTxEnvelope {
        let mut txn = OpTransactionRequest::default()
            .value(U256::from(10_000))
            .gas_limit(21_000)
            .max_fee_per_gas(200)
            .max_priority_fee_per_gas(100)
            .from(from.address())
            .to(to)
            .nonce(nonce)
            .build_typed_tx()
            .unwrap();

        let sig = from.sign_transaction_sync(&mut txn).unwrap();
        OpTxEnvelope::Eip1559(txn.eip1559().cloned().unwrap().into_signed(sig).clone())
    }

    fn create_test_parsed_bundle(txs: Vec<Bytes>) -> ParsedBundle {
        tips_core::Bundle {
            txs,
            block_number: 1,
            ..Default::default()
        }
        .try_into()
        .unwrap()
    }

    #[test]
    fn test_backrun_bundle_store() {
        let alice = PrivateKeySigner::random();
        let bob = PrivateKeySigner::random();

        // Create test transactions
        let target_tx = create_transaction(alice.clone(), 0, bob.address());
        let backrun_tx1 = create_transaction(alice.clone(), 1, bob.address());
        let backrun_tx2 = create_transaction(alice.clone(), 2, bob.address());

        let target_tx_hash = target_tx.tx_hash();

        let store = BackrunBundleStore::new(100);

        // Test insert fails with only 1 tx (need target + at least 1 backrun)
        let single_tx_bundle = create_test_parsed_bundle(vec![target_tx.encoded_2718().into()]);
        assert!(store.insert(single_tx_bundle).is_err());
        assert_eq!(store.len(), 0);

        // Test insert succeeds with 2+ txs
        let valid_bundle = create_test_parsed_bundle(vec![
            target_tx.encoded_2718().into(),
            backrun_tx1.encoded_2718().into(),
        ]);
        assert!(store.insert(valid_bundle).is_ok());
        assert_eq!(store.len(), 1);

        // Test get returns the backrun txs (not the target)
        let retrieved = store.get(&target_tx_hash).unwrap();
        assert_eq!(retrieved.len(), 1); // 1 bundle
        assert_eq!(retrieved[0].len(), 1); // 1 backrun tx in that bundle
        assert_eq!(retrieved[0][0].tx_hash(), backrun_tx1.tx_hash());

        // Test multiple backrun bundles for same target
        let second_bundle = create_test_parsed_bundle(vec![
            target_tx.encoded_2718().into(),
            backrun_tx2.encoded_2718().into(),
        ]);
        assert!(store.insert(second_bundle).is_ok());
        assert_eq!(store.len(), 1); // Still 1 target, but 2 backrun bundles

        let retrieved = store.get(&target_tx_hash).unwrap();
        assert_eq!(retrieved.len(), 2); // Now 2 bundles for same target

        // Test remove
        store.remove(&target_tx_hash);
        assert_eq!(store.len(), 0);
        assert!(store.get(&target_tx_hash).is_none());

        // Test remove on non-existent key doesn't panic
        store.remove(&TxHash::ZERO);
    }

    #[test]
    fn test_backrun_bundle_store_lru_eviction() {
        let alice = PrivateKeySigner::random();
        let bob = PrivateKeySigner::random();

        // Small buffer to test eviction
        let store = BackrunBundleStore::new(2);

        // Insert 3 bundles, first should be evicted
        for nonce in 0..3u64 {
            let target = create_transaction(alice.clone(), nonce * 2, bob.address());
            let backrun = create_transaction(alice.clone(), nonce * 2 + 1, bob.address());
            let bundle = create_test_parsed_bundle(vec![
                target.encoded_2718().into(),
                backrun.encoded_2718().into(),
            ]);
            let _ = store.insert(bundle);
        }

        // Only 2 should remain due to LRU eviction
        assert_eq!(store.len(), 2);
    }
}
