//! WETH (Wrapped Ether) Precompile Implementation
//!
//! This precompile replaces the WETH contract at address 0x4200000000000000000000000000000000000006
//! with a more efficient implementation using journal operations for storage access.
//!
//! The implementation matches the behavior of the original Solidity WETH9 contract:
//! - deposit(): Wraps ETH into WETH
//! - withdraw(uint256): Unwraps WETH into ETH
//! - transfer/transferFrom: ERC20 token transfers
//! - approve: ERC20 allowance approval
//! - balanceOf/allowance/totalSupply: View functions

use alloy_primitives::{Address, Bytes, Log, LogData, U256, address, b256, keccak256};
use revm::{
    context_interface::{ContextTr, JournalTr},
    precompile::{PrecompileError, PrecompileOutput, PrecompileResult},
};

/// WETH contract address on OP Stack chains
pub const WETH_ADDRESS: Address = address!("4200000000000000000000000000000000000006");

// Function selectors (first 4 bytes of keccak256 of function signature)
// deposit() = keccak256("deposit()")[0..4]
const DEPOSIT_SELECTOR: [u8; 4] = {
    let hash = b256!("d0e30db03f2e24c6531d8ae2f6c09d8e7a6ad7f7e87a81cb75dfda61c9d83286");
    [hash.0[0], hash.0[1], hash.0[2], hash.0[3]]
};
// withdraw(uint256) = keccak256("withdraw(uint256)")[0..4]
const WITHDRAW_SELECTOR: [u8; 4] = {
    let hash = b256!("2e1a7d4d13322e7b96f9a57413e1525c250fb7a9021cf91d1540d5b69f16a49f");
    [hash.0[0], hash.0[1], hash.0[2], hash.0[3]]
};
// totalSupply() = keccak256("totalSupply()")[0..4]
const TOTAL_SUPPLY_SELECTOR: [u8; 4] = {
    let hash = b256!("18160ddd7f15c72528c2f94fd8dfe3c8d5aa26e2c50c7d81f4bc7bee8d4b7932");
    [hash.0[0], hash.0[1], hash.0[2], hash.0[3]]
};
// balanceOf(address) = keccak256("balanceOf(address)")[0..4]
const BALANCE_OF_SELECTOR: [u8; 4] = {
    let hash = b256!("70a08231b98ef4ca268c9cc3f6b4590e4bfec28280db06bb5d45e689f2a360be");
    [hash.0[0], hash.0[1], hash.0[2], hash.0[3]]
};
// transfer(address,uint256) = keccak256("transfer(address,uint256)")[0..4]
const TRANSFER_SELECTOR: [u8; 4] = {
    let hash = b256!("a9059cbb2ab09eb219583f4a59a5d0623ade346d962bcd4e46b11da047c9049b");
    [hash.0[0], hash.0[1], hash.0[2], hash.0[3]]
};
// approve(address,uint256) = keccak256("approve(address,uint256)")[0..4]
const APPROVE_SELECTOR: [u8; 4] = {
    let hash = b256!("095ea7b334ae44009aa867bfb386f5c3b4b443ac6f0ee573fa91c4608fbadfba");
    [hash.0[0], hash.0[1], hash.0[2], hash.0[3]]
};
// transferFrom(address,address,uint256) = keccak256("transferFrom(address,address,uint256)")[0..4]
const TRANSFER_FROM_SELECTOR: [u8; 4] = {
    let hash = b256!("23b872dd7302113369cda2901243429419bec145408fa8b352b3dd92b66c680b");
    [hash.0[0], hash.0[1], hash.0[2], hash.0[3]]
};
// allowance(address,address) = keccak256("allowance(address,address)")[0..4]
const ALLOWANCE_SELECTOR: [u8; 4] = {
    let hash = b256!("dd62ed3e90e97b3d417db9c0c7522647811bafca5afc6694f143588d255fdfb4");
    [hash.0[0], hash.0[1], hash.0[2], hash.0[3]]
};

// Event signatures (pre-computed keccak256 hashes)
// Deposit(address,uint256)
const DEPOSIT_EVENT: [u8; 32] =
    b256!("e1fffcc4923d04b559f4d29a8bfc6cda04eb5b0d3c460751c2402c5c5cc9109c").0;
// Withdrawal(address,uint256)
const WITHDRAWAL_EVENT: [u8; 32] =
    b256!("7fcf532c15f0a6db0bd6d0e038bea71d30d808c7d98cb3bf7268a95bf5081b65").0;
// Transfer(address,address,uint256)
const TRANSFER_EVENT: [u8; 32] =
    b256!("ddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef").0;
// Approval(address,address,uint256)
const APPROVAL_EVENT: [u8; 32] =
    b256!("8c5be1e5ebec7d5bd14f71427d1e84f3dd0314c0f7b2291e5b200ac8c7c3b925").0;

// Gas costs based on EVM operations (see https://github.com/wolflo/evm-opcodes/blob/main/gas.md)
// These are calculated dynamically based on EIP-2929 warm/cold storage access tracking.
//
// EIP-2929 Gas Costs:
//   - Cold SLOAD: 2,100 gas (first access in transaction)
//   - Warm SLOAD: 100 gas (subsequent accesses)
//   - Cold SSTORE (zero→non-zero): 20,000 + 2,100 gas
//   - Warm SSTORE (zero→non-zero): 20,000 gas
//   - Cold SSTORE (non-zero→non-zero): 2,900 + 2,100 gas
//   - Warm SSTORE (non-zero→non-zero): 2,900 gas
//
// Event Gas Costs:
//   - LOG2 (2 topics + 32 bytes): 375 + 2×375 + 8×32 = 1,381 gas
//   - LOG3 (3 topics + 32 bytes): 375 + 3×375 + 8×32 = 1,750 gas
//
// CALL Gas Costs:
//   - CALL with value (warm): 100 + 9,000 = 9,100 gas

const GAS_SLOAD_COLD: u64 = 2_100;
const GAS_SLOAD_WARM: u64 = 100;
const GAS_SSTORE_ZERO_TO_NONZERO: u64 = 20_000;
const GAS_SSTORE_NONZERO: u64 = 2_900;
const GAS_LOG2: u64 = 1_381;
const GAS_LOG3: u64 = 1_750;
const GAS_CALL_VALUE: u64 = 9_100;

/// Calculate gas cost for an SLOAD operation based on warm/cold access
fn gas_sload(is_cold: bool) -> u64 {
    if is_cold {
        GAS_SLOAD_COLD
    } else {
        GAS_SLOAD_WARM
    }
}

/// Calculate gas cost for an SSTORE operation based on warm/cold access and value transition
fn gas_sstore(is_cold: bool, present_value: U256, new_value: U256) -> u64 {
    let is_zero_to_nonzero = present_value.is_zero() && !new_value.is_zero();
    let base_cost = if is_zero_to_nonzero {
        GAS_SSTORE_ZERO_TO_NONZERO
    } else {
        GAS_SSTORE_NONZERO
    };
    base_cost
        + if is_cold {
            GAS_SLOAD_COLD // Cold access adds SLOAD cost
        } else {
            0
        }
}

