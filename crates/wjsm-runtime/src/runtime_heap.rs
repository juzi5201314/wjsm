use super::*;
use crate::wasm_env::WasmEnv;

use wjsm_ir::{SHADOW_STACK_SIZE, constants};

/// handle 表上界（止于 shadow stack 基址），与 WASM emit_handle_table_alloc_check 一致。
fn handle_table_end_byte<C: AsContextMut<Data = RuntimeState>>(
    env: &WasmEnv,
    ctx: &mut C,
) -> usize {
    let Some(g) = env.shadow_stack_end else {
        return env.memory.data(&*ctx).len();
    };
    let end = g.get(&mut *ctx).i32().unwrap_or(0).max(0) as usize;
    end.saturating_sub(SHADOW_STACK_SIZE as usize)
}

pub(crate) fn host_handle_slot_fits<C: AsContextMut<Data = RuntimeState>>(
    env: &WasmEnv,
    ctx: &mut C,
    candidate: u32,
) -> bool {
    let obj_table_ptr = env.obj_table_ptr.get(&mut *ctx).i32().unwrap_or(0).max(0) as usize;
    let need_end = obj_table_ptr
        .saturating_add(
            (candidate as usize).saturating_mul(constants::HANDLE_TABLE_ENTRY_SIZE as usize),
        )
        .saturating_add(constants::HANDLE_TABLE_ENTRY_SIZE as usize);
    need_end <= handle_table_end_byte(env, ctx)
}

fn heap_limit_bytes<C: AsContextMut<Data = RuntimeState>>(ctx: &mut C, env: &WasmEnv) -> usize {
    env.heap_limit
        .and_then(|g| g.get(&mut *ctx).i32())
        .map(|v| v as u32 as usize)
        .unwrap_or(u32::MAX as usize)
}

pub(crate) fn heap_used_bytes<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
) -> usize {
    let heap_start = env
        .object_heap_start
        .and_then(|g| g.get(&mut *ctx).i32())
        .unwrap_or(0)
        .max(0) as usize;
    let heap_ptr = env.heap_ptr.get(&mut *ctx).i32().unwrap_or(0).max(0) as usize;
    heap_ptr.saturating_sub(heap_start)
}

fn collect_for_host_allocation_pressure<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
) {
    let gc_arc = ctx.as_context().data().gc_algorithm.clone();
    let mut gc = gc_arc.lock().unwrap_or_else(|e| e.into_inner());
    let mut gc_ctx = crate::runtime_gc::GcContext::new(ctx, env, gc.name());
    let mut roots = crate::runtime_gc::roots::RuntimeRoots;
    let stats = gc.collect_full(&mut gc_ctx, &mut roots as _);
    ctx.as_context().data().store_last_gc_stats(stats);
}

/// 线性内存不足时按页扩展，供 host 侧 bump / resize 使用；同时遵守 JS 堆预算。
pub(crate) fn ensure_heap_allocation_bytes<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    heap_ptr: usize,
    size: usize,
) -> bool {
    let Some(need_end) = heap_ptr.checked_add(size) else {
        let used = heap_used_bytes(ctx, env);
        ctx.as_context().data().set_heap_oom_error(used, size);
        return false;
    };

    if need_end > heap_limit_bytes(ctx, env) {
        collect_for_host_allocation_pressure(ctx, env);
        if need_end > heap_limit_bytes(ctx, env) {
            let used = heap_used_bytes(ctx, env);
            ctx.as_context().data().set_heap_oom_error(used, size);
            return false;
        }
    }

    while env.memory.data(&*ctx).len() < need_end {
        let current = env.memory.data(&*ctx).len();
        let pages = (need_end - current).div_ceil(65536).max(1) as u64;
        if env.memory.grow(&mut *ctx, pages).is_err() {
            break;
        }
    }
    if need_end > env.memory.data(&*ctx).len() {
        collect_for_host_allocation_pressure(ctx, env);
        while env.memory.data(&*ctx).len() < need_end {
            let current = env.memory.data(&*ctx).len();
            let pages = (need_end - current).div_ceil(65536).max(1) as u64;
            if env.memory.grow(&mut *ctx, pages).is_err() {
                break;
            }
        }
        if need_end > env.memory.data(&*ctx).len() {
            let used = heap_used_bytes(ctx, env);
            ctx.as_context().data().set_heap_oom_error(used, size);
            return false;
        }
    }
    true
}

