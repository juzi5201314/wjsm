use super::*;

/// TAG_FUNCTION 在无 function_props 对象时的属性解析。
/// 覆盖 Array.prototype 宿主方法（arr_proto_* table entries）的
/// `call`/`apply`/`bind`/`length`/`name`。
pub(crate) fn function_value_get_property_impl(
    caller: &mut Caller<'_, RuntimeState>,
    func_val: i64,
    name_id: i32,
) -> i64 {
    if !value::is_function(func_val) {
        return value::encode_undefined();
    }
    let name_id = name_id as u32;
    if is_symbol_name_id(name_id) {
        return value::encode_undefined();
    }
    let prop_bytes = read_string_bytes(caller, name_id);
    let prop_name = match std::str::from_utf8(&prop_bytes) {
        Ok(s) => s,
        Err(_) => return value::encode_undefined(),
    };

    // call / apply / bind → Function.prototype 上的方法
    if prop_name == "call" || prop_name == "apply" || prop_name == "bind" {
        if !value::is_object(caller.data().function_prototype)
            && let Some(env) = WasmEnv::from_caller(caller)
        {
            crate::runtime_heap::ensure_function_prototype_initialized(caller, &env);
        }
        let proto = caller.data().function_prototype;
        if value::is_object(proto)
            && let Some(env) = WasmEnv::from_caller(caller)
            && let Some(ptr) = resolve_handle_idx_with_env(
                caller,
                &env,
                value::decode_object_handle(proto) as usize,
            )
            && let Some(val) = read_object_property_by_name_with_env(caller, &env, ptr, prop_name)
        {
            return val;
        }
        return value::encode_undefined();
    }

    // length / name：仅对 arr_proto 宿主方法给出规范数据属性。
    if (prop_name == "length" || prop_name == "name")
        && let Some((name, length)) = array_proto_method_metadata(caller, func_val)
    {
        if prop_name == "length" {
            return value::encode_f64(length as f64);
        }
        return store_runtime_string(caller, name);
    }

    value::encode_undefined()
}

/// 若 `func_val` 落在 arr_proto table 区间，返回 (property_name, length)。
fn array_proto_method_metadata(
    caller: &mut Caller<'_, RuntimeState>,
    func_val: i64,
) -> Option<(String, u32)> {
    let env = WasmEnv::from_caller(caller)?;
    let base = env
        .arr_proto_table_base
        .and_then(|g| g.get(&mut *caller).i32())
        .unwrap_or(0)
        .max(0) as u32;
    let len = env
        .arr_proto_table_len
        .and_then(|g| g.get(&mut *caller).i32())
        .unwrap_or(0)
        .max(0) as u32;
    if len == 0 {
        return None;
    }
    let idx = value::decode_function_idx(func_val);
    if idx < base || idx >= base.saturating_add(len) {
        return None;
    }
    let offset = (idx - base) as usize;
    let (_, spec) =
        wjsm_backend_wasm::host_import_registry::array_proto_method_specs().nth(offset)?;
    let name = wjsm_backend_wasm::host_import_registry::array_proto_property_name(spec.name)?;
    let length = array_proto_method_length(&name);
    Some((name, length))
}

fn array_proto_method_length(name: &str) -> u32 {
    match name {
        "concat" | "push" | "unshift" | "every" | "filter" | "find" | "findIndex" | "findLast"
        | "flatMap" | "forEach" | "includes" | "indexOf" | "join" | "lastIndexOf" | "map"
        | "reduce" | "reduceRight" | "some" | "sort" | "toSorted" | "at" | "fill" => 1,
        "copyWithin" | "slice" | "splice" | "toSpliced" | "with" => 2,
        // entries/keys/values/pop/reverse/shift/toString/toLocaleString/toReversed/flat 等
        _ => 0,
    }
}
fn function_proto_method_meta(nc: &NativeCallable) -> Option<(u32, &'static str)> {
    match nc {
        NativeCallable::FunctionProtoCall => Some((1, "call")),
        NativeCallable::FunctionProtoApply => Some((2, "apply")),
        NativeCallable::FunctionProtoBind => Some((1, "bind")),
        _ => None,
    }
}

// ── Linker 注册辅助函数 ─────────────────────────────────────────

