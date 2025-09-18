use std::{future::Future, sync::Arc};

use futures_util::future::BoxFuture;
use reth_primitives_traits::Recovered;
use reth_transaction_pool::{
    validate::TransactionValidationOutcome,
    TransactionValidator, PoolTransaction, TransactionOrigin,
};

use crate::tx::{SimOutcome, MaybeSimulatedTransaction};
use reth_optimism_evm::{OpEvmConfig, OpNextBlockEnvAttributes};
use reth_optimism_primitives::OpTransactionSigned;
use reth_primitives::SealedHeader;
use reth_provider::{HashedPostStateProvider, StateProvider, StateRootProvider};
use reth_revm::State;
use reth_evm::{ConfigureEvm, execute::BlockBuilder, Evm};
use reth_revm::database::StateProviderDatabase;

/// A pluggable interface used by `SimulatingValidator` to simulate consensus transactions.
pub trait TxSimulator<ConsensusTx>: Send + Sync + 'static {
    /// Simulate a consensus transaction. Implementations should perform any required
    /// asynchronous work (e.g. EVM execution) and must not panic.
    fn simulate(&self, origin: TransactionOrigin, tx: Recovered<ConsensusTx>) -> BoxFuture<'static, SimOutcome>;
}

impl<ConsensusTx, F, Fut> TxSimulator<ConsensusTx> for F
where
    ConsensusTx: Send + Sync + 'static,
    F: Fn(TransactionOrigin, Recovered<ConsensusTx>) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = SimOutcome> + Send + 'static,
{
    fn simulate(&self, origin: TransactionOrigin, tx: Recovered<ConsensusTx>) -> BoxFuture<'static, SimOutcome> {
        Box::pin((self)(origin, tx))
    }
}

/// Wraps a [`TransactionValidator`] and performs simulation for valid transactions.
///
/// - Delegates validation to the inner validator.
/// - If outcome is `Valid`, converts to consensus tx and simulates on the same-thread
///   (awaits), then injects the results back into the validated pooled transaction where
///   supported.
pub struct SimulatingValidator<V, S> {
    inner: V,
    simulator: Arc<S>,
}

impl<V: std::fmt::Debug, S> std::fmt::Debug for SimulatingValidator<V, S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SimulatingValidator").field("inner", &self.inner).finish()
    }
}

impl<V, S> SimulatingValidator<V, S> {
    pub fn new(inner: V, simulator: Arc<S>) -> Self {
        Self { inner, simulator }
    }
}

impl<V, S> TransactionValidator for SimulatingValidator<V, S>
where
    V: TransactionValidator + Send + Sync + Clone + 'static,
    <V as TransactionValidator>::Transaction: PoolTransaction + MaybeSimulatedTransaction + Send + Sync + 'static,
    S: TxSimulator<<<V as TransactionValidator>::Transaction as PoolTransaction>::Consensus>,
{
    type Transaction = V::Transaction;

    async fn validate_transaction(
        &self,
        origin: TransactionOrigin,
        transaction: Self::Transaction,
    ) -> TransactionValidationOutcome<Self::Transaction> {
        let mut outcome = self.inner.validate_transaction(origin, transaction).await;

        if let TransactionValidationOutcome::Valid { transaction: valid_tx, .. } = &mut outcome {
            let consensus: reth_primitives_traits::Recovered<
                <Self::Transaction as PoolTransaction>::Consensus,
            > = valid_tx.transaction().clone_into_consensus();
            let sim_outcome = self.simulator.simulate(origin, consensus).await;
            match valid_tx {
                reth_transaction_pool::validate::ValidTransaction::Valid(tx) => {
                    tx.set_sim_outcome(sim_outcome);
                }
                reth_transaction_pool::validate::ValidTransaction::ValidWithSidecar { transaction, .. } => {
                    transaction.set_sim_outcome(sim_outcome);
                }
            }
        }

        outcome
    }

    async fn validate_transactions(
        &self,
        transactions: Vec<(TransactionOrigin, Self::Transaction)>,
    ) -> Vec<TransactionValidationOutcome<Self::Transaction>> {
        // Keep the origins to associate outcomes with inputs
        let origins: Vec<TransactionOrigin> = transactions.iter().map(|(o, _)| *o).collect();
        let mut outcomes = self.inner.validate_transactions(transactions).await;

        for (idx, outcome) in outcomes.iter_mut().enumerate() {
            if let TransactionValidationOutcome::Valid { transaction: valid_tx, .. } = outcome {
                let consensus = valid_tx.transaction().clone_into_consensus();
                let origin = origins[idx];
                let sim_outcome = self.simulator.simulate(origin, consensus).await;
                match valid_tx {
                    reth_transaction_pool::validate::ValidTransaction::Valid(tx) => {
                        tx.set_sim_outcome(sim_outcome);
                    }
                    reth_transaction_pool::validate::ValidTransaction::ValidWithSidecar { transaction, .. } => {
                        transaction.set_sim_outcome(sim_outcome);
                    }
                }
            }
        }

        outcomes
    }

    fn on_new_head_block<B>(&self, new_tip_block: &reth_primitives_traits::SealedBlock<B>)
    where
        B: reth_primitives_traits::Block,
    {
        self.inner.on_new_head_block(new_tip_block)
    }
}

