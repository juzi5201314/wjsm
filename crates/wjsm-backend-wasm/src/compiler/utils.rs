use wasm_encoder::{BlockType, Function, Instruction as WasmInstruction};
use wjsm_ir::{Builtin, Function as IrFunction, value};

use super::state::{CompileMode, Compiler};
use super::cfg_analysis::max_instruction_value_id;

impl Compiler {
    pub(crate) fn local_idx(&self, val_id: u32) -> u32 {
        val_id + self.ssa_local_base
    }

    pub(crate) fn call_func_idx_scratch(&self) -> u32 {
        self.shadow_sp_scratch_idx + 1
    }

    pub(crate) fn call_env_obj_scratch(&self) -> u32 {
        self.string_concat_scratch_idx + 1
    }

    pub(crate) fn emit_resolve_callable_for_helper(
        &self,
        func: &mut Function,
        callee_local: u32,
        func_idx_local: u32,
        env_obj_local: u32,
    ) {
        func.instruction(&WasmInstruction::LocalGet(callee_local));
        func.instruction(&WasmInstruction::I64Const(32));
        func.instruction(&WasmInstruction::I64ShrU);
        func.instruction(&WasmInstruction::I64Const(value::TAG_MASK as i64));
        func.instruction(&WasmInstruction::I64And);
        func.instruction(&WasmInstruction::I64Const(value::TAG_CLOSURE as i64));
        func.instruction(&WasmInstruction::I64Eq);
        func.instruction(&WasmInstruction::If(BlockType::Empty));

        func.instruction(&WasmInstruction::LocalGet(callee_local));
        func.instruction(&WasmInstruction::I32WrapI64);
        func.instruction(&WasmInstruction::Call(self.closure_get_func_idx));
        func.instruction(&WasmInstruction::LocalSet(func_idx_local));
        func.instruction(&WasmInstruction::LocalGet(callee_local));
        func.instruction(&WasmInstruction::I32WrapI64);
        func.instruction(&WasmInstruction::Call(self.closure_get_env_idx));
        func.instruction(&WasmInstruction::LocalSet(env_obj_local));

        func.instruction(&WasmInstruction::Else);
        func.instruction(&WasmInstruction::LocalGet(callee_local));
        func.instruction(&WasmInstruction::I32WrapI64);
        func.instruction(&WasmInstruction::LocalSet(func_idx_local));
        func.instruction(&WasmInstruction::I64Const(value::encode_undefined()));
        func.instruction(&WasmInstruction::LocalSet(env_obj_local));
        func.instruction(&WasmInstruction::End);
    }

    pub(crate) fn required_local_count(&self, function: &IrFunction) -> u32 {
        let max_ssa = function
            .blocks()
            .iter()
            .flat_map(|block| block.instructions())
            .map(max_instruction_value_id)
            .max()
            .map_or(0, |max| max + 1);

        (max_ssa + self.ssa_local_base)
            .max(self.next_var_local)
            .max(self.phi_locals.values().copied().max().map_or(0, |m| m + 1))
    }

    pub(crate) fn emit_shadow_stack_overflow_check(&mut self, arg_count_bytes: i32) {
        self.emit(WasmInstruction::LocalGet(self.shadow_sp_scratch_idx));
        self.emit(WasmInstruction::I32Const(arg_count_bytes));
        self.emit(WasmInstruction::I32Add);
        self.emit(WasmInstruction::GlobalGet(self.shadow_stack_end_global_idx));
        self.emit(WasmInstruction::I32GtU);
        self.emit(WasmInstruction::If(BlockType::Empty));
        let func_idx = self
            .builtin_func_indices
            .get(&Builtin::AbortShadowStackOverflow)
            .copied()
            .unwrap_or(76);
        self.emit(WasmInstruction::LocalGet(self.shadow_sp_scratch_idx));
        self.emit(WasmInstruction::I32Const(arg_count_bytes));
        self.emit(WasmInstruction::GlobalGet(self.shadow_stack_end_global_idx));
        self.emit(WasmInstruction::Call(func_idx));
        self.emit(WasmInstruction::Unreachable);
        self.emit(WasmInstruction::End);
    }

    pub(crate) fn emit(&mut self, instruction: WasmInstruction<'_>) {
        self.current_func
            .as_mut()
            .expect("compiler function should be initialized before emission")
            .instruction(&instruction);
    }

    pub(crate) fn finish(mut self) -> Vec<u8> {
        self.module.section(&self.types);
        self.module.section(&self.imports);
        self.module.section(&self.functions);
        self.module.section(&self.table);
        if self.mode == CompileMode::Normal {
            self.module.section(&self.memory);
            self.module.section(&self.globals);
        }
        self.module.section(&self.exports);
        self.module.section(&self.elements);
        self.module.section(&self.codes);

        if !self.string_data.is_empty() {
            self.module.section(&self.data);
        }

        self.module.finish()
    }
}