/// 注册 16 个简单桥接（无 WASM 回调，sync/async 共享）
pub(super) fn register_common_bridges(
    linker: &mut Linker<RuntimeState>,
    store: &mut Store<RuntimeState>,
) -> Result<()> {
    // new_target
    let f = Func::wrap(
        &mut *store,
        |caller: Caller<'_, RuntimeState>, _dummy: i64| -> i64 {
            caller.data().new_target.load(Ordering::Relaxed)
        },
    );
    linker.define(&mut *store, "env", "new_target", f)?;
    // new_target_set
    let f = Func::wrap(
        &mut *store,
        |caller: Caller<'_, RuntimeState>, new_target: i64| -> i64 {
            caller.data().new_target.swap(new_target, Ordering::Relaxed)
        },
    );
    linker.define(&mut *store, "env", "new_target_set", f)?;
    // create_unmapped_arguments_object
    let f = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, args_array: i64, param_count: i64| -> i64 {
            create_unmapped_arguments_object(&mut caller, args_array, param_count)
        },
    );
    linker.define(&mut *store, "env", "create_unmapped_arguments_object", f)?;
    // create_mapped_arguments_object
    let f = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>,
         args_array: i64,
         param_count: i64,
         func_ref: i64|
         -> i64 {
            create_mapped_arguments_object(&mut caller, args_array, param_count, func_ref)
        },
    );
    linker.define(&mut *store, "env", "create_mapped_arguments_object", f)?;
    // scope_record_create
    let f = Func::wrap(
        &mut *store,
        |caller: Caller<'_, RuntimeState>, capacity: i64| -> i64 {
            scope_record_create(caller, capacity)
        },
    );
    linker.define(&mut *store, "env", "scope_record_create", f)?;
    // scope_record_add_binding
    let f = Func::wrap(
        &mut *store,
        |caller: Caller<'_, RuntimeState>,
         record: i64,
         name: i64,
         val: i64,
         is_tdz: i64,
         is_const: i64| {
            scope_record_add_binding(caller, record, name, val, is_tdz, is_const)
        },
    );
    linker.define(&mut *store, "env", "scope_record_add_binding", f)?;
    // eval_get_binding
    let f = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, record: i64, name: i64| -> i64 {
            eval_get_binding(&mut caller, record, name)
        },
    );
    linker.define(&mut *store, "env", "eval_get_binding", f)?;
    // eval_set_binding
    let f = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, record: i64, name: i64, val: i64| -> i64 {
            eval_set_binding(&mut caller, record, name, val)
        },
    );
    linker.define(&mut *store, "env", "eval_set_binding", f)?;
    // eval_has_binding
    let f = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, record: i64, name: i64| -> i64 {
            eval_has_binding(&mut caller, record, name)
        },
    );
    linker.define(&mut *store, "env", "eval_has_binding", f)?;
    // eval_super_base
    let f = Func::wrap(
        &mut *store,
        |caller: Caller<'_, RuntimeState>, record: i64| -> i64 { eval_super_base(caller, record) },
    );
    linker.define(&mut *store, "env", "eval_super_base", f)?;
    // scope_record_set_meta
    let f = Func::wrap(
        &mut *store,
        |caller: Caller<'_, RuntimeState>, record: i64, key: i64, val: i64| {
            scope_record_set_meta(caller, record, key, val)
        },
    );
    linker.define(&mut *store, "env", "scope_record_set_meta", f)?;
    // scope_record_destroy
    let f = Func::wrap(
        &mut *store,
        |caller: Caller<'_, RuntimeState>, record: i64| scope_record_destroy(caller, record),
    );
    linker.define(&mut *store, "env", "scope_record_destroy", f)?;
    // symbol_property_key
    let f = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, key: i64| -> i32 {
            property_key_value_to_name_id(&mut caller, key, true).unwrap_or(0) as i32
        },
    );
    linker.define(&mut *store, "env", "symbol_property_key", f)?;
    // string_to_array_index：key 为「规范数字索引字符串」（CanonicalNumericIndexString，
    // 范围 [0, 2^31)）时返回该索引，否则 -1。用于 a["5"] 这类字符串键索引数组——
    // "5"→5（元素），"05"/"5.0"/"x"/" 5"/"length"→-1（命名属性）。
    let f = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, key: i64| -> i32 {
            if !value::is_string(key) {
                return -1;
            }
            let Ok(s) = render_value(&mut caller, key) else {
                return -1;
            };
            match s.parse::<u32>() {
                // 规范性：解析值回写字符串须与原串完全相等（排除前导零、空白、符号、小数点）；
                // 限 < i32::MAX（elem_get 用 i32 索引，且远超任何真实数组长度）。
                Ok(n) if (n as i64) < i32::MAX as i64 && n.to_string() == s => n as i32,
                _ => -1,
            }
        },
    );
    linker.define(&mut *store, "env", "string_to_array_index", f)?;
    define_property_helpers(linker, store)
}