fn alloc_host_object_impl<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    capacity: u32,
    proto: u32,
) -> i64 {
    let size = constants::HEAP_OBJECT_HEADER_SIZE
        .saturating_add(capacity.saturating_mul(constants::HEAP_OBJECT_PROPERTY_SLOT_SIZE));
    let Some(ptr) =
        alloc_heap_region_for_host(ctx, env, size as usize, wjsm_ir::HEAP_TYPE_OBJECT, capacity)
    else {
        return value::encode_undefined();
    };
    let heap_ptr = ptr as u32;
    let obj_table_count = env.obj_table_count.get(&mut *ctx).i32().unwrap_or(0) as u32;
    let obj_table_ptr = env.obj_table_ptr.get(&mut *ctx).i32().unwrap_or(0) as u32;
    if !host_handle_slot_fits(env, ctx, obj_table_count) {
        return value::encode_undefined();
    }
    let ptr = heap_ptr as usize;
    let slot_addr = obj_table_ptr as usize
        + obj_table_count as usize * constants::HANDLE_TABLE_ENTRY_SIZE as usize;
    {
        let data = env.memory.data_mut(&mut *ctx);
        data[ptr + constants::HEAP_OBJECT_PROTO_OFFSET as usize
            ..ptr + constants::HEAP_OBJECT_PROTO_OFFSET as usize + 4]
            .copy_from_slice(&proto.to_le_bytes());
        data[ptr + constants::HEAP_OBJECT_TYPE_OFFSET as usize] = wjsm_ir::HEAP_TYPE_OBJECT;
        data[ptr + constants::HEAP_OBJECT_HEADER_PAD_START as usize
            ..ptr + constants::HEAP_OBJECT_HEADER_PAD_END as usize]
            .fill(0);
        data[ptr + constants::HEAP_OBJECT_CAPACITY_OFFSET as usize
            ..ptr + constants::HEAP_OBJECT_CAPACITY_OFFSET as usize + 4]
            .copy_from_slice(&capacity.to_le_bytes());
        data[ptr + constants::HEAP_OBJECT_PROPERTY_COUNT_OFFSET as usize
            ..ptr + constants::HEAP_OBJECT_PROPERTY_COUNT_OFFSET as usize + 4]
            .copy_from_slice(&0u32.to_le_bytes());
        data[slot_addr..slot_addr + constants::HANDLE_TABLE_ENTRY_SIZE as usize]
            .copy_from_slice(&heap_ptr.to_le_bytes());
    }
    let _ = env
        .obj_table_count
        .set(&mut *ctx, Val::I32((obj_table_count + 1) as i32));
    value::encode_object_handle(obj_table_count)
}

pub(crate) fn alloc_heap_region_for_host<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    size: usize,
    heap_type: u8,
    capacity: u32,
) -> Option<usize> {
    let gc_arc = ctx.as_context().data().gc_algorithm.clone();
    {
        let mut gc = gc_arc.lock().unwrap_or_else(|e| e.into_inner());
        let mut roots = crate::runtime_gc::roots::RuntimeRoots;
        let mut gc_ctx = crate::runtime_gc::GcContext::new(ctx, env, gc.name());
        let req = crate::runtime_gc::api::AllocRequest {
            size,
            heap_type,
            capacity,
        };
        if let Some(ptr) = gc.alloc_slow(&mut gc_ctx, &mut roots as _, req) {
            return Some(ptr);
        }
    }
    let used = heap_used_bytes(ctx, env);
    ctx.as_context().data().set_heap_oom_error(used, size);
    None
}

pub(crate) fn alloc_host_object<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    capacity: u32,
) -> i64 {
    let proto = env.object_proto_handle.get(&mut *ctx).i32().unwrap_or(-1) as u32;
    alloc_host_object_impl(ctx, env, capacity, proto)
}

pub(crate) fn alloc_host_null_proto_object<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    capacity: u32,
) -> i64 {
    alloc_host_object_impl(ctx, env, capacity, u32::MAX)
}
/// 各 Error 子类 prototype 对象（bootstrap 后由 `ensure_error_prototypes_initialized` 填充）。
#[derive(Clone, Copy, Default)]
pub(crate) struct ErrorPrototypes {
    pub error: i64,
    pub type_error: i64,
    pub range_error: i64,
    pub syntax_error: i64,
    pub reference_error: i64,
    pub uri_error: i64,
    pub eval_error: i64,
    pub aggregate_error: i64,
}

impl ErrorPrototypes {
    pub(crate) fn is_initialized(self) -> bool {
        value::is_object(self.error)
    }

    pub(crate) fn proto_for_error_name(self, error_name: &str) -> Option<i64> {
        if !self.is_initialized() {
            return None;
        }
        let proto = match error_name {
            "Error" => self.error,
            "TypeError" => self.type_error,
            "RangeError" => self.range_error,
            "SyntaxError" => self.syntax_error,
            "ReferenceError" => self.reference_error,
            "URIError" => self.uri_error,
            "EvalError" => self.eval_error,
            "AggregateError" => self.aggregate_error,
            _ => self.error,
        };
        if value::is_object(proto) {
            Some(proto)
        } else {
            None
        }
    }
}

pub(crate) fn set_object_proto_header<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    obj: i64,
    proto: i64,
) {
    let Some(obj_ptr) =
        resolve_handle_idx_with_env(ctx, env, value::decode_object_handle(obj) as usize)
    else {
        return;
    };
    let proto_handle = if value::is_null(proto) {
        0xFFFF_FFFF
    } else if value::is_object(proto) {
        value::decode_object_handle(proto)
    } else {
        return;
    };
    let data = env.memory.data_mut(ctx);
    if obj_ptr + 4 > data.len() {
        return;
    }
    data[obj_ptr..obj_ptr + 4].copy_from_slice(&proto_handle.to_le_bytes());
}