/// A concrete Optimism transaction simulator to be used at validation-time.
///
/// This simulates a single consensus transaction on a fresh overlay created from the
/// current parent header returned by `get_parent_header`, using the given `OpEvmConfig`.
pub struct OpValidationSimulator<MakeProvider, MakeHeader, P>
where
    MakeProvider: Fn() -> P + Send + Sync + 'static,
    MakeHeader: Fn() -> SealedHeader + Send + Sync + 'static,
    P: StateProvider + HashedPostStateProvider + StateRootProvider + Send + 'static,
{
    make_provider: std::sync::Arc<MakeProvider>,
    get_parent_header: std::sync::Arc<MakeHeader>,
    evm_config: OpEvmConfig,
}

impl<MakeProvider, MakeHeader, P> OpValidationSimulator<MakeProvider, MakeHeader, P>
where
    MakeProvider: Fn() -> P + Send + Sync + 'static,
    MakeHeader: Fn() -> SealedHeader + Send + Sync + 'static,
    P: StateProvider + HashedPostStateProvider + StateRootProvider + Send + 'static,
{
    pub fn new(
        evm_config: OpEvmConfig,
        make_provider: MakeProvider,
        get_parent_header: MakeHeader,
    ) -> Self {
        Self {
            evm_config,
            make_provider: std::sync::Arc::new(make_provider),
            get_parent_header: std::sync::Arc::new(get_parent_header),
        }
    }
}

impl<MakeProvider, MakeHeader, P> TxSimulator<OpTransactionSigned>
    for OpValidationSimulator<MakeProvider, MakeHeader, P>
where
    MakeProvider: Fn() -> P + Send + Sync + 'static,
    MakeHeader: Fn() -> SealedHeader + Send + Sync + 'static,
    P: StateProvider + HashedPostStateProvider + StateRootProvider + Send + 'static,
{
    fn simulate(
        &self,
        _origin: TransactionOrigin,
        tx: reth_primitives_traits::Recovered<OpTransactionSigned>,
    ) -> BoxFuture<'static, SimOutcome> {
        let evm_config = self.evm_config.clone();
        let make_provider = self.make_provider.clone();
        let get_parent_header = self.get_parent_header.clone();
        Box::pin(async move {
            // Build overlay state
            let parent = (get_parent_header)();
            let provider = (make_provider)();
            let base_state_db = StateProviderDatabase::new(provider);
            let mut sim_state: State<_> = State::builder()
                .with_database(base_state_db)
                .with_bundle_update()
                .build();

            // Derive minimal block env attributes from parent header
            let block_env_attributes = OpNextBlockEnvAttributes {
                timestamp: parent.timestamp,
                suggested_fee_recipient: parent.beneficiary,
                prev_randao: parent.mix_hash,
                gas_limit: parent.gas_limit,
                parent_beacon_block_root: parent.parent_beacon_block_root,
                extra_data: Default::default(),
            };

            // Prepare EVM env
            let evm_env = match evm_config.next_evm_env(&parent, &block_env_attributes) {
                Ok(env) => env,
                Err(_) => {
                    return SimOutcome {
                        success: false,
                        invalid_nonce_too_low: false,
                        invalid_other: true,
                        simulated_gas_used: None,
                        execution_time_us: None,
                    }
                }
            };

            // Apply pre-exec changes
            let mut builder = match evm_config
                .builder_for_next_block(&mut sim_state, &parent, block_env_attributes.clone())
            {
                Ok(b) => b,
                Err(_) => {
                    return SimOutcome {
                        success: false,
                        invalid_nonce_too_low: false,
                        invalid_other: true,
                        simulated_gas_used: None,
                        execution_time_us: None,
                    }
                }
            };

            if builder.apply_pre_execution_changes().is_err() {
                return SimOutcome {
                    success: false,
                    invalid_nonce_too_low: false,
                    invalid_other: true,
                    simulated_gas_used: None,
                    execution_time_us: None,
                };
            }
            // release the borrow on sim_state held by builder
            drop(builder);

            // Simulate transaction
            let mut evm = evm_config.evm_with_env(&mut sim_state, evm_env);
            let start = std::time::Instant::now();
            match evm.transact(&tx) {
                Ok(res) => {
                    let success = res.result.is_success();
                    let gas_used = res.result.gas_used();
                    let elapsed = start.elapsed().as_micros();
                    SimOutcome {
                        success,
                        invalid_nonce_too_low: false,
                        invalid_other: false,
                        simulated_gas_used: Some(gas_used),
                        execution_time_us: Some(elapsed),
                    }
                }
                Err(_err) => {
                    SimOutcome {
                        success: false,
                        invalid_nonce_too_low: false,
                        invalid_other: true,
                        simulated_gas_used: None,
                        execution_time_us: None,
                    }
                }
            }
        })
    }
}