/// NATIVE_CALLABLE 的属性解析单一 owner：`native_callable_get_property` host import
/// 与 V2 `gc_obj_get_v2` 分派共用。
pub(crate) fn native_callable_get_property_impl(
    caller: &mut Caller<'_, RuntimeState>,
    native: i64,
    name_id: i32,
) -> i64 {
    let name_id = name_id as u32;
    if is_symbol_name_id(name_id) {
        return value::encode_undefined();
    }
    let prop_bytes = read_string_bytes(caller, name_id);
    let prop_name = match std::str::from_utf8(&prop_bytes) {
        Ok(s) => s,
        Err(_) => return value::encode_undefined(),
    };
    if let Some(val) = crate::symbol_well_known::native_callable_symbol_constructor_static_property(
        caller, native, prop_name,
    ) {
        return val;
    }
    let idx = value::decode_native_callable_idx(native) as usize;
    let record = {
        let table = caller
            .data()
            .native_callables
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        table.get(idx).cloned()
    };
    match &record {
        Some(NativeCallable::ProcessHrtime) if prop_name == "bigint" => {
            return create_native_callable(caller.data(), NativeCallable::ProcessHrtimeBigint);
        }
        Some(NativeCallable::CjsRequire { referrer }) => {
            if let Some(value) = cjs_require_property(caller, referrer.clone(), prop_name) {
                return value;
            }
        }
        Some(NativeCallable::CjsRequireResolve { referrer }) => {
            if let Some(value) = cjs_require_resolve_property(caller, referrer.clone(), prop_name) {
                return value;
            }
        }
        _ => {}
    }
    if prop_name == "call" || prop_name == "apply" || prop_name == "bind" {
        if !value::is_object(caller.data().function_prototype)
            && let Some(env) = WasmEnv::from_caller(caller)
        {
            crate::runtime_heap::ensure_function_prototype_initialized(caller, &env);
        }
        let proto = caller.data().function_prototype;
        if value::is_object(proto)
            && let Some(env) = WasmEnv::from_caller(caller)
            && let Some(ptr) = resolve_handle_idx_with_env(
                caller,
                &env,
                value::decode_object_handle(proto) as usize,
            )
            && let Some(val) = read_object_property_by_name_with_env(caller, &env, ptr, prop_name)
        {
            return val;
        }
        return value::encode_undefined();
    }
    if prop_name != "prototype" {
        if matches!(record, Some(NativeCallable::BufferConstructor)) {
            let kind = match prop_name {
                "alloc" => BufferStaticKind::Alloc,
                "allocUnsafe" => BufferStaticKind::AllocUnsafe,
                "from" => BufferStaticKind::From,
                "concat" => BufferStaticKind::Concat,
                "isBuffer" => BufferStaticKind::IsBuffer,
                "byteLength" => BufferStaticKind::ByteLength,
                _ => return value::encode_undefined(),
            };
            let mut table = caller
                .data()
                .native_callables
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let idx = table.len() as u32;
            table.push(NativeCallable::BufferStatic { kind });
            return value::encode_native_callable_idx(idx);
        }
        // Function.prototype.call/apply/bind 的 length / name
        if let Some(nc) = record.as_ref()
            && let Some((length, name)) = function_proto_method_meta(nc)
        {
            if prop_name == "length" {
                return value::encode_f64(length as f64);
            }
            if prop_name == "name" {
                return store_runtime_string(&*caller, name.to_string());
            }
        }
        // Object / Promise 静态方法：可获取函数值（typeof === "function"）
        if matches!(
            record,
            Some(NativeCallable::ObjectConstructor) | Some(NativeCallable::PromiseConstructor)
        ) {
            use crate::types::{ObjectStaticKind, PromiseStaticKind};
            let static_nc = match (&record, prop_name) {
                (Some(NativeCallable::ObjectConstructor), "keys") => {
                    Some(NativeCallable::ObjectStatic {
                        kind: ObjectStaticKind::Keys,
                    })
                }
                (Some(NativeCallable::ObjectConstructor), "values") => {
                    Some(NativeCallable::ObjectStatic {
                        kind: ObjectStaticKind::Values,
                    })
                }
                (Some(NativeCallable::ObjectConstructor), "entries") => {
                    Some(NativeCallable::ObjectStatic {
                        kind: ObjectStaticKind::Entries,
                    })
                }
                (Some(NativeCallable::ObjectConstructor), "assign") => {
                    Some(NativeCallable::ObjectStatic {
                        kind: ObjectStaticKind::Assign,
                    })
                }
                (Some(NativeCallable::ObjectConstructor), "create") => {
                    Some(NativeCallable::ObjectStatic {
                        kind: ObjectStaticKind::Create,
                    })
                }
                (Some(NativeCallable::ObjectConstructor), "getPrototypeOf") => {
                    Some(NativeCallable::ObjectStatic {
                        kind: ObjectStaticKind::GetPrototypeOf,
                    })
                }
                (Some(NativeCallable::ObjectConstructor), "setPrototypeOf") => {
                    Some(NativeCallable::ObjectStatic {
                        kind: ObjectStaticKind::SetPrototypeOf,
                    })
                }
                (Some(NativeCallable::ObjectConstructor), "getOwnPropertyNames") => {
                    Some(NativeCallable::ObjectStatic {
                        kind: ObjectStaticKind::GetOwnPropertyNames,
                    })
                }
                (Some(NativeCallable::ObjectConstructor), "is") => {
                    Some(NativeCallable::ObjectStatic {
                        kind: ObjectStaticKind::Is,
                    })
                }
                (Some(NativeCallable::ObjectConstructor), "hasOwn") => {
                    Some(NativeCallable::ObjectStatic {
                        kind: ObjectStaticKind::HasOwn,
                    })
                }
                (Some(NativeCallable::ObjectConstructor), "fromEntries") => {
                    Some(NativeCallable::ObjectStatic {
                        kind: ObjectStaticKind::FromEntries,
                    })
                }
                (Some(NativeCallable::PromiseConstructor), "resolve") => {
                    Some(NativeCallable::PromiseStatic {
                        kind: PromiseStaticKind::Resolve,
                    })
                }
                (Some(NativeCallable::PromiseConstructor), "reject") => {
                    Some(NativeCallable::PromiseStatic {
                        kind: PromiseStaticKind::Reject,
                    })
                }
                (Some(NativeCallable::PromiseConstructor), "all") => {
                    Some(NativeCallable::PromiseStatic {
                        kind: PromiseStaticKind::All,
                    })
                }
                (Some(NativeCallable::PromiseConstructor), "race") => {
                    Some(NativeCallable::PromiseStatic {
                        kind: PromiseStaticKind::Race,
                    })
                }
                (Some(NativeCallable::PromiseConstructor), "allSettled") => {
                    Some(NativeCallable::PromiseStatic {
                        kind: PromiseStaticKind::AllSettled,
                    })
                }
                (Some(NativeCallable::PromiseConstructor), "any") => {
                    Some(NativeCallable::PromiseStatic {
                        kind: PromiseStaticKind::Any,
                    })
                }
                (Some(NativeCallable::PromiseConstructor), "withResolvers") => {
                    Some(NativeCallable::PromiseStatic {
                        kind: PromiseStaticKind::WithResolvers,
                    })
                }
                _ => None,
            };
            if let Some(nc) = static_nc {
                let mut table = caller
                    .data()
                    .native_callables
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                let idx = table.len() as u32;
                table.push(nc);
                return value::encode_native_callable_idx(idx);
            }
        }
        // EvalFunction（含 vm.compileFunction）：暴露 length / name
        if let Some(NativeCallable::EvalFunction(func)) = record.as_ref() {
            if prop_name == "length" {
                return value::encode_f64(func.params.len() as f64);
            }
            if prop_name == "name" {
                return store_runtime_string(&*caller, String::new());
            }
        }
        return value::encode_undefined();
    }
    match &record {
        Some(nc) => crate::runtime_heap::native_callable_prototype(caller, nc)
            .unwrap_or_else(value::encode_undefined),
        None => value::encode_undefined(),
    }
}