/// ECMAScript §20.5.3.4 Error.prototype.toString
pub(crate) fn error_proto_to_string_impl(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
) -> i64 {
    if !value::is_object(this_val) {
        return make_type_error_exception(
            caller,
            "TypeError: Error.prototype.toString called on incompatible receiver",
        );
    }
    let obj_ptr = resolve_handle_idx(caller, value::decode_object_handle(this_val) as usize);
    let name_raw = obj_ptr.and_then(|p| read_object_property_by_name(caller, p, "name"));
    let name = if let Some(v) = name_raw {
        if value::is_undefined(v) {
            "Error".to_string()
        } else {
            eval_to_string(caller, v)
        }
    } else {
        "Error".to_string()
    };
    let message_raw = obj_ptr.and_then(|p| read_object_property_by_name(caller, p, "message"));
    let message = if let Some(v) = message_raw {
        if value::is_undefined(v) {
            String::new()
        } else {
            eval_to_string(caller, v)
        }
    } else {
        String::new()
    };
    if name.is_empty() {
        store_runtime_string(caller, message)
    } else if message.is_empty() {
        store_runtime_string(caller, name)
    } else {
        store_runtime_string(caller, format!("{name}: {message}"))
    }
}

/// 在 `__wjsm_bootstrap_once` 之后调用：建立 Error 原型链并挂到 RuntimeState。
pub(crate) fn ensure_error_prototypes_initialized<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
) {
    if ctx.as_context().data().error_prototypes.is_initialized() {
        return;
    }
    let object_proto_handle = env.object_proto_handle.get(&mut *ctx).i32().unwrap_or(-1);
    if object_proto_handle < 0 {
        return;
    }
    let object_proto = value::encode_object_handle(object_proto_handle as u32);

    let error_proto = alloc_host_object(ctx, env, 2);
    set_object_proto_header(ctx, env, error_proto, object_proto);
    let to_string =
        create_native_callable(ctx.as_context().data(), NativeCallable::ErrorProtoToString);
    let _ = define_host_data_property_with_env(ctx, env, error_proto, "toString", to_string);

    let mut make_subclass = |name: &str, parent_proto: i64| -> i64 {
        let proto = alloc_host_object(ctx, env, 1);
        set_object_proto_header(ctx, env, proto, parent_proto);
        let name_val = store_runtime_string_in_state(ctx.as_context().data(), name.to_string());
        let _ = define_host_data_property_with_env(ctx, env, proto, "name", name_val);
        proto
    };

    let type_error = make_subclass("TypeError", error_proto);
    let range_error = make_subclass("RangeError", error_proto);
    let syntax_error = make_subclass("SyntaxError", error_proto);
    let reference_error = make_subclass("ReferenceError", error_proto);
    let uri_error = make_subclass("URIError", error_proto);
    let eval_error = make_subclass("EvalError", error_proto);
    let aggregate_error = make_subclass("AggregateError", error_proto);

    ctx.as_context_mut().data_mut().error_prototypes = ErrorPrototypes {
        error: error_proto,
        type_error,
        range_error,
        syntax_error,
        reference_error,
        uri_error,
        eval_error,
        aggregate_error,
    };
}

/// ECMAScript §20.4.3.2 Symbol.prototype.description getter
pub(crate) fn symbol_proto_description_getter_impl(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
) -> i64 {
    if !value::is_symbol(this_val) {
        return make_type_error_exception(
            caller,
            "TypeError: Symbol.prototype.description getter called on incompatible receiver",
        );
    }
    let handle = value::decode_symbol_handle(this_val) as usize;
    let table = caller
        .data()
        .symbol_table
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let Some(entry) = table.get(handle) else {
        return value::encode_undefined();
    };
    match &entry.description {
        Some(desc) => store_runtime_string(caller, desc.clone()),
        None => value::encode_undefined(),
    }
}

/// ECMAScript §20.4.3.4 Symbol.prototype.toString
pub(crate) fn symbol_proto_to_string_impl(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
) -> i64 {
    if !value::is_symbol(this_val) {
        return make_type_error_exception(
            caller,
            "TypeError: Symbol.prototype.toString called on incompatible receiver",
        );
    }
    let handle = value::decode_symbol_handle(this_val) as usize;
    let table = caller
        .data()
        .symbol_table
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let s = if let Some(entry) = table.get(handle) {
        if let Some(desc) = &entry.description {
            format!("Symbol({desc})")
        } else {
            "Symbol()".to_string()
        }
    } else {
        "Symbol()".to_string()
    };
    store_runtime_string(caller, s)
}

/// ECMAScript §20.4.3.5 Symbol.prototype.valueOf
pub(crate) fn symbol_proto_value_of_impl(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
) -> i64 {
    if value::is_symbol(this_val) {
        this_val
    } else {
        make_type_error_exception(
            caller,
            "TypeError: Symbol.prototype.valueOf called on incompatible receiver",
        )
    }
}

