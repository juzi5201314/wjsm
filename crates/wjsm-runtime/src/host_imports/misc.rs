use crate::*;


pub(crate) fn register_misc_imports(mut store: &mut Store<RuntimeState>) -> Vec<Extern> {
    // ECMAScript §7.2.3 IsCallable(argument) → boolean
    let is_callable_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, val: i64| -> i64 {
            value::encode_bool(is_callable_in_runtime(&mut caller, val))
        },
    );

    // ── Import 129: queue_microtask(i64) -> () ──────────────────────────────
    let queue_microtask_fn = Func::wrap(
        &mut store,
        |caller: Caller<'_, RuntimeState>, callback: i64| {
            let mut queue = caller
                .data()
                .microtask_queue
                .lock()
                .expect("microtask queue mutex");
            queue.push_back(Microtask::MicrotaskCallback { callback });
        },
    );

    // ── Import 130: drain_microtasks() -> () ────────────────────────────────
    let drain_microtasks_fn = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>| {
        let table = caller.get_export("__table").and_then(|e| e.into_table());
        let Some(func_table) = table else { return };
        drain_microtasks_from_caller(&mut caller, &func_table);
    });

    let native_call_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         callable: i64,
         this_val: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            let new_target_val = caller.data().new_target.get();
            caller.data().new_target.set(value::encode_undefined());

            if value::is_proxy(callable) {
                let handle = value::decode_proxy_handle(callable) as usize;
                let entry = {
                    let table = caller.data().proxy_table.lock().expect("proxy_table mutex");
                    table.get(handle).cloned()
                };
                if let Some(entry) = entry {
                    if entry.revoked {
                        set_runtime_error(caller.data(), "TypeError: Cannot perform call on a proxy that has been revoked".to_string());
                        return value::encode_undefined();
                    }

                    if !value::is_undefined(new_target_val) {
                        // 构造调用
                        if let Some(handler_ptr) = resolve_handle(&mut caller, entry.handler) {
                            let trap = read_object_property_by_name(&mut caller, handler_ptr, "construct")
                                .unwrap_or_else(value::encode_undefined);
                            if !value::is_undefined(trap) && !value::is_null(trap) {
                                let arr = alloc_array(&mut caller, args_count as u32);
                                for i in 0..args_count {
                                    let arg = read_shadow_arg(&mut caller, args_base, i as u32);
                                    set_array_elem(&mut caller, arr, i, arg);
                                }
                                let trap_res = call_wasm_callback(&mut caller, trap, entry.handler, &[entry.target, arr, new_target_val]);
                                return match trap_res {
                                    Ok(res) => {
                                        if !value::is_js_object(res) {
                                            set_runtime_error(caller.data(), "TypeError: Proxy construct trap returned non-object".to_string());
                                            value::encode_undefined()
                                        } else {
                                            res
                                        }
                                    }
                                    Err(e) => {
                                        set_runtime_error(caller.data(), format!("TypeError: Proxy construct trap failed: {}", e));
                                        value::encode_undefined()
                                    }
                                };
                            }
                        }
                        caller.data().new_target.set(new_target_val);
                        let result = resolve_and_call(&mut caller, entry.target, this_val, args_base, args_count);
                        caller.data().new_target.set(value::encode_undefined());
                        return result;
                    } else {
                        // 普通函数调用
                        if let Some(handler_ptr) = resolve_handle(&mut caller, entry.handler) {
                            let trap = read_object_property_by_name(&mut caller, handler_ptr, "apply")
                                .unwrap_or_else(value::encode_undefined);
                            if !value::is_undefined(trap) && !value::is_null(trap) {
                                let arr = alloc_array(&mut caller, args_count as u32);
                                for i in 0..args_count {
                                    let arg = read_shadow_arg(&mut caller, args_base, i as u32);
                                    set_array_elem(&mut caller, arr, i, arg);
                                }
                            let result = call_wasm_callback(&mut caller, trap, entry.handler, &[entry.target, this_val, arr]);
                            return result.unwrap_or_else(|_| {
                                set_runtime_error(caller.data(), "TypeError: Proxy apply trap failed".to_string());
                                value::encode_undefined()
                            });
                            }
                        }
                        return resolve_and_call(&mut caller, entry.target, this_val, args_base, args_count);
                    }
                }
                return value::encode_undefined();
            }

            if !value::is_undefined(new_target_val) {
                caller.data().new_target.set(new_target_val);
            }
            let args = (0..args_count.max(0))
                .map(|index| read_shadow_arg(&mut caller, args_base, index as u32))
                .collect();
            let result = call_native_callable_with_args_from_caller(&mut caller, callable, this_val, args)
                .unwrap_or_else(value::encode_undefined);
            caller.data().new_target.set(value::encode_undefined());
            result
        },
    );

    // ── Import 146: register_module_namespace(i64, i64) -> () ──────────────
    // 将模块命名空间对象注册到运行时缓存
    let register_module_namespace_fn = Func::wrap(
        &mut store,
        |caller: Caller<'_, RuntimeState>, module_id: i64, namespace_obj: i64| {
            let mid = module_id as u32;
            let mut cache = caller
                .data()
                .module_namespace_cache
                .lock()
                .expect("module namespace cache mutex");
            cache.insert(mid, namespace_obj);
        },
    );

    // ── Import 147: dynamic_import(i64) -> i64 ────────────────────────────
    // 动态导入：查找命名空间对象并返回 resolved Promise
    let dynamic_import_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, module_id: i64| -> i64 {
            let mid = module_id as u32;

            // 创建 Promise 并添加 .then/.catch/.finally 方法
            let promise = alloc_promise(&mut caller, PromiseEntry::pending());
            let then_fn = create_promise_resolving_function(
                caller.data(),
                promise,
                Arc::new(Mutex::new(false)),
                PromiseResolvingKind::Fulfill,
            );
            let catch_fn = create_promise_resolving_function(
                caller.data(),
                promise,
                Arc::new(Mutex::new(false)),
                PromiseResolvingKind::Reject,
            );
            let _ = define_host_data_property_from_caller(&mut caller, promise, "then", then_fn);
            let _ = define_host_data_property_from_caller(&mut caller, promise, "catch", catch_fn);

            // 从缓存查找命名空间对象
            let namespace_obj = {
                let cache = caller
                    .data()
                    .module_namespace_cache
                    .lock()
                    .expect("module namespace cache mutex");
                cache.get(&mid).copied()
            };

            match namespace_obj {
                Some(ns_obj) => {
                    // 直接 resolve Promise（AOT 模式下命名空间对象已构建完成）
                    resolve_promise_from_caller(&mut caller, promise, ns_obj);
                }
                None => {
                    // 模块未找到：reject Promise
                    let error_msg = format!("Cannot find module with id {}", mid);
                    let error_val = runtime_error_value(caller.data(), error_msg);
                    settle_promise(caller.data(), promise, PromiseSettlement::Reject(error_val));
                }
            }

            promise
        },
    );

    // ── Import 148/149: eval ────────────────────────────────────────────────
    let eval_direct_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, code: i64, scope_env: i64| -> i64 {
            perform_eval_from_caller(&mut caller, code, Some(scope_env))
        },
    );
    let eval_indirect_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, code: i64| -> i64 {
            perform_eval_from_caller(&mut caller, code, None)
        },
    );

    let jsx_create_element_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, tag: i64, props: i64, children: i64| -> i64 {
            let obj = { let _wjsm_env = WasmEnv::from_caller(&mut caller).expect("WasmEnv"); alloc_host_object(&mut caller, &_wjsm_env, 4) };
            let _ = define_host_data_property_from_caller(
                &mut caller, obj, "type", tag,
            );
            let _ = define_host_data_property_from_caller(
                &mut caller, obj, "props", props,
            );
            let _ = define_host_data_property_from_caller(
                &mut caller, obj, "children", children,
            );
            obj
        },
    );

    vec![
        queue_microtask_fn.into(),      // 129
        drain_microtasks_fn.into(),     // 130
        native_call_fn.into(),          // 141
        is_callable_fn.into(),          // 144
        register_module_namespace_fn.into(), // 146
        dynamic_import_fn.into(),       // 147
        eval_direct_fn.into(),          // 148
        eval_indirect_fn.into(),        // 149
        jsx_create_element_fn.into(),   // 150
    ]
}