fn define_property_helpers(
    linker: &mut Linker<RuntimeState>,
    store: &mut Store<RuntimeState>,
) -> Result<()> {
    // native_callable_get_property
    let f = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, native: i64, name_id: i32| -> i64 {
            native_callable_get_property_impl(&mut caller, native, name_id)
        },
    );
    linker.define(&mut *store, "env", "native_callable_get_property", f)?;
    // function_value_get_property — TAG_FUNCTION 无 function_props 时的属性解析
    // （arr_proto_* table 宿主函数、以及任何只编码为 function idx 的可调用值）。
    let f = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, func_val: i64, name_id: i32| -> i64 {
            function_value_get_property_impl(&mut caller, func_val, name_id)
        },
    );
    linker.define(&mut *store, "env", "function_value_get_property", f)?;
    // array.from 已移至 register_complex_bridges（async，支持迭代协议 + mapFn reentry）
    // obj_get_by_index
    let f = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, boxed: i64, index: i32| -> i64 {
            if index >= 0
                && let Some(value) = typedarray_element_read(&mut caller, boxed, index as u32)
            {
                return value;
            }
            if !value::is_object(boxed) && !value::is_array(boxed) && !value::is_function(boxed) {
                return value::encode_undefined();
            }
            let Some(ptr) = resolve_handle(&mut caller, boxed) else {
                return value::encode_undefined();
            };
            let key = index.to_string();
            let mut visited = std::collections::HashSet::new();
            read_object_property_by_name_proto_walk(&mut caller, ptr, &key, &mut visited)
                .unwrap_or(value::encode_undefined())
        },
    );
    linker.define(&mut *store, "env", "obj_get_by_index", f)?;
    // typedarray_set_by_index
    let f = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, boxed: i64, index: i32, value_raw: i64| -> i64 {
            if typedarray_entry_from_value(&mut caller, boxed).is_some() {
                if index >= 0 {
                    let _ = typedarray_element_write(&mut caller, boxed, index as u32, value_raw);
                }
                return value::encode_bool(true);
            }
            value::encode_bool(false)
        },
    );
    linker.define(&mut *store, "env", "typedarray_set_by_index", f)?;
    // to_number：ToNumber 抽象操作，将任意值转换为 Number
    let f = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, val: i64| -> i64 { to_number(&mut caller, val) },
    );
    linker.define(&mut *store, "env", "to_number", f)?;
    // to_bool：ToBoolean 抽象操作，将任意值转换为 i32 布尔值 (0 or 1)
    let f = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, val: i64| -> i32 {
            to_boolean(&mut caller, val) as i32
        },
    );
    linker.define(&mut *store, "env", "to_bool", f)?;

    Ok(())
}