/// Calculate storage slot for balanceOf mapping (slot 0)
fn balance_slot(addr: Address) -> U256 {
    let mut data = [0u8; 64];
    data[12..32].copy_from_slice(addr.as_slice());
    // slot 0 is already zeros in data[32..64]
    U256::from_be_bytes(keccak256(data).0)
}

/// Calculate storage slot for allowance mapping (slot 1)
fn allowance_slot(owner: Address, spender: Address) -> U256 {
    // First hash: keccak256(owner || slot(1))
    let mut data = [0u8; 64];
    data[12..32].copy_from_slice(owner.as_slice());
    data[63] = 1; // slot 1
    let inner_hash = keccak256(data);

    // Second hash: keccak256(spender || inner_hash)
    let mut data2 = [0u8; 64];
    data2[12..32].copy_from_slice(spender.as_slice());
    data2[32..64].copy_from_slice(&inner_hash.0);
    U256::from_be_bytes(keccak256(data2).0)
}

/// Run the WETH precompile
pub(super) fn run_weth_precompile<CTX: ContextTr>(
    context: &mut CTX,
    input: &[u8],
    gas_limit: u64,
    value: U256,
    caller: Address,
    is_static: bool,
) -> PrecompileResult {
    // Handle empty input or fallback (deposit)
    if input.is_empty() || input.len() < 4 {
        return handle_deposit(context, value, caller, gas_limit, is_static);
    }

    let selector = &input[0..4];
    match selector {
        s if s == DEPOSIT_SELECTOR => handle_deposit(context, value, caller, gas_limit, is_static),
        s if s == WITHDRAW_SELECTOR => {
            if input.len() < 36 {
                return Err(PrecompileError::Other(
                    "Invalid input length for withdraw".to_string(),
                ));
            }
            let amount = U256::from_be_slice(&input[4..36]);
            handle_withdraw(context, amount, caller, gas_limit, is_static)
        }
        s if s == TOTAL_SUPPLY_SELECTOR => handle_total_supply(context, gas_limit),
        s if s == BALANCE_OF_SELECTOR => {
            if input.len() < 36 {
                return Err(PrecompileError::Other(
                    "Invalid input length for balanceOf".to_string(),
                ));
            }
            let addr = Address::from_slice(&input[16..36]);
            handle_balance_of(context, addr, gas_limit)
        }
        s if s == TRANSFER_SELECTOR => {
            if input.len() < 68 {
                return Err(PrecompileError::Other(
                    "Invalid input length for transfer".to_string(),
                ));
            }
            let to = Address::from_slice(&input[16..36]);
            let amount = U256::from_be_slice(&input[36..68]);
            handle_transfer(context, caller, to, amount, gas_limit, is_static)
        }
        s if s == APPROVE_SELECTOR => {
            if input.len() < 68 {
                return Err(PrecompileError::Other(
                    "Invalid input length for approve".to_string(),
                ));
            }
            let spender = Address::from_slice(&input[16..36]);
            let amount = U256::from_be_slice(&input[36..68]);
            handle_approve(context, caller, spender, amount, gas_limit, is_static)
        }
        s if s == TRANSFER_FROM_SELECTOR => {
            if input.len() < 100 {
                return Err(PrecompileError::Other(
                    "Invalid input length for transferFrom".to_string(),
                ));
            }
            let from = Address::from_slice(&input[16..36]);
            let to = Address::from_slice(&input[48..68]);
            let amount = U256::from_be_slice(&input[68..100]);
            handle_transfer_from(context, caller, from, to, amount, gas_limit, is_static)
        }
        s if s == ALLOWANCE_SELECTOR => {
            if input.len() < 68 {
                return Err(PrecompileError::Other(
                    "Invalid input length for allowance".to_string(),
                ));
            }
            let owner = Address::from_slice(&input[16..36]);
            let spender = Address::from_slice(&input[48..68]);
            handle_allowance(context, owner, spender, gas_limit)
        }
        _ => Err(PrecompileError::Other(
            "Unknown function selector".to_string(),
        )),
    }
}

fn handle_deposit<CTX: ContextTr>(
    context: &mut CTX,
    value: U256,
    caller: Address,
    gas_limit: u64,
    is_static: bool,
) -> PrecompileResult {
    if is_static {
        return Err(PrecompileError::Other(
            "Cannot deposit in static context".to_string(),
        ));
    }

    // Ensure WETH account is loaded
    let _ = context
        .journal_mut()
        .load_account(WETH_ADDRESS)
        .map_err(|e| PrecompileError::Other(format!("Account load failed: {:?}", e)))?;

    // Track gas usage
    let mut gas_used = 0u64;

    // Read current balance (with warm/cold tracking)
    let slot = balance_slot(caller);
    let sload_result = context
        .journal_mut()
        .sload(WETH_ADDRESS, slot)
        .map_err(|e| PrecompileError::Other(format!("Storage read failed: {:?}", e)))?;

    gas_used += gas_sload(sload_result.is_cold);
    let current_balance = sload_result.data;

    // Update balance (with warm/cold tracking)
    let new_balance = current_balance.saturating_add(value);
    let sstore_result = context
        .journal_mut()
        .sstore(WETH_ADDRESS, slot, new_balance)
        .map_err(|e| PrecompileError::Other(format!("Storage write failed: {:?}", e)))?;

    gas_used += gas_sstore(
        sstore_result.is_cold,
        sstore_result.data.present_value,
        sstore_result.data.new_value,
    );

    // Check if we have enough gas
    if gas_limit < gas_used + GAS_LOG2 {
        return Err(PrecompileError::OutOfGas);
    }

    // Emit Deposit event
    let mut event_data = vec![0u8; 32];
    value
        .to_be_bytes_vec()
        .iter()
        .rev()
        .enumerate()
        .for_each(|(i, &b)| {
            event_data[31 - i] = b;
        });

    let log = Log {
        address: WETH_ADDRESS,
        data: LogData::new(
            vec![DEPOSIT_EVENT.into(), caller.into_word()],
            Bytes::from(event_data),
        )
        .expect("Valid log data"),
    };

    context.journal_mut().log(log);

    // Add event emission gas
    gas_used += GAS_LOG2;

    // Return true (success)
    let mut output = vec![0u8; 32];
    output[31] = 1;
    Ok(PrecompileOutput::new(gas_used, Bytes::from(output)))
}

