// obj_get / obj_set bodies — included from support_module.rs
// 移植自 compiler_helpers.rs::compile_object_helpers

fn emit_obj_get(flavor: GcFlavor) -> Function {
    // local 0 = $boxed (i64), local 1 = $name_id (i32)
    // local 2 = num_props (i32), local 3 = i (i32), local 4 = slot_addr (i32)
    // local 5 = resolved ptr (i32), local 6 = flags (i32), local 7 = getter (i64)
    // local 8 = getter env_obj (i64), local 9 = getter func_idx (i32)
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
    func.instruction(&WasmInstruction::I32Const(value::TAG_PROXY as i32));
    func.instruction(&WasmInstruction::I32Eq);
    func.instruction(&WasmInstruction::If(BlockType::Empty));
    func.instruction(&WasmInstruction::LocalGet(0));
    func.instruction(&WasmInstruction::LocalGet(1));
    func.instruction(&WasmInstruction::Call(
        HOST_PROXY_TRAP_GET,
    ));
    func.instruction(&WasmInstruction::Return);
    func.instruction(&WasmInstruction::End);
    func.instruction(&WasmInstruction::LocalGet(3));
    func.instruction(&WasmInstruction::I32Const(value::TAG_FUNCTION as i32));
    func.instruction(&WasmInstruction::I32Eq);
    func.instruction(&WasmInstruction::If(BlockType::Empty));
    func.instruction(&WasmInstruction::LocalGet(0));
    func.instruction(&WasmInstruction::I32WrapI64);
    func.instruction(&WasmInstruction::GlobalGet(G_NUM_IR_FUNCTIONS));
    func.instruction(&WasmInstruction::I32LtU);
    func.instruction(&WasmInstruction::If(BlockType::Empty));
    func.instruction(&WasmInstruction::LocalGet(0));
    func.instruction(&WasmInstruction::I32WrapI64);
    func.instruction(&WasmInstruction::GlobalGet(G_FUNCTION_PROPS_BASE));
    func.instruction(&WasmInstruction::I32Add);
    func.instruction(&WasmInstruction::LocalSet(4));
    emit_resolve_handle_ptr(&mut func, flavor, 4, 5);
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
    // 对 undefined/null/bool 等标量类型直接返回 undefined；string/bigint/symbol/regexp
    // 在下方按各自原型语义分派，避免把 NaN-boxed 低位误当 obj_table handle。
    // 保留 TAG_OBJECT(8)、TAG_FUNCTION(9)、TAG_ARRAY(11)、TAG_EXCEPTION(5)、
    // TAG_ITERATOR(6)、TAG_ENUMERATOR(7) 等对象类型通过。
    func.instruction(&WasmInstruction::LocalGet(3));
    func.instruction(&WasmInstruction::I32Const(value::TAG_UNDEFINED as i32));
    func.instruction(&WasmInstruction::I32Eq);
    func.instruction(&WasmInstruction::LocalGet(3));
    func.instruction(&WasmInstruction::I32Const(value::TAG_NULL as i32));
    func.instruction(&WasmInstruction::I32Eq);
    func.instruction(&WasmInstruction::I32Or);
    func.instruction(&WasmInstruction::If(BlockType::Empty));
    func.instruction(&WasmInstruction::I64Const(value::encode_undefined()));
    func.instruction(&WasmInstruction::Return);
    func.instruction(&WasmInstruction::End);
    // TAG_STRING：String 原始值的 length / 原型属性
    func.instruction(&WasmInstruction::LocalGet(3));
    func.instruction(&WasmInstruction::I32Const(value::TAG_STRING as i32));
    func.instruction(&WasmInstruction::I32Eq);
    func.instruction(&WasmInstruction::If(BlockType::Empty));
    func.instruction(&WasmInstruction::LocalGet(0));
    func.instruction(&WasmInstruction::LocalGet(1));
    func.instruction(&WasmInstruction::Call(HOST_PRIMITIVE_STRING_GET_PROPERTY));
    func.instruction(&WasmInstruction::Return);
    func.instruction(&WasmInstruction::End);

    // TAG_BIGINT：BigInt.prototype 方法名 → NativeCallable
    func.instruction(&WasmInstruction::LocalGet(3));
    func.instruction(&WasmInstruction::I32Const(value::TAG_BIGINT as i32));
    func.instruction(&WasmInstruction::I32Eq);
    func.instruction(&WasmInstruction::If(BlockType::Empty));
    func.instruction(&WasmInstruction::LocalGet(0));
    func.instruction(&WasmInstruction::LocalGet(1));
    func.instruction(&WasmInstruction::Call(
        HOST_PRIMITIVE_BIGINT_GET_METHOD,
    ));
    func.instruction(&WasmInstruction::Return);
    func.instruction(&WasmInstruction::End);

    // TAG_SYMBOL：Symbol.prototype 方法 / description
    func.instruction(&WasmInstruction::LocalGet(3));
    func.instruction(&WasmInstruction::I32Const(value::TAG_SYMBOL as i32));
    func.instruction(&WasmInstruction::I32Eq);
    func.instruction(&WasmInstruction::If(BlockType::Empty));
    func.instruction(&WasmInstruction::LocalGet(0));
    func.instruction(&WasmInstruction::LocalGet(1));
    func.instruction(&WasmInstruction::Call(
        HOST_PRIMITIVE_SYMBOL_GET_PROPERTY,
    ));
    func.instruction(&WasmInstruction::Return);
    func.instruction(&WasmInstruction::End);

    // TAG_REGEXP：RegExp.prototype 方法 / accessor-like data
    func.instruction(&WasmInstruction::LocalGet(3));
    func.instruction(&WasmInstruction::I32Const(value::TAG_REGEXP as i32));
    func.instruction(&WasmInstruction::I32Eq);
    func.instruction(&WasmInstruction::If(BlockType::Empty));
    func.instruction(&WasmInstruction::LocalGet(0));
    func.instruction(&WasmInstruction::LocalGet(1));
    func.instruction(&WasmInstruction::Call(
        HOST_PRIMITIVE_REGEXP_GET_PROPERTY,
    ));
    func.instruction(&WasmInstruction::Return);
    func.instruction(&WasmInstruction::End);

    // raw f64：Number.prototype 方法名 → NativeCallable
    func.instruction(&WasmInstruction::Block(BlockType::Empty));
    func.instruction(&WasmInstruction::LocalGet(0));
    func.instruction(&WasmInstruction::I64Const(value::BOX_BASE as i64));
    func.instruction(&WasmInstruction::I64And);
    func.instruction(&WasmInstruction::I64Const(value::BOX_BASE as i64));
    func.instruction(&WasmInstruction::I64Ne);
    func.instruction(&WasmInstruction::If(BlockType::Empty));
    func.instruction(&WasmInstruction::LocalGet(0));
    func.instruction(&WasmInstruction::LocalGet(1));
    func.instruction(&WasmInstruction::Call(
        HOST_PRIMITIVE_NUMBER_GET_METHOD,
    ));
    func.instruction(&WasmInstruction::Return);
    func.instruction(&WasmInstruction::End);
    func.instruction(&WasmInstruction::End);
    func.instruction(&WasmInstruction::LocalGet(3));
    func.instruction(&WasmInstruction::I32Const(
        value::TAG_NATIVE_CALLABLE as i32,
    ));
    func.instruction(&WasmInstruction::I32Eq);
    func.instruction(&WasmInstruction::If(BlockType::Empty));
    func.instruction(&WasmInstruction::LocalGet(0));
    func.instruction(&WasmInstruction::LocalGet(1));
    func.instruction(&WasmInstruction::Call(
        HOST_NATIVE_CALLABLE_GET_PROPERTY,
    ));
    func.instruction(&WasmInstruction::Return);
    func.instruction(&WasmInstruction::End);
    func.instruction(&WasmInstruction::LocalGet(0));
    func.instruction(&WasmInstruction::I32WrapI64);
    func.instruction(&WasmInstruction::LocalTee(4));
    emit_handle_bounds_check(&mut func, 4, Some(value::encode_undefined()));
    func.instruction(&WasmInstruction::Drop);
    emit_resolve_handle_ptr(&mut func, flavor, 4, 5);
    func.instruction(&WasmInstruction::End);
    // ptr == 0 → return undefined
    func.instruction(&WasmInstruction::LocalGet(5));
    func.instruction(&WasmInstruction::I32Eqz);
    func.instruction(&WasmInstruction::If(BlockType::Empty));
    func.instruction(&WasmInstruction::I64Const(value::encode_undefined()));
    func.instruction(&WasmInstruction::Return);
    func.instruction(&WasmInstruction::End);
    emit_runtime_string_name_id_test(&mut func, 1);
    func.instruction(&WasmInstruction::If(BlockType::Empty));
    func.instruction(&WasmInstruction::LocalGet(0));
    func.instruction(&WasmInstruction::LocalGet(1));
    func.instruction(&WasmInstruction::Call(HOST_OBJ_GET_RUNTIME_KEY));
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
    func.instruction(&WasmInstruction::I32Const(constants::PRIMORDIAL_LENGTH_OFFSET as i32));
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
    // 数组上的命名属性（symbol + 字符串）→ 宿主侧表。
    // 找到（非 undefined）即返回；未找到则落入原型链遍历以解析 Array.prototype 方法（如 .map）。
    func.instruction(&WasmInstruction::Block(BlockType::Empty));
    func.instruction(&WasmInstruction::LocalGet(5));
    func.instruction(&WasmInstruction::I32Load8U(MemArg {
        offset: 4,
        align: 0,
        memory_index: 0,
    }));
    func.instruction(&WasmInstruction::I32Const(wjsm_ir::HEAP_TYPE_ARRAY as i32));
    func.instruction(&WasmInstruction::I32Eq);
    func.instruction(&WasmInstruction::If(BlockType::Empty));
    func.instruction(&WasmInstruction::LocalGet(0));
    func.instruction(&WasmInstruction::LocalGet(1));
    func.instruction(&WasmInstruction::Call(HOST_ARRAY_NAMED_GET));
    func.instruction(&WasmInstruction::LocalTee(7));
    func.instruction(&WasmInstruction::I64Const(value::encode_undefined()));
    func.instruction(&WasmInstruction::I64Ne);
    func.instruction(&WasmInstruction::If(BlockType::Empty));
    func.instruction(&WasmInstruction::LocalGet(7));
    func.instruction(&WasmInstruction::Return);
    func.instruction(&WasmInstruction::End);
    func.instruction(&WasmInstruction::End);
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
    emit_property_name_id_match(&mut func, 6, 1);
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
    // 普通属性访问必须跳过类私有成员槽。
    func.instruction(&WasmInstruction::I32Const(constants::FLAG_PRIVATE));
    func.instruction(&WasmInstruction::I32And);
    func.instruction(&WasmInstruction::I32Eqz);
    func.instruction(&WasmInstruction::If(BlockType::Empty));
    func.instruction(&WasmInstruction::LocalGet(6));
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
    // 调用 getter: 检查是否为 NativeCallable
    func.instruction(&WasmInstruction::LocalGet(7));
    func.instruction(&WasmInstruction::I64Const(32));
    func.instruction(&WasmInstruction::I64ShrU);
    func.instruction(&WasmInstruction::I64Const(0x1F));
    func.instruction(&WasmInstruction::I64And);
    func.instruction(&WasmInstruction::I64Const(
        value::TAG_NATIVE_CALLABLE as i64,
    ));
    func.instruction(&WasmInstruction::I64Eq);
    func.instruction(&WasmInstruction::If(BlockType::Result(ValType::I64)));
    // NativeCallable: 直接通过宿主调用
    func.instruction(&WasmInstruction::LocalGet(7)); // getter (callee)
    func.instruction(&WasmInstruction::LocalGet(0)); // this_val
    func.instruction(&WasmInstruction::I32Const(0)); // args_base
    func.instruction(&WasmInstruction::I32Const(0)); // args_count
    func.instruction(&WasmInstruction::Call(
        HOST_NATIVE_CALL,
    ));
    func.instruction(&WasmInstruction::Else);
    // 闭包或普通函数: resolve callable + call_indirect
    emit_resolve_callable_for_helper(&mut func, 7, 9, 8);
    func.instruction(&WasmInstruction::LocalGet(8)); // env_obj
    func.instruction(&WasmInstruction::LocalGet(0)); // this_val
    func.instruction(&WasmInstruction::I32Const(0)); // args_base
    func.instruction(&WasmInstruction::I32Const(0)); // args_count
    func.instruction(&WasmInstruction::LocalGet(9)); // func_idx
    func.instruction(&WasmInstruction::CallIndirect {
        type_index: crate::shared_types::JS_FUNC_TYPE_INDEX,
        table_index: 0,
    });
    func.instruction(&WasmInstruction::End);
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
    // 如果 proto_handle == -1 或 0（null sentinel），退出循环
    func.instruction(&WasmInstruction::LocalGet(3));
    func.instruction(&WasmInstruction::I32Eqz);
    func.instruction(&WasmInstruction::LocalGet(3));
    func.instruction(&WasmInstruction::I32Const(-1));
    func.instruction(&WasmInstruction::I32Eq);
    func.instruction(&WasmInstruction::I32Or);
    func.instruction(&WasmInstruction::BrIf(1));
    // 通过 handle 表解析 proto_handle → proto_ptr
    // 检查 proto_handle 高位是否为 proxy 标记
    func.instruction(&WasmInstruction::LocalGet(3));
    func.instruction(&WasmInstruction::I32Const(i32::MIN));
    func.instruction(&WasmInstruction::I32And);
    func.instruction(&WasmInstruction::If(BlockType::Empty));
    func.instruction(&WasmInstruction::LocalGet(3));
    func.instruction(&WasmInstruction::I32Const(i32::MAX));
    func.instruction(&WasmInstruction::I32And);
    func.instruction(&WasmInstruction::I64ExtendI32U);
    func.instruction(&WasmInstruction::I64Const(
        (value::BOX_BASE | (value::TAG_PROXY << 32)) as i64,
    ));
    func.instruction(&WasmInstruction::I64Or);
    func.instruction(&WasmInstruction::LocalGet(1));
    func.instruction(&WasmInstruction::Call(HOST_PROXY_TRAP_GET));
    func.instruction(&WasmInstruction::Return);
    func.instruction(&WasmInstruction::End);
    emit_resolve_handle_ptr(&mut func, flavor, 3, 5);
    func.instruction(&WasmInstruction::Br(0));
    func.instruction(&WasmInstruction::End);
    func.instruction(&WasmInstruction::End);
    // 未找到
    func.instruction(&WasmInstruction::I64Const(value::encode_undefined()));
    func.instruction(&WasmInstruction::End);
    func
}

