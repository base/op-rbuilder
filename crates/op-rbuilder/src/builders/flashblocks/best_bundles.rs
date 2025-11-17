use crate::traits::BundleBounds;
use alloy_primitives::TxHash;
use alloy_primitives::map::HashMap;
use reth_transaction_pool::TransactionPool;
use rollup_boost::FlashblocksPayloadV1;
use std::collections::HashSet;
use tips_bundle_pool::{BundleStore, InMemoryBundlePool, pool::ProcessedBundle};
use tips_core::{AcceptedBundle, BundleTxs};

pub(super) struct BestFlashblocksBundles<Pool: TransactionPool> {
    pool: Pool,
    bundle_pool: InMemoryBundlePool,

    // State
    commited_transactions: HashSet<TxHash>,
    current_flashblock_number: u64,

    // Mut stuff
    bundles: Vec<AcceptedBundle>,
    backrun_bundles: HashMap<TxHash, AcceptedBundle>,
    curr_bundles_idx: usize,
    current_block_num: u64,
}

impl<Pool: TransactionPool> BestFlashblocksBundles<Pool> {
    pub(super) fn new(pool: Pool, bundle_pool: InMemoryBundlePool) -> Self {
        Self {
            pool,
            bundle_pool,
            current_flashblock_number: 0,
            current_block_num: 0,
            commited_transactions: Default::default(),
            bundles: vec![],
            backrun_bundles: Default::default(),
            curr_bundles_idx: 0,
        }
    }

    /// Replaces current iterator with new one. We use it on new flashblock building, to refresh
    /// priority boundaries
    pub(super) fn load_transactions(
        &mut self,
        current_block_num: u64,
        current_flashblock_number: u64,
    ) {
        self.current_flashblock_number = current_flashblock_number;
        self.current_block_num = current_block_num;

        self.bundles = self.bundle_pool.get_bundles();
        self.backrun_bundles = self.bundle_pool.get_backrun_bundles();
        self.curr_bundles_idx = 0;
    }

    /// Remove transaction from next iteration and it already in the state
    pub(super) fn on_new_flashblock(
        &mut self,
        block: u64,
        fb: &FlashblocksPayloadV1,
        bundles_processed: Vec<ProcessedBundle>,
    ) {
        self.bundle_pool
            .built_flashblock(block, fb.index, bundles_processed);
    }
}

impl<Pool: TransactionPool> BundleBounds for BestFlashblocksBundles<Pool> {
    fn next(&mut self, _ctx: ()) -> Option<&AcceptedBundle> {
        loop {
            let bundle: &AcceptedBundle = self.bundles.get(self.curr_bundles_idx)?;
            self.curr_bundles_idx += 1;

            for t in bundle.clone().transactions() {
                if self.commited_transactions.contains(t.hash()) {
                    continue;
                }
            }

            let block_num = bundle.block_number;
            let flashblock_number_min = bundle.flashblock_number_min;
            let flashblock_number_max = bundle.flashblock_number_max;

            if block_num != 0 && block_num != self.current_block_num {
                continue;
            }

            // Check min flashblock requirement
            if let Some(min) = flashblock_number_min
                && self.current_flashblock_number < min
            {
                continue;
            }

            // Check max flashblock requirement
            if let Some(max) = flashblock_number_max
                && self.current_flashblock_number > max
            {
                // self.inner.mark_invalid(tx.sender(), tx.nonce());
                continue;
            }

            return Some(bundle);
        }
    }

    fn get_backrun_bundle(&mut self, tx_hash: &TxHash) -> Option<&AcceptedBundle> {
        self.backrun_bundles.get(tx_hash)
    }

    // TODO
    /// Proxy to inner iterator
    fn mark_invalid(&mut self, _bundle: &AcceptedBundle) {
        // self.inner.mark_invalid(sender, nonce);
    }
}

