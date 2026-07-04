use super::*;
use crate::host_import_registry::SpecialHostImport;

impl Compiler {
    pub(crate) fn compile_array_helpers(&mut self) {
        let heap_global = self.heap_ptr_global_idx;
        let alloc_ptr_global = self.alloc_ptr_global_idx;
        let alloc_end_global = self.alloc_end_global_idx;
        let gc_alloc_bytes_global = self.gc_alloc_bytes_global_idx;
        let obj_table_global = self.obj_table_global_idx;
        let obj_table_count_global = self.obj_table_count_global_idx;
        let shadow_stack_end_global = self.shadow_stack_end_global_idx;
        let array_proto_global = self.array_proto_handle_global_idx;

        // ── $arr_new (param $capacity i32) (result i32) — Type 7 ──
        // v2：alloc_ptr/alloc_end 窗口 fast-path；失败走 gc_alloc_slow。
        //   数组内存布局: [proto(4), type(1), pad(3), length(4), capacity(4), elements(capacity*8)]
        {
            // local 0 = $capacity, local 1 = size, local 2 = ptr, local 3 = handle_idx, local 4 = new_end
            let locals: Vec<(u32, wasm_encoder::ValType)> = vec![(4, wasm_encoder::ValType::I32)];
            let mut func = Function::new(locals);
            let gc_alloc_slow_idx =
                self.special_host_import_indices[&SpecialHostImport::GcAllocSlow];
            let gc_take_freed_handle_idx =
                self.special_host_import_indices[&SpecialHostImport::GcTakeFreedHandle];

            // size = header + capacity * element_size
            func.instruction(&WasmInstruction::LocalGet(0));
            func.instruction(&WasmInstruction::I32Const(
                constants::HEAP_ARRAY_ELEMENT_SIZE as i32,
            ));
            func.instruction(&WasmInstruction::I32Mul);
            func.instruction(&WasmInstruction::I32Const(
                constants::HEAP_OBJECT_HEADER_SIZE as i32,
            ));
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::LocalSet(1));

            // ── handle 复用：take_or_alloc_handle ──
            func.instruction(&WasmInstruction::Call(gc_take_freed_handle_idx));
            func.instruction(&WasmInstruction::LocalTee(3));
            func.instruction(&WasmInstruction::I32Const(-1));
            func.instruction(&WasmInstruction::I32Ne);
            func.instruction(&WasmInstruction::If(BlockType::Empty));
            // 已取到 freed handle
            func.instruction(&WasmInstruction::Else);
            func.instruction(&WasmInstruction::GlobalGet(obj_table_count_global));
            func.instruction(&WasmInstruction::LocalTee(3));
            Self::emit_handle_table_alloc_check(
                &mut func,
                obj_table_global,
                shadow_stack_end_global,
                3,
            );
            func.instruction(&WasmInstruction::I32Const(1));
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::GlobalSet(obj_table_count_global));
            func.instruction(&WasmInstruction::End);

