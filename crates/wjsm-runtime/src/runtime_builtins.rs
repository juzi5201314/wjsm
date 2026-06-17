use super::*;

#[derive(Clone, Copy)]
pub(crate) enum PromiseSettlement {
    Fulfill(i64),
    Reject(i64),
}

pub(crate) fn raw_promise_handle(promise: i64) -> usize {
    if value::is_object(promise) {
        value::decode_object_handle(promise) as usize
    } else {
        promise as usize
    }
}

pub(crate) fn insert_promise_entry(
    table: &mut Vec<PromiseEntry>,
    handle: usize,
    entry: PromiseEntry,
) {
    if table.len() <= handle {
        table.resize_with(handle + 1, PromiseEntry::empty);
    }
    table[handle] = entry;
}

pub(crate) async fn call_iterable_method_async(
    caller: &mut Caller<'_, RuntimeState>,
    method: i64,
    receiver: i64,
) -> i64 {
    if value::is_native_callable(method) {
        return call_native_callable_with_args_from_caller_async(caller, method, receiver, vec![])
            .await
            .unwrap_or_else(value::encode_undefined);
    }
    // 若调用返回 Err 且包装了 TAG_EXCEPTION，需要保留异常而非转换成 undefined
    match call_wasm_callback_async(caller, method, receiver, &[]).await {
        Ok(val) => val,
        Err(_) => {
            // 通常 call_wasm_callback_async 的 Err 表示陷阱/panic，此时回传 undefined。
            // 但若 method 本身不可调用，GetMethod 已在上游返回 Err(TAG_EXCEPTION)，
            // 该路径不会走到这里（GetMethod 的 Err 在 lib.rs 的 match 中被处理）。
            // 这里保持原有语义：调用失败 → undefined。
            value::encode_undefined()
        }
    }
}

pub(crate) async fn call_iterator_method_async(
    caller: &mut Caller<'_, RuntimeState>,
    method: i64,
    iterator: i64,
    argument: i64,
) -> i64 {
    if value::is_native_callable(method) {
        return Box::pin(call_native_callable_with_args_from_caller_async(
            caller,
            method,
            iterator,
            if value::is_undefined(argument) {
                vec![]
            } else {
                vec![argument]
            },
        ))
        .await
        .unwrap_or_else(value::encode_undefined);
    }
    let args = if value::is_undefined(argument) {
        &[][..]
    } else {
        std::slice::from_ref(&argument)
    };
    call_wasm_callback_async(caller, method, iterator, args)
        .await
        .unwrap_or_else(|_| value::encode_undefined())
}

pub(crate) async fn advance_object_iterator_from_caller_async(
    caller: &mut Caller<'_, RuntimeState>,
    iterator: i64,
    next: i64,
) -> (i64, i64, bool, bool) {
    let result =
        call_iterator_method_async(caller, next, iterator, value::encode_undefined()).await;

    // A3: 若 next() 同步抛出（TAG_EXCEPTION），原样回传让上游处理（转 rejected promise）。
    if value::is_exception(result) {
        return (result, value::encode_undefined(), false, true);
    }

    let mut result = result;
    if is_promise_value(caller.data(), result) {
        let promise_handle = raw_promise_handle(result);
        let (fulfilled, rejected) = {
            let table_p = caller
                .data()
                .promise_table
                .lock()
                .expect("promise table mutex");
            match promise_entry(&table_p, promise_handle).map(|e| &e.state) {
                Some(PromiseState::Fulfilled(v)) => (Some(*v), None),
                Some(PromiseState::Rejected(r)) => (None, Some(*r)),
                _ => (None, None),
            }
        };
        if rejected.is_some() {
            return (result, value::encode_undefined(), false, false);
        }
        if let Some(settled_val) = fulfilled {
            result = settled_val;
        } else {
            return (result, value::encode_undefined(), false, false);
        }
    }
    if (value::is_object(result) || value::is_function(result) || value::is_array(result))
        && let Some(ptr) = resolve_handle(caller, result)
    {
        let done = read_object_property_by_name(caller, ptr, "done")
            .map(nanbox_to_bool)
            .unwrap_or(false);
        let current_value = read_object_property_by_name(caller, ptr, "value")
            .unwrap_or_else(value::encode_undefined);
        return (result, current_value, done, true);
    }

    // 非对象非异常 → 构造可捕获 TypeError
    let type_error = make_type_error_exception(caller, "iterator next must return an object");
    (type_error, value::encode_undefined(), false, true)
}

pub(crate) fn create_async_generator_identity(state: &RuntimeState, generator: i64) -> i64 {
    let mut table = state
        .native_callables
        .lock()
        .expect("native callable table mutex");
    let handle = table.len() as u32;
    table.push(NativeCallable::AsyncGeneratorIdentity { generator });
    value::encode_native_callable_idx(handle)
}

pub(crate) fn create_map_set_method(state: &RuntimeState, kind: MapSetMethodKind) -> i64 {
    let mut table = state
        .native_callables
        .lock()
        .expect("native callable table mutex");
    let handle = table.len() as u32;
    table.push(NativeCallable::MapSetMethod { kind });
    value::encode_native_callable_idx(handle)
}

pub(crate) fn create_date_method(state: &RuntimeState, kind: DateMethodKind) -> i64 {
    let mut table = state
        .native_callables
        .lock()
        .expect("native callable table mutex");
    let handle = table.len() as u32;
    table.push(NativeCallable::DateMethod { kind });
    value::encode_native_callable_idx(handle)
}

pub(crate) fn create_weakmap_method(state: &RuntimeState, kind: WeakMapMethodKind) -> i64 {
    let mut table = state
        .native_callables
        .lock()
        .expect("native callable table mutex");
    let handle = table.len() as u32;
    table.push(NativeCallable::WeakMapMethod { kind });
    value::encode_native_callable_idx(handle)
}

pub(crate) fn create_weakset_method(state: &RuntimeState, kind: WeakSetMethodKind) -> i64 {
    let mut table = state
        .native_callables
        .lock()
        .expect("native callable table mutex");
    let handle = table.len() as u32;
    table.push(NativeCallable::WeakSetMethod { kind });
    value::encode_native_callable_idx(handle)
}