fn handle_withdraw<CTX: ContextTr>(
    context: &mut CTX,
    amount: U256,
    caller: Address,
    gas_limit: u64,
    is_static: bool,
) -> PrecompileResult {
    if is_static {
        return Err(PrecompileError::Other(
            "Cannot withdraw in static context".to_string(),
        ));
    }

    // Ensure WETH account is loaded
    let _ = context
        .journal_mut()
        .load_account(WETH_ADDRESS)
        .map_err(|e| PrecompileError::Other(format!("Account load failed: {:?}", e)))?;

    // Track gas usage
    let mut gas_used = 0u64;

    // Read current balance
    let slot = balance_slot(caller);
    let sload_result = context
        .journal_mut()
        .sload(WETH_ADDRESS, slot)
        .map_err(|e| PrecompileError::Other(format!("Storage read failed: {:?}", e)))?;

    gas_used += gas_sload(sload_result.is_cold);
    let current_balance = sload_result.data;

    // Check sufficient balance
    if current_balance < amount {
        return Err(PrecompileError::Other(
            "Insufficient WETH balance".to_string(),
        ));
    }

    // Update balance
    let new_balance = current_balance - amount;
    let sstore_result = context
        .journal_mut()
        .sstore(WETH_ADDRESS, slot, new_balance)
        .map_err(|e| PrecompileError::Other(format!("Storage write failed: {:?}", e)))?;

    gas_used += gas_sstore(
        sstore_result.is_cold,
        sstore_result.data.present_value,
        sstore_result.data.new_value,
    );

    // Transfer ETH to caller (warm CALL with value)
    gas_used += GAS_CALL_VALUE;

    if gas_limit < gas_used + GAS_LOG2 {
        return Err(PrecompileError::OutOfGas);
    }

    let transfer_result = context
        .journal_mut()
        .transfer(WETH_ADDRESS, caller, amount)
        .map_err(|e| PrecompileError::Other(format!("ETH transfer failed: {:?}", e)))?;

    if transfer_result.is_some() {
        return Err(PrecompileError::Other("ETH transfer failed".to_string()));
    }

    // Emit Withdrawal event
    let mut event_data = vec![0u8; 32];
    amount
        .to_be_bytes_vec()
        .iter()
        .rev()
        .enumerate()
        .for_each(|(i, &b)| {
            event_data[31 - i] = b;
        });

    let log = Log {
        address: WETH_ADDRESS,
        data: LogData::new(
            vec![WITHDRAWAL_EVENT.into(), caller.into_word()],
            Bytes::from(event_data),
        )
        .expect("Valid log data"),
    };

    context.journal_mut().log(log);

    // Add event gas
    gas_used += GAS_LOG2;

    // Return true (success)
    let mut output = vec![0u8; 32];
    output[31] = 1;
    Ok(PrecompileOutput::new(gas_used, Bytes::from(output)))
}

fn handle_total_supply<CTX: ContextTr>(context: &mut CTX, gas_limit: u64) -> PrecompileResult {
    // totalSupply = WETH contract's ETH balance
    let account_info = context
        .journal_mut()
        .load_account(WETH_ADDRESS)
        .map_err(|e| PrecompileError::Other(format!("Account load failed: {:?}", e)))?;

    // Account load has a cost based on warm/cold
    let gas_used = if account_info.is_cold {
        GAS_SLOAD_COLD
    } else {
        GAS_SLOAD_WARM
    };

    if gas_limit < gas_used {
        return Err(PrecompileError::OutOfGas);
    }

    let balance = account_info.info.balance;

    let mut output = vec![0u8; 32];
    balance
        .to_be_bytes_vec()
        .iter()
        .rev()
        .enumerate()
        .for_each(|(i, &b)| {
            output[31 - i] = b;
        });

    Ok(PrecompileOutput::new(gas_used, Bytes::from(output)))
}

fn handle_balance_of<CTX: ContextTr>(
    context: &mut CTX,
    addr: Address,
    gas_limit: u64,
) -> PrecompileResult {
    // Ensure WETH account is loaded
    let _ = context
        .journal_mut()
        .load_account(WETH_ADDRESS)
        .map_err(|e| PrecompileError::Other(format!("Account load failed: {:?}", e)))?;

    // Read balance with warm/cold tracking
    let slot = balance_slot(addr);
    let sload_result = context
        .journal_mut()
        .sload(WETH_ADDRESS, slot)
        .map_err(|e| PrecompileError::Other(format!("Storage read failed: {:?}", e)))?;

    let gas_used = gas_sload(sload_result.is_cold);

    if gas_limit < gas_used {
        return Err(PrecompileError::OutOfGas);
    }

    let balance = sload_result.data;

    let mut output = vec![0u8; 32];
    balance
        .to_be_bytes_vec()
        .iter()
        .rev()
        .enumerate()
        .for_each(|(i, &b)| {
            output[31 - i] = b;
        });

    Ok(PrecompileOutput::new(gas_used, Bytes::from(output)))
}

fn handle_transfer<CTX: ContextTr>(
    context: &mut CTX,
    from: Address,
    to: Address,
    amount: U256,
    gas_limit: u64,
    is_static: bool,
) -> PrecompileResult {
    if is_static {
        return Err(PrecompileError::Other(
            "Cannot transfer in static context".to_string(),
        ));
    }

    // Ensure WETH account is loaded
    let _ = context
        .journal_mut()
        .load_account(WETH_ADDRESS)
        .map_err(|e| PrecompileError::Other(format!("Account load failed: {:?}", e)))?;

    // Track gas usage
    let mut gas_used = 0u64;

    // Read sender balance
    let from_slot = balance_slot(from);
    let from_sload = context
        .journal_mut()
        .sload(WETH_ADDRESS, from_slot)
        .map_err(|e| PrecompileError::Other(format!("Storage read failed: {:?}", e)))?;

    gas_used += gas_sload(from_sload.is_cold);
    let from_balance = from_sload.data;

    // Check sufficient balance
    if from_balance < amount {
        return Err(PrecompileError::Other(
            "Insufficient WETH balance".to_string(),
        ));
    }

    // Update sender balance
    let new_from_balance = from_balance - amount;
    let from_sstore = context
        .journal_mut()
        .sstore(WETH_ADDRESS, from_slot, new_from_balance)
        .map_err(|e| PrecompileError::Other(format!("Storage write failed: {:?}", e)))?;

    gas_used += gas_sstore(
        from_sstore.is_cold,
        from_sstore.data.present_value,
        from_sstore.data.new_value,
    );

    // Update receiver balance
    let to_slot = balance_slot(to);
    let to_sload = context
        .journal_mut()
        .sload(WETH_ADDRESS, to_slot)
        .map_err(|e| PrecompileError::Other(format!("Storage read failed: {:?}", e)))?;

    gas_used += gas_sload(to_sload.is_cold);
    let to_balance = to_sload.data;

    let new_to_balance = to_balance.saturating_add(amount);
    let to_sstore = context
        .journal_mut()
        .sstore(WETH_ADDRESS, to_slot, new_to_balance)
        .map_err(|e| PrecompileError::Other(format!("Storage write failed: {:?}", e)))?;

    gas_used += gas_sstore(
        to_sstore.is_cold,
        to_sstore.data.present_value,
        to_sstore.data.new_value,
    );

    // Check gas before emitting event
    if gas_limit < gas_used + GAS_LOG3 {
        return Err(PrecompileError::OutOfGas);
    }

    // Emit Transfer event
    let mut event_data = vec![0u8; 32];
    amount
        .to_be_bytes_vec()
        .iter()
        .rev()
        .enumerate()
        .for_each(|(i, &b)| {
            event_data[31 - i] = b;
        });

    let log = Log {
        address: WETH_ADDRESS,
        data: LogData::new(
            vec![TRANSFER_EVENT.into(), from.into_word(), to.into_word()],
            Bytes::from(event_data),
        )
        .expect("Valid log data"),
    };

    context.journal_mut().log(log);

    // Add event gas
    gas_used += GAS_LOG3;

    // Return true (success)
    let mut output = vec![0u8; 32];
    output[31] = 1;
    Ok(PrecompileOutput::new(gas_used, Bytes::from(output)))
}

