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

    /// 新 handle 分配前检查：candidate 槽位不得越过 handle 表（止于 shadow stack 基址）。
    pub(crate) fn emit_handle_table_alloc_check(
        func: &mut Function,
        obj_table_ptr_global: u32,
        shadow_stack_end_global: u32,
        candidate_local: u32,
    ) {
        func.instruction(&WasmInstruction::GlobalGet(obj_table_ptr_global));
        func.instruction(&WasmInstruction::LocalGet(candidate_local));
        func.instruction(&WasmInstruction::I32Const(4));
        func.instruction(&WasmInstruction::I32Mul);
        func.instruction(&WasmInstruction::I32Add);
        func.instruction(&WasmInstruction::I32Const(4));
        func.instruction(&WasmInstruction::I32Add);
        func.instruction(&WasmInstruction::GlobalGet(shadow_stack_end_global));
        func.instruction(&WasmInstruction::I32Const(crate::SHADOW_STACK_SIZE as i32));
        func.instruction(&WasmInstruction::I32Sub);
        func.instruction(&WasmInstruction::I32GtU);
        func.instruction(&WasmInstruction::If(BlockType::Empty));
        func.instruction(&WasmInstruction::Unreachable);
        func.instruction(&WasmInstruction::End);
    }

    pub(super) fn emit_property_name_id_match(
        &self,
        func: &mut Function,
        left_local: u32,
        right_local: u32,
    ) {
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

    /// 对象扩容路径：size = 16 + capacity_local * 32；与 obj_new 相同用 memory.size 比较，不足则 memory.grow 一次再检查，仍不足则 trap。
    pub(super) fn emit_heap_bump_for_object_resize(
        func: &mut Function,
        heap_global: u32,
        capacity_local: u32,
        size_scratch_local: u32,
    ) {
        // size_scratch = 16 + capacity * 32
        func.instruction(&WasmInstruction::LocalGet(capacity_local));
        func.instruction(&WasmInstruction::I32Const(32));
        func.instruction(&WasmInstruction::I32Mul);
        func.instruction(&WasmInstruction::I32Const(16));
        func.instruction(&WasmInstruction::I32Add);
        func.instruction(&WasmInstruction::LocalSet(size_scratch_local));

        func.instruction(&WasmInstruction::Block(BlockType::Empty));
        // new_end = heap_ptr + size；若 new_end <= mem_size 则直接 bump
        func.instruction(&WasmInstruction::GlobalGet(heap_global));
        func.instruction(&WasmInstruction::LocalGet(size_scratch_local));
        func.instruction(&WasmInstruction::I32Add);
        func.instruction(&WasmInstruction::MemorySize(0));
        func.instruction(&WasmInstruction::I64ExtendI32U);
        func.instruction(&WasmInstruction::I64Const(65536));
        func.instruction(&WasmInstruction::I64Mul);
        func.instruction(&WasmInstruction::I32WrapI64);
        func.instruction(&WasmInstruction::I32LeU);
        func.instruction(&WasmInstruction::If(BlockType::Empty));
        func.instruction(&WasmInstruction::GlobalGet(heap_global));
        func.instruction(&WasmInstruction::LocalGet(size_scratch_local));
        func.instruction(&WasmInstruction::I32Add);
        func.instruction(&WasmInstruction::GlobalSet(heap_global));
        func.instruction(&WasmInstruction::Br(1));
        func.instruction(&WasmInstruction::End);
        // memory.grow(ceil((new_end - mem_size) / 65536))
        func.instruction(&WasmInstruction::GlobalGet(heap_global));
        func.instruction(&WasmInstruction::LocalGet(size_scratch_local));
        func.instruction(&WasmInstruction::I32Add);
        func.instruction(&WasmInstruction::MemorySize(0));
        func.instruction(&WasmInstruction::I64ExtendI32U);
        func.instruction(&WasmInstruction::I64Const(65536));
        func.instruction(&WasmInstruction::I64Mul);
        func.instruction(&WasmInstruction::I32WrapI64);
        func.instruction(&WasmInstruction::I32Sub);
        func.instruction(&WasmInstruction::I32Const(65535));
        func.instruction(&WasmInstruction::I32Add);
        func.instruction(&WasmInstruction::I32Const(65536));
        func.instruction(&WasmInstruction::I32DivU);
        func.instruction(&WasmInstruction::MemoryGrow(0));
        func.instruction(&WasmInstruction::I32Const(-1));
        func.instruction(&WasmInstruction::I32Eq);
        func.instruction(&WasmInstruction::If(BlockType::Empty));
        func.instruction(&WasmInstruction::Unreachable);
        func.instruction(&WasmInstruction::End);
        // grow 后再次检查并 bump
        func.instruction(&WasmInstruction::GlobalGet(heap_global));
        func.instruction(&WasmInstruction::LocalGet(size_scratch_local));
        func.instruction(&WasmInstruction::I32Add);
        func.instruction(&WasmInstruction::MemorySize(0));
        func.instruction(&WasmInstruction::I64ExtendI32U);
        func.instruction(&WasmInstruction::I64Const(65536));
        func.instruction(&WasmInstruction::I64Mul);
        func.instruction(&WasmInstruction::I32WrapI64);
        func.instruction(&WasmInstruction::I32LeU);
        func.instruction(&WasmInstruction::If(BlockType::Empty));
        func.instruction(&WasmInstruction::GlobalGet(heap_global));
        func.instruction(&WasmInstruction::LocalGet(size_scratch_local));
        func.instruction(&WasmInstruction::I32Add);
        func.instruction(&WasmInstruction::GlobalSet(heap_global));
        func.instruction(&WasmInstruction::Br(1));
        func.instruction(&WasmInstruction::End);
        func.instruction(&WasmInstruction::Unreachable);
        func.instruction(&WasmInstruction::End);
    }
}