fn emit_obj_set(flavor: GcFlavor) -> Function {
    // local 0 = $boxed (i64), local 1 = $name_id (i32), local 2 = $value (i64)
    // local 3 = proto_handle (i32, reused from unused pad)
    // local 4 = num_props (i32), local 5 = i (i32), local 6 = slot_addr (i32), local 7 = capacity (i32)
    // local 4 = num_props (i32), local 5 = i (i32), local 6 = slot_addr (i32), local 7 = capacity (i32)
    // local 8 = resolved ptr (i32), local 9 = handle_idx (i32), local 10 = flags (i32), local 11 = setter (i64)
    // local 12 = shadow_sp_scratch (i32), local 13 = setter func_idx (i32), local 14 = proto_ptr (i32), local 15 = setter env_obj (i64)
    let mut func = Function::new(vec![
        (8, ValType::I32),
        (1, ValType::I64),
        (3, ValType::I32),
        (1, ValType::I64),
        (1, ValType::I32),
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
    func.instruction(&WasmInstruction::I32Const(value::TAG_PROXY as i32));
    func.instruction(&WasmInstruction::I32Eq);
    func.instruction(&WasmInstruction::If(BlockType::Empty));
    func.instruction(&WasmInstruction::LocalGet(0));
    func.instruction(&WasmInstruction::LocalGet(1));
    func.instruction(&WasmInstruction::LocalGet(2));
    func.instruction(&WasmInstruction::Call(
        HOST_PROXY_TRAP_SET,
    ));
    func.instruction(&WasmInstruction::Return);
    func.instruction(&WasmInstruction::End);
    func.instruction(&WasmInstruction::LocalGet(5));
    func.instruction(&WasmInstruction::I32Const(value::TAG_REGEXP as i32));
    func.instruction(&WasmInstruction::I32Eq);
    func.instruction(&WasmInstruction::If(BlockType::Empty));
    func.instruction(&WasmInstruction::LocalGet(0));
    func.instruction(&WasmInstruction::LocalGet(1));
    func.instruction(&WasmInstruction::LocalGet(2));
    func.instruction(&WasmInstruction::Call(
        HOST_PRIMITIVE_REGEXP_SET_PROPERTY,
    ));
    func.instruction(&WasmInstruction::Return);
    func.instruction(&WasmInstruction::End);
    func.instruction(&WasmInstruction::LocalGet(5));
    func.instruction(&WasmInstruction::I32Const(value::TAG_FUNCTION as i32));
    func.instruction(&WasmInstruction::I32Eq);
    func.instruction(&WasmInstruction::If(BlockType::Empty));
    func.instruction(&WasmInstruction::LocalGet(0));
    func.instruction(&WasmInstruction::I32WrapI64);
    func.instruction(&WasmInstruction::GlobalGet(G_NUM_IR_FUNCTIONS));
    func.instruction(&WasmInstruction::I32LtU);
    func.instruction(&WasmInstruction::If(BlockType::Empty));
    func.instruction(&WasmInstruction::LocalGet(0));
    func.instruction(&WasmInstruction::I32WrapI64);
    func.instruction(&WasmInstruction::LocalTee(9));
    func.instruction(&WasmInstruction::GlobalGet(G_FUNCTION_PROPS_BASE));
    func.instruction(&WasmInstruction::I32Add);
    func.instruction(&WasmInstruction::LocalSet(9));
    emit_resolve_handle_ptr(&mut func, flavor, 9, 8);
    func.instruction(&WasmInstruction::Br(2));
    func.instruction(&WasmInstruction::End);
    func.instruction(&WasmInstruction::Return);
    func.instruction(&WasmInstruction::End);
    func.instruction(&WasmInstruction::LocalGet(0));
    func.instruction(&WasmInstruction::I32WrapI64);
    func.instruction(&WasmInstruction::LocalTee(9));
    emit_handle_bounds_check(&mut func, 9, None);
    func.instruction(&WasmInstruction::Drop);
    emit_resolve_handle_ptr(&mut func, flavor, 9, 8);
    func.instruction(&WasmInstruction::End);
    // 数组 length 赋值：ECMAScript §23.1.3.2 ArraySetLength
    func.instruction(&WasmInstruction::LocalGet(8));
    func.instruction(&WasmInstruction::I32Load8U(MemArg {
        offset: 4,
        align: 0,
        memory_index: 0,
    }));
    func.instruction(&WasmInstruction::I32Const(wjsm_ir::HEAP_TYPE_ARRAY as i32));
    func.instruction(&WasmInstruction::I32Eq);
    func.instruction(&WasmInstruction::LocalGet(1));
    func.instruction(&WasmInstruction::I32Const(constants::PRIMORDIAL_LENGTH_OFFSET as i32));
    func.instruction(&WasmInstruction::I32Eq);
    func.instruction(&WasmInstruction::I32And);
    func.instruction(&WasmInstruction::If(BlockType::Empty));
    func.instruction(&WasmInstruction::LocalGet(0));
    func.instruction(&WasmInstruction::LocalGet(2));
    func.instruction(&WasmInstruction::Call(HOST_ARRAY_SET_LENGTH));
    func.instruction(&WasmInstruction::Return);
    func.instruction(&WasmInstruction::End);
    // 数组命名属性（symbol + 字符串，length 已在上方处理）→ 宿主侧表。
    // 数组元素通过 elem_get/elem_set 单独处理，故此处 name_id 必为非索引命名属性。
    func.instruction(&WasmInstruction::Block(BlockType::Empty));
    func.instruction(&WasmInstruction::LocalGet(8));
    func.instruction(&WasmInstruction::I32Load8U(MemArg {
        offset: 4,
        align: 0,
        memory_index: 0,
    }));
    func.instruction(&WasmInstruction::I32Const(wjsm_ir::HEAP_TYPE_ARRAY as i32));
    func.instruction(&WasmInstruction::I32Eq);
    func.instruction(&WasmInstruction::If(BlockType::Empty));
    func.instruction(&WasmInstruction::LocalGet(0));
    func.instruction(&WasmInstruction::LocalGet(1));
    func.instruction(&WasmInstruction::LocalGet(2));
    func.instruction(&WasmInstruction::Call(HOST_ARRAY_NAMED_SET));
    func.instruction(&WasmInstruction::Return);
    func.instruction(&WasmInstruction::End);
    func.instruction(&WasmInstruction::End);
    emit_runtime_string_name_id_test(&mut func, 1);
    func.instruction(&WasmInstruction::If(BlockType::Empty));
    func.instruction(&WasmInstruction::LocalGet(0));
    func.instruction(&WasmInstruction::LocalGet(1));
    func.instruction(&WasmInstruction::LocalGet(2));
    func.instruction(&WasmInstruction::Call(HOST_OBJ_SET_RUNTIME_KEY));
    func.instruction(&WasmInstruction::Return);
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
    emit_property_name_id_match(&mut func, 10, 1);
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
    // 普通属性写入必须跳过类私有成员槽。
    func.instruction(&WasmInstruction::I32Const(constants::FLAG_PRIVATE));
    func.instruction(&WasmInstruction::I32And);
    func.instruction(&WasmInstruction::I32Eqz);
    func.instruction(&WasmInstruction::If(BlockType::Empty));
    func.instruction(&WasmInstruction::LocalGet(10));
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
    emit_resolve_callable_for_helper(&mut func, 11, 13, 15);
    // 需要将 value (local 2) 写入影子栈
    // 保存 shadow_sp 到 local 12
    func.instruction(&WasmInstruction::GlobalGet(G_SHADOW_SP));
    func.instruction(&WasmInstruction::LocalSet(12));
    // 写入 value 到影子栈
    func.instruction(&WasmInstruction::GlobalGet(G_SHADOW_SP));
    func.instruction(&WasmInstruction::LocalGet(2)); // value
    func.instruction(&WasmInstruction::I64Store(MemArg {
        offset: 0,
        align: 3,
        memory_index: 0,
    }));
    // shadow_sp += 8 (虽然这里只有1个参数，但保持一致性)
    func.instruction(&WasmInstruction::GlobalGet(G_SHADOW_SP));
    func.instruction(&WasmInstruction::I32Const(8));
    func.instruction(&WasmInstruction::I32Add);
    func.instruction(&WasmInstruction::GlobalSet(G_SHADOW_SP));
    // 推入参数: env_obj, this_val (local 0), args_base (local 12), args_count (1)
    func.instruction(&WasmInstruction::LocalGet(15)); // env_obj
    func.instruction(&WasmInstruction::LocalGet(0)); // this_val
    func.instruction(&WasmInstruction::LocalGet(12)); // args_base
    func.instruction(&WasmInstruction::I32Const(1)); // args_count
    func.instruction(&WasmInstruction::LocalGet(13)); // func_idx
    // call_indirect type 12, table 0
    func.instruction(&WasmInstruction::CallIndirect {
        type_index: crate::shared_types::JS_FUNC_TYPE_INDEX,
        table_index: 0,
    });
    // 恢复 shadow_sp
    func.instruction(&WasmInstruction::LocalGet(12));
    func.instruction(&WasmInstruction::GlobalSet(G_SHADOW_SP));
    func.instruction(&WasmInstruction::Drop); // 丢弃返回值
    func.instruction(&WasmInstruction::Return);
    func.instruction(&WasmInstruction::End);
    // 是数据属性，检查 writable 位
    func.instruction(&WasmInstruction::LocalGet(10));
    func.instruction(&WasmInstruction::I32Const(constants::FLAG_WRITABLE));
    func.instruction(&WasmInstruction::I32And);
    func.instruction(&WasmInstruction::I32Eqz);
    func.instruction(&WasmInstruction::If(BlockType::Empty));
    // 不可写：sloppy 模式静默失败
    func.instruction(&WasmInstruction::Return);
    func.instruction(&WasmInstruction::End);
    // 可写，更新 value (offset 8)
    emit_reference_barrier_event(&mut func, flavor, 6, 8, 2);
    func.instruction(&WasmInstruction::LocalGet(6));
    func.instruction(&WasmInstruction::LocalGet(2));
    func.instruction(&WasmInstruction::I64Store(MemArg {
        offset: 8,
        align: 3,
        memory_index: 0,
    }));
    func.instruction(&WasmInstruction::Return);
    func.instruction(&WasmInstruction::End);
    func.instruction(&WasmInstruction::End);
    func.instruction(&WasmInstruction::LocalGet(5));
    func.instruction(&WasmInstruction::I32Const(1));
    func.instruction(&WasmInstruction::I32Add);
    func.instruction(&WasmInstruction::LocalSet(5));
    func.instruction(&WasmInstruction::Br(0));
    func.instruction(&WasmInstruction::End);
    func.instruction(&WasmInstruction::End);
    // ── 原型链遍历：查找可能需要调用的 setter ──
    func.instruction(&WasmInstruction::Block(BlockType::Empty)); // proto_chain_done
    // 读取当前对象的 proto_handle
    func.instruction(&WasmInstruction::LocalGet(8));
    func.instruction(&WasmInstruction::I32Load(MemArg {
        offset: 0,
        align: 2,
        memory_index: 0,
    }));
    func.instruction(&WasmInstruction::LocalSet(3)); // proto_handle
    // 如果 proto_handle == 0 或 -1，跳过原型链遍历
    func.instruction(&WasmInstruction::LocalGet(3));
    func.instruction(&WasmInstruction::I32Eqz);
    func.instruction(&WasmInstruction::LocalGet(3));
    func.instruction(&WasmInstruction::I32Const(-1));
    func.instruction(&WasmInstruction::I32Eq);
    func.instruction(&WasmInstruction::I32Or);
    func.instruction(&WasmInstruction::BrIf(0)); // 跳出 proto_chain_done → fall through
    emit_resolve_handle_ptr(&mut func, flavor, 3, 14);
    func.instruction(&WasmInstruction::Loop(BlockType::Empty)); // proto_chain_loop
    // 搜索 proto 对象的 own properties
    func.instruction(&WasmInstruction::LocalGet(14));
    func.instruction(&WasmInstruction::I32Load(MemArg {
        offset: 12,
        align: 2,
        memory_index: 0,
    }));
    func.instruction(&WasmInstruction::LocalSet(4)); // num_props
    func.instruction(&WasmInstruction::I32Const(0));
    func.instruction(&WasmInstruction::LocalSet(5)); // i = 0
    func.instruction(&WasmInstruction::Block(BlockType::Empty)); // search_exit
    func.instruction(&WasmInstruction::Loop(BlockType::Empty)); // search_loop
    func.instruction(&WasmInstruction::LocalGet(5));
    func.instruction(&WasmInstruction::LocalGet(4));
    func.instruction(&WasmInstruction::I32GeU);
    func.instruction(&WasmInstruction::BrIf(1)); // 跳出 search_exit
    // slot_addr = proto_ptr + 16 + i * 32
    func.instruction(&WasmInstruction::LocalGet(14));
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
    emit_property_name_id_match(&mut func, 10, 1);
    func.instruction(&WasmInstruction::If(BlockType::Empty)); // name_found
    // 在原型上找到属性，检查是否为 accessor
    func.instruction(&WasmInstruction::LocalGet(6));
    func.instruction(&WasmInstruction::I32Load(MemArg {
        offset: 4,
        align: 2,
        memory_index: 0,
    }));
    func.instruction(&WasmInstruction::LocalTee(10));
    // 原型链上的普通 setter 查找也必须跳过类私有成员槽。
    func.instruction(&WasmInstruction::I32Const(constants::FLAG_PRIVATE));
    func.instruction(&WasmInstruction::I32And);
    func.instruction(&WasmInstruction::I32Eqz);
    func.instruction(&WasmInstruction::If(BlockType::Empty));
    func.instruction(&WasmInstruction::LocalGet(10));
    func.instruction(&WasmInstruction::I32Const(constants::FLAG_IS_ACCESSOR));
    func.instruction(&WasmInstruction::I32And);
    func.instruction(&WasmInstruction::I32Const(0));
    func.instruction(&WasmInstruction::I32Ne);
    func.instruction(&WasmInstruction::If(BlockType::Empty)); // is_accessor
    // 是 accessor 属性，加载 setter (offset 24)
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
    func.instruction(&WasmInstruction::If(BlockType::Empty)); // no_setter
    // getter-only accessor，直接返回（不创建 own property）
    func.instruction(&WasmInstruction::Return);
    func.instruction(&WasmInstruction::End); // end no_setter
    // 调用 setter: 检查是否为 NativeCallable
    func.instruction(&WasmInstruction::LocalGet(11));
    func.instruction(&WasmInstruction::I64Const(32));
    func.instruction(&WasmInstruction::I64ShrU);
    func.instruction(&WasmInstruction::I64Const(0x1F));
    func.instruction(&WasmInstruction::I64And);
    func.instruction(&WasmInstruction::I64Const(
        value::TAG_NATIVE_CALLABLE as i64,
    ));
    func.instruction(&WasmInstruction::I64Eq);
    func.instruction(&WasmInstruction::If(BlockType::Empty));
    // NativeCallable: 推入 value 到影子栈，通过宿主调用
    func.instruction(&WasmInstruction::GlobalGet(G_SHADOW_SP));
    func.instruction(&WasmInstruction::LocalSet(12));
    func.instruction(&WasmInstruction::GlobalGet(G_SHADOW_SP));
    func.instruction(&WasmInstruction::LocalGet(2)); // value
    func.instruction(&WasmInstruction::I64Store(MemArg {
        offset: 0,
        align: 3,
        memory_index: 0,
    }));
    func.instruction(&WasmInstruction::GlobalGet(G_SHADOW_SP));
    func.instruction(&WasmInstruction::I32Const(8));
    func.instruction(&WasmInstruction::I32Add);
    func.instruction(&WasmInstruction::GlobalSet(G_SHADOW_SP));
    func.instruction(&WasmInstruction::LocalGet(11)); // setter (callee)
    func.instruction(&WasmInstruction::LocalGet(0)); // this_val
    func.instruction(&WasmInstruction::LocalGet(12)); // args_base
    func.instruction(&WasmInstruction::I32Const(1)); // args_count
    func.instruction(&WasmInstruction::Call(
        HOST_NATIVE_CALL,
    ));
    // 恢复 shadow_sp
    func.instruction(&WasmInstruction::LocalGet(12));
    func.instruction(&WasmInstruction::GlobalSet(G_SHADOW_SP));
    func.instruction(&WasmInstruction::Drop);
    func.instruction(&WasmInstruction::Return);
    func.instruction(&WasmInstruction::Else);
    // 闭包或普通函数: resolve callable + call_indirect
    emit_resolve_callable_for_helper(&mut func, 11, 13, 15);
    // 将 value (local 2) 写入影子栈
    func.instruction(&WasmInstruction::GlobalGet(G_SHADOW_SP));
    func.instruction(&WasmInstruction::LocalSet(12));
    func.instruction(&WasmInstruction::GlobalGet(G_SHADOW_SP));
    func.instruction(&WasmInstruction::LocalGet(2)); // value
    func.instruction(&WasmInstruction::I64Store(MemArg {
        offset: 0,
        align: 3,
        memory_index: 0,
    }));
    func.instruction(&WasmInstruction::GlobalGet(G_SHADOW_SP));
    func.instruction(&WasmInstruction::I32Const(8));
    func.instruction(&WasmInstruction::I32Add);
    func.instruction(&WasmInstruction::GlobalSet(G_SHADOW_SP));
    // 推入参数: env_obj, this_val (local 0), args_base (local 12), args_count (1)
    func.instruction(&WasmInstruction::LocalGet(15)); // env_obj
    func.instruction(&WasmInstruction::LocalGet(0)); // this_val
    func.instruction(&WasmInstruction::LocalGet(12)); // args_base
    func.instruction(&WasmInstruction::I32Const(1)); // args_count
    func.instruction(&WasmInstruction::LocalGet(13)); // func_idx
    func.instruction(&WasmInstruction::CallIndirect {
        type_index: crate::shared_types::JS_FUNC_TYPE_INDEX,
        table_index: 0,
    });
    // 恢复 shadow_sp
    func.instruction(&WasmInstruction::LocalGet(12));
    func.instruction(&WasmInstruction::GlobalSet(G_SHADOW_SP));
    func.instruction(&WasmInstruction::Drop);
    func.instruction(&WasmInstruction::Return);
    func.instruction(&WasmInstruction::End);
    func.instruction(&WasmInstruction::End); // end is_accessor
    // 是数据属性 → 跳出原型链遍历，fall through 到创建 own data property
    // br depth: If(private_ok)=0, If(name_found)=1, Loop(search_loop)=2, Block(search_exit)=3, Loop(proto_chain_loop)=4, Block(proto_chain_done)=5
    func.instruction(&WasmInstruction::Br(5));
    func.instruction(&WasmInstruction::End);
    func.instruction(&WasmInstruction::End); // end name_found
    func.instruction(&WasmInstruction::LocalGet(5));
    func.instruction(&WasmInstruction::I32Const(1));
    func.instruction(&WasmInstruction::I32Add);
    func.instruction(&WasmInstruction::LocalSet(5));
    func.instruction(&WasmInstruction::Br(0)); // continue search_loop
    func.instruction(&WasmInstruction::End); // end search_loop
    func.instruction(&WasmInstruction::End); // end search_exit
    // 未在当前 proto 上找到属性 → 遍历到下一个 proto
    func.instruction(&WasmInstruction::LocalGet(14));
    func.instruction(&WasmInstruction::I32Load(MemArg {
        offset: 0,
        align: 2,
        memory_index: 0,
    }));
    func.instruction(&WasmInstruction::LocalSet(3)); // proto_handle
    // 如果 proto_handle == 0 或 -1，跳出原型链
    func.instruction(&WasmInstruction::LocalGet(3));
    func.instruction(&WasmInstruction::I32Eqz);
    func.instruction(&WasmInstruction::LocalGet(3));
    func.instruction(&WasmInstruction::I32Const(-1));
    func.instruction(&WasmInstruction::I32Eq);
    func.instruction(&WasmInstruction::I32Or);
    func.instruction(&WasmInstruction::BrIf(1)); // 跳出 proto_chain_done
    // 解析下一个 proto_handle → proto_ptr
    emit_resolve_handle_ptr(&mut func, flavor, 3, 14);
    func.instruction(&WasmInstruction::Br(0)); // continue proto_chain_loop
    func.instruction(&WasmInstruction::End); // end proto_chain_loop
    func.instruction(&WasmInstruction::End); // end proto_chain_done
    // 恢复 num_props 为原始对象的值（原型链遍历可能覆盖了 local 4）
    func.instruction(&WasmInstruction::LocalGet(8));
    func.instruction(&WasmInstruction::I32Load(MemArg {
        offset: 12,
        align: 2,
        memory_index: 0,
    }));
    func.instruction(&WasmInstruction::LocalSet(4));
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
    // 分配扩容后的新区域；fast-path 失败时由 gc_alloc_slow 负责 GC/grow/OOM。
    emit_heap_bump_for_object_resize_support(&mut func, 7, 16, 8);
    // GC slow-path 可能移动正在扩容的对象；拷贝前必须用 handle 重新解析 old_ptr。
    emit_resolve_handle_ptr(&mut func, flavor, 9, 6);

    // 拷贝旧数据到新内存：memory.copy(dst=new_ptr, src=old_ptr, len=16+num_props*32)
    func.instruction(&WasmInstruction::LocalGet(8));
    func.instruction(&WasmInstruction::LocalGet(6));
    func.instruction(&WasmInstruction::LocalGet(4));
    func.instruction(&WasmInstruction::I32Const(32));
    func.instruction(&WasmInstruction::I32Mul);
    func.instruction(&WasmInstruction::I32Const(16));
    func.instruction(&WasmInstruction::I32Add);
    func.instruction(&WasmInstruction::MemoryCopy {
        src_mem: 0,
        dst_mem: 0,
    });
    // 更新 handle 表：obj_table[handle_idx] = new_ptr
    func.instruction(&WasmInstruction::GlobalGet(G_OBJ_TABLE_PTR));
    func.instruction(&WasmInstruction::LocalGet(9));
    func.instruction(&WasmInstruction::I32Const(4));
    func.instruction(&WasmInstruction::I32Mul);
    func.instruction(&WasmInstruction::I32Add);
    emit_obj_table_entry_value(&mut func, flavor, 8);
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
    // 不可扩展对象不能添加新属性（header offset 5）
    func.instruction(&WasmInstruction::LocalGet(8));
    func.instruction(&WasmInstruction::I32Load8U(MemArg {
        offset: 5,
        align: 0,
        memory_index: 0,
    }));
    func.instruction(&WasmInstruction::I32Const(0));
    func.instruction(&WasmInstruction::I32Ne);
    func.instruction(&WasmInstruction::If(BlockType::Empty));
    func.instruction(&WasmInstruction::Return);
    func.instruction(&WasmInstruction::End);
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
    emit_reference_barrier_event(&mut func, flavor, 6, 8, 2);
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
    func
}

// ── obj_delete (param $boxed i64) (param $name_id i32) (result i64) — Type 1 ──
// 移植自 compiler_helpers.rs::compile_object_helpers obj_delete 段。
// 返回 NaN-boxed bool。
fn emit_obj_delete(flavor: GcFlavor) -> Function {
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
    func.instruction(&WasmInstruction::I32Const(value::TAG_PROXY as i32));
    func.instruction(&WasmInstruction::I32Eq);
    func.instruction(&WasmInstruction::If(BlockType::Empty));
    func.instruction(&WasmInstruction::LocalGet(0));
    func.instruction(&WasmInstruction::LocalGet(1));
    func.instruction(&WasmInstruction::Call(HOST_PROXY_TRAP_DELETE));
    func.instruction(&WasmInstruction::Return);
    func.instruction(&WasmInstruction::End);
    func.instruction(&WasmInstruction::LocalGet(3));
    func.instruction(&WasmInstruction::I32Const(value::TAG_REGEXP as i32));
    func.instruction(&WasmInstruction::I32Eq);
    func.instruction(&WasmInstruction::If(BlockType::Empty));
    func.instruction(&WasmInstruction::I64Const(value::encode_bool(true)));
    func.instruction(&WasmInstruction::Return);
    func.instruction(&WasmInstruction::End);
    func.instruction(&WasmInstruction::LocalGet(3));
    func.instruction(&WasmInstruction::I32Const(value::TAG_FUNCTION as i32));
    func.instruction(&WasmInstruction::I32Eq);
    func.instruction(&WasmInstruction::If(BlockType::Empty));
    func.instruction(&WasmInstruction::LocalGet(0));
    func.instruction(&WasmInstruction::I32WrapI64);
    func.instruction(&WasmInstruction::GlobalGet(G_NUM_IR_FUNCTIONS));
    func.instruction(&WasmInstruction::I32LtU);
    func.instruction(&WasmInstruction::If(BlockType::Empty));
    func.instruction(&WasmInstruction::LocalGet(0));
    func.instruction(&WasmInstruction::I32WrapI64);
    func.instruction(&WasmInstruction::GlobalGet(G_FUNCTION_PROPS_BASE));
    func.instruction(&WasmInstruction::I32Add);
    func.instruction(&WasmInstruction::LocalSet(4));
    emit_resolve_handle_ptr(&mut func, flavor, 4, 5);
    func.instruction(&WasmInstruction::Br(2));
    func.instruction(&WasmInstruction::End);
    func.instruction(&WasmInstruction::I64Const(value::encode_bool(false)));
    func.instruction(&WasmInstruction::Return);
    func.instruction(&WasmInstruction::End);
    func.instruction(&WasmInstruction::LocalGet(0));
    func.instruction(&WasmInstruction::I32WrapI64);
    func.instruction(&WasmInstruction::LocalTee(4));
    emit_handle_bounds_check(&mut func, 4, Some(value::encode_bool(false)));
    func.instruction(&WasmInstruction::Drop);
    emit_resolve_handle_ptr(&mut func, flavor, 4, 5);
    func.instruction(&WasmInstruction::End);

    // ptr == 0 → return false
    func.instruction(&WasmInstruction::LocalGet(5));
    func.instruction(&WasmInstruction::I32Eqz);
    func.instruction(&WasmInstruction::If(BlockType::Empty));
    func.instruction(&WasmInstruction::I64Const(value::encode_bool(false)));
    func.instruction(&WasmInstruction::Return);
    func.instruction(&WasmInstruction::End);
    emit_runtime_string_name_id_test(&mut func, 1);
    func.instruction(&WasmInstruction::If(BlockType::Empty));
    func.instruction(&WasmInstruction::LocalGet(0));
    func.instruction(&WasmInstruction::LocalGet(1));
    func.instruction(&WasmInstruction::Call(HOST_OBJ_DELETE_RUNTIME_KEY));
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
    emit_property_name_id_match(&mut func, 6, 1);
    func.instruction(&WasmInstruction::If(BlockType::Empty));

    // 普通 delete 必须跳过类私有成员槽。
    func.instruction(&WasmInstruction::LocalGet(4)); // slot_addr
    func.instruction(&WasmInstruction::I32Load(MemArg {
        offset: 4,
        align: 2,
        memory_index: 0,
    })); // flags
    func.instruction(&WasmInstruction::LocalTee(6));
    func.instruction(&WasmInstruction::I32Const(constants::FLAG_PRIVATE));
    func.instruction(&WasmInstruction::I32And);
    func.instruction(&WasmInstruction::I32Eqz);
    func.instruction(&WasmInstruction::If(BlockType::Empty));

    // 检查 configurable 标志 (flags bit 0)
    func.instruction(&WasmInstruction::LocalGet(6));
    func.instruction(&WasmInstruction::I32Const(constants::FLAG_CONFIGURABLE));
    func.instruction(&WasmInstruction::I32And); // flags & configurable
    func.instruction(&WasmInstruction::I32Eqz); // (flags & configurable) == 0 → not configurable
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
    func.instruction(&WasmInstruction::End);

    // 继续搜索
    func.instruction(&WasmInstruction::LocalGet(3));
    func.instruction(&WasmInstruction::I32Const(1));
    func.instruction(&WasmInstruction::I32Add);
    func.instruction(&WasmInstruction::LocalSet(3));
    func.instruction(&WasmInstruction::Br(0));
    func.instruction(&WasmInstruction::End);
    func.instruction(&WasmInstruction::End);

    // 未找到 → 返回 true（delete 不存在属性视为成功）
    func.instruction(&WasmInstruction::I64Const(value::encode_bool(true)));
    func.instruction(&WasmInstruction::End);
    func
}