/// `sym.*` 属性读取：委托 %SymbolPrototype%（含 description 访问器）。
pub(crate) fn primitive_symbol_get_property_impl(
    caller: &mut Caller<'_, RuntimeState>,
    boxed: i64,
    name_id: u32,
) -> i64 {
    if !value::is_symbol(boxed) {
        return value::encode_undefined();
    }
    let Some(env) = WasmEnv::from_caller(caller) else {
        return value::encode_undefined();
    };
    ensure_symbol_prototype_initialized(caller, &env);

    if name_id == encode_symbol_name_id(5) {
        return create_native_callable(caller.data(), NativeCallable::SymbolProtoToPrimitive);
    }
    if name_id == encode_symbol_name_id(2) {
        return store_runtime_string_in_state(caller.data(), "Symbol".to_string());
    }

    let key = read_string_bytes(caller, name_id);
    match key.as_slice() {
        b"toString" => create_native_callable(
            caller.data(),
            NativeCallable::SymbolPrimitiveMethod { method: 0 },
        ),
        b"valueOf" => create_native_callable(
            caller.data(),
            NativeCallable::SymbolPrimitiveMethod { method: 1 },
        ),
        b"description" => symbol_proto_description_getter_impl(caller, boxed),
        _ => value::encode_undefined(),
    }
}

/// 在 bootstrap 后建立 %SymbolPrototype% 并挂到 RuntimeState（供 Symbol 构造函数 .prototype）。
pub(crate) fn ensure_symbol_prototype_initialized<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
) {
    if value::is_object(ctx.as_context().data().symbol_prototype) {
        return;
    }
    let object_proto_handle = env.object_proto_handle.get(&mut *ctx).i32().unwrap_or(-1);
    if object_proto_handle < 0 {
        return;
    }
    let object_proto = value::encode_object_handle(object_proto_handle as u32);
    let symbol_proto = alloc_host_object(ctx, env, 6);
    set_object_proto_header(ctx, env, symbol_proto, object_proto);

    let to_string = create_native_callable(
        ctx.as_context().data(),
        NativeCallable::SymbolPrimitiveMethod { method: 0 },
    );
    let value_of = create_native_callable(
        ctx.as_context().data(),
        NativeCallable::SymbolPrimitiveMethod { method: 1 },
    );
    let description_getter = create_native_callable(
        ctx.as_context().data(),
        NativeCallable::SymbolProtoDescriptionGetter,
    );
    let to_primitive = create_native_callable(
        ctx.as_context().data(),
        NativeCallable::SymbolProtoToPrimitive,
    );
    let _ = define_host_data_property_with_env(ctx, env, symbol_proto, "toString", to_string);
    let _ = define_host_data_property_with_env(ctx, env, symbol_proto, "valueOf", value_of);
    let _ = define_host_accessor_property_with_env(
        ctx,
        env,
        symbol_proto,
        "description",
        description_getter,
        value::encode_undefined(),
    );
    let _ = define_host_data_property_by_name_id_with_env(
        ctx,
        env,
        symbol_proto,
        encode_symbol_name_id(5),
        to_primitive,
        constants::FLAG_CONFIGURABLE | constants::FLAG_WRITABLE,
    );
    let tag = store_runtime_string_in_state(ctx.as_context().data(), "Symbol".to_string());
    let _ = define_host_data_property_by_name_id_with_env(
        ctx,
        env,
        symbol_proto,
        encode_symbol_name_id(2),
        tag,
        constants::FLAG_CONFIGURABLE,
    );

    ctx.as_context_mut().data_mut().symbol_prototype = symbol_proto;
}

/// 在 bootstrap 后建立 %PromisePrototype% 并挂到 RuntimeState（供 Promise 构造函数 .prototype）。
/// 与 `ensure_symbol_prototype_initialized` 同构：proto = Object.prototype，
/// 定义 constructor（PromiseConstructor）+ [Symbol.toStringTag] = "Promise"。
/// `.then`/`.catch`/`.finally` 由编译器语法分派（builtin_from_promise_proto_method），
/// 不经过原型链查找，故此处不定义。
pub(crate) fn ensure_promise_prototype_initialized<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
) {
    if value::is_object(ctx.as_context().data().promise_prototype) {
        return;
    }
    let object_proto_handle = env.object_proto_handle.get(&mut *ctx).i32().unwrap_or(-1);
    if object_proto_handle < 0 {
        return;
    }
    let object_proto = value::encode_object_handle(object_proto_handle as u32);
    let promise_proto = alloc_host_object(ctx, env, 2);
    set_object_proto_header(ctx, env, promise_proto, object_proto);

    let ctor = create_native_callable(ctx.as_context().data(), NativeCallable::PromiseConstructor);
    let _ = define_host_data_property_with_env(ctx, env, promise_proto, "constructor", ctor);

    let tag = store_runtime_string_in_state(ctx.as_context().data(), "Promise".to_string());
    let _ = define_host_data_property_by_name_id_with_env(
        ctx,
        env,
        promise_proto,
        encode_symbol_name_id(2),
        tag,
        constants::FLAG_CONFIGURABLE,
    );

    ctx.as_context_mut().data_mut().promise_prototype = promise_proto;
}

pub(crate) fn native_callable_promise_prototype(
    caller: &mut Caller<'_, RuntimeState>,
    record: &NativeCallable,
) -> Option<i64> {
    if !matches!(record, NativeCallable::PromiseConstructor) {
        return None;
    }
    if !value::is_object(caller.data().promise_prototype) {
        if let Some(env) = WasmEnv::from_caller(caller) {
            ensure_promise_prototype_initialized(caller, &env);
        }
    }
    let proto = caller.data().promise_prototype;
    if value::is_object(proto) {
        Some(proto)
    } else {
        None
    }
}

