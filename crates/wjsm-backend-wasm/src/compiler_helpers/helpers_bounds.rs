use super::*;

impl Compiler {
    pub(super) fn emit_handle_bounds_check(
        func: &mut Function,
        obj_table_count_global: u32,
        handle_local: u32,
        sentinel: Option<i64>,
    ) {
        func.instruction(&WasmInstruction::GlobalGet(obj_table_count_global));
        func.instruction(&WasmInstruction::I32GeU);
        func.instruction(&WasmInstruction::If(BlockType::Empty));
        if let Some(val) = sentinel {
            func.instruction(&WasmInstruction::I64Const(val));
        }
        func.instruction(&WasmInstruction::Return);
        func.instruction(&WasmInstruction::End);
        func.instruction(&WasmInstruction::LocalGet(handle_local));
    }

    pub(super) fn emit_property_name_id_match(&self, func: &mut Function, left_local: u32, right_local: u32) {
        func.instruction(&WasmInstruction::LocalGet(left_local));
        func.instruction(&WasmInstruction::LocalGet(right_local));
        func.instruction(&WasmInstruction::I32Eq);
        func.instruction(&WasmInstruction::LocalGet(left_local));
        func.instruction(&WasmInstruction::I32Const(
            constants::NAME_ID_SYMBOL_FLAG as i32,
        ));
        func.instruction(&WasmInstruction::I32And);
        func.instruction(&WasmInstruction::LocalGet(right_local));
        func.instruction(&WasmInstruction::I32Const(
            constants::NAME_ID_SYMBOL_FLAG as i32,
        ));
        func.instruction(&WasmInstruction::I32And);
        func.instruction(&WasmInstruction::I32Or);
        func.instruction(&WasmInstruction::I32Eqz);
        func.instruction(&WasmInstruction::If(BlockType::Result(ValType::I32)));
        func.instruction(&WasmInstruction::LocalGet(left_local));
        func.instruction(&WasmInstruction::LocalGet(right_local));
        func.instruction(&WasmInstruction::Call(self.string_eq_func_idx));
        func.instruction(&WasmInstruction::Else);
        func.instruction(&WasmInstruction::I32Const(0));
        func.instruction(&WasmInstruction::End);
        func.instruction(&WasmInstruction::I32Or);
    }

}