pub(crate) fn read_weakmap_handle(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
) -> Option<usize> {
    if !value::is_object(this_val) {
        return None;
    }
    let obj_ptr = resolve_handle_idx(caller, value::decode_object_handle(this_val) as usize);
    let op = obj_ptr?;
    let handle_val = read_object_property_by_name(caller, op, "__weakmap_handle__")?;
    Some(value::decode_f64(handle_val) as usize)
}

pub(crate) fn read_weakset_handle(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
) -> Option<usize> {
    if !value::is_object(this_val) {
        return None;
    }
    let obj_ptr = resolve_handle_idx(caller, value::decode_object_handle(this_val) as usize);
    let op = obj_ptr?;
    let handle_val = read_object_property_by_name(caller, op, "__weakset_handle__")?;
    Some(value::decode_f64(handle_val) as usize)
}

/// `NumberPrimitiveMethod`：this 为 raw f64，按 method 调用已有 number_proto 语义。
fn invoke_number_primitive_method(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    method: u8,
    args: &[i64],
) -> i64 {
    if !value::is_f64(this_val) {
        return value::encode_undefined();
    }
    let radix_or_digits = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    match method {
        0 => {
            // toString — 复用与 number_proto_to_string 相同逻辑（radix 默认 10）
            let x = value::decode_f64(this_val);
            let radix = if value::is_undefined(radix_or_digits) || value::is_null(radix_or_digits) {
                10
            } else if value::is_f64(radix_or_digits) {
                let r = value::decode_f64(radix_or_digits) as i32;
                if !(2..=36).contains(&r) {
                    return store_runtime_string(caller, "NaN".to_string());
                }
                r
            } else {
                10
            };
            if x.is_nan() {
                return store_runtime_string(caller, "NaN".to_string());
            }
            if x.is_infinite() {
                return store_runtime_string(
                    caller,
                    if x > 0.0 { "Infinity" } else { "-Infinity" }.to_string(),
                );
            }
            if radix == 10 {
                return store_runtime_string(caller, format_number_js(x));
            }
            let int_part = x.trunc() as i64;
            store_runtime_string(caller, format_radix(int_part, radix as u32))
        }
        1 => this_val,
        2 => {
            let x = value::decode_f64(this_val);
            let digits = if value::is_undefined(radix_or_digits) || value::is_null(radix_or_digits)
            {
                0
            } else if value::is_f64(radix_or_digits) {
                value::decode_f64(radix_or_digits) as i32
            } else {
                0
            };
            if !(0..=100).contains(&digits) {
                return store_runtime_string(
                    caller,
                    "RangeError: toFixed() digits argument must be between 0 and 100".to_string(),
                );
            }
            store_runtime_string(caller, format_number_to_fixed_js(x, digits))
        }
        3 => {
            let x = value::decode_f64(this_val);
            let digits = if value::is_undefined(radix_or_digits) || value::is_null(radix_or_digits)
            {
                None
            } else if value::is_f64(radix_or_digits) {
                Some(value::decode_f64(radix_or_digits) as i32)
            } else {
                None
            };
            store_runtime_string(caller, format_number_to_exponential_js(x, digits))
        }
        4 => {
            let x = value::decode_f64(this_val);
            let precision = if value::is_undefined(radix_or_digits) {
                None
            } else if value::is_f64(radix_or_digits) {
                Some(value::decode_f64(radix_or_digits) as i32)
            } else {
                Some(-1)
            };
            if let Some(precision) = precision
                && !(1..=21).contains(&precision)
            {
                return store_runtime_string(
                    caller,
                    "RangeError: toPrecision() argument must be between 1 and 21".to_string(),
                );
            }
            store_runtime_string(caller, format_number_to_precision_js(x, precision))
        }
        _ => value::encode_undefined(),
    }
}

fn array_like_length(caller: &mut Caller<'_, RuntimeState>, target: i64) -> u32 {
    if value::is_array(target)
        && let Some(ptr) = resolve_handle(caller, target)
    {
        return read_array_length(caller, ptr).unwrap_or(0);
    }
    let Some(ptr) = resolve_handle(caller, target) else {
        return 0;
    };
    read_object_property_by_name(caller, ptr, "length")
        .map(value::decode_f64)
        .unwrap_or(0.0)
        .max(0.0) as u32
}

fn create_array_like_values_iterator(caller: &mut Caller<'_, RuntimeState>, target: i64) -> i64 {
    let length = array_like_length(caller, target);
    let index = Arc::new(Mutex::new(0));
    let next = create_native_callable(
        caller.data(),
        NativeCallable::ArrayLikeIteratorNext {
            target,
            index,
            length,
        },
    );
    let obj = {
        let env = WasmEnv::from_caller(caller).expect("WasmEnv");
        alloc_host_object(caller, &env, 1)
    };
    let _ = define_host_data_property_from_caller(caller, obj, "next", next);
    obj
}

fn advance_array_like_values_iterator(
    caller: &mut Caller<'_, RuntimeState>,
    target: i64,
    index: Arc<Mutex<u32>>,
    length: u32,
) -> i64 {
    let idx = {
        let mut index = index.lock().expect("array-like iterator index mutex");
        if *index >= length {
            return alloc_iterator_result_from_caller(caller, value::encode_undefined(), true);
        }
        let idx = *index;
        *index += 1;
        idx
    };
    let value = if value::is_array(target) {
        resolve_handle(caller, target)
            .and_then(|ptr| read_array_elem(caller, ptr, idx))
            .unwrap_or_else(value::encode_undefined)
    } else {
        let key = idx.to_string();
        resolve_handle(caller, target)
            .and_then(|ptr| read_object_property_by_name(caller, ptr, &key))
            .unwrap_or_else(value::encode_undefined)
    };
    alloc_iterator_result_from_caller(caller, value, false)
}