pub(crate) fn native_callable_symbol_prototype(
    caller: &mut Caller<'_, RuntimeState>,
    record: &NativeCallable,
) -> Option<i64> {
    if !matches!(record, NativeCallable::SymbolConstructor) {
        return None;
    }
    if !value::is_object(caller.data().symbol_prototype) {
        if let Some(env) = WasmEnv::from_caller(caller) {
            ensure_symbol_prototype_initialized(caller, &env);
        }
    }
    let proto = caller.data().symbol_prototype;
    if value::is_object(proto) {
        Some(proto)
    } else {
        None
    }
}

/// 在 bootstrap 后建立 %RegExpPrototype% 并挂到 RuntimeState（供 RegExp 构造函数 .prototype
/// + instanceof 原型链遍历）。与 `ensure_promise_prototype_initialized` 同构：
/// proto = Object.prototype，定义 constructor + [Symbol.toStringTag] = "RegExp"。
/// RegExp 实例（TAG_REGEXP）的方法由 `primitive_regexp_get_property_impl` 分派，
/// 不经过原型链查找，故此处不定义 exec/test 等方法。
pub(crate) fn ensure_regexp_prototype_initialized<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
) {
    if value::is_object(ctx.as_context().data().regexp_prototype) {
        return;
    }
    let object_proto_handle = env.object_proto_handle.get(&mut *ctx).i32().unwrap_or(-1);
    if object_proto_handle < 0 {
        return;
    }
    let object_proto = value::encode_object_handle(object_proto_handle as u32);
    let regexp_proto = alloc_host_object(ctx, env, 2);
    set_object_proto_header(ctx, env, regexp_proto, object_proto);

    let ctor = create_native_callable(ctx.as_context().data(), NativeCallable::RegExpConstructor);
    let _ = define_host_data_property_with_env(ctx, env, regexp_proto, "constructor", ctor);

    let tag = store_runtime_string_in_state(ctx.as_context().data(), "RegExp".to_string());
    let _ = define_host_data_property_by_name_id_with_env(
        ctx,
        env,
        regexp_proto,
        encode_symbol_name_id(2),
        tag,
        constants::FLAG_CONFIGURABLE,
    );

    ctx.as_context_mut().data_mut().regexp_prototype = regexp_proto;
}

pub(crate) fn native_callable_regexp_prototype(
    caller: &mut Caller<'_, RuntimeState>,
    record: &NativeCallable,
) -> Option<i64> {
    if !matches!(record, NativeCallable::RegExpConstructor) {
        return None;
    }
    if !value::is_object(caller.data().regexp_prototype) {
        if let Some(env) = WasmEnv::from_caller(caller) {
            ensure_regexp_prototype_initialized(caller, &env);
        }
    }
    let proto = caller.data().regexp_prototype;
    if value::is_object(proto) {
        Some(proto)
    } else {
        None
    }
}

pub(crate) fn native_callable_error_prototype(
    caller: &mut Caller<'_, RuntimeState>,
    record: &NativeCallable,
) -> Option<i64> {
    let protos = {
        if !caller.data().error_prototypes.is_initialized() {
            if let Some(env) = WasmEnv::from_caller(caller) {
                ensure_error_prototypes_initialized(caller, &env);
            }
        }
        caller.data().error_prototypes
    };
    if !protos.is_initialized() {
        return None;
    }
    let proto = match record {
        NativeCallable::ErrorConstructor => protos.error,
        NativeCallable::TypeErrorConstructor => protos.type_error,
        NativeCallable::RangeErrorConstructor => protos.range_error,
        NativeCallable::SyntaxErrorConstructor => protos.syntax_error,
        NativeCallable::ReferenceErrorConstructor => protos.reference_error,
        NativeCallable::URIErrorConstructor => protos.uri_error,
        NativeCallable::EvalErrorConstructor => protos.eval_error,
        NativeCallable::AggregateErrorConstructor => protos.aggregate_error,
        _ => return None,
    };
    if value::is_object(proto) {
        Some(proto)
    } else {
        None
    }
}

/// 统一解析 native callable 构造器的 `.prototype` 值。
/// 供直接属性读取（native_callable_get_property）与 instanceof / Reflect.get
/// 反射路径（reflect_get_impl_with_receiver_async）共用，消除两条路径对
/// Object/Array/Symbol/Promise/RegExp/Error 原型分派不一致的缺陷。
/// 返回 `None` 表示该 record 不是已知构造器或原型尚未就绪。
pub(crate) fn native_callable_prototype(
    caller: &mut Caller<'_, RuntimeState>,
    record: &NativeCallable,
) -> Option<i64> {
    match record {
        NativeCallable::ObjectConstructor => {
            let env = WasmEnv::from_caller(caller)?;
            let handle = env
                .object_proto_handle
                .get(&mut *caller)
                .i32()
                .unwrap_or(-1);
            (handle >= 0).then(|| value::encode_object_handle(handle as u32))
        }
        NativeCallable::ArrayConstructor => {
            let env = WasmEnv::from_caller(caller)?;
            let handle = env.array_proto_handle.get(&mut *caller).i32().unwrap_or(-1);
            (handle >= 0).then(|| value::encode_object_handle(handle as u32))
        }
        NativeCallable::SymbolConstructor => native_callable_symbol_prototype(caller, record),
        NativeCallable::PromiseConstructor => native_callable_promise_prototype(caller, record),
        NativeCallable::RegExpConstructor => native_callable_regexp_prototype(caller, record),
        _ => native_callable_error_prototype(caller, record),
    }
}

