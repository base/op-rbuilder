//! Trait bounds and type aliases for op-rbuilder components.
//!
//! This module defines trait bounds that constrain the generic types used throughout
//! the block builder. These traits ensure that components work correctly with the
//! Optimism-specific types and configurations.
//!
//! # Overview
//!
//! - [`NodeBounds`]: Constraints for full node type parameters
//! - [`NodeComponents`]: Constraints for full node component parameters
//! - [`PoolBounds`]: Constraints for transaction pool implementations
//! - [`ClientBounds`]: Constraints for state provider and chain spec clients
//! - [`PayloadTxsBounds`]: Constraints for payload transaction iterators

use alloy_consensus::Header;
use reth_node_api::{FullNodeComponents, FullNodeTypes, NodeTypes};
use reth_optimism_chainspec::OpChainSpec;
use reth_optimism_node::OpEngineTypes;
use reth_optimism_primitives::{OpPrimitives, OpTransactionSigned};
use reth_payload_util::PayloadTransactions;
use reth_provider::{BlockReaderIdExt, ChainSpecProvider, StateProviderFactory};
use reth_transaction_pool::TransactionPool;

use crate::tx::FBPoolTransaction;

/// Trait bound for full node types compatible with op-rbuilder.
///
/// This trait ensures that the node types are configured for Optimism,
/// including the correct engine types, chain specification, and primitives.
pub trait NodeBounds:
    FullNodeTypes<
    Types: NodeTypes<Payload = OpEngineTypes, ChainSpec = OpChainSpec, Primitives = OpPrimitives>,
>
{
}

impl<T> NodeBounds for T where
    T: FullNodeTypes<
        Types: NodeTypes<
            Payload = OpEngineTypes,
            ChainSpec = OpChainSpec,
            Primitives = OpPrimitives,
        >,
    >
{
}

/// Trait bound for full node components compatible with op-rbuilder.
///
/// Similar to [`NodeBounds`], but for component types that include
/// additional functionality beyond the base node types.
pub trait NodeComponents:
    FullNodeComponents<
    Types: NodeTypes<Payload = OpEngineTypes, ChainSpec = OpChainSpec, Primitives = OpPrimitives>,
>
{
}

impl<T> NodeComponents for T where
    T: FullNodeComponents<
        Types: NodeTypes<
            Payload = OpEngineTypes,
            ChainSpec = OpChainSpec,
            Primitives = OpPrimitives,
        >,
    >
{
}

/// Trait bound for transaction pools compatible with op-rbuilder.
///
/// The transaction pool must support flashblock-aware transactions and
/// work with Optimism's signed transaction type.
pub trait PoolBounds:
    TransactionPool<Transaction: FBPoolTransaction<Consensus = OpTransactionSigned>> + Unpin + 'static
where
    <Self as TransactionPool>::Transaction: FBPoolTransaction,
{
}

impl<T> PoolBounds for T
where
    T: TransactionPool<Transaction: FBPoolTransaction<Consensus = OpTransactionSigned>>
        + Unpin
        + 'static,
    <Self as TransactionPool>::Transaction: FBPoolTransaction,
{
}

/// Trait bound for state and chain specification clients.
///
/// Clients must be able to provide state, chain specification information,
/// and block header data for Optimism chains.
pub trait ClientBounds:
    StateProviderFactory
    + ChainSpecProvider<ChainSpec = OpChainSpec>
    + BlockReaderIdExt<Header = Header>
    + Clone
{
}

impl<T> ClientBounds for T where
    T: StateProviderFactory
        + ChainSpecProvider<ChainSpec = OpChainSpec>
        + BlockReaderIdExt<Header = Header>
        + Clone
{
}

/// Trait bound for payload transaction iterators.
///
/// Transaction iterators used during payload building must provide
/// flashblock-compatible transactions that can be converted to
/// Optimism's consensus transaction type.
pub trait PayloadTxsBounds:
    PayloadTransactions<Transaction: FBPoolTransaction<Consensus = OpTransactionSigned>>
{
}

impl<T> PayloadTxsBounds for T where
    T: PayloadTransactions<Transaction: FBPoolTransaction<Consensus = OpTransactionSigned>>
{
}
