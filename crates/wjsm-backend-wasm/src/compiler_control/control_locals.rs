use super::*;

impl Compiler {
    pub(crate) fn emit_eval_var_frame_enter(&mut self) {
        let frame_bytes = (self.var_memory_offsets.len() as u32) * 8;
        if frame_bytes == 0 {
            return;
        }

        self.emit(WasmInstruction::GlobalGet(self.shadow_sp_global_idx));
        self.emit(WasmInstruction::LocalTee(self.eval_var_base_local_idx));
        self.emit(WasmInstruction::LocalSet(self.shadow_sp_scratch_idx));
        self.emit_shadow_stack_overflow_check(frame_bytes as i32);
        self.emit(WasmInstruction::GlobalGet(self.shadow_sp_global_idx));
        self.emit(WasmInstruction::I32Const(frame_bytes as i32));
        self.emit(WasmInstruction::I32Add);
        self.emit(WasmInstruction::GlobalSet(self.shadow_sp_global_idx));
    }

    pub(crate) fn emit_eval_var_frame_exit(&mut self) {
        if self.var_memory_offsets.is_empty() {
            return;
        }
        self.emit(WasmInstruction::LocalGet(self.eval_var_base_local_idx));
        self.emit(WasmInstruction::GlobalSet(self.shadow_sp_global_idx));
    }

    pub(crate) fn emit_eval_var_address(&mut self, offset: u32) {
        self.emit(WasmInstruction::LocalGet(self.eval_var_base_local_idx));
        if offset != 0 {
            self.emit(WasmInstruction::I32Const(offset as i32));
            self.emit(WasmInstruction::I32Add);
        }
    }

    pub(crate) fn emit_store_stacked_binding(
        &mut self,
        memory_offset: Option<u32>,
        local_idx: Option<u32>,
    ) {
        if let Some(offset) = memory_offset {
            self.emit(WasmInstruction::LocalSet(self.string_concat_scratch_idx));
            self.emit_eval_var_address(offset);
            self.emit(WasmInstruction::LocalGet(self.string_concat_scratch_idx));
            self.emit(WasmInstruction::I64Store(crate::shadow_mem_arg(0)));
        } else if let Some(local_idx) = local_idx {
            self.emit(WasmInstruction::LocalSet(local_idx));
        }
    }
}