            // ── alloc window fast-path：alloc_ptr + size <= alloc_end ──
            func.instruction(&WasmInstruction::GlobalGet(alloc_ptr_global));
            func.instruction(&WasmInstruction::LocalGet(1));
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::GlobalGet(alloc_end_global));
            func.instruction(&WasmInstruction::I32LeU);
            func.instruction(&WasmInstruction::If(BlockType::Result(ValType::I32)));
            // fast-path：ptr = alloc_ptr; alloc_ptr/heap_ptr = ptr + size; gc_alloc_bytes += size
            func.instruction(&WasmInstruction::GlobalGet(alloc_ptr_global));
            func.instruction(&WasmInstruction::LocalTee(2));
            func.instruction(&WasmInstruction::LocalGet(1));
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::LocalTee(4));
            func.instruction(&WasmInstruction::GlobalSet(alloc_ptr_global));
            func.instruction(&WasmInstruction::LocalGet(4));
            func.instruction(&WasmInstruction::GlobalSet(heap_global));
            func.instruction(&WasmInstruction::GlobalGet(gc_alloc_bytes_global));
            func.instruction(&WasmInstruction::LocalGet(1));
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::GlobalSet(gc_alloc_bytes_global));
            func.instruction(&WasmInstruction::LocalGet(2));
            func.instruction(&WasmInstruction::Else);
            // slow-path：gc_alloc_slow(size, HEAP_TYPE_ARRAY, capacity)
            func.instruction(&WasmInstruction::LocalGet(1));
            func.instruction(&WasmInstruction::I32Const(wjsm_ir::HEAP_TYPE_ARRAY as i32));
            func.instruction(&WasmInstruction::LocalGet(0));
            func.instruction(&WasmInstruction::Call(gc_alloc_slow_idx));
            func.instruction(&WasmInstruction::LocalTee(2));
            func.instruction(&WasmInstruction::I32Const(-1));
            func.instruction(&WasmInstruction::I32Eq);
            func.instruction(&WasmInstruction::If(BlockType::Empty));
            func.instruction(&WasmInstruction::Unreachable);
            func.instruction(&WasmInstruction::End);
            func.instruction(&WasmInstruction::LocalGet(2));
            func.instruction(&WasmInstruction::End);
            func.instruction(&WasmInstruction::LocalSet(2));

            // ── 初始化数组 header ──
            // proto = array_proto_handle at offset 0
            func.instruction(&WasmInstruction::LocalGet(2));
            func.instruction(&WasmInstruction::GlobalGet(array_proto_global));
            func.instruction(&WasmInstruction::I32Store(MemArg {
                offset: constants::HEAP_OBJECT_PROTO_OFFSET as u64,
                align: 2,
                memory_index: 0,
            }));
            // type byte HEAP_TYPE_ARRAY at layout-defined offset
            func.instruction(&WasmInstruction::LocalGet(2));
            func.instruction(&WasmInstruction::I32Const(wjsm_ir::HEAP_TYPE_ARRAY as i32));
            func.instruction(&WasmInstruction::I32Store8(MemArg {
                offset: constants::HEAP_OBJECT_TYPE_OFFSET as u64,
                align: 0,
                memory_index: 0,
            }));
            // Zero pad bytes
            for off in
                constants::HEAP_OBJECT_HEADER_PAD_START..constants::HEAP_OBJECT_HEADER_PAD_END
            {
                func.instruction(&WasmInstruction::LocalGet(2));
                func.instruction(&WasmInstruction::I32Const(0));
                func.instruction(&WasmInstruction::I32Store8(MemArg {
                    offset: off as u64,
                    align: 0,
                    memory_index: 0,
                }));
            }
            // length = 0
            func.instruction(&WasmInstruction::LocalGet(2));
            func.instruction(&WasmInstruction::I32Const(0));
            func.instruction(&WasmInstruction::I32Store(MemArg {
                offset: constants::HEAP_ARRAY_LENGTH_OFFSET as u64,
                align: 2,
                memory_index: 0,
            }));
            // capacity = capacity (param 0)
            func.instruction(&WasmInstruction::LocalGet(2));
            func.instruction(&WasmInstruction::LocalGet(0));
            func.instruction(&WasmInstruction::I32Store(MemArg {
                offset: constants::HEAP_ARRAY_CAPACITY_OFFSET as u64,
                align: 2,
                memory_index: 0,
            }));

            // ── obj_table[handle_idx] = ptr ──
            func.instruction(&WasmInstruction::LocalGet(3));
            func.instruction(&WasmInstruction::I32Const(
                constants::HANDLE_TABLE_ENTRY_SIZE as i32,
            ));
            func.instruction(&WasmInstruction::I32Mul);
            func.instruction(&WasmInstruction::GlobalGet(obj_table_global));
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::LocalGet(2));
            func.instruction(&WasmInstruction::I32Store(MemArg {
                offset: 0,
                align: 2,
                memory_index: 0,
            }));

            // 返回 handle_idx
            func.instruction(&WasmInstruction::LocalGet(3));
            func.instruction(&WasmInstruction::End);
            self.codes.function(&func);
        }

        // ── $elem_get (param $boxed i64) (param $index i32) (result i64) — Type 8 ──
        {
            // local 0 = $boxed (i64), local 1 = $index (i32)
            // local 2 = ptr (i32)
            let mut func = Function::new(vec![(2, ValType::I32)]);

            // 检查是否为 TAG_ARRAY
            // ((boxed >> 32) & TAG_MASK) == TAG_ARRAY
            func.instruction(&WasmInstruction::LocalGet(0));
            func.instruction(&WasmInstruction::I64Const(32));
            func.instruction(&WasmInstruction::I64ShrU);
            func.instruction(&WasmInstruction::I64Const(value::TAG_MASK as i64));
            func.instruction(&WasmInstruction::I64And);
            func.instruction(&WasmInstruction::I64Const(value::TAG_ARRAY as i64));
            func.instruction(&WasmInstruction::I64Eq);
            func.instruction(&WasmInstruction::If(BlockType::Result(ValType::I64)));

            // ── Array path ──
            // 解析 handle → ptr
            func.instruction(&WasmInstruction::LocalGet(0));
            func.instruction(&WasmInstruction::I32WrapI64);
            func.instruction(&WasmInstruction::I32Const(4));
            func.instruction(&WasmInstruction::I32Mul);
            func.instruction(&WasmInstruction::GlobalGet(obj_table_global));
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::I32Load(MemArg {
                offset: 0,
                align: 2,
                memory_index: 0,
            }));
            func.instruction(&WasmInstruction::LocalTee(2));

            // ptr == 0 → return undefined
            func.instruction(&WasmInstruction::I32Eqz);
            func.instruction(&WasmInstruction::If(BlockType::Result(ValType::I64)));
            func.instruction(&WasmInstruction::I64Const(value::encode_undefined()));
            func.instruction(&WasmInstruction::Else);

            // 读取 length (offset 8)
            func.instruction(&WasmInstruction::LocalGet(2));
            func.instruction(&WasmInstruction::I32Load(MemArg {
                offset: 8,
                align: 2,
                memory_index: 0,
            }));
            func.instruction(&WasmInstruction::LocalSet(3)); // save length, consume stack
            func.instruction(&WasmInstruction::LocalGet(1)); // index
            func.instruction(&WasmInstruction::LocalGet(3)); // length
            func.instruction(&WasmInstruction::I32LtU); // index < length
            func.instruction(&WasmInstruction::If(BlockType::Result(ValType::I64)));
            // 读取 elements[ index ] at ptr + 16 + index * 8
            func.instruction(&WasmInstruction::LocalGet(2));
            func.instruction(&WasmInstruction::I32Const(16));
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::LocalGet(1));
            func.instruction(&WasmInstruction::I32Const(8));
            func.instruction(&WasmInstruction::I32Mul);
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::I64Load(MemArg {
                offset: 0,
                align: 3,
                memory_index: 0,
            }));
            func.instruction(&WasmInstruction::Else);
            func.instruction(&WasmInstruction::I64Const(value::encode_undefined()));
            func.instruction(&WasmInstruction::End);
            func.instruction(&WasmInstruction::End);

            func.instruction(&WasmInstruction::Else);
            // 不是 TAG_ARRAY → 委托给 $obj_get_by_index 进行属性访问（将 i32 转换为字符串后查找）
            func.instruction(&WasmInstruction::LocalGet(0));
            func.instruction(&WasmInstruction::LocalGet(1));
            func.instruction(&WasmInstruction::Call(
                self.special_host_import_indices[&SpecialHostImport::ObjGetByIndex],
            ));
            func.instruction(&WasmInstruction::End);
            func.instruction(&WasmInstruction::End);
            self.codes.function(&func);
        }

        // ── $elem_set (param $boxed i64) (param $index i32) (param $value i64) — Type 9 ──
        // 简化实现：不处理扩容（假设容量充足）
        {
            // local 0 = $boxed (i64), local 1 = $index (i32), local 2 = $value (i64)
            // local 3 = ptr (i32), local 4 = length (i32)
            let mut func = Function::new(vec![(2, ValType::I32)]);

            // 检查 TAG_ARRAY
            func.instruction(&WasmInstruction::LocalGet(0));
            func.instruction(&WasmInstruction::I64Const(32));
            func.instruction(&WasmInstruction::I64ShrU);
            func.instruction(&WasmInstruction::I64Const(value::TAG_MASK as i64));
            func.instruction(&WasmInstruction::I64And);
            func.instruction(&WasmInstruction::I64Const(value::TAG_ARRAY as i64));
            func.instruction(&WasmInstruction::I64Eq);
            func.instruction(&WasmInstruction::If(BlockType::Empty));

            // ── Array path ──
            // 解析 handle → ptr
            func.instruction(&WasmInstruction::LocalGet(0));
            func.instruction(&WasmInstruction::I32WrapI64);
            func.instruction(&WasmInstruction::I32Const(4));
            func.instruction(&WasmInstruction::I32Mul);
            func.instruction(&WasmInstruction::GlobalGet(obj_table_global));
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::I32Load(MemArg {
                offset: 0,
                align: 2,
                memory_index: 0,
            }));
            func.instruction(&WasmInstruction::LocalTee(3));

            // ptr == 0 → no-op
            func.instruction(&WasmInstruction::I32Eqz);
            func.instruction(&WasmInstruction::If(BlockType::Empty));
            func.instruction(&WasmInstruction::Return);
            func.instruction(&WasmInstruction::End);

            // 读取 length (offset 8)
            func.instruction(&WasmInstruction::LocalGet(3));
            func.instruction(&WasmInstruction::I32Load(MemArg {
                offset: 8,
                align: 2,
                memory_index: 0,
            }));
            func.instruction(&WasmInstruction::LocalSet(4));

            // 写入 elements[index] = value at ptr + 16 + index * 8
            func.instruction(&WasmInstruction::LocalGet(3));
            func.instruction(&WasmInstruction::I32Const(16));
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::LocalGet(1));
            func.instruction(&WasmInstruction::I32Const(8));
            func.instruction(&WasmInstruction::I32Mul);
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::LocalGet(2));
            func.instruction(&WasmInstruction::I64Store(MemArg {
                offset: 0,
                align: 3,
                memory_index: 0,
            }));

            // 更新 length 如果 index >= length
            func.instruction(&WasmInstruction::LocalGet(1));
            func.instruction(&WasmInstruction::LocalGet(4));
            func.instruction(&WasmInstruction::I32GeU);
            func.instruction(&WasmInstruction::If(BlockType::Empty));
            func.instruction(&WasmInstruction::LocalGet(3));
            func.instruction(&WasmInstruction::LocalGet(1));
            func.instruction(&WasmInstruction::I32Const(1));
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::I32Store(MemArg {
                offset: 8,
                align: 2,
                memory_index: 0,
            }));
            func.instruction(&WasmInstruction::End);

            func.instruction(&WasmInstruction::Else);
            // 不是 TAG_ARRAY → TypedArray 数字索引由宿主处理；未处理才回退到普通属性设置。
            func.instruction(&WasmInstruction::LocalGet(0));
            func.instruction(&WasmInstruction::LocalGet(1));
            func.instruction(&WasmInstruction::LocalGet(2));
            func.instruction(&WasmInstruction::Call(
                self.special_host_import_indices[&SpecialHostImport::TypedArraySetByIndex],
            ));
            func.instruction(&WasmInstruction::I64Const(value::encode_bool(true)));
            func.instruction(&WasmInstruction::I64Eq);
            func.instruction(&WasmInstruction::If(BlockType::Empty));
            func.instruction(&WasmInstruction::Else);
            // 普通对象的数字 key（o[5]=v）：把 i32 索引装回数字 → symbol_property_key 取
            // 稳定 name_id（"5"），不能直接拿索引当 name_id（那是 data 偏移，会错位）。
            func.instruction(&WasmInstruction::LocalGet(0));
            func.instruction(&WasmInstruction::LocalGet(1));
            func.instruction(&WasmInstruction::F64ConvertI32U);
            func.instruction(&WasmInstruction::I64ReinterpretF64);
            func.instruction(&WasmInstruction::Call(
                self.special_host_import_indices[&SpecialHostImport::SymbolPropertyKey],
            ));
            func.instruction(&WasmInstruction::LocalGet(2));
            func.instruction(&WasmInstruction::Call(self.obj_set_func_idx));
            func.instruction(&WasmInstruction::End);
            func.instruction(&WasmInstruction::End);
            func.instruction(&WasmInstruction::End);
            self.codes.function(&func);
        }
    }

    pub(crate) fn compile_get_proto_from_ctor(&mut self) {
        // local 0 = $ctor (i64), local 1 = $proto (i64)
        let mut func = Function::new(vec![(1, ValType::I64)]);
        let proto_name_id = self.intern_data_string("prototype");
        func.instruction(&WasmInstruction::LocalGet(0));
        func.instruction(&WasmInstruction::I32Const(proto_name_id as i32));
        func.instruction(&WasmInstruction::Call(self.obj_get_func_idx));
        func.instruction(&WasmInstruction::LocalSet(1));
        // Tag checks and fallback...
        func.instruction(&WasmInstruction::LocalGet(1));
        func.instruction(&WasmInstruction::I64Const(32));
        func.instruction(&WasmInstruction::I64ShrU);
        func.instruction(&WasmInstruction::I64Const(value::TAG_MASK as i64));
        func.instruction(&WasmInstruction::I64And);
        func.instruction(&WasmInstruction::I64Const(value::TAG_OBJECT as i64));
        func.instruction(&WasmInstruction::I64Eq);
        func.instruction(&WasmInstruction::If(BlockType::Empty));
        func.instruction(&WasmInstruction::LocalGet(1));
        func.instruction(&WasmInstruction::Return);
        func.instruction(&WasmInstruction::End);
        func.instruction(&WasmInstruction::LocalGet(1));
        func.instruction(&WasmInstruction::I64Const(32));
        func.instruction(&WasmInstruction::I64ShrU);
        func.instruction(&WasmInstruction::I64Const(value::TAG_MASK as i64));
        func.instruction(&WasmInstruction::I64And);
        func.instruction(&WasmInstruction::I64Const(value::TAG_FUNCTION as i64));
        func.instruction(&WasmInstruction::I64Eq);
        func.instruction(&WasmInstruction::If(BlockType::Empty));
        func.instruction(&WasmInstruction::LocalGet(1));
        func.instruction(&WasmInstruction::Return);
        func.instruction(&WasmInstruction::End);
        func.instruction(&WasmInstruction::LocalGet(1));
        func.instruction(&WasmInstruction::I64Const(32));
        func.instruction(&WasmInstruction::I64ShrU);
        func.instruction(&WasmInstruction::I64Const(value::TAG_MASK as i64));
        func.instruction(&WasmInstruction::I64And);
        func.instruction(&WasmInstruction::I64Const(value::TAG_CLOSURE as i64));
        func.instruction(&WasmInstruction::I64Eq);
        func.instruction(&WasmInstruction::If(BlockType::Empty));
        func.instruction(&WasmInstruction::LocalGet(1));
        func.instruction(&WasmInstruction::Return);
        func.instruction(&WasmInstruction::End);
        func.instruction(&WasmInstruction::LocalGet(1));
        func.instruction(&WasmInstruction::I64Const(32));
        func.instruction(&WasmInstruction::I64ShrU);
        func.instruction(&WasmInstruction::I64Const(value::TAG_MASK as i64));
        func.instruction(&WasmInstruction::I64And);
        func.instruction(&WasmInstruction::I64Const(value::TAG_ARRAY as i64));
        func.instruction(&WasmInstruction::I64Eq);
        func.instruction(&WasmInstruction::If(BlockType::Empty));
        func.instruction(&WasmInstruction::LocalGet(1));
        func.instruction(&WasmInstruction::Return);
        func.instruction(&WasmInstruction::End);
        func.instruction(&WasmInstruction::LocalGet(1));
        func.instruction(&WasmInstruction::I64Const(32));
        func.instruction(&WasmInstruction::I64ShrU);
        func.instruction(&WasmInstruction::I64Const(value::TAG_MASK as i64));
        func.instruction(&WasmInstruction::I64And);
        func.instruction(&WasmInstruction::I64Const(value::TAG_BOUND as i64));
        func.instruction(&WasmInstruction::I64Eq);
        func.instruction(&WasmInstruction::If(BlockType::Empty));
        func.instruction(&WasmInstruction::LocalGet(1));
        func.instruction(&WasmInstruction::Return);
        func.instruction(&WasmInstruction::End);
        func.instruction(&WasmInstruction::GlobalGet(
            self.object_proto_handle_global_idx,
        ));
        func.instruction(&WasmInstruction::I64ExtendI32U);
        let box_base = value::BOX_BASE as i64;
        let tag_object = (value::TAG_OBJECT << 32) as i64;
        func.instruction(&WasmInstruction::I64Const(box_base | tag_object));
        func.instruction(&WasmInstruction::I64Or);
        func.instruction(&WasmInstruction::End);
        self.codes.function(&func);
    }
}