pub(crate) fn call_native_callable_with_args_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    callable: i64,
    this_val: i64,
    args: Vec<i64>,
) -> Option<i64> {
    if !value::is_native_callable(callable) {
        return None;
    }

    let idx = value::decode_native_callable_idx(callable) as usize;
    let record = {
        let table = caller
            .data()
            .native_callables
            .lock()
            .expect("native callable table mutex");
        table.get(idx).cloned()
    }?;
    let argument = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);

    match record {
        NativeCallable::ArgumentsStrictCalleeGetter => Some(
            crate::runtime_arguments::arguments_strict_callee_getter(caller, this_val),
        ),
        NativeCallable::ArrayProtoValues => {
            Some(create_array_like_values_iterator(caller, this_val))
        }
        NativeCallable::ArrayLikeIteratorNext {
            target,
            index,
            length,
        } => Some(advance_array_like_values_iterator(
            caller, target, index, length,
        )),
        NativeCallable::NumberPrimitiveMethod { method } => Some(invoke_number_primitive_method(
            caller, this_val, method, &args,
        )),
        NativeCallable::EvalIndirect => Some(perform_eval_from_caller(caller, argument, None)),
        NativeCallable::EvalFunction(function) => {
            Some(call_eval_function_from_caller(caller, function, args))
        }
        NativeCallable::PromiseResolvingFunction {
            promise,
            already_resolved,
            kind,
        } => {
            let mut already = already_resolved.lock().expect("promise resolver mutex");
            if *already {
                return Some(value::encode_undefined());
            }
            *already = true;
            drop(already);
            match kind {
                PromiseResolvingKind::Fulfill => {
                    resolve_promise_from_caller(caller, promise, argument);
                }
                PromiseResolvingKind::Reject => {
                    settle_promise(caller.data(), promise, PromiseSettlement::Reject(argument));
                }
            }
            Some(value::encode_undefined())
        }
        NativeCallable::PromiseCombinatorReaction { .. } => Some(value::encode_undefined()),
        NativeCallable::AsyncGeneratorIdentity { generator } => Some(generator),
        NativeCallable::AsyncGeneratorMethod { generator, kind } => {
            let result_promise = alloc_promise_from_caller(caller, PromiseEntry::pending());
            let request = AsyncGeneratorRequest {
                completion_type: kind,
                value: argument,
                promise: result_promise,
            };
            let completed = {
                let mut table = caller
                    .data()
                    .async_generator_table
                    .lock()
                    .expect("async generator table mutex");
                let Some(entry) = table.get_mut(value::decode_object_handle(generator) as usize)
                else {
                    return Some(result_promise);
                };
                if matches!(entry.state, AsyncGeneratorState::Completed) {
                    true
                } else {
                    entry.queue.push_back(request);
                    false
                }
            };
            if completed {
                match kind {
                    AsyncGeneratorCompletionType::Throw => settle_promise(
                        caller.data(),
                        result_promise,
                        PromiseSettlement::Reject(argument),
                    ),
                    _ => {
                        let result = alloc_iterator_result_from_caller(caller, argument, true);
                        resolve_promise_from_caller(caller, result_promise, result);
                    }
                }
            } else {
                pump_async_generator_from_caller(caller, generator);
            }
            Some(result_promise)
        }
        NativeCallable::MapSetMethod { kind } => Some(call_map_set_method_from_caller(
            caller, this_val, kind, args,
        )),
        NativeCallable::DateMethod { kind } => {
            Some(call_date_method_from_caller(caller, this_val, kind, args))
        }
        NativeCallable::WeakMapMethod { kind } => Some(call_weakmap_method_from_caller(
            caller, this_val, kind, args,
        )),
        NativeCallable::WeakSetMethod { kind } => Some(call_weakset_method_from_caller(
            caller, this_val, kind, args,
        )),
        NativeCallable::ArrayConstructor => {
            if value::is_object(this_val) {
                Some(this_val)
            } else {
                Some({
                    let _wjsm_env = WasmEnv::from_caller(caller).expect("WasmEnv");
                    alloc_host_object(caller, &_wjsm_env, 4)
                })
            }
        }
        NativeCallable::ObjectConstructor => {
            if value::is_object(this_val) || value::is_function(this_val) {
                Some(this_val)
            } else {
                Some({
                    let _wjsm_env = WasmEnv::from_caller(caller).expect("WasmEnv");
                    alloc_host_object(caller, &_wjsm_env, 4)
                })
            }
        }
        NativeCallable::ObjectProtoToString => Some(obj_proto_to_string_impl(caller, this_val)),
        NativeCallable::ObjectProtoValueOf => Some(this_val),
        NativeCallable::FunctionConstructor
        | NativeCallable::StringConstructor
        | NativeCallable::BooleanConstructor
        | NativeCallable::NumberConstructor
        | NativeCallable::BigIntConstructor
        | NativeCallable::RegExpConstructor => Some(value::encode_undefined()),
        NativeCallable::SymbolConstructor => Some({
            let desc = args
                .first()
                .copied()
                .unwrap_or_else(value::encode_undefined);
            let description = if value::is_undefined(desc) {
                None
            } else if value::is_string(desc) {
                Some(get_string_value(caller, desc))
            } else {
                Some(
                    render_value(caller, desc)
                        .unwrap_or_default()
                        .trim_matches('"')
                        .to_string(),
                )
            };
            let mut table = caller
                .data()
                .symbol_table
                .lock()
                .expect("symbol_table mutex");
            let handle = table.len() as u32;
            table.push(SymbolEntry {
                description,
                global_key: None,
            });
            value::encode_symbol_handle(handle)
        }),
        NativeCallable::ErrorConstructor
        | NativeCallable::TypeErrorConstructor
        | NativeCallable::RangeErrorConstructor
        | NativeCallable::SyntaxErrorConstructor
        | NativeCallable::ReferenceErrorConstructor
        | NativeCallable::URIErrorConstructor
        | NativeCallable::EvalErrorConstructor
        | NativeCallable::AggregateErrorConstructor => {
            let error_name = match &record {
                NativeCallable::ErrorConstructor => "Error",
                NativeCallable::TypeErrorConstructor => "TypeError",
                NativeCallable::RangeErrorConstructor => "RangeError",
                NativeCallable::SyntaxErrorConstructor => "SyntaxError",
                NativeCallable::ReferenceErrorConstructor => "ReferenceError",
                NativeCallable::URIErrorConstructor => "URIError",
                NativeCallable::EvalErrorConstructor => "EvalError",
                NativeCallable::AggregateErrorConstructor => "AggregateError",
                _ => "Error",
            };
            let msg = args
                .first()
                .copied()
                .unwrap_or_else(value::encode_undefined);
            Some(create_error_object(caller, error_name, msg))
        }
        NativeCallable::MapConstructor => Some({
            let _wjsm_env = WasmEnv::from_caller(caller).expect("WasmEnv");
            alloc_host_object(caller, &_wjsm_env, 0)
        }),
        NativeCallable::SetConstructor => Some({
            let _wjsm_env = WasmEnv::from_caller(caller).expect("WasmEnv");
            alloc_host_object(caller, &_wjsm_env, 0)
        }),
        NativeCallable::WeakMapConstructor => Some({
            let _wjsm_env = WasmEnv::from_caller(caller).expect("WasmEnv");
            alloc_host_object(caller, &_wjsm_env, 0)
        }),
        NativeCallable::WeakSetConstructor => Some({
            let _wjsm_env = WasmEnv::from_caller(caller).expect("WasmEnv");
            alloc_host_object(caller, &_wjsm_env, 0)
        }),
        NativeCallable::WeakRefConstructor => Some({
            let _wjsm_env = WasmEnv::from_caller(caller).expect("WasmEnv");
            alloc_host_object(caller, &_wjsm_env, 0)
        }),
        NativeCallable::FinalizationRegistryConstructor => Some({
            let _wjsm_env = WasmEnv::from_caller(caller).expect("WasmEnv");
            alloc_host_object(caller, &_wjsm_env, 0)
        }),
        NativeCallable::DateConstructorGlobal => Some({
            let _wjsm_env = WasmEnv::from_caller(caller).expect("WasmEnv");
            alloc_host_object(caller, &_wjsm_env, 4)
        }),
        NativeCallable::PromiseConstructor => Some({
            let _wjsm_env = WasmEnv::from_caller(caller).expect("WasmEnv");
            alloc_host_object(caller, &_wjsm_env, 0)
        }),
        NativeCallable::ArrayBufferConstructorGlobal => Some({
            let _wjsm_env = WasmEnv::from_caller(caller).expect("WasmEnv");
            alloc_host_object(caller, &_wjsm_env, 4)
        }),
        NativeCallable::DataViewConstructorGlobal => Some({
            let _wjsm_env = WasmEnv::from_caller(caller).expect("WasmEnv");
            alloc_host_object(caller, &_wjsm_env, 4)
        }),
        NativeCallable::TypedArrayConstructor(_) => Some({
            let _wjsm_env = WasmEnv::from_caller(caller).expect("WasmEnv");
            alloc_host_object(caller, &_wjsm_env, 4)
        }),
        NativeCallable::BigInt64ArrayConstructor => Some(typedarray_construct(
            caller,
            argument,
            args.get(1).copied().unwrap_or_else(value::encode_undefined),
            args.get(2).copied().unwrap_or_else(value::encode_undefined),
            8,
            4,
            Some(this_val),
        )),
        NativeCallable::BigUint64ArrayConstructor => Some(typedarray_construct(
            caller,
            argument,
            args.get(1).copied().unwrap_or_else(value::encode_undefined),
            args.get(2).copied().unwrap_or_else(value::encode_undefined),
            8,
            5,
            Some(this_val),
        )),
        NativeCallable::ProxyConstructor => Some({
            let target = args
                .first()
                .copied()
                .unwrap_or_else(value::encode_undefined);
            let handler = args.get(1).copied().unwrap_or_else(value::encode_undefined);
            if !value::is_js_object(target) {
                make_type_error_exception(caller, "TypeError: Proxy target must be an object")
            } else if !value::is_js_object(handler) {
                make_type_error_exception(caller, "TypeError: Proxy handler must be an object")
            } else {
                let handle = {
                    let mut table = caller.data().proxy_table.lock().expect("proxy_table mutex");
                    let handle = table.len() as u32;
                    table.push(ProxyEntry {
                        target,
                        handler,
                        revoked: false,
                    });
                    handle
                };
                value::encode_proxy_handle(handle)
            }
        }),
        NativeCallable::ProxyRevoker { proxy_handle } => {
            let mut table = caller.data().proxy_table.lock().expect("proxy_table mutex");
            if let Some(entry) = table.get_mut(proxy_handle as usize) {
                entry.revoked = true;
            }
            Some(value::encode_undefined())
        }
        NativeCallable::WeakRefDerefMethod => Some(weakref_deref_impl(caller, this_val)),
        NativeCallable::FinalizationRegistryRegisterMethod => {
            Some(fr_register_impl_with_args(caller, this_val, args))
        }
        NativeCallable::FinalizationRegistryUnregisterMethod => {
            let token = args
                .first()
                .copied()
                .unwrap_or_else(value::encode_undefined);
            Some(fr_unregister_impl(caller, this_val, token))
        }
        NativeCallable::StubGlobal(_) => Some(value::encode_undefined()),
        NativeCallable::GcCollect => {
            // P4：gc() global 重接到 GC 框架（不再调旧 trigger_gc，P5 删除）。
            let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
                return Some(value::encode_undefined());
            };
            let gc_arc = caller.data().gc_algorithm.clone();
            let mut gc = gc_arc.lock().expect("gc_algorithm mutex");
            let mut ctx =
                crate::runtime_gc::GcContext::new(&mut *caller, memory, gc.algorithm_name());
            let mut roots = crate::runtime_gc::roots::RuntimeRoots;
            gc.collect_with_provider(&mut ctx, &mut roots as _);
            Some(value::encode_undefined())
        }
        NativeCallable::SharedArrayBufferConstructor => {
            let length = argument;
            let options = args.get(1).copied().unwrap_or_else(value::encode_undefined);
            Some(crate::shared_buffer::construct_shared_array_buffer(
                caller, length, options, this_val,
            ))
        }
        // ── Agent harness ──
        NativeCallable::AgentStart => {
            let script = argument;
            crate::agent_cluster::agent_start(caller, script)
        }
        NativeCallable::AgentBroadcast => {
            let sab = argument;
            crate::agent_cluster::agent_broadcast(caller, sab)
        }
        NativeCallable::AgentReceiveBroadcast => {
            crate::agent_cluster::agent_receive_broadcast(caller, argument)
        }
        NativeCallable::AgentReport => {
            let msg = argument;
            crate::agent_cluster::agent_report(caller, msg)
        }
        NativeCallable::AgentGetReport => {
            let shared = match caller.data().shared_state.clone() {
                Some(s) => s,
                None => return Some(value::encode_undefined()),
            };
            let report = shared.agent_state.reports.lock().unwrap().pop();
            match report {
                Some(r) => Some(store_runtime_string(caller, r)),
                None => Some(value::encode_null()),
            }
        }
        NativeCallable::AgentSleep => {
            let ms = args.first().copied().map(value::decode_f64).unwrap_or(0.0) as u64;
            std::thread::sleep(std::time::Duration::from_millis(ms));
            Some(value::encode_undefined())
        }
        NativeCallable::AgentMonotonicNow => {
            static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
            let start = START.get_or_init(std::time::Instant::now);
            Some(value::encode_f64(start.elapsed().as_millis() as f64))
        }
        NativeCallable::AtomicsGlobal => Some({
            let _wjsm_env = WasmEnv::from_caller(caller).expect("WasmEnv");
            alloc_host_object(caller, &_wjsm_env, 4)
        }),
        // ── Async iterator methods ──
        NativeCallable::AsyncIteratorProtoSymbolAsyncIterator => Some(this_val),
        NativeCallable::AsyncFromSyncNext { handle: _ } => {
            // 仅由 iterator_next_async → call_native_callable_with_args_from_caller_async 调用
            Some(value::encode_undefined())
        }
        NativeCallable::AsyncFromSyncReturn { handle: _ } => {
            // 仅由 async 路径（call_native_callable_with_args_from_caller_async）处理
            Some(value::encode_undefined())
        }
        NativeCallable::AsyncFromSyncThrow { handle } => {
            let arg = args.first().copied().unwrap_or(value::encode_undefined());
            let (_sync_iter_handle, sync_done) = {
                let table = caller
                    .data()
                    .async_from_sync_iterators
                    .lock()
                    .expect("async-from-sync iterators mutex");
                let entry = match table.get(handle as usize) {
                    Some(e) => e,
                    None => return Some(value::encode_undefined()),
                };
                (entry.sync_iterator, entry.sync_done)
            };
            if sync_done {
                let promise = alloc_promise_from_caller(caller, PromiseEntry::pending());
                settle_promise(caller.data(), promise, PromiseSettlement::Reject(arg));
                return Some(promise);
            }
            {
                let mut table = caller
                    .data()
                    .async_from_sync_iterators
                    .lock()
                    .expect("async-from-sync iterators mutex");
                if let Some(entry) = table.get_mut(handle as usize) {
                    entry.sync_done = true;
                }
            }
            let promise = alloc_promise_from_caller(caller, PromiseEntry::pending());
            settle_promise(caller.data(), promise, PromiseSettlement::Reject(arg));
            Some(promise)
        }
        NativeCallable::HeadersMethod { kind, .. } => {
            call_headers_method_from_caller(caller, this_val, kind, &args)
        }
        NativeCallable::ResponseMethod { kind, .. } => {
            call_response_method_from_caller(caller, this_val, kind, &args)
        }
        NativeCallable::RequestMethod { kind, .. } => {
            call_request_method_from_caller(caller, this_val, kind, &args)
        }
        NativeCallable::HeadersConstructor => construct_headers(caller, this_val, &args),
        NativeCallable::ResponseConstructor => construct_response(caller, this_val, &args),
        NativeCallable::RequestConstructor => construct_request(caller, this_val, &args),
        NativeCallable::AbortControllerConstructor => {
            construct_abort_controller(caller, this_val, &args)
        }
        NativeCallable::AbortControllerAbort { signal_handle } => {
            abort_controller_abort(caller, signal_handle, &args)
        }
        // ── ReadableStream (WHATWG Streams Phase 1) ──
        // ReadableStreamConstructor is async-only: routed through the host-import
        // `readable_stream_constructor` (linker.func_wrap_async in fetch.rs). It is
        // never dispatched via the sync NativeCallable path.
        NativeCallable::ReadableStreamConstructor => Some(value::encode_undefined()),
        NativeCallable::ReadableStreamMethod { handle, kind } => {
            call_readable_stream_method_from_caller(caller, this_val, handle, kind, &args)
        }
        NativeCallable::ReadableStreamDefaultReaderMethod { handle, kind } => {
            call_default_reader_method_from_caller(caller, this_val, handle, kind, &args)
        }
        NativeCallable::ReadableStreamDefaultControllerMethod { handle, kind } => {
            call_default_controller_method_from_caller(caller, this_val, handle, kind, &args)
        }
        NativeCallable::ReadableStreamByobRequestMethod { handle, kind } => {
            call_byob_request_method_from_caller(caller, this_val, handle, kind, &args)
        }
        // ── ReadableStream async iterator (WHATWG Streams Phase 2) ──
        NativeCallable::ReadableStreamAsyncIteratorNext { reader_handle } => {
            call_default_reader_method_from_caller(
                caller,
                this_val,
                reader_handle,
                ReadableStreamDefaultReaderMethodKind::Read,
                &args,
            )
        }
        NativeCallable::ReadableStreamAsyncIteratorReturn { reader_handle } => {
            // releaseLock：释放流的锁定
            let stream_handle = {
                let reader_table = caller.data().reader_table.lock().expect("reader mutex");
                reader_table
                    .get(reader_handle as usize)
                    .map(|e| e.stream_handle)
            };
            if let Some(sh) = stream_handle {
                let mut stream_table = caller
                    .data()
                    .readable_stream_table
                    .lock()
                    .expect("stream mutex");
                if let Some(entry) = stream_table.get_mut(sh as usize) {
                    entry.locked = false;
                }
            }
            // 返回 {done: true, value: undefined} 作为 resolved Promise
            let p = alloc_promise_from_caller(caller, PromiseEntry::pending());
            let result = build_reader_result(caller, true, None);
            settle_promise(caller.data(), p, PromiseSettlement::Fulfill(result));
            Some(p)
        }
        // ── WritableStream (WHATWG Streams Phase 4) ──
        // WritableStreamConstructor is async-only: routed through the host-import
        // `writable_stream_constructor` (linker.func_wrap_async in fetch.rs). It is
        // never dispatched via the sync NativeCallable path.
        NativeCallable::WritableStreamConstructor => Some(value::encode_undefined()),
        NativeCallable::WritableStreamMethod { handle, kind } => {
            call_writable_stream_method_from_caller(caller, this_val, handle, kind, &args)
        }
        NativeCallable::WritableStreamDefaultWriterMethod { handle, kind } => {
            call_default_writer_method_from_caller(caller, this_val, handle, kind, &args)
        }
        NativeCallable::WritableStreamDefaultControllerMethod { handle, kind } => {
            call_writable_controller_method_from_caller(caller, this_val, handle, kind, &args)
        }
        // ── TransformStream (WHATWG Streams Phase 5) ──
        // TransformStreamConstructor is async-only: routed through the host-import
        // `transform_stream_constructor`. It is never dispatched via the sync NativeCallable path.
        NativeCallable::TransformStreamConstructor => Some(value::encode_undefined()),
        NativeCallable::TransformStreamMethod { handle, kind } => {
            call_transform_stream_method_from_caller(caller, this_val, handle, kind, &args)
        }
        // ── QueuingStrategy (WHATWG Streams Phase 2) ──
        NativeCallable::CountQueuingStrategyConstructor => {
            construct_count_queuing_strategy(caller, this_val, &args)
        }
        NativeCallable::ByteLengthQueuingStrategyConstructor => {
            construct_byte_length_queuing_strategy(caller, this_val, &args)
        }
        NativeCallable::QueuingStrategySize { kind } => {
            call_queuing_strategy_size_from_caller(caller, kind, &args)
        }
    }
}