/// 注册 18 个 define_* 宿主函数模块
pub(super) fn register_linker(
    linker: &mut Linker<RuntimeState>,
    store: &mut Store<RuntimeState>,
) -> Result<()> {
    define_core(linker, store)?;
    #[cfg(feature = "managed-heap-v2")]
    crate::host_imports::define_v2(linker)?;
    crate::array_named_props::define_array_named_props(linker, store)?;
    define_timers_arrays(linker, store)?;
    define_fetch(linker, store)?;
    define_array_object(linker, store)?;
    define_primitive_core(linker, store)?;
    define_promise(linker, store)?;
    define_promise_combinators(linker, store)?;
    define_misc(linker, store)?;
    define_inspector_host(linker, store)?;
    define_modules(linker, store)?;
    define_async_fn(linker, store)?;
    define_async_generator(linker, store)?;
    define_generator(linker, store)?;
    define_proxy_reflect(linker, store)?;
    define_proxy_reflect_async(linker, store)?;
    define_object_builtins(linker, store)?;
    define_string_methods(linker, store)?;
    define_math_number_error(linker, store)?;
    define_collections_buffers(linker, store)?;
    define_proxy_traps(linker, store)?;
    define_typedarray_new_methods(linker, store)?;
    define_weakref_finalization(linker, store)?;
    define_atomics(linker, store)?;
    define_get_builtin_global(linker, store)?;
    define_misc_async(linker, store)?;
    define_timers_arrays_async(linker, store)?;
    define_array_object_async(linker, store)?;
    define_typedarray_new_methods_async(linker, store)?;
    define_proxy_traps_async(linker, store)?;
    define_object_builtins_async(linker, store)?;
    define_core_async(linker, store)?;
    define_primitive_core_async(linker, store)?;
    Ok(())
}