/// 将 `message` 参数转换为 Error message 文本。
fn error_message_from_arg(caller: &mut Caller<'_, RuntimeState>, arg: i64) -> String {
    if value::is_undefined(arg) {
        String::new()
    } else if value::is_string(arg) {
        read_value_string_bytes(caller, arg)
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_default()
    } else if value::is_null(arg) {
        String::new()
    } else if value::is_f64(arg) {
        format_number_js(value::decode_f64(arg))
    } else if value::is_bool(arg) {
        if value::decode_bool(arg) {
            "true".to_string()
        } else {
            "false".to_string()
        }
    } else {
        String::new()
    }
}

/// 捕获当前 WASM 调用栈，格式化为 V8 风格的 stack 字符串。
/// 使用 wasmtime WasmBacktrace::force_capture 获取帧信息（后端 NameSection 提供函数名）。
fn capture_stack_trace<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    error_name: &str,
    message: &str,
) -> String {
    let mut stack = if message.is_empty() {
        error_name.to_string()
    } else {
        format!("{}: {}", error_name, message)
    };
    let backtrace = wasmtime::WasmBacktrace::force_capture(ctx.as_context());
    let mut has_frames = false;
    for frame in backtrace.frames() {
        let func_name = frame.func_name().unwrap_or("<anonymous>");
        stack.push_str(&format!("\n    at {}", func_name));
        has_frames = true;
    }
    if !has_frames {
        stack.push_str("\n    at <anonymous>");
    }
    stack
}

/// 从 options 参数中提取 cause 值（ECMAScript §20.5.1.1 step 7）。
/// `options` 必须是对象；若 options.cause 存在且非 undefined，返回 Some(cause)。
fn extract_cause_from_options(
    caller: &mut Caller<'_, RuntimeState>,
    env: &WasmEnv,
    options: i64,
) -> Option<i64> {
    if !value::is_js_object(options) {
        return None;
    }
    let name_id = find_memory_c_string_with_env(caller, env, "cause")
        .or_else(|| alloc_heap_c_string_with_env(caller, env, "cause"))?;
    let cause = get_by_name_id_sync(caller, options, name_id);
    if value::is_undefined(cause) {
        None
    } else {
        Some(cause)
    }
}

fn define_error_properties_with_env<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    obj: i64,
    error_name: &str,
    message: String,
    cause: Option<i64>,
    stack: String,
) {
    let name_val = {
        let state = ctx.as_context().data();
        store_runtime_string_in_state(state, error_name.to_string())
    };
    let message_val = {
        let state = ctx.as_context().data();
        store_runtime_string_in_state(state, message)
    };
    let non_enum_flags = constants::FLAG_CONFIGURABLE | constants::FLAG_WRITABLE;
    // name: { writable, non-enumerable, configurable }
    let name_name_id = find_memory_c_string_with_env(ctx, env, "name")
        .or_else(|| alloc_heap_c_string_with_env(ctx, env, "name"))
        .unwrap();
    let _ = define_host_data_property_by_name_id_with_env(
        ctx,
        env,
        obj,
        encode_string_name_id(name_name_id),
        name_val,
        non_enum_flags,
    );
    // message: { writable, non-enumerable, configurable }
    let msg_name_id = find_memory_c_string_with_env(ctx, env, "message")
        .or_else(|| alloc_heap_c_string_with_env(ctx, env, "message"))
        .unwrap();
    let _ = define_host_data_property_by_name_id_with_env(
        ctx,
        env,
        obj,
        encode_string_name_id(msg_name_id),
        message_val,
        non_enum_flags,
    );
    // C2: 隐藏品牌标记，用于 render_value 区分真实 Error vs 普通对象 {name:"TypeError"}。
    let brand_val = value::encode_bool(true);
    let brand_name_id = find_memory_c_string_with_env(ctx, env, "__error_brand__")
        .or_else(|| alloc_heap_c_string_with_env(ctx, env, "__error_brand__"))
        .unwrap();
    let _ = define_host_data_property_by_name_id_with_env(
        ctx,
        env,
        obj,
        encode_string_name_id(brand_name_id),
        brand_val,
        0,
    );
    // cause (ES2022): { writable, non-enumerable, configurable } — 仅当存在时定义
    if let Some(cause_val) = cause {
        let cause_name_id = find_memory_c_string_with_env(ctx, env, "cause")
            .or_else(|| alloc_heap_c_string_with_env(ctx, env, "cause"))
            .unwrap();
        let _ = define_host_data_property_by_name_id_with_env(
            ctx,
            env,
            obj,
            encode_string_name_id(cause_name_id),
            cause_val,
            non_enum_flags,
        );
    }
    // stack: { writable, non-enumerable, configurable } — V8 约定
    let stack_val = {
        let state = ctx.as_context().data();
        store_runtime_string_in_state(state, stack)
    };
    let stack_name_id = find_memory_c_string_with_env(ctx, env, "stack")
        .or_else(|| alloc_heap_c_string_with_env(ctx, env, "stack"))
        .unwrap();
    let _ = define_host_data_property_by_name_id_with_env(
        ctx,
        env,
        obj,
        encode_string_name_id(stack_name_id),
        stack_val,
        non_enum_flags,
    );
}

