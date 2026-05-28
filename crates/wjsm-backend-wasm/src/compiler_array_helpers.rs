use super::*;

impl Compiler {
    pub(crate) fn compile_array_helpers(&mut self) {
        let heap_global = self.heap_ptr_global_idx;
        let obj_table_global = self.obj_table_global_idx;
        let obj_table_count_global = self.obj_table_count_global_idx;
        let array_proto_global = self.array_proto_handle_global_idx;

        // ── $arr_new (param $capacity i32) (result i32) — Type 7 ──
        // 分配数组对象到堆上，注册到 handle 表，返回 handle_idx。
        // 数组内存布局: [proto(4), length(4), capacity(4), elements(capacity*8)]
        {
            // local 0 = $capacity, local 1 = size, local 2 = ptr, local 3 = handle_idx
            let locals: Vec<(u32, wasm_encoder::ValType)> = vec![(3, wasm_encoder::ValType::I32)];
            let mut func = Function::new(locals);
            let gc_collect_idx = self.gc_collect_func_idx;

            // size = 16 + capacity * 8 (4 proto + 1 type + 3 pad + 4 length + 4 capacity + cap*8)
            func.instruction(&WasmInstruction::LocalGet(0));
            func.instruction(&WasmInstruction::I32Const(8));
            func.instruction(&WasmInstruction::I32Mul);
            func.instruction(&WasmInstruction::I32Const(16));
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::LocalSet(1));

            // ── GC 检查 ──
            func.instruction(&WasmInstruction::GlobalGet(heap_global));
            func.instruction(&WasmInstruction::LocalGet(1));
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::MemorySize(0));
            func.instruction(&WasmInstruction::I64ExtendI32U);
            func.instruction(&WasmInstruction::I64Const(65536));
            func.instruction(&WasmInstruction::I64Mul);
            func.instruction(&WasmInstruction::I32WrapI64);
            func.instruction(&WasmInstruction::I32GtU);
            func.instruction(&WasmInstruction::If(BlockType::Empty));
            func.instruction(&WasmInstruction::LocalGet(1));
            func.instruction(&WasmInstruction::Call(gc_collect_idx));
            func.instruction(&WasmInstruction::I32Eqz);
            func.instruction(&WasmInstruction::If(BlockType::Empty));
            func.instruction(&WasmInstruction::Unreachable);
            func.instruction(&WasmInstruction::End);
            func.instruction(&WasmInstruction::End);

