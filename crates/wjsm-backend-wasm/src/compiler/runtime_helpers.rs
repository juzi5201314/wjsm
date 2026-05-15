use wasm_encoder::{BlockType, Function, Instruction as WasmInstruction, MemArg, ValType};
use wjsm_ir::{constants, value};

use super::state::Compiler;

impl Compiler {
    pub(crate) fn compile_object_helpers(&mut self) {
        let heap_global = self.heap_ptr_global_idx;
        let obj_table_global = self.obj_table_global_idx;
        let obj_table_count_global = self.obj_table_count_global_idx;
        let num_ir_functions_global = self.num_ir_functions_global_idx;

        // ── $obj_new (param $capacity i32) (result i32) — Type 7 ──
        // 分配对象到堆上，将 ptr 存入 handle 表，返回 handle_idx。
        // 属性槽格式: [name_id(4), flags(4), value(8), getter(8), setter(8)] = 32 字节
        // GC 检查：如果 heap_ptr + size > memory.size * 64KB，调用 gc_collect
        {
            // local 0 = $capacity, local 1 = size, local 2 = ptr, local 3 = handle_idx
            let mut func = Function::new(vec![(3, ValType::I32)]);
            let gc_collect_idx = self.gc_collect_func_idx;

            // size = 16 + capacity * 32 (4 proto + 1 type + 3 pad + 4 capacity + 4 num_props + cap*32)
            func.instruction(&WasmInstruction::LocalGet(0));
            func.instruction(&WasmInstruction::I32Const(32));
            func.instruction(&WasmInstruction::I32Mul);
            func.instruction(&WasmInstruction::I32Const(16));
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::LocalSet(1));

            // ── GC 检查 ──
            // 检查: heap_ptr + size > memory.size * 65536
            // 如果 true，调用 gc_collect(size)

            // 计算 heap_ptr + size
            func.instruction(&WasmInstruction::GlobalGet(heap_global));
            func.instruction(&WasmInstruction::LocalGet(1));
            func.instruction(&WasmInstruction::I32Add);

            // 计算 memory.size * 65536 (使用 i64 避免溢出)
            func.instruction(&WasmInstruction::MemorySize(0));
            func.instruction(&WasmInstruction::I64ExtendI32U);
            func.instruction(&WasmInstruction::I64Const(65536));
            func.instruction(&WasmInstruction::I64Mul);
            func.instruction(&WasmInstruction::I32WrapI64);

            // 比较: heap_ptr + size > memory_limit
            func.instruction(&WasmInstruction::I32GtU);

            func.instruction(&WasmInstruction::If(BlockType::Empty));
            // 需要 GC - 调用 gc_collect(size)
            func.instruction(&WasmInstruction::LocalGet(1)); // size
            func.instruction(&WasmInstruction::Call(gc_collect_idx));
            // 检查返回值是否为 0（失败）
            func.instruction(&WasmInstruction::I32Eqz);
            func.instruction(&WasmInstruction::If(BlockType::Empty));
            // OOM - unreachable
            func.instruction(&WasmInstruction::Unreachable);
            func.instruction(&WasmInstruction::End);
            func.instruction(&WasmInstruction::End);

            // ── Proactive GC: check alloc_counter threshold ──
            // 每 1000 次分配触发一次 gc_collect(0)
            func.instruction(&WasmInstruction::GlobalGet(self.alloc_counter_global_idx));
            func.instruction(&WasmInstruction::I32Const(1));
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::LocalTee(3)); // reuse handle_idx local as tmp
            func.instruction(&WasmInstruction::GlobalSet(self.alloc_counter_global_idx));
            // Re-load counter value for comparison (consumed by GlobalSet)
            func.instruction(&WasmInstruction::LocalGet(3));
            // Check if counter >= GC_THRESHOLD (1000)
            func.instruction(&WasmInstruction::I32Const(1000));
            func.instruction(&WasmInstruction::I32GeU);
            func.instruction(&WasmInstruction::If(BlockType::Empty));
            // Call gc_collect(0) — proactive collection
            func.instruction(&WasmInstruction::I32Const(0));
            func.instruction(&WasmInstruction::Call(gc_collect_idx));
            func.instruction(&WasmInstruction::Drop); // ignore result
            // Reset alloc_counter
            func.instruction(&WasmInstruction::I32Const(0));
            func.instruction(&WasmInstruction::GlobalSet(self.alloc_counter_global_idx));
            func.instruction(&WasmInstruction::End);