pub(crate) async fn call_native_callable_with_args_from_caller_async(
    caller: &mut Caller<'_, RuntimeState>,
    callable: i64,
    this_val: i64,
    args: Vec<i64>,
) -> Option<i64> {
    if !value::is_native_callable(callable) {
        return None;
    }

    let idx = value::decode_native_callable_idx(callable) as usize;
    let record = {
        let table = caller
            .data()
            .native_callables
            .lock()
            .expect("native callable table mutex");
        table.get(idx).cloned()
    }?;
    let argument = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);

    match record {
        NativeCallable::ArgumentsStrictCalleeGetter => Some(
            crate::runtime_arguments::arguments_strict_callee_getter(caller, this_val),
        ),
        NativeCallable::EvalIndirect => {
            Some(perform_eval_from_caller_async(caller, argument, None).await)
        }
        NativeCallable::EvalFunction(function) => {
            Some(call_eval_function_from_caller_async(caller, function, args).await)
        }
        NativeCallable::AgentReceiveBroadcast => {
            crate::agent_cluster::agent_receive_broadcast_async(caller, argument).await
        }
        NativeCallable::AgentReport => crate::agent_cluster::agent_report(caller, argument),
        NativeCallable::AgentGetReport => {
            let shared = match caller.data().shared_state.clone() {
                Some(s) => s,
                None => return Some(value::encode_undefined()),
            };
            let report = shared.agent_state.reports.lock().unwrap().pop();
            match report {
                Some(r) => Some(store_runtime_string(caller, r)),
                None => Some(value::encode_null()),
            }
        }
        NativeCallable::AgentSleep => {
            let ms = args.first().copied().map(value::decode_f64).unwrap_or(0.0) as u64;
            std::thread::sleep(std::time::Duration::from_millis(ms));
            Some(value::encode_undefined())
        }
        NativeCallable::AsyncFromSyncNext { handle } => {
            Some(Box::pin(advance_async_from_sync_async(caller, handle)).await)
        }
        NativeCallable::AsyncFromSyncReturn { handle } => {
            let arg = args.first().copied().unwrap_or(value::encode_undefined());
            let (sync_iter_handle, sync_done) = {
                let table = caller
                    .data()
                    .async_from_sync_iterators
                    .lock()
                    .expect("async-from-sync iterators mutex");
                let entry = match table.get(handle as usize) {
                    Some(e) => e,
                    None => return Some(value::encode_undefined()),
                };
                (entry.sync_iterator, entry.sync_done)
            };
            if sync_done {
                let promise = alloc_promise_from_caller(caller, PromiseEntry::pending());
                let result = alloc_iterator_result_from_caller(caller, arg, true);
                resolve_promise_from_caller(caller, promise, result);
                return Some(promise);
            }
            {
                let mut table = caller
                    .data()
                    .async_from_sync_iterators
                    .lock()
                    .expect("async-from-sync iterators mutex");
                if let Some(entry) = table.get_mut(handle as usize) {
                    entry.sync_done = true;
                }
            }
            Some(call_sync_iter_and_wrap_async(caller, sync_iter_handle, Some(arg), false).await)
        }
        NativeCallable::AgentStart
        | NativeCallable::AgentBroadcast
        | NativeCallable::AgentMonotonicNow => {
            call_native_callable_with_args_from_caller(caller, callable, this_val, args)
        }
        _ => call_native_callable_with_args_from_caller(caller, callable, this_val, args),
    }
}
/// 创建 AsyncFromSyncIterator：将同步迭代器包装为异步迭代器协议。
/// 在 iterators 表中注册 ObjectIter（next/return 为 NativeCallable），
/// 返回 TAG_ITERATOR 句柄供 for-await 使用。
pub(crate) fn create_async_from_sync_iterator(
    caller: &mut Caller<'_, RuntimeState>,
    sync_iter_handle: i64,
) -> i64 {
    let mut iters = caller.data().iterators.lock().expect("iterators mutex");
    let iter_handle = iters.len() as u32;
    let outer_iter = value::encode_handle(value::TAG_ITERATOR, iter_handle);

    let table_idx = {
        let mut table = caller
            .data()
            .async_from_sync_iterators
            .lock()
            .expect("async-from-sync iterators mutex");
        let idx = table.len() as u32;
        table.push(AsyncFromSyncIteratorEntry {
            sync_iterator: sync_iter_handle,
            sync_done: false,
            outer_iter,
            outer_handle_idx: iter_handle,
        });
        idx
    };

    let next_callable = {
        let mut nc = caller
            .data()
            .native_callables
            .lock()
            .expect("native callables mutex");
        let handle = nc.len() as u32;
        nc.push(NativeCallable::AsyncFromSyncNext { handle: table_idx });
        value::encode_native_callable_idx(handle)
    };
    let return_callable = {
        let mut nc = caller
            .data()
            .native_callables
            .lock()
            .expect("native callables mutex");
        let handle = nc.len() as u32;
        nc.push(NativeCallable::AsyncFromSyncReturn { handle: table_idx });
        value::encode_native_callable_idx(handle)
    };

    iters.push(IteratorState::ObjectIter {
        iterator: sync_iter_handle,
        next: next_callable,
        return_method: Some(return_callable),
        current_value: value::encode_undefined(),
        has_current: false,
        done: false,
    });
    outer_iter
}