/// 注册 3 个复杂桥接（Linker::func_wrap_async + call_wasm_callback_async）
pub(super) fn register_complex_bridges(
    linker: &mut Linker<RuntimeState>,
    _store: &mut Store<RuntimeState>,
) -> Result<()> {
    // async_iterator_from
    linker.func_wrap_async(
        "env",
        "async_iterator_from",
        |mut caller: Caller<'_, RuntimeState>, (iterable,): (i64,)| {
            Box::new(async move {
                if value::is_iterator(iterable) {
                    return create_async_from_sync_iterator(&mut caller, iterable);
                }
                if !(value::is_object(iterable)
                    || value::is_array(iterable)
                    || value::is_function(iterable)
                    || value::is_proxy(iterable))
                {
                    let mut iters = caller
                        .data()
                        .iterators
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    let handle = iters.len() as u32;
                    iters.push(IteratorState::Error);
                    return value::encode_handle(value::TAG_ITERATOR, handle);
                }

                let Some(_ptr) = resolve_handle(&mut caller, iterable) else {
                    let mut iters = caller
                        .data()
                        .iterators
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    let handle = iters.len() as u32;
                    iters.push(IteratorState::Error);
                    return value::encode_handle(value::TAG_ITERATOR, handle);
                };
                // 数组 fast path
                if value::is_array(iterable)
                    && let Some(arr_ptr) = resolve_handle(&mut caller, iterable)
                {
                    let length = read_array_length(&mut caller, arr_ptr).unwrap_or(0);
                    let sync_iter_handle = {
                        let mut iters = caller
                            .data()
                            .iterators
                            .lock()
                            .unwrap_or_else(|e| e.into_inner());
                        let sync_handle = iters.len() as u32;
                        iters.push(IteratorState::ArrayIter {
                            ptr: arr_ptr,
                            index: 0,
                            length,
                        });
                        value::encode_handle(value::TAG_ITERATOR, sync_handle)
                    };
                    return create_async_from_sync_iterator(&mut caller, sync_iter_handle);
                }
                // 尝试 @@asyncIterator（使用 GetMethod 规范实现）
                match crate::host_imports::get_method_by_name_id(
                    &mut caller,
                    iterable,
                    encode_symbol_name_id(3),
                ) {
                    Ok(Some(method)) => {
                        let iterator =
                            call_iterable_method_async(&mut caller, method, iterable).await;
                        // 若 method 调用返回异常（如内部抛出 TypeError），直接返回
                        if value::is_exception(iterator) {
                            return iterator;
                        }
                        if value::is_object(iterator)
                            && let Some(iter_ptr) = resolve_handle(&mut caller, iterator)
                        {
                            let next = read_object_property_by_name(&mut caller, iter_ptr, "next")
                                .filter(|n| value::is_callable(*n));
                            if let Some(next_fn) = next {
                                let return_method =
                                    read_object_property_by_name(&mut caller, iter_ptr, "return")
                                        .filter(|c| value::is_callable(*c));
                                let mut iters = caller
                                    .data()
                                    .iterators
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner());
                                let handle = iters.len() as u32;
                                iters.push(IteratorState::ObjectIter {
                                    iterator,
                                    next: next_fn,
                                    return_method,
                                    throw_method: None,
                                    current_value: value::encode_undefined(),
                                    has_current: false,
                                    done: false,
                                });
                                return value::encode_handle(value::TAG_ITERATOR, handle);
                            }
                        }
                    }
                    Err(exc) => return exc,
                    Ok(None) => {}
                }

                // 回退到 @@iterator（使用 GetMethod 规范实现）
                match crate::host_imports::get_method_by_name_id(
                    &mut caller,
                    iterable,
                    encode_symbol_name_id(0),
                ) {
                    Ok(Some(method)) => {
                        let sync_iter =
                            call_iterable_method_async(&mut caller, method, iterable).await;
                        // 若 method 调用返回异常（如内部抛出 TypeError），直接返回
                        if value::is_exception(sync_iter) {
                            return sync_iter;
                        }
                        if value::is_object(sync_iter)
                            && let Some(sync_ptr) = resolve_handle(&mut caller, sync_iter)
                        {
                            let next_fn =
                                read_object_property_by_name(&mut caller, sync_ptr, "next")
                                    .filter(|n| value::is_callable(*n));
                            if let Some(next_fn) = next_fn {
                                let return_method =
                                    read_object_property_by_name(&mut caller, sync_ptr, "return")
                                        .filter(|c| value::is_callable(*c));
                                let sync_iter_handle = {
                                    let mut iters = caller
                                        .data()
                                        .iterators
                                        .lock()
                                        .unwrap_or_else(|e| e.into_inner());
                                    let sync_handle = iters.len() as u32;
                                    iters.push(IteratorState::ObjectIter {
                                        iterator: sync_iter,
                                        next: next_fn,
                                        return_method,
                                        throw_method: None,
                                        current_value: value::encode_undefined(),
                                        has_current: false,
                                        done: false,
                                    });
                                    value::encode_handle(value::TAG_ITERATOR, sync_handle)
                                };
                                return create_async_from_sync_iterator(
                                    &mut caller,
                                    sync_iter_handle,
                                );
                            }
                        }
                    }
                    Err(exc) => return exc,
                    Ok(None) => {}
                }
                // GetIterator 收尾：@@asyncIterator / @@iterator 均不可用，或方法返回的
                // 对象缺少可调用 next。规范要求抛出 TypeError。返回可捕获的 TAG_EXCEPTION
                // （而非裸 error 对象）：该值作为迭代器句柄存入后，首次 iterator.next 会被
                // iterator_next_async 转成 rejected promise，经 await 的 is_rejected 路径在
                // for-await 外层 try/catch 捕获，避免把不可用对象当作迭代器句柄继续迭代。
                make_type_error_exception(&mut caller, "value is not async iterable")
            })
        },
    )?;
    // object.group_by
    linker.func_wrap_async(
        "env",
        "object.group_by",
        |mut caller: Caller<'_, RuntimeState>, (items, callbackfn): (i64, i64)| {
            Box::new(async move {
                if value::is_null(items) || value::is_undefined(items) {
                    *caller
                        .data()
                        .runtime_error
                        .lock()
                        .unwrap_or_else(|e| e.into_inner()) =
                        Some("TypeError: Cannot group null or undefined".to_string());
                    return value::encode_undefined();
                }
                if !value::is_callable(callbackfn) {
                    *caller
                        .data()
                        .runtime_error
                        .lock()
                        .unwrap_or_else(|e| e.into_inner()) =
                        Some("TypeError: callbackfn is not callable".to_string());
                    return value::encode_undefined();
                }
                let result = alloc_object(&mut caller, 0);
                let mut groups: HashMap<String, Vec<i64>> = HashMap::new();
                let mut index = 0u32;
                if value::is_array(items)
                    && let Some(arr_ptr) = resolve_array_ptr(&mut caller, items)
                {
                    let len = read_array_length(&mut caller, arr_ptr).unwrap_or(0);
                    for i in 0..len {
                        let elem = read_array_elem(&mut caller, arr_ptr, i)
                            .unwrap_or(value::encode_undefined());
                        let idx_val = value::encode_f64(index as f64);
                        let key = match call_wasm_callback_async(
                            &mut caller,
                            callbackfn,
                            value::encode_undefined(),
                            &[elem, idx_val],
                        )
                        .await
                        {
                            Ok(k) => k,
                            Err(_) => return value::encode_undefined(),
                        };
                        let key_str = to_property_key(&mut caller, key);
                        if caller
                            .data()
                            .runtime_error
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .is_some()
                        {
                            return value::encode_undefined();
                        }
                        groups.entry(key_str).or_default().push(elem);
                        index += 1;
                    }
                    for (key_str, elements) in &groups {
                        let arr = alloc_array(&mut caller, elements.len() as u32);
                        if let Some(arr_ptr) = resolve_array_ptr(&mut caller, arr) {
                            for (i, &elem) in elements.iter().enumerate() {
                                write_array_elem(&mut caller, arr_ptr, i as u32, elem);
                            }
                            write_array_length(&mut caller, arr_ptr, elements.len() as u32);
                        }
                        define_host_data_property(&mut caller, result, key_str, arr);
                    }
                    return result;
                }
                result
            })
        },
    )?;
    // map.group_by
    linker.func_wrap_async(
        "env",
        "map.group_by",
        |mut caller: Caller<'_, RuntimeState>, (items, callbackfn): (i64, i64)| {
            Box::new(async move {
                if value::is_null(items) || value::is_undefined(items) {
                    *caller
                        .data()
                        .runtime_error
                        .lock()
                        .unwrap_or_else(|e| e.into_inner()) =
                        Some("TypeError: Cannot group null or undefined".to_string());
                    return value::encode_undefined();
                }
                if !value::is_callable(callbackfn) {
                    *caller
                        .data()
                        .runtime_error
                        .lock()
                        .unwrap_or_else(|e| e.into_inner()) =
                        Some("TypeError: callbackfn is not callable".to_string());
                    return value::encode_undefined();
                }
                let map_handle = caller.data().alloc_map_entry() as usize;
                let map_result = alloc_object(&mut caller, 13);
                if !value::is_object(map_result) {
                    caller.data().release_unowned_map_entry(map_handle as u32);
                    return map_result;
                }
                caller.data().bind_map_entry_owner(
                    map_handle as u32,
                    value::decode_object_handle(map_result),
                );
                {
                    let state = caller.data();
                    let set_fn = create_map_set_method(state, MapSetMethodKind::MapSet);
                    let get_fn = create_map_set_method(state, MapSetMethodKind::MapGet);
                    let has_fn = create_map_set_method(state, MapSetMethodKind::Has);
                    let delete_fn = create_map_set_method(state, MapSetMethodKind::Delete);
                    let clear_fn = create_map_set_method(state, MapSetMethodKind::Clear);
                    let size_fn = create_map_set_method(state, MapSetMethodKind::Size);
                    let for_each_fn = create_map_set_method(state, MapSetMethodKind::ForEach);
                    let keys_fn = create_map_set_method(state, MapSetMethodKind::Keys);
                    let values_fn = create_map_set_method(state, MapSetMethodKind::Values);
                    let entries_fn = create_map_set_method(state, MapSetMethodKind::Entries);
                    let _ = define_host_data_property(&mut caller, map_result, "set", set_fn);
                    let _ = define_host_data_property(&mut caller, map_result, "get", get_fn);
                    let _ = define_host_data_property(&mut caller, map_result, "has", has_fn);
                    let _ = define_host_data_property(&mut caller, map_result, "delete", delete_fn);
                    let _ = define_host_data_property(&mut caller, map_result, "clear", clear_fn);
                    let _ = define_host_accessor_property(
                        &mut caller,
                        map_result,
                        "size",
                        size_fn,
                        value::encode_undefined(),
                    );
                    let _ =
                        define_host_data_property(&mut caller, map_result, "forEach", for_each_fn);
                    let _ = define_host_data_property(&mut caller, map_result, "keys", keys_fn);
                    let _ = define_host_data_property(&mut caller, map_result, "values", values_fn);
                    let _ =
                        define_host_data_property(&mut caller, map_result, "entries", entries_fn);
                    let _ = define_host_data_property_by_name_id_with_flags(
                        &mut caller,
                        map_result,
                        encode_symbol_name_id(wjsm_ir::wk_symbol::ITERATOR),
                        entries_fn,
                        constants::FLAG_CONFIGURABLE | constants::FLAG_WRITABLE,
                    );
                }
                if let Some(_map_ptr) = resolve_handle(&mut caller, map_result) {
                    let handle_val = value::encode_f64(map_handle as f64);
                    define_host_data_property(
                        &mut caller,
                        map_result,
                        "__map_handle__",
                        handle_val,
                    );
                }
                let mut groups: Vec<(i64, Vec<i64>)> = Vec::new();
                let mut key_to_index: HashMap<i64, usize> = HashMap::new();
                let mut index = 0u32;
                if value::is_array(items)
                    && let Some(arr_ptr) = resolve_array_ptr(&mut caller, items)
                {
                    let len = read_array_length(&mut caller, arr_ptr).unwrap_or(0);
                    for i in 0..len {
                        let elem = read_array_elem(&mut caller, arr_ptr, i)
                            .unwrap_or(value::encode_undefined());
                        let idx_val = value::encode_f64(index as f64);
                        let key = match call_wasm_callback_async(
                            &mut caller,
                            callbackfn,
                            value::encode_undefined(),
                            &[elem, idx_val],
                        )
                        .await
                        {
                            Ok(k) => k,
                            Err(_) => return value::encode_undefined(),
                        };
                        let group_index = if let Some(&idx) = key_to_index.get(&key) {
                            if same_value_zero(&caller, groups[idx].0, key) {
                                Some(idx)
                            } else {
                                None
                            }
                        } else {
                            None
                        };
                        if let Some(idx) = group_index {
                            groups[idx].1.push(elem);
                        } else {
                            let mut found = false;
                            for (existing_key, elements) in &mut groups {
                                if same_value_zero(&caller, *existing_key, key) {
                                    elements.push(elem);
                                    key_to_index.insert(*existing_key, groups.len() - 1);
                                    found = true;
                                    break;
                                }
                            }
                            if !found {
                                key_to_index.insert(key, groups.len());
                                groups.push((key, vec![elem]));
                            }
                        }
                        index += 1;
                    }
                    for (group_key, elements) in &groups {
                        let arr = alloc_array(&mut caller, elements.len() as u32);
                        if let Some(arr_ptr) = resolve_array_ptr(&mut caller, arr) {
                            for (i, &elem) in elements.iter().enumerate() {
                                write_array_elem(&mut caller, arr_ptr, i as u32, elem);
                            }
                            write_array_length(&mut caller, arr_ptr, elements.len() as u32);
                        }
                        let mut table = caller
                            .data()
                            .map_table
                            .lock()
                            .unwrap_or_else(|e| e.into_inner());
                        table[map_handle].keys.push(*group_key);
                        table[map_handle].values.push(arr);
                    }
                }
                map_result
            })
        },
    )?;
    // array.from（async：可迭代对象需 @@iterator/next reentry，mapFn 需回调 reentry）
    linker.func_wrap_async(
        "env",
        "array.from",
        |mut caller: Caller<'_, RuntimeState>,
         (_env, _this, args_base, args_count): (i64, i64, i32, i32)| {
            Box::new(async move {
                if args_count < 1 {
                    return value::encode_undefined();
                }
                let source = read_shadow_arg(&mut caller, args_base, 0);
                let map_fn = if args_count >= 2 {
                    read_shadow_arg(&mut caller, args_base, 1)
                } else {
                    value::encode_undefined()
                };
                crate::host_imports::array_from_impl_async(&mut caller, source, map_fn).await
            })
        },
    )?;
    linker.func_wrap_async(
        "env",
        "object.from_entries",
        |mut caller: Caller<'_, RuntimeState>, (iterable,): (i64,)| {
            Box::new(async move {
                crate::host_imports::object_from_entries_impl_async(&mut caller, iterable).await
            })
        },
    )?;
    linker.func_wrap(
        "env",
        "object.get_own_property_descriptors",
        |mut caller: Caller<'_, RuntimeState>, target: i64| -> i64 {
            crate::host_imports::object_get_own_property_descriptors_impl(&mut caller, target)
        },
    )?;

    Ok(())
}