/// 共享的错误对象创建逻辑：分配 host 对象，设置 name/message/cause/stack 属性和 __error_brand__ 隐藏标记。
/// `create_error_object`（Caller 路径）和 `alloc_type_error_with_env`（泛型 C 路径）均委托此函数。
pub(crate) fn alloc_error_object_with_env<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    error_name: &str,
    message: String,
    cause: Option<i64>,
) -> i64 {
    // 容量：name + message + __error_brand__ + cause + stack = 5，预留 1 槽以支持后续扩展
    let obj = alloc_host_object(ctx, env, 6);
    let stack = capture_stack_trace(ctx, error_name, &message);
    define_error_properties_with_env(ctx, env, obj, error_name, message, cause, stack);
    if let Some(proto) = {
        let state = ctx.as_context().data();
        state.error_prototypes.proto_for_error_name(error_name)
    } {
        set_object_proto_header(ctx, env, obj, proto);
    }
    obj
}

fn record_error_entry(caller: &mut Caller<'_, RuntimeState>, error_name: &str, message: String) {
    let mut table = caller
        .data()
        .error_table
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    table.push(ErrorEntry {
        name: error_name.to_string(),
        message,
        value: value::encode_undefined(),
    });
}

pub(crate) fn create_error_object(
    caller: &mut Caller<'_, RuntimeState>,
    error_name: &str,
    arg: i64,
    options: i64,
) -> i64 {
    let message = error_message_from_arg(caller, arg);
    record_error_entry(caller, error_name, message.clone());
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    ensure_error_prototypes_initialized(caller, &env);
    let cause = extract_cause_from_options(caller, &env, options);
    alloc_error_object_with_env(caller, &env, error_name, message, cause)
}

pub(crate) fn create_error_object_with_receiver(
    caller: &mut Caller<'_, RuntimeState>,
    error_name: &str,
    arg: i64,
    options: i64,
    receiver: i64,
) -> i64 {
    let message = error_message_from_arg(caller, arg);
    record_error_entry(caller, error_name, message.clone());
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    ensure_error_prototypes_initialized(caller, &env);
    let cause = extract_cause_from_options(caller, &env, options);
    if value::is_js_object(receiver) {
        let stack = capture_stack_trace(caller, error_name, &message);
        define_error_properties_with_env(caller, &env, receiver, error_name, message, cause, stack);
        receiver
    } else {
        alloc_error_object_with_env(caller, &env, error_name, message, cause)
    }
}

pub(crate) fn alloc_type_error_with_env<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    message: String,
) -> i64 {
    alloc_error_object_with_env(ctx, env, "TypeError", message, None)
}
pub(crate) fn obj_proto_to_string_impl(caller: &mut Caller<'_, RuntimeState>, obj: i64) -> i64 {
    if value::is_undefined(obj) {
        store_runtime_string(caller, "[object Undefined]".to_string())
    } else if value::is_null(obj) {
        store_runtime_string(caller, "[object Null]".to_string())
    } else if value::is_array(obj) {
        store_runtime_string(caller, "[object Array]".to_string())
    } else if value::is_function(obj) || value::is_callable(obj) {
        store_runtime_string(caller, "[object Function]".to_string())
    } else if is_promise_value(caller.data(), obj) {
        store_runtime_string(caller, "[object Promise]".to_string())
    } else if value::is_regexp(obj) {
        store_runtime_string(caller, "[object RegExp]".to_string())
    } else if value::is_object(obj) {
        let obj_ptr = resolve_handle_idx(caller, value::decode_object_handle(obj) as usize);
        if let Some(op) = obj_ptr {
            if let Some(Extern::Memory(mem)) = caller.get_export("memory") {
                let data = mem.data(&caller);
                if op + 4 < data.len() && data[op + 4] == wjsm_ir::HEAP_TYPE_ARGUMENTS {
                    return store_runtime_string(caller, "[object Arguments]".to_string());
                }
            }
            let map_handle = read_object_property_by_name(caller, op, "__map_handle__");
            if map_handle.is_some() {
                return store_runtime_string(caller, "[object Map]".to_string());
            }
            let set_handle = read_object_property_by_name(caller, op, "__set_handle__");
            if set_handle.is_some() {
                return store_runtime_string(caller, "[object Set]".to_string());
            }
        }
        let name_val = obj_ptr.and_then(|p| read_object_property_by_name(caller, p, "name"));
        let msg_val = obj_ptr.and_then(|p| read_object_property_by_name(caller, p, "message"));
        match (name_val, msg_val) {
            (Some(nv), Some(_mv)) => {
                let name_str = read_value_string_bytes(caller, nv)
                    .map(|b| String::from_utf8_lossy(&b).into_owned())
                    .unwrap_or_default();
                if matches!(
                    name_str.as_str(),
                    "Error"
                        | "TypeError"
                        | "RangeError"
                        | "SyntaxError"
                        | "ReferenceError"
                        | "URIError"
                        | "EvalError"
                        | "AggregateError"
                ) {
                    let obj_ptr2 =
                        resolve_handle_idx(caller, value::decode_object_handle(obj) as usize);
                    let msg_str = obj_ptr2
                        .and_then(|p| read_object_property_by_name(caller, p, "message"))
                        .and_then(|v| read_value_string_bytes(caller, v))
                        .map(|b| String::from_utf8_lossy(&b).into_owned())
                        .unwrap_or_default();
                    if msg_str.is_empty() {
                        store_runtime_string(caller, name_str)
                    } else {
                        store_runtime_string(caller, format!("{}: {}", name_str, msg_str))
                    }
                } else {
                    store_runtime_string(caller, "[object Object]".to_string())
                }
            }
            _ => store_runtime_string(caller, "[object Object]".to_string()),
        }
    } else {
        store_runtime_string(caller, "[object Object]".to_string())
    }
}