/// 调用同步迭代器的方法并将结果包装为 resolved Promise。
async fn call_sync_iter_and_wrap_async(
    caller: &mut Caller<'_, RuntimeState>,
    sync_iter_handle: i64,
    arg_if_return: Option<i64>,
    is_throw: bool,
) -> i64 {
    let sync_handle_idx = value::decode_handle(sync_iter_handle) as usize;

    let (iterator, method_to_call) = {
        let iters = caller.data().iterators.lock().expect("iterators mutex");
        match iters.get(sync_handle_idx) {
            Some(IteratorState::ObjectIter {
                iterator,
                next,
                return_method,
                ..
            }) => {
                let method = if arg_if_return.is_some() {
                    return_method.unwrap_or(*next)
                } else {
                    *next
                };
                (*iterator, method)
            }
            _ => return value::encode_undefined(),
        }
    };

    if is_throw {
        let promise = alloc_promise_from_caller(caller, PromiseEntry::pending());
        settle_promise(
            caller.data(),
            promise,
            PromiseSettlement::Reject(arg_if_return.unwrap_or(value::encode_undefined())),
        );
        return promise;
    }

    let call_arg = arg_if_return.unwrap_or(value::encode_undefined());
    let raw_result = call_iterator_method_async(caller, method_to_call, iterator, call_arg).await;

    if value::is_exception(raw_result) {
        {
            let mut iters = caller.data().iterators.lock().expect("iterators mutex");
            if let Some(IteratorState::ObjectIter { done, .. }) = iters.get_mut(sync_handle_idx) {
                *done = true;
            }
        }
        let promise = alloc_promise_from_caller(caller, PromiseEntry::pending());
        let reason = exception_reason(caller, raw_result);
        settle_promise(caller.data(), promise, PromiseSettlement::Reject(reason));
        return promise;
    }

    let (done, current_value) = if (value::is_object(raw_result)
        || value::is_function(raw_result)
        || value::is_array(raw_result))
        && let Some(ptr) = resolve_handle(caller, raw_result)
    {
        let done = read_object_property_by_name(caller, ptr, "done")
            .map(nanbox_to_bool)
            .unwrap_or(true);
        let value =
            read_object_property_by_name(caller, ptr, "value").unwrap_or(value::encode_undefined());
        (done, value)
    } else {
        (true, value::encode_undefined())
    };

    let promise = alloc_promise_from_caller(caller, PromiseEntry::pending());
    let result = alloc_iterator_result_from_caller(caller, current_value, done);
    resolve_promise_from_caller(caller, promise, result);
    promise
}

