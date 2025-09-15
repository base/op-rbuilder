use alloy_primitives::{Address, TxHash};
use std::collections::HashSet;
use alloy_primitives::hex::ToHexExt;
use tips_datastore::postgres::{BundleFilter, BundleWithMetadata};
use tips_datastore::{BundleDatastore, PostgresDatastore};
use tracing::{debug, warn};

pub struct BestFlashblocksTxs
{
    db: PostgresDatastore,
    bundle_idx: usize,
    bundles: Vec<BundleWithMetadata>,

    current_flashblock_number: u64,
    // Transactions that were already commited to the state. Using them again would cause NonceTooLow
    // so we skip them
    commited_transactions: HashSet<TxHash>,
}

impl BestFlashblocksTxs {
    pub fn new(db: PostgresDatastore) -> Self {
        // let db = tokio::task::block_in_place(|| {
        //     tokio::runtime::Handle::current().block_on(async {
        //         PostgresDatastore::connect(String::from("postgresql://postgres:postgres@localhost:5432/postgres"))
        //             .await.expect("cannot connect to db")
        //     })
        // });

        Self {
            db,
            bundle_idx: 0,
            bundles: Vec::new(),
            current_flashblock_number: 0,
            commited_transactions: Default::default(),
        }
    }

    /// Replaces current iterator with new one. We use it on new flashblock building, to refresh
    /// priority boundaries
    pub fn refresh_iterator(&mut self, current_flashblock_number: u64) {
        let db_copy = self.db.clone();

        let bundles = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                db_copy.select_bundles(BundleFilter::new()).await.expect("should fetch bundles")
            })
        });

        // let bundles = tokio::runtime::Handle::current().block_on(async {
        //     db_copy.select_bundles(BundleFilter::new()).await.expect("should fetch bundles")
        // });

        for bundle in bundles.iter() {
            for txn in bundle.txn_hashes.iter() {
                warn!(message = "danyal loaded txn", txn = txn.encode_hex());
            }
        }

        self.bundle_idx = 0;
        self.bundles = bundles;
        self.current_flashblock_number = current_flashblock_number;
    }

    /// Remove transaction from next iteration and it already in the state
    pub fn mark_commited(&mut self, txs: Vec<TxHash>) {
        self.commited_transactions.extend(txs);
    }
}

impl BestFlashblocksTxs {

    pub fn next(&mut self, _ctx: ()) -> Option<BundleWithMetadata> {
        loop {
            if self.bundle_idx >= self.bundles.len() {
                return None;
            }

            let tx = self.bundles[self.bundle_idx].clone();
            self.bundle_idx += 1;

            for txn in tx.txn_hashes.iter() {
                warn!(message = "danyal considering txn", txn = %tx.txn_hashes[0].encode_hex());
            }

            for hash in tx.txn_hashes.iter() {
                if self.commited_transactions.contains(hash) {
                    continue;
                }
            }

            for txn in tx.txn_hashes.iter() {
                warn!(message = "danyal good txn", txn = %tx.txn_hashes[0].encode_hex());
            }

            // Skip transaction we already included
            // let flashblock_number_min = tx.flashblock_number_min();
            // let flashblock_number_max = tx.flashblock_number_max();

            // Check min flashblock requirement
            // if let Some(min) = flashblock_number_min {
            //     if self.current_flashblock_number < min {
            //         continue;
            //     }
            // }

            // Check max flashblock requirement
            // if let Some(max) = flashblock_number_max {
            //     if self.current_flashblock_number > max {
            //         debug!(
            //             target: "payload_builder",
            //             tx_hash = ?tx.hash(),
            //             sender = ?tx.sender(),
            //             nonce = tx.nonce(),
            //             current_flashblock = self.current_flashblock_number,
            //             max_flashblock = max,
            //             "Bundle flashblock max exceeded"
            //         );
            //         self.inner.mark_invalid(tx.sender(), tx.nonce());
            //         continue;
            //     }
            // }

            return Some(tx);
        }
    }

    /// Proxy to inner iterator
    pub fn mark_invalid(&mut self, _sender: Address, _nonce: u64) {
        // TODO
        // self.inner.mark_invalid(sender, nonce);
    }
}