            // ── Proactive GC ──
            func.instruction(&WasmInstruction::GlobalGet(self.alloc_counter_global_idx));
            func.instruction(&WasmInstruction::I32Const(1));
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::LocalTee(3));
            func.instruction(&WasmInstruction::GlobalSet(self.alloc_counter_global_idx));
            func.instruction(&WasmInstruction::LocalGet(3));
            func.instruction(&WasmInstruction::I32Const(1000));
            func.instruction(&WasmInstruction::I32GeU);
            func.instruction(&WasmInstruction::If(BlockType::Empty));
            func.instruction(&WasmInstruction::I32Const(0));
            func.instruction(&WasmInstruction::Call(gc_collect_idx));
            func.instruction(&WasmInstruction::Drop);
            func.instruction(&WasmInstruction::I32Const(0));
            func.instruction(&WasmInstruction::GlobalSet(self.alloc_counter_global_idx));
            func.instruction(&WasmInstruction::End);

            // ptr = heap_ptr; heap_ptr += size
            func.instruction(&WasmInstruction::GlobalGet(heap_global));
            func.instruction(&WasmInstruction::LocalTee(2));
            func.instruction(&WasmInstruction::LocalGet(1));
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::GlobalSet(heap_global));

            // Write header
            // proto = array_proto_handle from global (or -1 if not set)
            func.instruction(&WasmInstruction::LocalGet(2));
            func.instruction(&WasmInstruction::GlobalGet(array_proto_global));
            func.instruction(&WasmInstruction::I32Store(MemArg {
                offset: 0,
                align: 2,
                memory_index: 0,
            }));
            // Write type byte HEAP_TYPE_ARRAY (0x01) at offset 4
            func.instruction(&WasmInstruction::LocalGet(2));
            func.instruction(&WasmInstruction::I32Const(1));
            func.instruction(&WasmInstruction::I32Store8(MemArg {
                offset: 4,
                align: 0,
                memory_index: 0,
            }));
            // Zero pad at offsets 5-7
            func.instruction(&WasmInstruction::LocalGet(2));
            func.instruction(&WasmInstruction::I32Const(0));
            func.instruction(&WasmInstruction::I32Store8(MemArg {
                offset: 5,
                align: 0,
                memory_index: 0,
            }));
            func.instruction(&WasmInstruction::LocalGet(2));
            func.instruction(&WasmInstruction::I32Const(0));
            func.instruction(&WasmInstruction::I32Store8(MemArg {
                offset: 6,
                align: 0,
                memory_index: 0,
            }));
            func.instruction(&WasmInstruction::LocalGet(2));
            func.instruction(&WasmInstruction::I32Const(0));
            func.instruction(&WasmInstruction::I32Store8(MemArg {
                offset: 7,
                align: 0,
                memory_index: 0,
            }));
            // length = 0 at offset 8
            func.instruction(&WasmInstruction::LocalGet(2));
            func.instruction(&WasmInstruction::I32Const(0));
            func.instruction(&WasmInstruction::I32Store(MemArg {
                offset: 8,
                align: 2,
                memory_index: 0,
            }));
            // capacity = capacity (param 0) at offset 12
            func.instruction(&WasmInstruction::LocalGet(2));
            func.instruction(&WasmInstruction::LocalGet(0));
            func.instruction(&WasmInstruction::I32Store(MemArg {
                offset: 12,
                align: 2,
                memory_index: 0,
            }));

            // handle_idx = obj_table_count
            func.instruction(&WasmInstruction::GlobalGet(obj_table_count_global));
            func.instruction(&WasmInstruction::LocalTee(3));
            // obj_table[handle_idx] = ptr
            func.instruction(&WasmInstruction::I32Const(4));
            func.instruction(&WasmInstruction::I32Mul);
            func.instruction(&WasmInstruction::GlobalGet(obj_table_global));
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::LocalGet(2));
            func.instruction(&WasmInstruction::I32Store(MemArg {
                offset: 0,
                align: 2,
                memory_index: 0,
            }));
            // obj_table_count++
            func.instruction(&WasmInstruction::GlobalGet(obj_table_count_global));
            func.instruction(&WasmInstruction::I32Const(1));
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::GlobalSet(obj_table_count_global));
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
            // ((boxed >> 32) & 0xF) == TAG_ARRAY
            func.instruction(&WasmInstruction::LocalGet(0));
            func.instruction(&WasmInstruction::I64Const(32));
            func.instruction(&WasmInstruction::I64ShrU);
            func.instruction(&WasmInstruction::I64Const(0xF));
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
            func.instruction(&WasmInstruction::Call(self.obj_get_by_index_func_idx));
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
            func.instruction(&WasmInstruction::I64Const(0xF));
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
                self.typedarray_set_by_index_func_idx,
            ));
            func.instruction(&WasmInstruction::I64Const(value::encode_bool(true)));
            func.instruction(&WasmInstruction::I64Eq);
            func.instruction(&WasmInstruction::If(BlockType::Empty));
            func.instruction(&WasmInstruction::Else);
            func.instruction(&WasmInstruction::LocalGet(0));
            func.instruction(&WasmInstruction::LocalGet(1));
            func.instruction(&WasmInstruction::LocalGet(2));
            func.instruction(&WasmInstruction::Call(self.obj_set_func_idx));
            func.instruction(&WasmInstruction::End);
            func.instruction(&WasmInstruction::End);
            func.instruction(&WasmInstruction::End);
            self.codes.function(&func);
        }

        // ── $get_prototype_from_constructor (param $ctor i64) (result i64) — Type 3 ──
        // GetPrototypeFromConstructor(F): 读取 F.prototype，若非 Object 类型则回退到 Object.prototype
        {
            // local 0 = $ctor (i64), local 1 = $proto (i64)
            let mut func = Function::new(vec![(1, ValType::I64)]);

            // 获取 "prototype" 的 name_id
            let proto_name_id = self.intern_data_string("prototype");

            // 调用 $obj_get(ctor, "prototype") — 遍历原型链
            func.instruction(&WasmInstruction::LocalGet(0));
            func.instruction(&WasmInstruction::I32Const(proto_name_id as i32));
            func.instruction(&WasmInstruction::Call(self.obj_get_func_idx));
            func.instruction(&WasmInstruction::LocalSet(1)); // $proto

            // 检查结果是否为 TAG_OBJECT (0x8)
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

            // 检查结果是否为 TAG_FUNCTION (0x9)
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
            // 检查是否为 TAG_CLOSURE (0xA)
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
            // 检查是否为 TAG_ARRAY (0xB)
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
            // 检查是否为 TAG_BOUND (0xC)
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

            // 回退：返回 Object.prototype (Global 10)
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
}