fn handle_approve<CTX: ContextTr>(
    context: &mut CTX,
    owner: Address,
    spender: Address,
    amount: U256,
    gas_limit: u64,
    is_static: bool,
) -> PrecompileResult {
    if is_static {
        return Err(PrecompileError::Other(
            "Cannot approve in static context".to_string(),
        ));
    }

    // Ensure WETH account is loaded
    let _ = context
        .journal_mut()
        .load_account(WETH_ADDRESS)
        .map_err(|e| PrecompileError::Other(format!("Account load failed: {:?}", e)))?;

    // Track gas usage
    let mut gas_used = 0u64;

    // Update allowance
    let slot = allowance_slot(owner, spender);
    let sstore_result = context
        .journal_mut()
        .sstore(WETH_ADDRESS, slot, amount)
        .map_err(|e| PrecompileError::Other(format!("Storage write failed: {:?}", e)))?;

    gas_used += gas_sstore(
        sstore_result.is_cold,
        sstore_result.data.present_value,
        sstore_result.data.new_value,
    );

    // Check gas before emitting event
    if gas_limit < gas_used + GAS_LOG3 {
        return Err(PrecompileError::OutOfGas);
    }

    // Emit Approval event
    let mut event_data = vec![0u8; 32];
    amount
        .to_be_bytes_vec()
        .iter()
        .rev()
        .enumerate()
        .for_each(|(i, &b)| {
            event_data[31 - i] = b;
        });

    let log = Log {
        address: WETH_ADDRESS,
        data: LogData::new(
            vec![
                APPROVAL_EVENT.into(),
                owner.into_word(),
                spender.into_word(),
            ],
            Bytes::from(event_data),
        )
        .expect("Valid log data"),
    };

    context.journal_mut().log(log);

    // Add event gas
    gas_used += GAS_LOG3;

    // Return true (success)
    let mut output = vec![0u8; 32];
    output[31] = 1;
    Ok(PrecompileOutput::new(gas_used, Bytes::from(output)))
}

fn handle_transfer_from<CTX: ContextTr>(
    context: &mut CTX,
    spender: Address,
    from: Address,
    to: Address,
    amount: U256,
    gas_limit: u64,
    is_static: bool,
) -> PrecompileResult {
    if is_static {
        return Err(PrecompileError::Other(
            "Cannot transferFrom in static context".to_string(),
        ));
    }

    // Ensure WETH account is loaded
    let _ = context
        .journal_mut()
        .load_account(WETH_ADDRESS)
        .map_err(|e| PrecompileError::Other(format!("Account load failed: {:?}", e)))?;

    // Track gas usage
    let mut gas_used = 0u64;

    // Check and update allowance if spender != from
    if spender != from {
        let allowance_slot_val = allowance_slot(from, spender);
        let allowance_sload = context
            .journal_mut()
            .sload(WETH_ADDRESS, allowance_slot_val)
            .map_err(|e| PrecompileError::Other(format!("Storage read failed: {:?}", e)))?;

        gas_used += gas_sload(allowance_sload.is_cold);
        let allowance = allowance_sload.data;

        // Check if allowance is not infinite (uint256::MAX)
        if allowance != U256::MAX {
            if allowance < amount {
                return Err(PrecompileError::Other("Insufficient allowance".to_string()));
            }

            // Decrease allowance
            let new_allowance = allowance - amount;
            let allowance_sstore = context
                .journal_mut()
                .sstore(WETH_ADDRESS, allowance_slot_val, new_allowance)
                .map_err(|e| PrecompileError::Other(format!("Storage write failed: {:?}", e)))?;

            gas_used += gas_sstore(
                allowance_sstore.is_cold,
                allowance_sstore.data.present_value,
                allowance_sstore.data.new_value,
            );
        }
    }

    // Read sender balance
    let from_slot = balance_slot(from);
    let from_sload = context
        .journal_mut()
        .sload(WETH_ADDRESS, from_slot)
        .map_err(|e| PrecompileError::Other(format!("Storage read failed: {:?}", e)))?;

    gas_used += gas_sload(from_sload.is_cold);
    let from_balance = from_sload.data;

    // Check sufficient balance
    if from_balance < amount {
        return Err(PrecompileError::Other(
            "Insufficient WETH balance".to_string(),
        ));
    }

    // Update sender balance
    let new_from_balance = from_balance - amount;
    let from_sstore = context
        .journal_mut()
        .sstore(WETH_ADDRESS, from_slot, new_from_balance)
        .map_err(|e| PrecompileError::Other(format!("Storage write failed: {:?}", e)))?;

    gas_used += gas_sstore(
        from_sstore.is_cold,
        from_sstore.data.present_value,
        from_sstore.data.new_value,
    );

    // Update receiver balance
    let to_slot = balance_slot(to);
    let to_sload = context
        .journal_mut()
        .sload(WETH_ADDRESS, to_slot)
        .map_err(|e| PrecompileError::Other(format!("Storage read failed: {:?}", e)))?;

    gas_used += gas_sload(to_sload.is_cold);
    let to_balance = to_sload.data;

    let new_to_balance = to_balance.saturating_add(amount);
    let to_sstore = context
        .journal_mut()
        .sstore(WETH_ADDRESS, to_slot, new_to_balance)
        .map_err(|e| PrecompileError::Other(format!("Storage write failed: {:?}", e)))?;

    gas_used += gas_sstore(
        to_sstore.is_cold,
        to_sstore.data.present_value,
        to_sstore.data.new_value,
    );

    // Check gas before emitting event
    if gas_limit < gas_used + GAS_LOG3 {
        return Err(PrecompileError::OutOfGas);
    }

    // Emit Transfer event
    let mut event_data = vec![0u8; 32];
    amount
        .to_be_bytes_vec()
        .iter()
        .rev()
        .enumerate()
        .for_each(|(i, &b)| {
            event_data[31 - i] = b;
        });

    let log = Log {
        address: WETH_ADDRESS,
        data: LogData::new(
            vec![TRANSFER_EVENT.into(), from.into_word(), to.into_word()],
            Bytes::from(event_data),
        )
        .expect("Valid log data"),
    };

    context.journal_mut().log(log);

    // Add event gas
    gas_used += GAS_LOG3;

    // Return true (success)
    let mut output = vec![0u8; 32];
    output[31] = 1;
    Ok(PrecompileOutput::new(gas_used, Bytes::from(output)))
}

