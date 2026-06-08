use super::*;
use crate::host_import_registry::{HostImportGroup, host_import_specs};

impl Compiler {
    /// 将 Number.prototype 宿主导入登记到函数表（供 encode_function_idx 字面量快路径等）。
    pub(crate) fn compile_number_proto_wrappers(&mut self) {
        let number_proto_base = self.function_table.len() as u32;
        let mut offset: u32 = 0;
        for (import_idx, spec) in host_import_specs().iter().enumerate() {
            if spec.group != Some(HostImportGroup::NumberPrototypeMethod) {
                continue;
            }
            self.push_func_table(import_idx as u32);
            offset += 1;
        }
        self.number_proto_table_base = number_proto_base;
        self.number_proto_method_count = offset;
        self.number_proto_lookup.clear();

        for (name, off) in [
            ("toString", 0u32),
            ("valueOf", 1),
            ("toFixed", 2),
            ("toExponential", 3),
            ("toPrecision", 4),
        ] {
            if off >= offset {
                continue;
            }
            let name_id = self.intern_data_string(name);
            self.number_proto_lookup.push((name_id, off));
        }
    }

    fn emit_number_primitive_get_in_obj_get(
        &self,
        func: &mut Function,
        name_id: u32,
        table_off: u32,
    ) {
        let table_idx = self.number_proto_table_base + table_off;
        let encoded = value::encode_function_idx(table_idx);
        func.instruction(&WasmInstruction::Block(BlockType::Empty));
        func.instruction(&WasmInstruction::LocalGet(0));
        func.instruction(&WasmInstruction::I64Const(value::BOX_BASE as i64));
        func.instruction(&WasmInstruction::I64And);
        func.instruction(&WasmInstruction::I64Const(value::BOX_BASE as i64));
        func.instruction(&WasmInstruction::I64Ne);
        func.instruction(&WasmInstruction::LocalGet(1));
        func.instruction(&WasmInstruction::I32Const(name_id as i32));
        func.instruction(&WasmInstruction::I32Eq);
        func.instruction(&WasmInstruction::I32And);
        func.instruction(&WasmInstruction::If(BlockType::Empty));
        func.instruction(&WasmInstruction::I64Const(encoded));
        func.instruction(&WasmInstruction::Return);
        func.instruction(&WasmInstruction::End);
        func.instruction(&WasmInstruction::End);
    }

    pub(crate) fn emit_all_number_primitive_gets_in_obj_get(&self, func: &mut Function) {
        for &(name_id, off) in &self.number_proto_lookup {
            self.emit_number_primitive_get_in_obj_get(func, name_id, off);
        }
    }
}