            // ptr = heap_ptr; heap_ptr += size
            func.instruction(&WasmInstruction::GlobalGet(heap_global));
            func.instruction(&WasmInstruction::LocalTee(2));
            func.instruction(&WasmInstruction::LocalGet(1));
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::GlobalSet(heap_global));
            func.instruction(&WasmInstruction::LocalGet(2));
            func.instruction(&WasmInstruction::I32Const(-1)); // proto sentinel (0xFFFFFFFF)
            func.instruction(&WasmInstruction::I32Store(MemArg {
                offset: 0,
                align: 2,
                memory_index: 0,
            }));
            func.instruction(&WasmInstruction::LocalGet(2));
            // Write type byte HEAP_TYPE_OBJECT (0x00)
            func.instruction(&WasmInstruction::I32Const(0));
            func.instruction(&WasmInstruction::I32Store8(MemArg {
                offset: 4,
                align: 0,
                memory_index: 0,
            }));
            // Zero pad bytes at offset 5-7
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
            // capacity at offset 8
            func.instruction(&WasmInstruction::LocalGet(2));
            func.instruction(&WasmInstruction::LocalGet(0));
            func.instruction(&WasmInstruction::I32Store(MemArg {
                offset: 8,
                align: 2,
                memory_index: 0,
            }));
            // num_props = 0 at offset 12
            func.instruction(&WasmInstruction::LocalGet(2));
            func.instruction(&WasmInstruction::I32Const(0));
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

        // ── $obj_get (param $boxed i64) (param $name_id i32) (result i64) — Type 8 ──
        // 通过 handle 表解析 boxed value，搜索属性（含原型链）。
        {
            // local 0 = $boxed (i64), local 1 = $name_id (i32)
            // local 2 = num_props (i32), local 3 = i (i32), local 4 = slot_addr (i32)
            // local 5 = resolved ptr (i32), local 6 = flags (i32), local 7 = getter (i64)
            // local 8 = getter env_obj (i64), local 9 = getter func_idx (i32)
            let length_name_id = self.ensure_string_ptr_const(&"length".to_string());
            let mut func = Function::new(vec![
                (5, ValType::I32),
                (2, ValType::I64),
                (1, ValType::I32),
            ]);

            // ── 检查 tag 以确定 handle_idx ──
            func.instruction(&WasmInstruction::Block(BlockType::Empty));
            func.instruction(&WasmInstruction::LocalGet(0));
            func.instruction(&WasmInstruction::I64Const(32));
            func.instruction(&WasmInstruction::I64ShrU);
            func.instruction(&WasmInstruction::I32WrapI64);
            func.instruction(&WasmInstruction::I32Const(0x1F));
            func.instruction(&WasmInstruction::I32And);
            func.instruction(&WasmInstruction::LocalTee(3));
            func.instruction(&WasmInstruction::I32Const(value::TAG_FUNCTION as i32));
            func.instruction(&WasmInstruction::I32Eq);
            func.instruction(&WasmInstruction::If(BlockType::Empty));
            func.instruction(&WasmInstruction::LocalGet(0));
            func.instruction(&WasmInstruction::I32WrapI64);
            func.instruction(&WasmInstruction::GlobalGet(num_ir_functions_global));
            func.instruction(&WasmInstruction::I32LtU);
            func.instruction(&WasmInstruction::If(BlockType::Empty));
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
            func.instruction(&WasmInstruction::LocalSet(5));
            func.instruction(&WasmInstruction::Br(2));
            func.instruction(&WasmInstruction::End);
            func.instruction(&WasmInstruction::I64Const(value::encode_undefined()));
            func.instruction(&WasmInstruction::Return);
            func.instruction(&WasmInstruction::End);
            func.instruction(&WasmInstruction::LocalGet(3));
            func.instruction(&WasmInstruction::I32Const(value::TAG_CLOSURE as i32));
            func.instruction(&WasmInstruction::I32Eq);
            func.instruction(&WasmInstruction::If(BlockType::Empty));
            func.instruction(&WasmInstruction::I64Const(value::encode_undefined()));
            func.instruction(&WasmInstruction::Return);
            func.instruction(&WasmInstruction::End);
            func.instruction(&WasmInstruction::LocalGet(3));
            func.instruction(&WasmInstruction::I32Const(value::TAG_BOUND as i32));
            func.instruction(&WasmInstruction::I32Eq);
            func.instruction(&WasmInstruction::If(BlockType::Empty));
            func.instruction(&WasmInstruction::I64Const(value::encode_undefined()));
            func.instruction(&WasmInstruction::Return);
            func.instruction(&WasmInstruction::End);
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
            func.instruction(&WasmInstruction::LocalSet(5));
            func.instruction(&WasmInstruction::End);

            // ptr == 0 → return undefined
            func.instruction(&WasmInstruction::LocalGet(5));
            func.instruction(&WasmInstruction::I32Eqz);
            func.instruction(&WasmInstruction::If(BlockType::Empty));
            func.instruction(&WasmInstruction::I64Const(value::encode_undefined()));
            func.instruction(&WasmInstruction::Return);
            func.instruction(&WasmInstruction::End);

            // 数组的 length 是内建数据属性，不存放在对象属性槽里。
            func.instruction(&WasmInstruction::LocalGet(5));
            func.instruction(&WasmInstruction::I32Load8U(MemArg {
                offset: 4,
                align: 0,
                memory_index: 0,
            }));
            func.instruction(&WasmInstruction::I32Const(wjsm_ir::HEAP_TYPE_ARRAY as i32));
            func.instruction(&WasmInstruction::I32Eq);
            func.instruction(&WasmInstruction::LocalGet(1));
            func.instruction(&WasmInstruction::I32Const(length_name_id as i32));
            func.instruction(&WasmInstruction::I32Eq);
            func.instruction(&WasmInstruction::I32And);
            func.instruction(&WasmInstruction::If(BlockType::Empty));
            func.instruction(&WasmInstruction::LocalGet(5));
            func.instruction(&WasmInstruction::I32Load(MemArg {
                offset: 8,
                align: 2,
                memory_index: 0,
            }));
            func.instruction(&WasmInstruction::F64ConvertI32U);
            func.instruction(&WasmInstruction::I64ReinterpretF64);
            func.instruction(&WasmInstruction::Return);
            func.instruction(&WasmInstruction::End);

            // ── 原型链遍历 ──
            func.instruction(&WasmInstruction::Block(BlockType::Empty));
            func.instruction(&WasmInstruction::Loop(BlockType::Empty));
            // 读 type byte (offset 4) → 数组没有 own property slots
            func.instruction(&WasmInstruction::LocalGet(5));
            func.instruction(&WasmInstruction::I32Load8U(MemArg {
                offset: 4,
                align: 0,
                memory_index: 0,
            }));
            func.instruction(&WasmInstruction::I32Const(wjsm_ir::HEAP_TYPE_ARRAY as i32));
            func.instruction(&WasmInstruction::I32Eq);
            func.instruction(&WasmInstruction::If(BlockType::Empty));
            // 数组 → num_props = 0 (跳过属性搜索)
            func.instruction(&WasmInstruction::I32Const(0));
            func.instruction(&WasmInstruction::LocalSet(2));
            func.instruction(&WasmInstruction::Else);
            // 普通对象 → 读取 num_props (offset 12)
            func.instruction(&WasmInstruction::LocalGet(5));
            func.instruction(&WasmInstruction::I32Load(MemArg {
                offset: 12,
                align: 2,
                memory_index: 0,
            }));
            func.instruction(&WasmInstruction::LocalSet(2));
            func.instruction(&WasmInstruction::End);

            func.instruction(&WasmInstruction::I32Const(0));
            func.instruction(&WasmInstruction::LocalSet(3));
            func.instruction(&WasmInstruction::Block(BlockType::Empty));
            func.instruction(&WasmInstruction::Loop(BlockType::Empty));
            func.instruction(&WasmInstruction::LocalGet(3));
            func.instruction(&WasmInstruction::LocalGet(2));
            func.instruction(&WasmInstruction::I32GeU);
            func.instruction(&WasmInstruction::BrIf(1));
            // slot_addr = ptr + 12 + i * 32
            func.instruction(&WasmInstruction::LocalGet(5));
            func.instruction(&WasmInstruction::I32Const(16));
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::LocalGet(3));
            func.instruction(&WasmInstruction::I32Const(32));
            func.instruction(&WasmInstruction::I32Mul);
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::LocalTee(4));
            func.instruction(&WasmInstruction::I32Load(MemArg {
                offset: 0,
                align: 2,
                memory_index: 0,
            }));
            func.instruction(&WasmInstruction::LocalTee(6));
            func.instruction(&WasmInstruction::LocalGet(1));
            func.instruction(&WasmInstruction::I32Eq);
            func.instruction(&WasmInstruction::LocalGet(6));
            func.instruction(&WasmInstruction::LocalGet(1));
            func.instruction(&WasmInstruction::Call(self.string_eq_func_idx));
            func.instruction(&WasmInstruction::I32Or);
            func.instruction(&WasmInstruction::If(BlockType::Empty));
            // 找到！检查是否为访问器属性
            // 加载 flags (offset 4)
            func.instruction(&WasmInstruction::LocalGet(4));
            func.instruction(&WasmInstruction::I32Load(MemArg {
                offset: 4,
                align: 2,
                memory_index: 0,
            }));
            func.instruction(&WasmInstruction::LocalTee(6));
            // 检查 is_accessor 位
            func.instruction(&WasmInstruction::I32Const(constants::FLAG_IS_ACCESSOR));
            func.instruction(&WasmInstruction::I32And);
            func.instruction(&WasmInstruction::I32Const(0));
            func.instruction(&WasmInstruction::I32Ne);
            func.instruction(&WasmInstruction::If(BlockType::Empty));
            // 是访问器属性，加载 getter (offset 16)
            func.instruction(&WasmInstruction::LocalGet(4));
            func.instruction(&WasmInstruction::I64Load(MemArg {
                offset: 16,
                align: 3,
                memory_index: 0,
            }));
            func.instruction(&WasmInstruction::LocalTee(7));
            // 检查 getter 是否为 undefined
            func.instruction(&WasmInstruction::I64Const(value::encode_undefined()));
            func.instruction(&WasmInstruction::I64Eq);
            func.instruction(&WasmInstruction::If(BlockType::Empty));
            // getter 是 undefined，返回 undefined
            func.instruction(&WasmInstruction::I64Const(value::encode_undefined()));
            func.instruction(&WasmInstruction::Return);
            func.instruction(&WasmInstruction::End);
            // 调用 getter: Type 12 签名 (env_obj, this_val, args_base, args_count) -> i64
            self.emit_resolve_callable_for_helper(&mut func, 7, 9, 8);
            // this_val = local 0, args_base = 0 (no args), args_count = 0
            func.instruction(&WasmInstruction::LocalGet(8)); // env_obj
            func.instruction(&WasmInstruction::LocalGet(0)); // this_val
            func.instruction(&WasmInstruction::I32Const(0)); // args_base (doesn't matter, no args)
            func.instruction(&WasmInstruction::I32Const(0)); // args_count
            func.instruction(&WasmInstruction::LocalGet(9)); // func_idx
            // call_indirect type 12, table 0
            func.instruction(&WasmInstruction::CallIndirect {
                type_index: 12,
                table_index: 0,
            });
            func.instruction(&WasmInstruction::Return);
            func.instruction(&WasmInstruction::End);
            // 是数据属性，返回 value (offset 8)
            func.instruction(&WasmInstruction::LocalGet(4));
            func.instruction(&WasmInstruction::I64Load(MemArg {
                offset: 8,
                align: 3,
                memory_index: 0,
            }));
            func.instruction(&WasmInstruction::Return);
            func.instruction(&WasmInstruction::End);
            func.instruction(&WasmInstruction::LocalGet(3));
            func.instruction(&WasmInstruction::I32Const(1));
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::LocalSet(3));
            func.instruction(&WasmInstruction::Br(0));
            func.instruction(&WasmInstruction::End);
            func.instruction(&WasmInstruction::End);

            // 跟随 __proto__（现在存储的是 handle_idx）
            // 读取 proto_handle = obj[0]
            func.instruction(&WasmInstruction::LocalGet(5));
            func.instruction(&WasmInstruction::I32Load(MemArg {
                offset: 0,
                align: 2,
                memory_index: 0,
            }));
            func.instruction(&WasmInstruction::LocalTee(3)); // 暂存 proto_handle 到 local 3
            // 如果 proto_handle == -1 (哨兵)，退出循环
            func.instruction(&WasmInstruction::I32Const(-1));
            func.instruction(&WasmInstruction::I32Eq);
            func.instruction(&WasmInstruction::BrIf(1));
            // 通过 handle 表解析 proto_handle → proto_ptr
            func.instruction(&WasmInstruction::LocalGet(3));
            func.instruction(&WasmInstruction::I32Const(4));
            func.instruction(&WasmInstruction::I32Mul);
            func.instruction(&WasmInstruction::GlobalGet(obj_table_global));
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::I32Load(MemArg {
                offset: 0,
                align: 2,
                memory_index: 0,
            }));
            func.instruction(&WasmInstruction::LocalSet(5)); // 更新 ptr 为 proto_ptr
            func.instruction(&WasmInstruction::Br(0));
            func.instruction(&WasmInstruction::End);
            func.instruction(&WasmInstruction::End);

            // 未找到
            func.instruction(&WasmInstruction::I64Const(value::encode_undefined()));
            func.instruction(&WasmInstruction::End);
            self.codes.function(&func);
        }

        // ── $obj_set (param $boxed i64) (param $name_id i32) (param $value i64) — Type 9 ──
        // 通过 handle 表解析 boxed value，设置属性。
        {
            // local 0 = $boxed (i64), local 1 = $name_id (i32), local 2 = $value (i64)
            // local 3 = (unused pad)
            // local 4 = num_props (i32), local 5 = i (i32), local 6 = slot_addr (i32), local 7 = capacity (i32)
            // local 8 = resolved ptr (i32), local 9 = handle_idx (i32), local 10 = flags (i32), local 11 = setter (i64)
            // local 12 = shadow_sp_scratch (i32), local 13 = setter func_idx (i32), local 15 = setter env_obj (i64)
            let mut func = Function::new(vec![
                (8, ValType::I32),
                (1, ValType::I64),
                (3, ValType::I32),
                (1, ValType::I64),
            ]);

            // ── 通过 handle 表解析 ptr（支持 TAG_OBJECT 和 TAG_FUNCTION）──
            func.instruction(&WasmInstruction::Block(BlockType::Empty));
            func.instruction(&WasmInstruction::LocalGet(0));
            func.instruction(&WasmInstruction::I64Const(32));
            func.instruction(&WasmInstruction::I64ShrU);
            func.instruction(&WasmInstruction::I32WrapI64);
            func.instruction(&WasmInstruction::I32Const(0x1F));
            func.instruction(&WasmInstruction::I32And);
            func.instruction(&WasmInstruction::LocalTee(5));
            func.instruction(&WasmInstruction::I32Const(value::TAG_FUNCTION as i32));
            func.instruction(&WasmInstruction::I32Eq);
            func.instruction(&WasmInstruction::If(BlockType::Empty));
            func.instruction(&WasmInstruction::LocalGet(0));
            func.instruction(&WasmInstruction::I32WrapI64);
            func.instruction(&WasmInstruction::GlobalGet(num_ir_functions_global));
            func.instruction(&WasmInstruction::I32LtU);
            func.instruction(&WasmInstruction::If(BlockType::Empty));
            func.instruction(&WasmInstruction::LocalGet(0));
            func.instruction(&WasmInstruction::I32WrapI64);
            func.instruction(&WasmInstruction::LocalTee(9));
            func.instruction(&WasmInstruction::I32Const(4));
            func.instruction(&WasmInstruction::I32Mul);
            func.instruction(&WasmInstruction::GlobalGet(obj_table_global));
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::I32Load(MemArg {
                offset: 0,
                align: 2,
                memory_index: 0,
            }));
            func.instruction(&WasmInstruction::LocalSet(8));
            func.instruction(&WasmInstruction::Br(2));
            func.instruction(&WasmInstruction::End);
            func.instruction(&WasmInstruction::Return);
            func.instruction(&WasmInstruction::End);
            func.instruction(&WasmInstruction::LocalGet(0));
            func.instruction(&WasmInstruction::I32WrapI64);
            func.instruction(&WasmInstruction::LocalTee(9));
            func.instruction(&WasmInstruction::I32Const(4));
            func.instruction(&WasmInstruction::I32Mul);
            func.instruction(&WasmInstruction::GlobalGet(obj_table_global));
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::I32Load(MemArg {
                offset: 0,
                align: 2,
                memory_index: 0,
            }));
            func.instruction(&WasmInstruction::LocalSet(8));
            func.instruction(&WasmInstruction::End);

            // ── 搜索已有属性 ──
            func.instruction(&WasmInstruction::LocalGet(8));
            func.instruction(&WasmInstruction::I32Load(MemArg {
                offset: 12,
                align: 2,
                memory_index: 0,
            }));
            func.instruction(&WasmInstruction::LocalSet(4));
            func.instruction(&WasmInstruction::I32Const(0));
            func.instruction(&WasmInstruction::LocalSet(5));
            func.instruction(&WasmInstruction::Block(BlockType::Empty));
            func.instruction(&WasmInstruction::Loop(BlockType::Empty));
            func.instruction(&WasmInstruction::LocalGet(5));
            func.instruction(&WasmInstruction::LocalGet(4));
            func.instruction(&WasmInstruction::I32GeU);
            func.instruction(&WasmInstruction::BrIf(1));
            // slot_addr = ptr + 12 + i * 32
            func.instruction(&WasmInstruction::LocalGet(8));
            func.instruction(&WasmInstruction::I32Const(16));
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::LocalGet(5));
            func.instruction(&WasmInstruction::I32Const(32));
            func.instruction(&WasmInstruction::I32Mul);
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::LocalTee(6));
            func.instruction(&WasmInstruction::I32Load(MemArg {
                offset: 0,
                align: 2,
                memory_index: 0,
            }));
            func.instruction(&WasmInstruction::LocalTee(10));
            func.instruction(&WasmInstruction::LocalGet(1));
            func.instruction(&WasmInstruction::I32Eq);
            func.instruction(&WasmInstruction::LocalGet(10));
            func.instruction(&WasmInstruction::LocalGet(1));
            func.instruction(&WasmInstruction::Call(self.string_eq_func_idx));
            func.instruction(&WasmInstruction::I32Or);
            func.instruction(&WasmInstruction::If(BlockType::Empty));
            // 找到！检查是否为访问器属性
            // 加载 flags (offset 4)
            func.instruction(&WasmInstruction::LocalGet(6));
            func.instruction(&WasmInstruction::I32Load(MemArg {
                offset: 4,
                align: 2,
                memory_index: 0,
            }));
            func.instruction(&WasmInstruction::LocalTee(10));
            // 检查 is_accessor 位
            func.instruction(&WasmInstruction::I32Const(constants::FLAG_IS_ACCESSOR));
            func.instruction(&WasmInstruction::I32And);
            func.instruction(&WasmInstruction::I32Const(0));
            func.instruction(&WasmInstruction::I32Ne);
            func.instruction(&WasmInstruction::If(BlockType::Empty));
            // 是访问器属性，加载 setter (offset 24)
            func.instruction(&WasmInstruction::LocalGet(6));
            func.instruction(&WasmInstruction::I64Load(MemArg {
                offset: 24,
                align: 3,
                memory_index: 0,
            }));
            func.instruction(&WasmInstruction::LocalTee(11));
            // 检查 setter 是否为 undefined
            func.instruction(&WasmInstruction::I64Const(value::encode_undefined()));
            func.instruction(&WasmInstruction::I64Eq);
            func.instruction(&WasmInstruction::If(BlockType::Empty));
            // setter 是 undefined，直接返回（静默失败）
            func.instruction(&WasmInstruction::Return);
            func.instruction(&WasmInstruction::End);
            // 调用 setter: Type 12 签名 (env_obj, this_val, args_base, args_count) -> i64
            self.emit_resolve_callable_for_helper(&mut func, 11, 13, 15);
            // 需要将 value (local 2) 写入影子栈
            // 保存 shadow_sp 到 local 12
            func.instruction(&WasmInstruction::GlobalGet(self.shadow_sp_global_idx));
            func.instruction(&WasmInstruction::LocalSet(12));
            // 写入 value 到影子栈
            func.instruction(&WasmInstruction::GlobalGet(self.shadow_sp_global_idx));
            func.instruction(&WasmInstruction::LocalGet(2)); // value
            func.instruction(&WasmInstruction::I64Store(MemArg {
                offset: 0,
                align: 3,
                memory_index: 0,
            }));
            // shadow_sp += 8 (虽然这里只有1个参数，但保持一致性)
            func.instruction(&WasmInstruction::GlobalGet(self.shadow_sp_global_idx));
            func.instruction(&WasmInstruction::I32Const(8));
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::GlobalSet(self.shadow_sp_global_idx));
            // 推入参数: env_obj, this_val (local 0), args_base (local 12), args_count (1)
            func.instruction(&WasmInstruction::LocalGet(15)); // env_obj
            func.instruction(&WasmInstruction::LocalGet(0)); // this_val
            func.instruction(&WasmInstruction::LocalGet(12)); // args_base
            func.instruction(&WasmInstruction::I32Const(1)); // args_count
            func.instruction(&WasmInstruction::LocalGet(13)); // func_idx
            // call_indirect type 12, table 0
            func.instruction(&WasmInstruction::CallIndirect {
                type_index: 12,
                table_index: 0,
            });
            // 恢复 shadow_sp
            func.instruction(&WasmInstruction::LocalGet(12));
            func.instruction(&WasmInstruction::GlobalSet(self.shadow_sp_global_idx));
            func.instruction(&WasmInstruction::Drop); // 丢弃返回值
            func.instruction(&WasmInstruction::Return);
            func.instruction(&WasmInstruction::End);
            // 是数据属性，更新 value (offset 8)
            func.instruction(&WasmInstruction::LocalGet(6));
            func.instruction(&WasmInstruction::LocalGet(2));
            func.instruction(&WasmInstruction::I64Store(MemArg {
                offset: 8,
                align: 3,
                memory_index: 0,
            }));
            func.instruction(&WasmInstruction::Return);
            func.instruction(&WasmInstruction::End);
            func.instruction(&WasmInstruction::LocalGet(5));
            func.instruction(&WasmInstruction::I32Const(1));
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::LocalSet(5));
            func.instruction(&WasmInstruction::Br(0));
            func.instruction(&WasmInstruction::End);
            func.instruction(&WasmInstruction::End);

            // ── 未找到 → 检查是否需要扩容 ──
            func.instruction(&WasmInstruction::LocalGet(8));
            func.instruction(&WasmInstruction::I32Load(MemArg {
                offset: 8,
                align: 2,
                memory_index: 0,
            }));
            func.instruction(&WasmInstruction::LocalSet(7));

            // 如果 num_props >= capacity，需要扩容
            func.instruction(&WasmInstruction::LocalGet(4));
            func.instruction(&WasmInstruction::LocalGet(7));
            func.instruction(&WasmInstruction::I32GeU);
            func.instruction(&WasmInstruction::If(BlockType::Empty));

            // 保存旧 ptr
            func.instruction(&WasmInstruction::LocalGet(8));
            func.instruction(&WasmInstruction::LocalSet(6)); // old_ptr

            // new_capacity = capacity * 2
            func.instruction(&WasmInstruction::LocalGet(7));
            func.instruction(&WasmInstruction::I32Const(2));
            func.instruction(&WasmInstruction::I32Mul);
            func.instruction(&WasmInstruction::LocalSet(7));
            func.instruction(&WasmInstruction::LocalGet(7));
            func.instruction(&WasmInstruction::I32Eqz);
            func.instruction(&WasmInstruction::If(BlockType::Empty));
            func.instruction(&WasmInstruction::I32Const(4));
            func.instruction(&WasmInstruction::LocalSet(7));
            func.instruction(&WasmInstruction::End);

            // new_ptr = heap_ptr
            func.instruction(&WasmInstruction::GlobalGet(heap_global));
            func.instruction(&WasmInstruction::LocalSet(8));

            // heap_ptr += 12 + new_capacity * 32
            func.instruction(&WasmInstruction::GlobalGet(heap_global));
            func.instruction(&WasmInstruction::LocalGet(7));
            func.instruction(&WasmInstruction::I32Const(32));
            func.instruction(&WasmInstruction::I32Mul);
            func.instruction(&WasmInstruction::I32Const(16));
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::GlobalSet(heap_global));

            // 拷贝旧数据到新内存
            func.instruction(&WasmInstruction::I32Const(0));
            func.instruction(&WasmInstruction::LocalSet(5)); // copy_offset = 0
            func.instruction(&WasmInstruction::Block(BlockType::Empty));
            func.instruction(&WasmInstruction::Loop(BlockType::Empty));
            // copy_offset >= 12 + num_props * 32?
            func.instruction(&WasmInstruction::LocalGet(5));
            func.instruction(&WasmInstruction::LocalGet(4));
            func.instruction(&WasmInstruction::I32Const(32));
            func.instruction(&WasmInstruction::I32Mul);
            func.instruction(&WasmInstruction::I32Const(16));
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::I32GeU);
            func.instruction(&WasmInstruction::BrIf(1)); // break
            // new_ptr[copy_offset] = old_ptr[copy_offset] (i32)
            func.instruction(&WasmInstruction::LocalGet(8));
            func.instruction(&WasmInstruction::LocalGet(5));
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::LocalGet(6));
            func.instruction(&WasmInstruction::LocalGet(5));
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::I32Load(MemArg {
                offset: 0,
                align: 2,
                memory_index: 0,
            }));
            func.instruction(&WasmInstruction::I32Store(MemArg {
                offset: 0,
                align: 2,
                memory_index: 0,
            }));
            // copy_offset += 4
            func.instruction(&WasmInstruction::LocalGet(5));
            func.instruction(&WasmInstruction::I32Const(4));
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::LocalSet(5));
            func.instruction(&WasmInstruction::Br(0));
            func.instruction(&WasmInstruction::End); // end loop
            func.instruction(&WasmInstruction::End); // end block

            // 更新 handle 表：obj_table[handle_idx] = new_ptr
            func.instruction(&WasmInstruction::GlobalGet(obj_table_global));
            func.instruction(&WasmInstruction::LocalGet(9));
            func.instruction(&WasmInstruction::I32Const(4));
            func.instruction(&WasmInstruction::I32Mul);
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::LocalGet(8));
            func.instruction(&WasmInstruction::I32Store(MemArg {
                offset: 0,
                align: 2,
                memory_index: 0,
            }));

            // 更新 header 中的 capacity
            func.instruction(&WasmInstruction::LocalGet(8));
            func.instruction(&WasmInstruction::LocalGet(7));
            func.instruction(&WasmInstruction::I32Store(MemArg {
                offset: 8,
                align: 2,
                memory_index: 0,
            }));

            func.instruction(&WasmInstruction::End); // end if reallocation

            // 添加新属性（无论是否扩容）
            func.instruction(&WasmInstruction::LocalGet(8));
            func.instruction(&WasmInstruction::I32Const(16));
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::LocalGet(4));
            func.instruction(&WasmInstruction::I32Const(32));
            func.instruction(&WasmInstruction::I32Mul);
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::LocalTee(6));
            func.instruction(&WasmInstruction::LocalGet(1));
            func.instruction(&WasmInstruction::I32Store(MemArg {
                offset: 0,
                align: 2,
                memory_index: 0,
            }));
            func.instruction(&WasmInstruction::LocalGet(6));
            // 默认 flags: configurable | enumerable | writable
            func.instruction(&WasmInstruction::I32Const(
                constants::FLAG_CONFIGURABLE
                    | constants::FLAG_ENUMERABLE
                    | constants::FLAG_WRITABLE,
            ));
            func.instruction(&WasmInstruction::I32Store(MemArg {
                offset: 4,
                align: 2,
                memory_index: 0,
            }));
            func.instruction(&WasmInstruction::LocalGet(6));
            func.instruction(&WasmInstruction::LocalGet(2));
            func.instruction(&WasmInstruction::I64Store(MemArg {
                offset: 8,
                align: 3,
                memory_index: 0,
            }));
            // 初始化 getter 和 setter 为 undefined（防御性）
            func.instruction(&WasmInstruction::LocalGet(6));
            func.instruction(&WasmInstruction::I64Const(value::encode_undefined()));
            func.instruction(&WasmInstruction::I64Store(MemArg {
                offset: 16,
                align: 3,
                memory_index: 0,
            }));
            func.instruction(&WasmInstruction::LocalGet(6));
            func.instruction(&WasmInstruction::I64Const(value::encode_undefined()));
            func.instruction(&WasmInstruction::I64Store(MemArg {
                offset: 24,
                align: 3,
                memory_index: 0,
            }));
            // num_props++
            func.instruction(&WasmInstruction::LocalGet(8));
            func.instruction(&WasmInstruction::LocalGet(4));
            func.instruction(&WasmInstruction::I32Const(1));
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::I32Store(MemArg {
                offset: 12,
                align: 2,
                memory_index: 0,
            }));

            func.instruction(&WasmInstruction::End); // end function
            self.codes.function(&func);
        }

        // ── $obj_delete (param $boxed i64) (param $name_id i32) (result i64) — Type 8 ──
        // 通过 handle 表解析 boxed value，删除属性。返回 NaN-boxed bool。
        {
            // local 0 = $boxed (i64), local 1 = $name_id (i32)
            // local 2 = num_props (i32), local 3 = i (i32), local 4 = slot_addr (i32)
            // local 5 = resolved ptr (i32), local 6 = last_slot_addr (i32)
            let mut func = Function::new(vec![(5, ValType::I32)]);

            // ── 通过 handle 表解析 ptr（支持 TAG_OBJECT 和 TAG_FUNCTION）──
            func.instruction(&WasmInstruction::Block(BlockType::Empty));
            func.instruction(&WasmInstruction::LocalGet(0));
            func.instruction(&WasmInstruction::I64Const(32));
            func.instruction(&WasmInstruction::I64ShrU);
            func.instruction(&WasmInstruction::I32WrapI64);
            func.instruction(&WasmInstruction::I32Const(0x1F));
            func.instruction(&WasmInstruction::I32And);
            func.instruction(&WasmInstruction::LocalTee(3));
            func.instruction(&WasmInstruction::I32Const(value::TAG_FUNCTION as i32));
            func.instruction(&WasmInstruction::I32Eq);
            func.instruction(&WasmInstruction::If(BlockType::Empty));
            func.instruction(&WasmInstruction::LocalGet(0));
            func.instruction(&WasmInstruction::I32WrapI64);
            func.instruction(&WasmInstruction::GlobalGet(num_ir_functions_global));
            func.instruction(&WasmInstruction::I32LtU);
            func.instruction(&WasmInstruction::If(BlockType::Empty));
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
            func.instruction(&WasmInstruction::LocalSet(5));
            func.instruction(&WasmInstruction::Br(2));
            func.instruction(&WasmInstruction::End);
            func.instruction(&WasmInstruction::I64Const(value::encode_bool(false)));
            func.instruction(&WasmInstruction::Return);
            func.instruction(&WasmInstruction::End);
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
            func.instruction(&WasmInstruction::LocalSet(5));
            func.instruction(&WasmInstruction::End);

            // ptr == 0 → return false
            func.instruction(&WasmInstruction::LocalGet(5));
            func.instruction(&WasmInstruction::I32Eqz);
            func.instruction(&WasmInstruction::If(BlockType::Empty));
            func.instruction(&WasmInstruction::I64Const(value::encode_bool(false)));
            func.instruction(&WasmInstruction::Return);
            func.instruction(&WasmInstruction::End);

            // 搜索属性
            func.instruction(&WasmInstruction::LocalGet(5));
            func.instruction(&WasmInstruction::I32Load(MemArg {
                offset: 12,
                align: 2,
                memory_index: 0,
            }));
            func.instruction(&WasmInstruction::LocalSet(2));
            func.instruction(&WasmInstruction::I32Const(0));
            func.instruction(&WasmInstruction::LocalSet(3));
            func.instruction(&WasmInstruction::Block(BlockType::Empty));
            func.instruction(&WasmInstruction::Loop(BlockType::Empty));
            func.instruction(&WasmInstruction::LocalGet(3));
            func.instruction(&WasmInstruction::LocalGet(2));
            func.instruction(&WasmInstruction::I32GeU);
            func.instruction(&WasmInstruction::BrIf(1));

            // slot_addr = ptr + 12 + i * 32
            func.instruction(&WasmInstruction::LocalGet(5));
            func.instruction(&WasmInstruction::I32Const(16));
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::LocalGet(3));
            func.instruction(&WasmInstruction::I32Const(32));
            func.instruction(&WasmInstruction::I32Mul);
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::LocalTee(4));
            func.instruction(&WasmInstruction::I32Load(MemArg {
                offset: 0,
                align: 2,
                memory_index: 0,
            }));
            func.instruction(&WasmInstruction::LocalTee(6));
            func.instruction(&WasmInstruction::LocalGet(1));
            func.instruction(&WasmInstruction::I32Eq);
            func.instruction(&WasmInstruction::LocalGet(6));
            func.instruction(&WasmInstruction::LocalGet(1));
            func.instruction(&WasmInstruction::Call(self.string_eq_func_idx));
            func.instruction(&WasmInstruction::I32Or);
            func.instruction(&WasmInstruction::If(BlockType::Empty));

            // 检查 configurable 标志 (flags bit 0)
            func.instruction(&WasmInstruction::LocalGet(4)); // slot_addr
            func.instruction(&WasmInstruction::I32Load(MemArg {
                offset: 4,
                align: 2,
                memory_index: 0,
            })); // flags
            func.instruction(&WasmInstruction::I32Const(1));
            func.instruction(&WasmInstruction::I32And); // flags & 1
            func.instruction(&WasmInstruction::I32Eqz); // (flags & 1) == 0 → not configurable
            func.instruction(&WasmInstruction::If(BlockType::Empty));
            func.instruction(&WasmInstruction::I64Const(value::encode_bool(false)));
            func.instruction(&WasmInstruction::Return);
            func.instruction(&WasmInstruction::End);
            // 找到！执行 swap-remove
            // num_props--
            func.instruction(&WasmInstruction::LocalGet(2));
            func.instruction(&WasmInstruction::I32Const(1));
            func.instruction(&WasmInstruction::I32Sub);
            func.instruction(&WasmInstruction::LocalTee(2));
            func.instruction(&WasmInstruction::LocalGet(5));
            func.instruction(&WasmInstruction::I32Store(MemArg {
                offset: 12,
                align: 2,
                memory_index: 0,
            }));

            // 如果 i < num_props（减后），将最后一个槽复制到当前位置
            func.instruction(&WasmInstruction::LocalGet(3));
            func.instruction(&WasmInstruction::LocalGet(2));
            func.instruction(&WasmInstruction::I32LtU);
            func.instruction(&WasmInstruction::If(BlockType::Empty));

            // last_slot_addr = ptr + 12 + num_props * 32
            func.instruction(&WasmInstruction::LocalGet(5));
            func.instruction(&WasmInstruction::I32Const(16));
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::LocalGet(2));
            func.instruction(&WasmInstruction::I32Const(32));
            func.instruction(&WasmInstruction::I32Mul);
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::LocalSet(6));

            // 复制 name_id
            func.instruction(&WasmInstruction::LocalGet(4));
            func.instruction(&WasmInstruction::LocalGet(6));
            func.instruction(&WasmInstruction::I32Load(MemArg {
                offset: 0,
                align: 2,
                memory_index: 0,
            }));
            func.instruction(&WasmInstruction::I32Store(MemArg {
                offset: 0,
                align: 2,
                memory_index: 0,
            }));

            // 复制 flags
            func.instruction(&WasmInstruction::LocalGet(4));
            func.instruction(&WasmInstruction::LocalGet(6));
            func.instruction(&WasmInstruction::I32Load(MemArg {
                offset: 4,
                align: 2,
                memory_index: 0,
            }));
            func.instruction(&WasmInstruction::I32Store(MemArg {
                offset: 4,
                align: 2,
                memory_index: 0,
            }));

            // 复制 value
            func.instruction(&WasmInstruction::LocalGet(4));
            func.instruction(&WasmInstruction::LocalGet(6));
            func.instruction(&WasmInstruction::I64Load(MemArg {
                offset: 8,
                align: 3,
                memory_index: 0,
            }));
            func.instruction(&WasmInstruction::I64Store(MemArg {
                offset: 8,
                align: 3,
                memory_index: 0,
            }));

            // 复制 getter
            func.instruction(&WasmInstruction::LocalGet(4));
            func.instruction(&WasmInstruction::LocalGet(6));
            func.instruction(&WasmInstruction::I64Load(MemArg {
                offset: 16,
                align: 3,
                memory_index: 0,
            }));
            func.instruction(&WasmInstruction::I64Store(MemArg {
                offset: 16,
                align: 3,
                memory_index: 0,
            }));

            // 复制 setter
            func.instruction(&WasmInstruction::LocalGet(4));
            func.instruction(&WasmInstruction::LocalGet(6));
            func.instruction(&WasmInstruction::I64Load(MemArg {
                offset: 24,
                align: 3,
                memory_index: 0,
            }));
            func.instruction(&WasmInstruction::I64Store(MemArg {
                offset: 24,
                align: 3,
                memory_index: 0,
            }));
            func.instruction(&WasmInstruction::End);

            // 返回 true
            func.instruction(&WasmInstruction::I64Const(value::encode_bool(true)));
            func.instruction(&WasmInstruction::Return);
            func.instruction(&WasmInstruction::End);

            // 继续搜索
            func.instruction(&WasmInstruction::LocalGet(3));
            func.instruction(&WasmInstruction::I32Const(1));
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::LocalSet(3));
            func.instruction(&WasmInstruction::Br(0));
            func.instruction(&WasmInstruction::End);
            func.instruction(&WasmInstruction::End);

            // 未找到 - 返回 false
            func.instruction(&WasmInstruction::I64Const(value::encode_bool(false)));
            func.instruction(&WasmInstruction::End);
            self.codes.function(&func);
        }

        // ── $to_int32 (param $val i64) (result i32) — Type 10 ──
        // Proper JS ToInt32: NaN/±Inf/sentinels → 0; numbers → ToInt32(wrap mod 2³²)
        {
            // local 0 = $val (i64, input), local 1 = f64 scratch
            let mut func = Function::new(vec![(1, ValType::F64)]);

            // Check: is this a raw f64 (not a NaN-box sentinel)?
            // is_f64: (val & 0x7FF8000000000000) != 0x7FF8000000000000
            func.instruction(&WasmInstruction::LocalGet(0));
            func.instruction(&WasmInstruction::I64Const(0x7FF8000000000000u64 as i64));
            func.instruction(&WasmInstruction::I64And);
            func.instruction(&WasmInstruction::I64Const(0x7FF8000000000000u64 as i64));
            func.instruction(&WasmInstruction::I64Ne);
            func.instruction(&WasmInstruction::If(BlockType::Empty));

            // Raw f64 path — convert to f64
            func.instruction(&WasmInstruction::LocalGet(0));
            func.instruction(&WasmInstruction::F64ReinterpretI64);
            func.instruction(&WasmInstruction::LocalTee(1));

            // NaN check: f != f
            func.instruction(&WasmInstruction::LocalGet(1));
            func.instruction(&WasmInstruction::F64Ne);
            func.instruction(&WasmInstruction::If(BlockType::Empty));
            func.instruction(&WasmInstruction::I32Const(0));
            func.instruction(&WasmInstruction::Return);
            func.instruction(&WasmInstruction::End);

            // ±Inf check: abs(f) == inf
            func.instruction(&WasmInstruction::LocalGet(1));
            func.instruction(&WasmInstruction::F64Abs);
            func.instruction(&WasmInstruction::F64Const(f64::INFINITY.into()));
            func.instruction(&WasmInstruction::F64Eq);
            func.instruction(&WasmInstruction::If(BlockType::Empty));
            func.instruction(&WasmInstruction::I32Const(0));
            func.instruction(&WasmInstruction::Return);
            func.instruction(&WasmInstruction::End);

            // Fast path: |f| < 2^31 → safe i32.trunc_f64_s
            func.instruction(&WasmInstruction::LocalGet(1));
            func.instruction(&WasmInstruction::F64Abs);
            func.instruction(&WasmInstruction::F64Const(2147483648.0f64.into())); // 2^31
            func.instruction(&WasmInstruction::F64Lt);
            func.instruction(&WasmInstruction::If(BlockType::Empty));
            func.instruction(&WasmInstruction::LocalGet(1));
            func.instruction(&WasmInstruction::I32TruncF64S);
            func.instruction(&WasmInstruction::Return);
            func.instruction(&WasmInstruction::End);

            // Medium path: |f| < 2^53 → i64.trunc_f64_s + mask 32 bits
            func.instruction(&WasmInstruction::LocalGet(1));
            func.instruction(&WasmInstruction::F64Abs);
            func.instruction(&WasmInstruction::F64Const(9007199254740992.0f64.into())); // 2^53
            func.instruction(&WasmInstruction::F64Lt);
            func.instruction(&WasmInstruction::If(BlockType::Empty));
            func.instruction(&WasmInstruction::LocalGet(1));
            func.instruction(&WasmInstruction::I64TruncF64S);
            func.instruction(&WasmInstruction::I64Const(0xFFFFFFFF));
            func.instruction(&WasmInstruction::I64And);
            func.instruction(&WasmInstruction::I32WrapI64);
            func.instruction(&WasmInstruction::Return);
            func.instruction(&WasmInstruction::End);

            // Large value path: manual modulo 2^32
            // mod = f - trunc(f / 2^32) * 2^32, then adjust if negative
            func.instruction(&WasmInstruction::LocalGet(1));
            func.instruction(&WasmInstruction::F64Const(4294967296.0f64.into())); // 2^32
            func.instruction(&WasmInstruction::F64Div);
            func.instruction(&WasmInstruction::F64Trunc);
            func.instruction(&WasmInstruction::F64Const(4294967296.0f64.into()));
            func.instruction(&WasmInstruction::F64Mul);
            func.instruction(&WasmInstruction::F64Neg);
            func.instruction(&WasmInstruction::LocalGet(1));
            func.instruction(&WasmInstruction::F64Add); // mod = f - trunc(f/2^32)*2^32
            func.instruction(&WasmInstruction::LocalTee(1));

            // If mod < 0: add 2^32
            func.instruction(&WasmInstruction::F64Const(0.0.into()));
            func.instruction(&WasmInstruction::F64Lt);
            func.instruction(&WasmInstruction::If(BlockType::Empty));
            func.instruction(&WasmInstruction::LocalGet(1));
            func.instruction(&WasmInstruction::F64Const(4294967296.0f64.into()));
            func.instruction(&WasmInstruction::F64Add);
            func.instruction(&WasmInstruction::LocalSet(1));
            func.instruction(&WasmInstruction::End);

            // Now mod in [0, 2^32) — use unsigned truncation
            func.instruction(&WasmInstruction::LocalGet(1));
            func.instruction(&WasmInstruction::I32TruncF64U);
            func.instruction(&WasmInstruction::Return);

            func.instruction(&WasmInstruction::End); // end raw f64 if

            // Not a raw number (sentinel) -> return 0
            func.instruction(&WasmInstruction::I32Const(0));
            func.instruction(&WasmInstruction::End);
            self.codes.function(&func);
        }

        // ── $str_eq (param $a i32) (param $b i32) (result i32) — Type 26 ──
        // 比较两个 nul-terminated 字符串内容，允许不同 module 使用不同 offset 表示同一属性名。
        {
            // local 0 = a, local 1 = b, local 2 = byte_a, local 3 = byte_b
            let mut func = Function::new(vec![(2, ValType::I32)]);
            func.instruction(&WasmInstruction::Block(BlockType::Empty));
            func.instruction(&WasmInstruction::Loop(BlockType::Empty));

            func.instruction(&WasmInstruction::LocalGet(0));
            func.instruction(&WasmInstruction::I32Load8U(MemArg {
                offset: 0,
                align: 0,
                memory_index: 0,
            }));
            func.instruction(&WasmInstruction::LocalTee(2));
            func.instruction(&WasmInstruction::LocalGet(1));
            func.instruction(&WasmInstruction::I32Load8U(MemArg {
                offset: 0,
                align: 0,
                memory_index: 0,
            }));
            func.instruction(&WasmInstruction::LocalTee(3));
            func.instruction(&WasmInstruction::I32Ne);
            func.instruction(&WasmInstruction::If(BlockType::Empty));
            func.instruction(&WasmInstruction::I32Const(0));
            func.instruction(&WasmInstruction::Return);
            func.instruction(&WasmInstruction::End);

            func.instruction(&WasmInstruction::LocalGet(2));
            func.instruction(&WasmInstruction::I32Eqz);
            func.instruction(&WasmInstruction::If(BlockType::Empty));
            func.instruction(&WasmInstruction::I32Const(1));
            func.instruction(&WasmInstruction::Return);
            func.instruction(&WasmInstruction::End);

            func.instruction(&WasmInstruction::LocalGet(0));
            func.instruction(&WasmInstruction::I32Const(1));
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::LocalSet(0));
            func.instruction(&WasmInstruction::LocalGet(1));
            func.instruction(&WasmInstruction::I32Const(1));
            func.instruction(&WasmInstruction::I32Add);
            func.instruction(&WasmInstruction::LocalSet(1));
            func.instruction(&WasmInstruction::Br(0));

            func.instruction(&WasmInstruction::End);
            func.instruction(&WasmInstruction::End);
            func.instruction(&WasmInstruction::I32Const(1));
            func.instruction(&WasmInstruction::End);
            self.codes.function(&func);
        }
    }

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
            // 不是 TAG_ARRAY → 委托给 $obj_get 进行属性访问
            func.instruction(&WasmInstruction::LocalGet(0));
            func.instruction(&WasmInstruction::LocalGet(1));
            func.instruction(&WasmInstruction::Call(self.obj_get_func_idx));
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
            // 不是 TAG_ARRAY → 委托给 $obj_set 进行属性设置
            func.instruction(&WasmInstruction::LocalGet(0));
            func.instruction(&WasmInstruction::LocalGet(1));
            func.instruction(&WasmInstruction::LocalGet(2));
            func.instruction(&WasmInstruction::Call(self.obj_set_func_idx));
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
