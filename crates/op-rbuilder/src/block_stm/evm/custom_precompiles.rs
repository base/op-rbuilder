//! Custom precompile provider that extends OP Stack precompiles with WETH precompile.

use super::weth_precompile::{WETH_ADDRESS, run_weth_precompile};
use alloy_primitives::Address;
use op_revm::{OpSpecId, precompiles::OpPrecompiles};
use revm::{
    context_interface::{Cfg, ContextTr, JournalTr, LocalContextTr, Transaction},
    handler::PrecompileProvider,
    interpreter::{CallInputs, Gas, InstructionResult, InterpreterResult},
    primitives::Bytes,
};
use std::{boxed::Box, string::String};

/// Custom precompile provider that includes WETH precompile alongside OP Stack precompiles.
#[derive(Debug, Clone)]
pub struct OpCustomPrecompiles {
    inner: OpPrecompiles,
    spec: OpSpecId,
}

impl OpCustomPrecompiles {
    pub fn new_with_spec(spec: OpSpecId) -> Self {
        Self {
            inner: OpPrecompiles::new_with_spec(spec),
            spec,
        }
    }
}

impl Default for OpCustomPrecompiles {
    fn default() -> Self {
        Self::new_with_spec(OpSpecId::REGOLITH)
    }
}

impl<CTX> PrecompileProvider<CTX> for OpCustomPrecompiles
where
    CTX: ContextTr<Cfg: Cfg<Spec = OpSpecId>>,
{
    type Output = InterpreterResult;

    fn set_spec(&mut self, spec: <CTX::Cfg as Cfg>::Spec) -> bool {
        if spec == self.spec {
            return false;
        }
        self.spec = spec;
        // Update inner precompiles with new spec
        self.inner = OpPrecompiles::new_with_spec(spec);
        true
    }

    fn run(
        &mut self,
        context: &mut CTX,
        inputs: &CallInputs,
    ) -> Result<Option<Self::Output>, String> {
        // Check if this is the WETH precompile
        if inputs.bytecode_address == WETH_ADDRESS {
            return Ok(Some(run_weth_precompile_adapter(context, inputs)?));
        }

        // Otherwise, delegate to standard OP precompiles
        PrecompileProvider::<CTX>::run(&mut self.inner, context, inputs)
    }

    fn warm_addresses(&self) -> Box<impl Iterator<Item = Address>> {
        // Include WETH address along with standard precompiles
        let inner_addresses: Vec<Address> =
            PrecompileProvider::<CTX>::warm_addresses(&self.inner).collect();
        let mut addresses = vec![WETH_ADDRESS];
        addresses.extend(inner_addresses);
        Box::new(addresses.into_iter())
    }

    fn contains(&self, address: &Address) -> bool {
        *address == WETH_ADDRESS || PrecompileProvider::<CTX>::contains(&self.inner, address)
    }
}

/// Adapter function to convert between CallInputs and our WETH precompile interface
fn run_weth_precompile_adapter<CTX: ContextTr>(
    context: &mut CTX,
    inputs: &CallInputs,
) -> Result<InterpreterResult, String> {
    // Extract input bytes from CallInputs
    let input_bytes = match &inputs.input {
        revm::interpreter::CallInput::SharedBuffer(range) => {
            if let Some(slice) = context.local().shared_memory_buffer_slice(range.clone()) {
                slice.to_vec()
            } else {
                vec![]
            }
        }
        revm::interpreter::CallInput::Bytes(bytes) => bytes.0.to_vec(),
    };

    // Get caller and value
    let caller = context.tx().caller();
    let value = inputs.call_value();

    // Check if this is a static call
    let is_static = inputs.is_static;

    // Run the WETH precompile
    let result = run_weth_precompile(
        context,
        &input_bytes,
        inputs.gas_limit,
        value,
        caller,
        is_static,
    );

    // Convert PrecompileResult to InterpreterResult
    match result {
        Ok(output) => {
            let mut interpreter_result = InterpreterResult {
                result: InstructionResult::Return,
                gas: Gas::new(inputs.gas_limit),
                output: output.bytes,
            };
            let underflow = interpreter_result.gas.record_cost(output.gas_used);
            if !underflow {
                interpreter_result.result = InstructionResult::PrecompileOOG;
            }
            Ok(interpreter_result)
        }
        Err(e) => {
            // If this is a top-level precompile call and error is non-OOG, record the message
            if !e.is_oog() && context.journal().depth() == 1 {
                context
                    .local_mut()
                    .set_precompile_error_context(e.to_string());
            }
            Ok(InterpreterResult {
                result: if e.is_oog() {
                    InstructionResult::PrecompileOOG
                } else {
                    InstructionResult::PrecompileError
                },
                gas: Gas::new(inputs.gas_limit),
                output: Bytes::new(),
            })
        }
    }
}