/// AsyncFromSyncIterator.next()：推进同步迭代器并返回 Promise<IteratorResult>。
pub(crate) async fn advance_async_from_sync_async(
    caller: &mut Caller<'_, RuntimeState>,
    handle: u32,
) -> i64 {
    let (sync_iter_handle, sync_done) = {
        let table = caller
            .data()
            .async_from_sync_iterators
            .lock()
            .expect("async-from-sync iterators mutex");
        let entry = match table.get(handle as usize) {
            Some(e) => e,
            None => return value::encode_undefined(),
        };
        (entry.sync_iterator, entry.sync_done)
    };

    if sync_done {
        let promise = alloc_promise_from_caller(caller, PromiseEntry::pending());
        let result = alloc_iterator_result_from_caller(caller, value::encode_undefined(), true);
        resolve_promise_from_caller(caller, promise, result);
        return promise;
    }
    let sync_handle_idx = value::decode_handle(sync_iter_handle) as usize;

    // Direct advancement for non-ObjectIter types
    let direct_result = {
        let mut iters = caller.data().iterators.lock().expect("iterators mutex");
        match iters.get_mut(sync_handle_idx) {
            Some(IteratorState::ArrayIter { ptr, index, length }) => {
                if *index < *length {
                    let idx = *index;
                    let array_ptr = *ptr;
                    *index += 1;
                    drop(iters);
                    let val = read_array_elem(caller, array_ptr, idx)
                        .unwrap_or(value::encode_undefined());
                    Some((false, val))
                } else {
                    Some((true, value::encode_undefined()))
                }
            }
            Some(IteratorState::MapKeyIter { keys, index }) => {
                if (*index as usize) < keys.len() {
                    let val = keys[*index as usize];
                    *index += 1;
                    Some((false, val))
                } else {
                    Some((true, value::encode_undefined()))
                }
            }
            Some(IteratorState::MapValueIter { values, index }) => {
                if (*index as usize) < values.len() {
                    let val = values[*index as usize];
                    *index += 1;
                    Some((false, val))
                } else {
                    Some((true, value::encode_undefined()))
                }
            }
            Some(IteratorState::TypedArrayValueIter {
                entry,
                index,
                length,
            }) => {
                if *index < *length {
                    let entry = entry.clone();
                    let idx = *index;
                    *index += 1;
                    drop(iters);
                    let val = typedarray_element_read_entry(caller, &entry, idx)
                        .unwrap_or(value::encode_undefined());
                    Some((false, val))
                } else {
                    Some((true, value::encode_undefined()))
                }
            }
            Some(IteratorState::TypedArrayEntryIter {
                entry,
                index,
                length,
            }) => {
                if *index < *length {
                    let typedarray_entry = entry.clone();
                    let idx = *index;
                    *index += 1;
                    drop(iters);
                    let entry = alloc_array(caller, 2);
                    if let Some(entry_ptr) = resolve_array_ptr(caller, entry) {
                        let elem = typedarray_element_read_entry(caller, &typedarray_entry, idx)
                            .unwrap_or(value::encode_undefined());
                        write_array_elem(caller, entry_ptr, 0, value::encode_f64(idx as f64));
                        write_array_elem(caller, entry_ptr, 1, elem);
                        write_array_length(caller, entry_ptr, 2);
                    }
                    Some((false, entry))
                } else {
                    Some((true, value::encode_undefined()))
                }
            }
            Some(IteratorState::StringIter { byte_pos, data }) => {
                if *byte_pos < data.len() {
                    let ch = data[*byte_pos] as char;
                    *byte_pos += 1;
                    drop(iters);
                    let val = store_runtime_string(caller, ch.to_string());
                    Some((false, val))
                } else {
                    Some((true, value::encode_undefined()))
                }
            }
            _ => None,
        }
    };

    if let Some((done, current_value)) = direct_result {
        if done {
            let mut table = caller
                .data()
                .async_from_sync_iterators
                .lock()
                .expect("async-from-sync iterators mutex");
            if let Some(entry) = table.get_mut(handle as usize) {
                entry.sync_done = true;
            }
        }
        let promise = alloc_promise_from_caller(caller, PromiseEntry::pending());
        let result = alloc_iterator_result_from_caller(caller, current_value, done);
        resolve_promise_from_caller(caller, promise, result);
        return promise;
    }

    let promise = call_sync_iter_and_wrap_async(caller, sync_iter_handle, None, false).await;

    {
        let iters = caller.data().iterators.lock().expect("iterators mutex");
        if let Some(IteratorState::ObjectIter { done, .. }) = iters.get(sync_handle_idx) {
            if *done {
                drop(iters);
                let mut table = caller
                    .data()
                    .async_from_sync_iterators
                    .lock()
                    .expect("async-from-sync iterators mutex");
                if let Some(entry) = table.get_mut(handle as usize) {
                    entry.sync_done = true;
                }
            }
        }
    }

    promise
}
pub(crate) fn weakref_deref_impl(caller: &mut Caller<'_, RuntimeState>, this_val: i64) -> i64 {
    if !value::is_object(this_val) {
        return value::encode_undefined();
    }
    let obj_ptr = resolve_handle_idx(caller, value::decode_object_handle(this_val) as usize);
    let handle_val =
        obj_ptr.and_then(|p| read_object_property_by_name(caller, p, "__weakref_handle__"));
    let handle = handle_val
        .map(|v| value::decode_f64(v) as usize)
        .unwrap_or(0);
    let table = caller
        .data()
        .weakref_table
        .lock()
        .expect("weakref table mutex");
    if handle >= table.len() {
        return value::encode_undefined();
    }
    let entry = &table[handle];
    if entry.target_handle == 0 {
        return value::encode_undefined();
    }
    value::encode_object_handle(entry.target_handle)
}
pub(crate) fn fr_unregister_impl(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    token: i64,
) -> i64 {
    if !value::is_object(this_val) {
        return value::encode_bool(false);
    }
    let obj_ptr = resolve_handle_idx(caller, value::decode_object_handle(this_val) as usize);
    let handle_val = obj_ptr
        .and_then(|p| read_object_property_by_name(caller, p, "__finalization_registry_handle__"));
    let Some(handle) = handle_val.map(|v| value::decode_f64(v) as usize) else {
        return value::encode_bool(false);
    };
    let mut table = caller
        .data()
        .finalization_registry_table
        .lock()
        .expect("finalization registry table mutex");
    if handle >= table.len() {
        return value::encode_bool(false);
    }
    let entry = &mut table[handle];
    let initial_len = entry.registrations.len();
    entry.registrations.retain(|r| match &r.unregister_token {
        Some(t) => !same_value_zero(*t, token),
        None => true,
    });
    value::encode_bool(entry.registrations.len() < initial_len)
}
pub(crate) fn fr_register_impl_with_args(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    args: Vec<i64>,
) -> i64 {
    if args.len() < 2 {
        return value::encode_undefined();
    }
    let target = args[0];
    let held_value = args[1];
    let unregister_token = if args.len() >= 3 {
        let token = args[2];
        if value::is_js_object(token) || value::is_symbol(token) {
            Some(token)
        } else {
            None
        }
    } else {
        None
    };
    if !value::is_js_object(target) {
        return value::encode_undefined();
    }
    let target_handle = match resolve_handle(caller, target) {
        Some(ptr) => ptr as u32,
        None => return value::encode_undefined(),
    };
    if !value::is_object(this_val) {
        return value::encode_undefined();
    }
    let obj_ptr = resolve_handle_idx(caller, value::decode_object_handle(this_val) as usize);
    let handle_val = obj_ptr
        .and_then(|p| read_object_property_by_name(caller, p, "__finalization_registry_handle__"));
    let Some(handle) = handle_val.map(|v| value::decode_f64(v) as usize) else {
        return value::encode_undefined();
    };
    {
        let mut table = caller
            .data()
            .finalization_registry_table
            .lock()
            .expect("finalization registry table mutex");
        if handle < table.len() {
            table[handle].registrations.push(FinalizationRegistration {
                target_handle,
                held_value,
                unregister_token,
            });
        }
    }
    value::encode_undefined()
}