fn handle_allowance<CTX: ContextTr>(
    context: &mut CTX,
    owner: Address,
    spender: Address,
    gas_limit: u64,
) -> PrecompileResult {
    // Ensure WETH account is loaded
    let _ = context
        .journal_mut()
        .load_account(WETH_ADDRESS)
        .map_err(|e| PrecompileError::Other(format!("Account load failed: {:?}", e)))?;

    // Read allowance with warm/cold tracking
    let slot = allowance_slot(owner, spender);
    let sload_result = context
        .journal_mut()
        .sload(WETH_ADDRESS, slot)
        .map_err(|e| PrecompileError::Other(format!("Storage read failed: {:?}", e)))?;

    let gas_used = gas_sload(sload_result.is_cold);

    if gas_limit < gas_used {
        return Err(PrecompileError::OutOfGas);
    }

    let allowance = sload_result.data;

    let mut output = vec![0u8; 32];
    allowance
        .to_be_bytes_vec()
        .iter()
        .rev()
        .enumerate()
        .for_each(|(i, &b)| {
            output[31 - i] = b;
        });

    Ok(PrecompileOutput::new(gas_used, Bytes::from(output)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{U256, address};
    use op_revm::{DefaultOp, OpBuilder};
    use revm::{
        Context,
        context::{BlockEnv, CfgEnv},
        database::{CacheDB, EmptyDB},
        precompile::PrecompileError,
    };

    // Helper addresses for testing
    const ALICE: Address = address!("1111111111111111111111111111111111111111");
    const BOB: Address = address!("2222222222222222222222222222222222222222");

    #[test]
    fn test_deposit_basic() {
        let mut context = create_test_context();
        let deposit_amount = U256::from(1000u64);

        // Deposit
        let result = run_weth_precompile(
            &mut context,
            &DEPOSIT_SELECTOR,
            100_000,
            deposit_amount,
            ALICE,
            false,
        );

        assert!(result.is_ok());
        let output = result.unwrap();

        // First deposit: SLOAD cold (2100) warms slot, then SSTORE warm (20000) + LOG2 (1381) = 23,481
        assert_eq!(output.gas_used, 23_481);

        // Check balance increased
        let balance = get_weth_balance(&mut context, ALICE);
        assert_eq!(balance, deposit_amount);
    }

    #[test]
    fn test_deposit_zero() {
        let mut context = create_test_context();

        let result = run_weth_precompile(
            &mut context,
            &DEPOSIT_SELECTOR,
            100_000,
            U256::ZERO,
            ALICE,
            false,
        );

        assert!(result.is_ok());
        let balance = get_weth_balance(&mut context, ALICE);
        assert_eq!(balance, U256::ZERO);
    }

    #[test]
    fn test_deposit_multiple_times() {
        let mut context = create_test_context();

        // First deposit
        run_weth_precompile(
            &mut context,
            &DEPOSIT_SELECTOR,
            100_000,
            U256::from(100u64),
            ALICE,
            false,
        )
        .unwrap();

        // Second deposit
        run_weth_precompile(
            &mut context,
            &DEPOSIT_SELECTOR,
            100_000,
            U256::from(200u64),
            ALICE,
            false,
        )
        .unwrap();

        let balance = get_weth_balance(&mut context, ALICE);
        assert_eq!(balance, U256::from(300u64));
    }

    #[test]
    fn test_withdraw_basic() {
        let mut context = create_test_context();

        // Fund WETH contract with ETH (simulating deposits)
        context
            .journal_mut()
            .balance_incr(WETH_ADDRESS, U256::from(1000u64))
            .unwrap();

        // Deposit first
        run_weth_precompile(
            &mut context,
            &DEPOSIT_SELECTOR,
            100_000,
            U256::from(1000u64),
            ALICE,
            false,
        )
        .unwrap();

        // Prepare withdraw input
        let mut input = WITHDRAW_SELECTOR.to_vec();
        input.extend_from_slice(&[0u8; 32]); // padding
        let withdraw_amount = U256::from(400u64);
        input[4..36].copy_from_slice(&withdraw_amount.to_be_bytes::<32>());

        // Withdraw
        let result = run_weth_precompile(&mut context, &input, 100_000, U256::ZERO, ALICE, false);

        assert!(result.is_ok());

        // Check balance decreased
        let balance = get_weth_balance(&mut context, ALICE);
        assert_eq!(balance, U256::from(600u64));
    }

    #[test]
    fn test_withdraw_insufficient_balance() {
        let mut context = create_test_context();

        // Deposit only 100
        run_weth_precompile(
            &mut context,
            &DEPOSIT_SELECTOR,
            100_000,
            U256::from(100u64),
            ALICE,
            false,
        )
        .unwrap();

        // Try to withdraw 200
        let mut input = WITHDRAW_SELECTOR.to_vec();
        input.extend_from_slice(&[0u8; 32]);
        let withdraw_amount = U256::from(200u64);
        input[4..36].copy_from_slice(&withdraw_amount.to_be_bytes::<32>());

        let result = run_weth_precompile(&mut context, &input, 100_000, U256::ZERO, ALICE, false);

        assert!(result.is_err());
    }

    #[test]
    fn test_transfer_basic() {
        let mut context = create_test_context();

        // Alice deposits
        run_weth_precompile(
            &mut context,
            &DEPOSIT_SELECTOR,
            100_000,
            U256::from(1000u64),
            ALICE,
            false,
        )
        .unwrap();

        // Alice transfers to Bob
        let mut input = TRANSFER_SELECTOR.to_vec();
        input.extend_from_slice(&[0u8; 12]); // padding for address
        input.extend_from_slice(BOB.as_slice());
        input.extend_from_slice(&[0u8; 32]); // amount slot
        let transfer_amount = U256::from(300u64);
        input[36..68].copy_from_slice(&transfer_amount.to_be_bytes::<32>());

        let result = run_weth_precompile(&mut context, &input, 100_000, U256::ZERO, ALICE, false);

        assert!(result.is_ok());

        // Check balances
        let alice_balance = get_weth_balance(&mut context, ALICE);
        let bob_balance = get_weth_balance(&mut context, BOB);
        assert_eq!(alice_balance, U256::from(700u64));
        assert_eq!(bob_balance, U256::from(300u64));
    }

    #[test]
    fn test_transfer_to_self() {
        let mut context = create_test_context();

        // Alice deposits
        run_weth_precompile(
            &mut context,
            &DEPOSIT_SELECTOR,
            100_000,
            U256::from(1000u64),
            ALICE,
            false,
        )
        .unwrap();

        // Alice transfers to herself
        let mut input = TRANSFER_SELECTOR.to_vec();
        input.extend_from_slice(&[0u8; 12]);
        input.extend_from_slice(ALICE.as_slice());
        input.extend_from_slice(&[0u8; 32]);
        let transfer_amount = U256::from(300u64);
        input[36..68].copy_from_slice(&transfer_amount.to_be_bytes::<32>());

        let result = run_weth_precompile(&mut context, &input, 100_000, U256::ZERO, ALICE, false);

        assert!(result.is_ok());

        // Balance should remain the same
        let balance = get_weth_balance(&mut context, ALICE);
        assert_eq!(balance, U256::from(1000u64));
    }

    #[test]
    fn test_transfer_insufficient_balance() {
        let mut context = create_test_context();

        // Alice deposits only 100
        run_weth_precompile(
            &mut context,
            &DEPOSIT_SELECTOR,
            100_000,
            U256::from(100u64),
            ALICE,
            false,
        )
        .unwrap();

        // Alice tries to transfer 200
        let mut input = TRANSFER_SELECTOR.to_vec();
        input.extend_from_slice(&[0u8; 12]);
        input.extend_from_slice(BOB.as_slice());
        input.extend_from_slice(&[0u8; 32]);
        let transfer_amount = U256::from(200u64);
        input[36..68].copy_from_slice(&transfer_amount.to_be_bytes::<32>());

        let result = run_weth_precompile(&mut context, &input, 100_000, U256::ZERO, ALICE, false);

        assert!(result.is_err());
    }

    #[test]
    fn test_approve_basic() {
        let mut context = create_test_context();

        // Alice approves Bob
        let mut input = APPROVE_SELECTOR.to_vec();
        input.extend_from_slice(&[0u8; 12]);
        input.extend_from_slice(BOB.as_slice());
        input.extend_from_slice(&[0u8; 32]);
        let approve_amount = U256::from(500u64);
        input[36..68].copy_from_slice(&approve_amount.to_be_bytes::<32>());

        let result = run_weth_precompile(&mut context, &input, 100_000, U256::ZERO, ALICE, false);

        assert!(result.is_ok());

        // Check allowance
        let allowance = get_allowance(&mut context, ALICE, BOB);
        assert_eq!(allowance, approve_amount);
    }

    #[test]
    fn test_approve_overwrite() {
        let mut context = create_test_context();

        // First approval
        let mut input = APPROVE_SELECTOR.to_vec();
        input.extend_from_slice(&[0u8; 12]);
        input.extend_from_slice(BOB.as_slice());
        input.extend_from_slice(&[0u8; 32]);
        let first_amount = U256::from(500u64);
        input[36..68].copy_from_slice(&first_amount.to_be_bytes::<32>());
        run_weth_precompile(&mut context, &input, 100_000, U256::ZERO, ALICE, false).unwrap();

        // Second approval (overwrite)
        let second_amount = U256::from(1000u64);
        input[36..68].copy_from_slice(&second_amount.to_be_bytes::<32>());
        run_weth_precompile(&mut context, &input, 100_000, U256::ZERO, ALICE, false).unwrap();

        // Check allowance
        let allowance = get_allowance(&mut context, ALICE, BOB);
        assert_eq!(allowance, second_amount);
    }

    #[test]
    fn test_transfer_from_with_allowance() {
        let mut context = create_test_context();

        // Alice deposits
        run_weth_precompile(
            &mut context,
            &DEPOSIT_SELECTOR,
            100_000,
            U256::from(1000u64),
            ALICE,
            false,
        )
        .unwrap();

        // Alice approves Bob
        let mut approve_input = APPROVE_SELECTOR.to_vec();
        approve_input.extend_from_slice(&[0u8; 12]);
        approve_input.extend_from_slice(BOB.as_slice());
        approve_input.extend_from_slice(&[0u8; 32]);
        let approve_amount = U256::from(500u64);
        approve_input[36..68].copy_from_slice(&approve_amount.to_be_bytes::<32>());
        run_weth_precompile(
            &mut context,
            &approve_input,
            100_000,
            U256::ZERO,
            ALICE,
            false,
        )
        .unwrap();

        // Bob transfers from Alice to himself
        let mut transfer_input = TRANSFER_FROM_SELECTOR.to_vec();
        transfer_input.extend_from_slice(&[0u8; 12]);
        transfer_input.extend_from_slice(ALICE.as_slice()); // from
        transfer_input.extend_from_slice(&[0u8; 12]);
        transfer_input.extend_from_slice(BOB.as_slice()); // to
        transfer_input.extend_from_slice(&[0u8; 32]);
        let transfer_amount = U256::from(300u64);
        transfer_input[68..100].copy_from_slice(&transfer_amount.to_be_bytes::<32>());

        let result = run_weth_precompile(
            &mut context,
            &transfer_input,
            100_000,
            U256::ZERO,
            BOB,
            false,
        );

        assert!(result.is_ok());

        // Check balances
        let alice_balance = get_weth_balance(&mut context, ALICE);
        let bob_balance = get_weth_balance(&mut context, BOB);
        assert_eq!(alice_balance, U256::from(700u64));
        assert_eq!(bob_balance, U256::from(300u64));

        // Check allowance decreased
        let remaining_allowance = get_allowance(&mut context, ALICE, BOB);
        assert_eq!(remaining_allowance, U256::from(200u64));
    }

    #[test]
    fn test_transfer_from_without_allowance() {
        let mut context = create_test_context();

        // Alice deposits
        run_weth_precompile(
            &mut context,
            &DEPOSIT_SELECTOR,
            100_000,
            U256::from(1000u64),
            ALICE,
            false,
        )
        .unwrap();

        // Bob tries to transfer from Alice without approval
        let mut input = TRANSFER_FROM_SELECTOR.to_vec();
        input.extend_from_slice(&[0u8; 12]);
        input.extend_from_slice(ALICE.as_slice());
        input.extend_from_slice(&[0u8; 12]);
        input.extend_from_slice(BOB.as_slice());
        input.extend_from_slice(&[0u8; 32]);
        let transfer_amount = U256::from(300u64);
        input[68..100].copy_from_slice(&transfer_amount.to_be_bytes::<32>());

        let result = run_weth_precompile(&mut context, &input, 100_000, U256::ZERO, BOB, false);

        assert!(result.is_err());
    }

    #[test]
    fn test_transfer_from_infinite_allowance() {
        let mut context = create_test_context();

        // Alice deposits
        run_weth_precompile(
            &mut context,
            &DEPOSIT_SELECTOR,
            100_000,
            U256::from(1000u64),
            ALICE,
            false,
        )
        .unwrap();

        // Alice approves Bob with uint256::MAX
        let mut approve_input = APPROVE_SELECTOR.to_vec();
        approve_input.extend_from_slice(&[0u8; 12]);
        approve_input.extend_from_slice(BOB.as_slice());
        approve_input.extend_from_slice(&[0xFFu8; 32]); // MAX
        run_weth_precompile(
            &mut context,
            &approve_input,
            100_000,
            U256::ZERO,
            ALICE,
            false,
        )
        .unwrap();

        // Bob transfers from Alice
        let mut transfer_input = TRANSFER_FROM_SELECTOR.to_vec();
        transfer_input.extend_from_slice(&[0u8; 12]);
        transfer_input.extend_from_slice(ALICE.as_slice());
        transfer_input.extend_from_slice(&[0u8; 12]);
        transfer_input.extend_from_slice(BOB.as_slice());
        transfer_input.extend_from_slice(&[0u8; 32]);
        let transfer_amount = U256::from(300u64);
        transfer_input[68..100].copy_from_slice(&transfer_amount.to_be_bytes::<32>());

        run_weth_precompile(
            &mut context,
            &transfer_input,
            100_000,
            U256::ZERO,
            BOB,
            false,
        )
        .unwrap();

        // Check allowance remains MAX
        let allowance = get_allowance(&mut context, ALICE, BOB);
        assert_eq!(allowance, U256::MAX);
    }

    #[test]
    fn test_balance_of() {
        let mut context = create_test_context();

        // Alice deposits
        run_weth_precompile(
            &mut context,
            &DEPOSIT_SELECTOR,
            100_000,
            U256::from(1234u64),
            ALICE,
            false,
        )
        .unwrap();

        // Query balance
        let mut input = BALANCE_OF_SELECTOR.to_vec();
        input.extend_from_slice(&[0u8; 12]);
        input.extend_from_slice(ALICE.as_slice());

        let result = run_weth_precompile(&mut context, &input, 100_000, U256::ZERO, ALICE, false);

        assert!(result.is_ok());
        let output = result.unwrap();

        let balance = U256::from_be_slice(&output.bytes);
        assert_eq!(balance, U256::from(1234u64));
    }

    #[test]
    fn test_gas_deposit_cold() {
        let mut context = create_test_context();
        let deposit_amount = U256::from(1000u64);

        let result = run_weth_precompile(
            &mut context,
            &DEPOSIT_SELECTOR,
            100_000,
            deposit_amount,
            ALICE,
            false,
        );

        assert!(result.is_ok());
        let output = result.unwrap();

        // First deposit: SLOAD cold (2100) warms the slot, then SSTORE warm (20000) + LOG2 (1381)
        // = 2100 + 20000 + 1381 = 23,481 gas
        // Note: SLOAD warms the slot, so SSTORE doesn't pay the cold penalty
        assert_eq!(output.gas_used, 23_481);
    }

    #[test]
    fn test_gas_approve_cold() {
        let mut context = create_test_context();

        let mut input = APPROVE_SELECTOR.to_vec();
        input.extend_from_slice(&[0u8; 12]);
        input.extend_from_slice(BOB.as_slice());
        input.extend_from_slice(&[0u8; 32]);
        let approve_amount = U256::from(500u64);
        input[36..68].copy_from_slice(&approve_amount.to_be_bytes::<32>());

        let result = run_weth_precompile(&mut context, &input, 100_000, U256::ZERO, ALICE, false);

        assert!(result.is_ok());
        let output = result.unwrap();

        // Cold approve: SSTORE (0→non-zero) cold = 20000 + 2100 + LOG3 (1750) = 23,850 gas
        assert_eq!(output.gas_used, 23_850);
    }

    #[test]
    fn test_gas_transfer_warm() {
        let mut context = create_test_context();

        // Alice deposits first (warms Alice's balance slot)
        run_weth_precompile(
            &mut context,
            &DEPOSIT_SELECTOR,
            100_000,
            U256::from(1000u64),
            ALICE,
            false,
        )
        .unwrap();

        // Alice transfers to Bob
        let mut input = TRANSFER_SELECTOR.to_vec();
        input.extend_from_slice(&[0u8; 12]);
        input.extend_from_slice(BOB.as_slice());
        input.extend_from_slice(&[0u8; 32]);
        let transfer_amount = U256::from(300u64);
        input[36..68].copy_from_slice(&transfer_amount.to_be_bytes::<32>());

        let result = run_weth_precompile(&mut context, &input, 100_000, U256::ZERO, ALICE, false);

        assert!(result.is_ok());
        let output = result.unwrap();

        // Transfer with Alice warm, Bob cold:
        // SLOAD warm (100) + SSTORE warm (2900) + SLOAD cold (2100 - warms Bob) + SSTORE warm (20000) + LOG3 (1750)
        // = 100 + 2900 + 2100 + 20000 + 1750 = 26,850 gas
        assert_eq!(output.gas_used, 26_850);
    }

    #[test]
    fn test_gas_balance_of_warm() {
        let mut context = create_test_context();

        // Alice deposits (warms her balance slot)
        run_weth_precompile(
            &mut context,
            &DEPOSIT_SELECTOR,
            100_000,
            U256::from(1234u64),
            ALICE,
            false,
        )
        .unwrap();

        // Query balance (warm access)
        let mut input = BALANCE_OF_SELECTOR.to_vec();
        input.extend_from_slice(&[0u8; 12]);
        input.extend_from_slice(ALICE.as_slice());

        let result = run_weth_precompile(&mut context, &input, 100_000, U256::ZERO, ALICE, false);

        assert!(result.is_ok());
        let output = result.unwrap();

        // Warm SLOAD = 100 gas
        assert_eq!(output.gas_used, 100);
    }

    #[test]
    fn test_gas_insufficient() {
        let mut context = create_test_context();

        // Try to deposit with insufficient gas
        let result = run_weth_precompile(
            &mut context,
            &DEPOSIT_SELECTOR,
            100, // Very low gas
            U256::from(1000u64),
            ALICE,
            false,
        );

        assert!(result.is_err());
        assert!(matches!(result, Err(PrecompileError::OutOfGas)));
    }

    #[test]
    fn test_gas_warm_vs_cold_deposit() {
        // This test verifies that warm storage access costs less than cold
        let mut context = create_test_context();

        // First deposit to Alice's balance (COLD access)
        let result1 = run_weth_precompile(
            &mut context,
            &DEPOSIT_SELECTOR,
            100_000,
            U256::from(1000u64),
            ALICE,
            false,
        );
        assert!(result1.is_ok());
        let gas_cold = result1.unwrap().gas_used;

        // Cold: SLOAD cold (2100) warms slot, then SSTORE warm (20000) + LOG2 (1381) = 23,481
        assert_eq!(gas_cold, 23_481);

        // Second deposit to Alice's same balance slot (WARM access)
        let result2 = run_weth_precompile(
            &mut context,
            &DEPOSIT_SELECTOR,
            100_000,
            U256::from(500u64),
            ALICE,
            false,
        );
        assert!(result2.is_ok());
        let gas_warm = result2.unwrap().gas_used;

        // Warm: SLOAD warm (100) + SSTORE warm (2900) + LOG2 (1381) = 4,381
        assert_eq!(gas_warm, 4_381);

        // Warm access should be MUCH cheaper than cold
        assert!(gas_warm < gas_cold);

        // Verify balance is correct
        let balance = get_weth_balance(&mut context, ALICE);
        assert_eq!(balance, U256::from(1500u64));
    }

    #[test]
    fn test_gas_warm_vs_cold_balance_read() {
        // This test verifies warm reads cost less than cold reads
        let mut context = create_test_context();

        // Deposit to Alice (warms her balance slot)
        run_weth_precompile(
            &mut context,
            &DEPOSIT_SELECTOR,
            100_000,
            U256::from(1000u64),
            ALICE,
            false,
        )
        .unwrap();

        // First balance read (warm - already accessed during deposit)
        let mut input = BALANCE_OF_SELECTOR.to_vec();
        input.extend_from_slice(&[0u8; 12]);
        input.extend_from_slice(ALICE.as_slice());

        let result1 = run_weth_precompile(&mut context, &input, 100_000, U256::ZERO, ALICE, false);
        assert!(result1.is_ok());
        let gas_warm = result1.unwrap().gas_used;

        // Warm SLOAD = 100 gas
        assert_eq!(gas_warm, 100);

        // Read Bob's balance (cold - never accessed before)
        let mut bob_input = BALANCE_OF_SELECTOR.to_vec();
        bob_input.extend_from_slice(&[0u8; 12]);
        bob_input.extend_from_slice(BOB.as_slice());

        let result2 =
            run_weth_precompile(&mut context, &bob_input, 100_000, U256::ZERO, ALICE, false);
        assert!(result2.is_ok());
        let gas_cold = result2.unwrap().gas_used;

        // Cold SLOAD = 2100 gas
        assert_eq!(gas_cold, 2_100);

        // Cold access should cost more
        assert!(gas_cold > gas_warm);
    }

    #[test]
    fn test_precompile_warms_slots() {
        // This test verifies that the precompile properly warms storage slots
        // according to EIP-2929, so subsequent accesses are cheaper
        let mut context = create_test_context();

        // Deposit to Alice - this should warm her balance slot
        let deposit1 = run_weth_precompile(
            &mut context,
            &DEPOSIT_SELECTOR,
            100_000,
            U256::from(1000u64),
            ALICE,
            false,
        );
        assert!(deposit1.is_ok());

        // First deposit includes cold SLOAD: 2100 + 20000 + 1381 = 23,481
        assert_eq!(deposit1.unwrap().gas_used, 23_481);

        // Second deposit to same address should be much cheaper (warm access)
        let deposit2 = run_weth_precompile(
            &mut context,
            &DEPOSIT_SELECTOR,
            100_000,
            U256::from(500u64),
            ALICE,
            false,
        );
        assert!(deposit2.is_ok());

        // Second deposit is all warm: 100 + 2900 + 1381 = 4,381
        assert_eq!(deposit2.unwrap().gas_used, 4_381);

        // Balance query should also be warm
        let mut input = BALANCE_OF_SELECTOR.to_vec();
        input.extend_from_slice(&[0u8; 12]);
        input.extend_from_slice(ALICE.as_slice());

        let balance_query =
            run_weth_precompile(&mut context, &input, 100_000, U256::ZERO, ALICE, false);
        assert!(balance_query.is_ok());

        // Warm SLOAD: 100 gas
        assert_eq!(balance_query.unwrap().gas_used, 100);
    }

    #[test]
    fn test_transfer_warms_recipient_slot() {
        // Verify that a transfer properly warms the recipient's balance slot
        let mut context = create_test_context();

        // Alice deposits
        run_weth_precompile(
            &mut context,
            &DEPOSIT_SELECTOR,
            100_000,
            U256::from(1000u64),
            ALICE,
            false,
        )
        .unwrap();

        // Transfer from Alice to Bob (first time - Bob's slot is cold)
        let mut transfer_input = TRANSFER_SELECTOR.to_vec();
        transfer_input.extend_from_slice(&[0u8; 12]);
        transfer_input.extend_from_slice(BOB.as_slice());
        transfer_input.extend_from_slice(&[0u8; 32]);
        let transfer_amount = U256::from(300u64);
        transfer_input[36..68].copy_from_slice(&transfer_amount.to_be_bytes::<32>());

        let transfer1 = run_weth_precompile(
            &mut context,
            &transfer_input,
            100_000,
            U256::ZERO,
            ALICE,
            false,
        );
        assert!(transfer1.is_ok());

        // Alice warm, Bob cold: 100 + 2900 + 2100 + 20000 + 1750 = 26,850
        assert_eq!(transfer1.unwrap().gas_used, 26_850);

        // Query Bob's balance (should be warm now)
        let mut bob_balance_input = BALANCE_OF_SELECTOR.to_vec();
        bob_balance_input.extend_from_slice(&[0u8; 12]);
        bob_balance_input.extend_from_slice(BOB.as_slice());

        let bob_query = run_weth_precompile(
            &mut context,
            &bob_balance_input,
            100_000,
            U256::ZERO,
            BOB,
            false,
        );
        assert!(bob_query.is_ok());

        // Bob's slot is warm: 100 gas
        assert_eq!(bob_query.unwrap().gas_used, 100);
    }

    #[test]
    fn test_approve_warms_allowance_slot() {
        // Verify that approve properly warms the allowance slot
        let mut context = create_test_context();

        // Alice approves Bob
        let mut approve_input = APPROVE_SELECTOR.to_vec();
        approve_input.extend_from_slice(&[0u8; 12]);
        approve_input.extend_from_slice(BOB.as_slice());
        approve_input.extend_from_slice(&[0u8; 32]);
        let approve_amount = U256::from(500u64);
        approve_input[36..68].copy_from_slice(&approve_amount.to_be_bytes::<32>());

        let approve1 = run_weth_precompile(
            &mut context,
            &approve_input,
            100_000,
            U256::ZERO,
            ALICE,
            false,
        );
        assert!(approve1.is_ok());

        // Cold SSTORE: 20000 + 2100 + 1750 = 23,850
        assert_eq!(approve1.unwrap().gas_used, 23_850);

        // Query allowance (should be warm)
        let mut allowance_input = ALLOWANCE_SELECTOR.to_vec();
        allowance_input.extend_from_slice(&[0u8; 12]);
        allowance_input.extend_from_slice(ALICE.as_slice());
        allowance_input.extend_from_slice(&[0u8; 12]);
        allowance_input.extend_from_slice(BOB.as_slice());

        let allowance_query = run_weth_precompile(
            &mut context,
            &allowance_input,
            100_000,
            U256::ZERO,
            ALICE,
            false,
        );
        assert!(allowance_query.is_ok());

        // Warm SLOAD: 100 gas
        assert_eq!(allowance_query.unwrap().gas_used, 100);
    }

    #[test]
    fn test_allowance_query() {
        let mut context = create_test_context();

        // Alice approves Bob
        let mut approve_input = APPROVE_SELECTOR.to_vec();
        approve_input.extend_from_slice(&[0u8; 12]);
        approve_input.extend_from_slice(BOB.as_slice());
        approve_input.extend_from_slice(&[0u8; 32]);
        let approve_amount = U256::from(777u64);
        approve_input[36..68].copy_from_slice(&approve_amount.to_be_bytes::<32>());
        run_weth_precompile(
            &mut context,
            &approve_input,
            100_000,
            U256::ZERO,
            ALICE,
            false,
        )
        .unwrap();

        // Query allowance
        let mut input = ALLOWANCE_SELECTOR.to_vec();
        input.extend_from_slice(&[0u8; 12]);
        input.extend_from_slice(ALICE.as_slice());
        input.extend_from_slice(&[0u8; 12]);
        input.extend_from_slice(BOB.as_slice());

        let result = run_weth_precompile(&mut context, &input, 100_000, U256::ZERO, ALICE, false);

        assert!(result.is_ok());
        let output = result.unwrap();

        let allowance = U256::from_be_slice(&output.bytes);
        assert_eq!(allowance, approve_amount);
    }

    // Helper functions
    fn create_test_context() -> op_revm::OpContext<CacheDB<EmptyDB>> {
        let evm = Context::op()
            .with_db(CacheDB::new(EmptyDB::new()))
            .with_block(BlockEnv::default())
            .with_cfg(CfgEnv::default())
            .build_op();
        evm.0.ctx // Extract the context from OpEvm(Evm{ ctx, ... })
    }

    fn get_weth_balance<CTX: ContextTr>(context: &mut CTX, addr: Address) -> U256 {
        // Ensure WETH account exists
        let _ = context.journal_mut().load_account(WETH_ADDRESS);

        let slot = balance_slot(addr);
        context
            .journal_mut()
            .sload(WETH_ADDRESS, slot)
            .unwrap()
            .data
    }

    fn get_allowance<CTX: ContextTr>(context: &mut CTX, owner: Address, spender: Address) -> U256 {
        // Ensure WETH account exists
        let _ = context.journal_mut().load_account(WETH_ADDRESS);

        let slot = allowance_slot(owner, spender);
        context
            .journal_mut()
            .sload(WETH_ADDRESS, slot)
            .unwrap()
            .data
    }
}