pub(crate) fn define_host_data_property_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
    name: &str,
    val: i64,
) -> Option<()> {
    define_host_data_property(caller, obj, name, val)
}

/// 定义一个访问器（getter/setter）属性到宿主创建的对象上（from_caller 便捷封装）。
pub(crate) fn define_host_accessor_property_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
    name: &str,
    getter: i64,
    setter: i64,
) -> Option<()> {
    define_host_accessor_property(caller, obj, name, getter, setter)
}

pub(crate) fn alloc_all_settled_result_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    status: &str,
    value_name: &str,
    val: i64,
) -> i64 {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let obj = alloc_host_object(caller, &env, 2);
    let status_value = store_runtime_string(caller, status.to_string());
    let _ = define_host_data_property_from_caller(caller, obj, "status", status_value);
    let _ = define_host_data_property_from_caller(caller, obj, value_name, val);
    obj
}

pub(crate) fn alloc_all_settled_result<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    status: &str,
    value_name: &str,
    val: i64,
) -> i64 {
    let obj = alloc_host_object(ctx, env, 2);
    let status_value =
        store_runtime_string_in_state(ctx.as_context_mut().data_mut(), status.to_string());
    let _ = define_host_data_property_with_env(ctx, env, obj, "status", status_value);
    let _ = define_host_data_property_with_env(ctx, env, obj, value_name, val);
    obj
}

pub(crate) fn alloc_heap_aggregate_error<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    errors: i64,
) -> i64 {
    // 容量：name + message + errors + __error_brand__ + stack = 5
    let obj = alloc_host_object(ctx, env, 5);
    let name = store_runtime_string_in_state(
        ctx.as_context_mut().data_mut(),
        "AggregateError".to_string(),
    );
    let message = store_runtime_string_in_state(
        ctx.as_context_mut().data_mut(),
        "All promises were rejected".to_string(),
    );
    let non_enum_flags = constants::FLAG_CONFIGURABLE | constants::FLAG_WRITABLE;
    let name_name_id = find_memory_c_string_with_env(ctx, env, "name")
        .or_else(|| alloc_heap_c_string_with_env(ctx, env, "name"))
        .unwrap();
    let _ = define_host_data_property_by_name_id_with_env(
        ctx,
        env,
        obj,
        encode_string_name_id(name_name_id),
        name,
        non_enum_flags,
    );
    let msg_name_id = find_memory_c_string_with_env(ctx, env, "message")
        .or_else(|| alloc_heap_c_string_with_env(ctx, env, "message"))
        .unwrap();
    let _ = define_host_data_property_by_name_id_with_env(
        ctx,
        env,
        obj,
        encode_string_name_id(msg_name_id),
        message,
        non_enum_flags,
    );
    let _ = define_host_data_property_with_env(ctx, env, obj, "errors", errors);
    // __error_brand__ 隐藏标记
    let brand_val = value::encode_bool(true);
    let brand_name_id = find_memory_c_string_with_env(ctx, env, "__error_brand__")
        .or_else(|| alloc_heap_c_string_with_env(ctx, env, "__error_brand__"))
        .unwrap();
    let _ = define_host_data_property_by_name_id_with_env(
        ctx,
        env,
        obj,
        encode_string_name_id(brand_name_id),
        brand_val,
        0,
    );
    // stack 属性
    let stack = capture_stack_trace(ctx, "AggregateError", "All promises were rejected");
    let stack_val = store_runtime_string_in_state(ctx.as_context().data(), stack);
    let stack_name_id = find_memory_c_string_with_env(ctx, env, "stack")
        .or_else(|| alloc_heap_c_string_with_env(ctx, env, "stack"))
        .unwrap();
    let _ = define_host_data_property_by_name_id_with_env(
        ctx,
        env,
        obj,
        encode_string_name_id(stack_name_id),
        stack_val,
        non_enum_flags,
    );
    // 设置原型为 error_prototypes（如有）
    if let Some(proto) = {
        let state = ctx.as_context().data();
        state
            .error_prototypes
            .proto_for_error_name("AggregateError")
    } {
        set_object_proto_header(ctx, env, obj, proto);
    }
    obj
}