pub(crate) fn register_all_imports(store: &mut Store<RuntimeState>) -> Vec<Extern> {
    let mut imports = Vec::with_capacity(50);
    
    let mut p = super::promise::register_promise_imports(store);
    // p: 116,117,118,119,120,121, 126,127,128, 142,143, 145
    let c = super::promise_combinators::register_promise_combinators_imports(store);
    // c: 122,123,124,125
    let mut m = register_misc_imports(store);
    // m: 129,130, 141, 144, 146,147,148,149,150
    let a = super::async_fn::register_async_fn_imports(store);
    // a: 131,132,133,134,135,136
    let g = super::async_generator::register_async_generator_imports(store);
    // g: 137,138,139,140
    let r = super::proxy_reflect::register_proxy_reflect_imports(store);
    // r: 151..=165
    
    imports.extend(p.drain(0..6));  // 116-121
    imports.extend(c);               // 122-125
    imports.extend(p.drain(0..3));  // 126-128
    imports.extend(m.drain(0..2));  // 129-130
    imports.extend(a);               // 131-136
    imports.extend(g);               // 137-140
    imports.push(m.remove(0));      // 141
    imports.extend(p.drain(0..2));  // 142-143
    imports.push(m.remove(0));      // 144
    imports.push(p.remove(0));      // 145
    imports.extend(m);               // 146-150
    imports.extend(r);               // 151-165
    imports
}