/*
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        mock_tx::{MockFbTransaction, MockFbTransactionFactory},
    };
    use alloy_consensus::Transaction;
    use reth_payload_util::{BestPayloadTransactions, PayloadTransactions};
    use reth_transaction_pool::{CoinbaseTipOrdering, PoolTransaction, pool::PendingPool};
    use std::sync::Arc;

    #[test]
    fn test_simple_case() {
        let mut pool = PendingPool::new(CoinbaseTipOrdering::<MockFbTransaction>::default());
        let mut f = MockFbTransactionFactory::default();

        // Add 3 regular transaction
        let tx_1 = f.create_eip1559();
        let tx_2 = f.create_eip1559();
        let tx_3 = f.create_eip1559();
        pool.add_transaction(Arc::new(tx_1), 0);
        pool.add_transaction(Arc::new(tx_2), 0);
        pool.add_transaction(Arc::new(tx_3), 0);

        // Create iterator
        let mut iterator = BestFlashblocksBundles::new(BestPayloadTransactions::new(pool.best()), );
        // ### First flashblock
        iterator.refresh_iterator(BestPayloadTransactions::new(pool.best()), 0);
        // Accept first tx
        let tx1 = iterator.next(()).unwrap();
        // Invalidate second tx
        let tx2 = iterator.next(()).unwrap();
        iterator.mark_invalid(tx2.sender(), tx2.nonce());
        // Accept third tx
        let tx3 = iterator.next(()).unwrap();
        // Check that it's empty
        assert!(iterator.next(()).is_none(), "Iterator should be empty");
        // Mark transaction as commited
        iterator.mark_commited(vec![*tx1.hash(), *tx3.hash()]);

        // ### Second flashblock
        // It should not return txs 1 and 3, but should return 2
        iterator.refresh_iterator(BestPayloadTransactions::new(pool.best()), 1);
        let tx2 = iterator.next(()).unwrap();
        // Check that it's empty
        assert!(iterator.next(()).is_none(), "Iterator should be empty");
        // Mark transaction as commited
        iterator.mark_commited(vec![*tx2.hash()]);

        // ### Third flashblock
        iterator.refresh_iterator(BestPayloadTransactions::new(pool.best()), 2);
        // Check that it's empty
        assert!(iterator.next(()).is_none(), "Iterator should be empty");
    }

    /// Test bundle cases
    /// We won't mark transactions as commited to test that boundaries are respected
    #[test]
    fn test_bundle_case() {
        let mut pool = PendingPool::new(CoinbaseTipOrdering::<MockFbTransaction>::default());
        let mut f = MockFbTransactionFactory::default();

        // Add 4 fb transaction
        let tx_1 = f.create_legacy_fb(None, None);
        let tx_1_hash = *tx_1.hash();
        let tx_2 = f.create_legacy_fb(None, Some(1));
        let tx_2_hash = *tx_2.hash();
        let tx_3 = f.create_legacy_fb(Some(1), None);
        let tx_3_hash = *tx_3.hash();
        let tx_4 = f.create_legacy_fb(Some(2), Some(3));
        let tx_4_hash = *tx_4.hash();
        pool.add_transaction(Arc::new(tx_1), 0);
        pool.add_transaction(Arc::new(tx_2), 0);
        pool.add_transaction(Arc::new(tx_3), 0);
        pool.add_transaction(Arc::new(tx_4), 0);

        // Create iterator
        let mut iterator = BestFlashblocksBundles::new(BestPayloadTransactions::new(pool.best()), );
        // ### First flashblock
        // should contain txs 1 and 2
        iterator.refresh_iterator(BestPayloadTransactions::new(pool.best()), 0);
        let tx1 = iterator.next(()).unwrap();
        assert_eq!(tx1.hash(), &tx_1_hash);
        let tx2 = iterator.next(()).unwrap();
        assert_eq!(tx2.hash(), &tx_2_hash);
        // Check that it's empty
        assert!(iterator.next(()).is_none(), "Iterator should be empty");

        // ### Second flashblock
        // should contain txs 1, 2, and 3
        iterator.refresh_iterator(BestPayloadTransactions::new(pool.best()), 1);
        let tx1 = iterator.next(()).unwrap();
        assert_eq!(tx1.hash(), &tx_1_hash);
        let tx2 = iterator.next(()).unwrap();
        assert_eq!(tx2.hash(), &tx_2_hash);
        let tx3 = iterator.next(()).unwrap();
        assert_eq!(tx3.hash(), &tx_3_hash);
        // Check that it's empty
        assert!(iterator.next(()).is_none(), "Iterator should be empty");

        // ### Third flashblock
        // should contain txs 1, 3, and 4
        iterator.refresh_iterator(BestPayloadTransactions::new(pool.best()), 2);
        let tx1 = iterator.next(()).unwrap();
        assert_eq!(tx1.hash(), &tx_1_hash);
        let tx3 = iterator.next(()).unwrap();
        assert_eq!(tx3.hash(), &tx_3_hash);
        let tx4 = iterator.next(()).unwrap();
        assert_eq!(tx4.hash(), &tx_4_hash);
        // Check that it's empty
        assert!(iterator.next(()).is_none(), "Iterator should be empty");

        // ### Forth flashblock
        // should contain txs 1, 3, and 4
        iterator.refresh_iterator(BestPayloadTransactions::new(pool.best()), 3);
        let tx1 = iterator.next(()).unwrap();
        assert_eq!(tx1.hash(), &tx_1_hash);
        let tx3 = iterator.next(()).unwrap();
        assert_eq!(tx3.hash(), &tx_3_hash);
        let tx4 = iterator.next(()).unwrap();
        assert_eq!(tx4.hash(), &tx_4_hash);
        // Check that it's empty
        assert!(iterator.next(()).is_none(), "Iterator should be empty");

        // ### Fifth flashblock
        // should contain txs 1 and 3
        iterator.refresh_iterator(BestPayloadTransactions::new(pool.best()), 4);
        let tx1 = iterator.next(()).unwrap();
        assert_eq!(tx1.hash(), &tx_1_hash);
        let tx3 = iterator.next(()).unwrap();
        assert_eq!(tx3.hash(), &tx_3_hash);
        // Check that it's empty
        assert!(iterator.next(()).is_none(), "Iterator should be empty");
    }
}

 */
