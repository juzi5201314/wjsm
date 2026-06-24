use anyhow::Result;
use wasmtime::Store;
use wasmtime::{Caller, Func, Linker};

use crate::*;

pub(crate) fn define_misc(
    linker: &mut Linker<RuntimeState>,
    mut store: &mut Store<RuntimeState>,
) -> Result<()> {
    // ECMAScript §7.2.3 IsCallable(argument) → boolean
    let is_callable_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, val: i64| -> i64 {
            value::encode_bool(is_callable_in_runtime(&mut caller, val))
        },
    );
    linker.define(&mut store, "env", "is_callable", is_callable_fn)?;

    // ── Import 129: queue_microtask(i64) -> () ──────────────────────────────
    let queue_microtask_fn = Func::wrap(
        &mut store,
        |caller: Caller<'_, RuntimeState>, callback: i64| {
            let mut queue = caller
                .data()
                .microtask_queue.lock().unwrap_or_else(|e| e.into_inner());
            queue.push_back(Microtask::MicrotaskCallback { callback });
        },
    );
    linker.define(&mut store, "env", "queue_microtask", queue_microtask_fn)?;

    // ── Import 146: register_module_namespace(i64, i64) -> () ──────────────
    // 将模块命名空间对象注册到运行时缓存
    let register_module_namespace_fn = Func::wrap(
        &mut store,
        |caller: Caller<'_, RuntimeState>, module_id: i64, namespace_obj: i64| {
            let mid = module_id as u32;
            let mut cache = caller
                .data()
                .module_namespace_cache.lock().unwrap_or_else(|e| e.into_inner());
            cache.insert(mid, namespace_obj);
        },
    );
    linker.define(
        &mut store,
        "env",
        "register_module_namespace",
        register_module_namespace_fn,
    )?;

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
                    .module_namespace_cache.lock().unwrap_or_else(|e| e.into_inner());
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
    linker.define(&mut store, "env", "dynamic_import", dynamic_import_fn)?;

    let jsx_create_element_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, tag: i64, props: i64, children: i64| -> i64 {
            let obj = {
                let _wjsm_env = WasmEnv::from_caller(&mut caller).expect("WasmEnv");
                alloc_host_object(&mut caller, &_wjsm_env, 4)
            };
            let _ = define_host_data_property_from_caller(&mut caller, obj, "type", tag);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "props", props);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "children", children);
            obj
        },
    );
    linker.define(
        &mut store,
        "env",
        "jsx_create_element",
        jsx_create_element_fn,
    )?;

    Ok(())
}
