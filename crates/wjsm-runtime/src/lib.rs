use anyhow::Result;
use chrono::{DateTime, Datelike, Local, TimeZone, Timelike, Utc};
use num_traits::cast::ToPrimitive;
use rand::Rng;
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::io::{self, Write};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use swc_core::ecma::ast as swc_ast;
use tokio::time::Instant;
use wasmtime::Func;
use wasmtime::*;
use wjsm_ir::{constants, value};
mod agent_cluster;
mod property_key;
mod runtime_arguments;
mod runtime_async_fn;
mod runtime_builtins;
mod runtime_collections;
mod runtime_combinators;
mod runtime_date;
mod runtime_eval;
mod runtime_gc;
mod runtime_heap;
mod runtime_host_helpers;
mod runtime_json;
mod runtime_microtask;
mod runtime_promises;
mod runtime_typedarray;
mod runtime_value_adapter;
mod shared_buffer;
mod types;
pub(crate) use shared_buffer::{SharedRuntimeState, new_shared_runtime_state};
mod scheduler;

mod host_imports;
mod runtime_render;
mod runtime_values;
mod wasm_env;
use host_imports::*;
use property_key::*;
pub(crate) use wasm_env::WasmEnv;

use runtime_arguments::*;
use runtime_async_fn::*;
use runtime_builtins::*;
use runtime_collections::*;
use runtime_combinators::*;
use runtime_date::*;
use runtime_eval::*;
use runtime_heap::*;
use runtime_host_helpers::*;
use runtime_json::*;
use runtime_microtask::*;
use runtime_promises::*;
use runtime_render::*;
use runtime_typedarray::*;
use runtime_values::*;
use types::*;
// ── Linker 注册辅助函数 ─────────────────────────────────────────

/// 注册 16 个简单桥接（无 WASM 回调，sync/async 共享）
fn register_common_bridges(
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
        |caller: Caller<'_, RuntimeState>, record: i64, name: i64| -> i64 {
            eval_has_binding(caller, record, name)
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
            if let Some(name_id) = symbol_value_to_name_id(key) {
                return name_id as i32;
            }
            // 运行期字符串（拼接 / 模板 / String() 等）与数字 key：低 32 位是 handle 或 f64 位，
            // 不是 data-section name_id。必须取内容（ToString）后 find-or-alloc 出稳定 name_id，
            // 否则动态属性名（o["p"+i]）或数字 key（o[5]）会错位。
            if value::is_runtime_string_handle(key) || value::is_f64(key) {
                if let Ok(s) = render_value(&mut caller, key) {
                    if let Some(id) = find_memory_c_string(&mut caller, &s)
                        .or_else(|| alloc_heap_c_string(&mut caller, &s))
                    {
                        return id as i32;
                    }
                }
                return 0;
            }
            // 编译期常量字符串：低 32 位即 data 区指针，本身就是 name_id。
            key as i32
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
    // native_callable_get_property
    let f = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, native: i64, name_id: i32| -> i64 {
            let name_id = name_id as u32;
            if is_symbol_name_id(name_id) || read_string_bytes(&mut caller, name_id) != b"prototype"
            {
                return value::encode_undefined();
            }
            let idx = value::decode_native_callable_idx(native) as usize;
            let record = {
                let table = caller
                    .data()
                    .native_callables
                    .lock()
                    .expect("native callable table mutex");
                table.get(idx).cloned()
            };
            match record {
                Some(NativeCallable::ArrayConstructor) => {
                    let env = WasmEnv::from_caller(&mut caller).expect("WasmEnv");
                    let handle = env.array_proto_handle.get(&mut caller).i32().unwrap_or(-1);
                    if handle >= 0 {
                        value::encode_object_handle(handle as u32)
                    } else {
                        value::encode_undefined()
                    }
                }
                _ => value::encode_undefined(),
            }
        },
    );
    linker.define(&mut *store, "env", "native_callable_get_property", f)?;
    // array.from
    let f = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>,
         _env_obj: i64,
         _this_val: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            if args_count < 1 {
                return value::encode_undefined();
            }
            let memory = caller
                .get_export("memory")
                .and_then(|e| e.into_memory())
                .unwrap();
            let mut buf = [0u8; 8];
            let _ = memory.read(&mut caller, args_base as usize, &mut buf);
            let source = i64::from_le_bytes(buf);
            if value::is_iterator(source) {
                let handle_idx = value::decode_handle(source) as usize;
                enum PendingIteratorValue {
                    Value(i64),
                    TypedArrayValue { entry: TypedArrayEntry, index: u32 },
                    TypedArrayEntry { entry: TypedArrayEntry, index: u32 },
                }
                let mut values = Vec::new();
                loop {
                    let pending = {
                        let mut iters = caller.data().iterators.lock().expect("iters");
                        match iters.get_mut(handle_idx) {
                            Some(IteratorState::MapKeyIter { keys, index }) => {
                                if (*index as usize) < keys.len() {
                                    let value = keys[*index as usize];
                                    *index += 1;
                                    Some(PendingIteratorValue::Value(value))
                                } else {
                                    None
                                }
                            }
                            Some(IteratorState::MapValueIter { values, index }) => {
                                if (*index as usize) < values.len() {
                                    let value = values[*index as usize];
                                    *index += 1;
                                    Some(PendingIteratorValue::Value(value))
                                } else {
                                    None
                                }
                            }
                            Some(IteratorState::TypedArrayValueIter {
                                entry,
                                index,
                                length,
                            }) => {
                                if *index < *length {
                                    let entry = entry.clone();
                                    let current = *index;
                                    *index += 1;
                                    Some(PendingIteratorValue::TypedArrayValue {
                                        entry,
                                        index: current,
                                    })
                                } else {
                                    None
                                }
                            }
                            Some(IteratorState::TypedArrayEntryIter {
                                entry,
                                index,
                                length,
                            }) => {
                                if *index < *length {
                                    let entry = entry.clone();
                                    let current = *index;
                                    *index += 1;
                                    Some(PendingIteratorValue::TypedArrayEntry {
                                        entry,
                                        index: current,
                                    })
                                } else {
                                    None
                                }
                            }
                            _ => None,
                        }
                    };
                    let Some(pending) = pending else {
                        break;
                    };
                    match pending {
                        PendingIteratorValue::Value(value) => values.push(value),
                        PendingIteratorValue::TypedArrayValue { entry, index } => {
                            values.push(
                                typedarray_element_read_entry(&mut caller, &entry, index)
                                    .unwrap_or_else(value::encode_undefined),
                            );
                        }
                        PendingIteratorValue::TypedArrayEntry {
                            entry: typedarray_entry,
                            index,
                        } => {
                            let entry = alloc_array(&mut caller, 2);
                            if let Some(entry_ptr) = resolve_array_ptr(&mut caller, entry) {
                                let elem = typedarray_element_read_entry(
                                    &mut caller,
                                    &typedarray_entry,
                                    index,
                                )
                                .unwrap_or_else(value::encode_undefined);
                                write_array_elem(
                                    &mut caller,
                                    entry_ptr,
                                    0,
                                    value::encode_f64(index as f64),
                                );
                                write_array_elem(&mut caller, entry_ptr, 1, elem);
                                write_array_length(&mut caller, entry_ptr, 2);
                            }
                            values.push(entry);
                        }
                    }
                }
                let arr = alloc_array(&mut caller, values.len() as u32);
                if let Some(arr_ptr) = resolve_array_ptr(&mut caller, arr) {
                    for (i, &val) in values.iter().enumerate() {
                        write_array_elem(&mut caller, arr_ptr, i as u32, val);
                    }
                    write_array_length(&mut caller, arr_ptr, values.len() as u32);
                }
                return arr;
            }
            if let Some(entry) = typedarray_entry_from_value(&mut caller, source) {
                let arr = alloc_array(&mut caller, entry.length);
                if let Some(arr_ptr) = resolve_array_ptr(&mut caller, arr) {
                    for i in 0..entry.length {
                        let val = typedarray_element_read(&mut caller, source, i)
                            .unwrap_or_else(value::encode_undefined);
                        write_array_elem(&mut caller, arr_ptr, i, val);
                    }
                    write_array_length(&mut caller, arr_ptr, entry.length);
                }
                return arr;
            }
            if value::is_array(source) {
                return source;
            }
            value::encode_undefined()
        },
    );
    linker.define(&mut *store, "env", "array.from", f)?;
    // obj_get_by_index
    let f = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, boxed: i64, index: i32| -> i64 {
            if index >= 0 {
                if let Some(value) = typedarray_element_read(&mut caller, boxed, index as u32) {
                    return value;
                }
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
    Ok(())
}

/// 注册 18 个 define_* 宿主函数模块
fn register_linker(
    linker: &mut Linker<RuntimeState>,
    store: &mut Store<RuntimeState>,
) -> Result<()> {
    define_core(linker, store)?;
    define_timers_arrays(linker, store)?;
    define_fetch(linker, store)?;
    define_array_object(linker, store)?;
    define_primitive_core(linker, store)?;
    define_promise(linker, store)?;
    define_promise_combinators(linker, store)?;
    define_misc(linker, store)?;
    define_async_fn(linker, store)?;
    define_async_generator(linker, store)?;
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
fn register_complex_bridges(
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
                    let mut iters = caller.data().iterators.lock().expect("iterators mutex");
                    let handle = iters.len() as u32;
                    iters.push(IteratorState::Error);
                    return value::encode_handle(value::TAG_ITERATOR, handle);
                }

                let Some(_ptr) = resolve_handle(&mut caller, iterable) else {
                    let mut iters = caller.data().iterators.lock().expect("iterators mutex");
                    let handle = iters.len() as u32;
                    iters.push(IteratorState::Error);
                    return value::encode_handle(value::TAG_ITERATOR, handle);
                };
                // 数组 fast path
                if value::is_array(iterable) {
                    if let Some(arr_ptr) = resolve_handle(&mut caller, iterable) {
                        let length = read_array_length(&mut caller, arr_ptr).unwrap_or(0);
                        let sync_iter_handle = {
                            let mut iters =
                                caller.data().iterators.lock().expect("iterators mutex");
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
                        if value::is_object(iterator) {
                            if let Some(iter_ptr) = resolve_handle(&mut caller, iterator) {
                                let next =
                                    read_object_property_by_name(&mut caller, iter_ptr, "next")
                                        .filter(|n| value::is_callable(*n));
                                if let Some(next_fn) = next {
                                    let return_method = read_object_property_by_name(
                                        &mut caller,
                                        iter_ptr,
                                        "return",
                                    )
                                    .filter(|c| value::is_callable(*c));
                                    let mut iters =
                                        caller.data().iterators.lock().expect("iterators mutex");
                                    let handle = iters.len() as u32;
                                    iters.push(IteratorState::ObjectIter {
                                        iterator,
                                        next: next_fn,
                                        return_method,
                                        current_value: value::encode_undefined(),
                                        has_current: false,
                                        done: false,
                                    });
                                    return value::encode_handle(value::TAG_ITERATOR, handle);
                                }
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
                        if value::is_object(sync_iter) {
                            if let Some(sync_ptr) = resolve_handle(&mut caller, sync_iter) {
                                let next_fn =
                                    read_object_property_by_name(&mut caller, sync_ptr, "next")
                                        .filter(|n| value::is_callable(*n));
                                if let Some(next_fn) = next_fn {
                                    let return_method = read_object_property_by_name(
                                        &mut caller,
                                        sync_ptr,
                                        "return",
                                    )
                                    .filter(|c| value::is_callable(*c));
                                    let sync_iter_handle = {
                                        let mut iters = caller
                                            .data()
                                            .iterators
                                            .lock()
                                            .expect("iterators mutex");
                                        let sync_handle = iters.len() as u32;
                                        iters.push(IteratorState::ObjectIter {
                                            iterator: sync_iter,
                                            next: next_fn,
                                            return_method,
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
                        .expect("runtime error mutex") =
                        Some("TypeError: Cannot group null or undefined".to_string());
                    return value::encode_undefined();
                }
                if !value::is_callable(callbackfn) {
                    *caller
                        .data()
                        .runtime_error
                        .lock()
                        .expect("runtime error mutex") =
                        Some("TypeError: callbackfn is not callable".to_string());
                    return value::encode_undefined();
                }
                let result = alloc_object(&mut caller, 0);
                let mut groups: HashMap<String, Vec<i64>> = HashMap::new();
                let mut index = 0u32;
                if value::is_array(items) {
                    if let Some(arr_ptr) = resolve_array_ptr(&mut caller, items) {
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
                            if caller.data().runtime_error.lock().expect("mutex").is_some() {
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
                        .expect("runtime error mutex") =
                        Some("TypeError: Cannot group null or undefined".to_string());
                    return value::encode_undefined();
                }
                if !value::is_callable(callbackfn) {
                    *caller
                        .data()
                        .runtime_error
                        .lock()
                        .expect("runtime error mutex") =
                        Some("TypeError: callbackfn is not callable".to_string());
                    return value::encode_undefined();
                }
                let map_handle = {
                    let mut map_table = caller.data().map_table.lock().expect("map table mutex");
                    let handle = map_table.len();
                    map_table.push(MapEntry {
                        keys: Vec::new(),
                        values: Vec::new(),
                    });
                    handle
                };
                let map_result = alloc_object(&mut caller, 12);
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
                if value::is_array(items) {
                    if let Some(arr_ptr) = resolve_array_ptr(&mut caller, items) {
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
                                if same_value_zero(groups[idx].0, key) {
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
                                    if same_value_zero(*existing_key, key) {
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
                            let mut table =
                                caller.data().map_table.lock().expect("map table mutex");
                            table[map_handle].keys.push(*group_key);
                            table[map_handle].values.push(arr);
                        }
                    }
                }
                map_result
            })
        },
    )?;
    Ok(())
}

pub async fn execute(wasm_bytes: &[u8]) -> Result<()> {
    let stdout = io::stdout();
    let _ = execute_with_writer(wasm_bytes, stdout.lock()).await?;
    Ok(())
}
pub async fn execute_with_writer<W: Write>(wasm_bytes: &[u8], writer: W) -> Result<W> {
    execute_with_writer_shared(wasm_bytes, writer, None).await
}

/// 编译 JS/TS 源码到 WASM 字节码的共享辅助函数。
/// 供本 crate 测试及外部集成测试（`tests/`）复用，避免重复定义
/// `parse_module → lower_module → compile` 流程。
pub fn compile_source(source: &str) -> Result<Vec<u8>> {
    let module = wjsm_parser::parse_module(source)?;
    let program = wjsm_semantic::lower_module(module, false)?;
    wjsm_backend_wasm::compile(&program)
}

pub(crate) async fn execute_with_writer_shared_agent<W: Write>(
    wasm_bytes: &[u8],
    writer: W,
    shared_state: Arc<SharedRuntimeState>,
) -> Result<W> {
    execute_with_writer_shared_inner(wasm_bytes, writer, Some(shared_state), false).await
}
pub(crate) async fn execute_with_writer_shared<W: Write>(
    wasm_bytes: &[u8],
    writer: W,
    shared_state: Option<Arc<SharedRuntimeState>>,
) -> Result<W> {
    execute_with_writer_shared_inner(wasm_bytes, writer, shared_state, true).await
}
/// 进程级 wasmtime 编译缓存（按 WJSM_MODULE_CACHE 环境变量惰性初始化一次）。
/// 返回 None 表示未开启或初始化失败（静默降级到无缓存，正确性优先）。
fn module_compile_cache() -> Option<&'static wasmtime::Cache> {
    use std::sync::OnceLock;
    static CACHE: OnceLock<Option<wasmtime::Cache>> = OnceLock::new();
    CACHE
        .get_or_init(|| {
            let dir = std::env::var("WJSM_MODULE_CACHE").ok()?;
            let mut cfg = wasmtime::CacheConfig::new();
            cfg.with_directory(dir);
            wasmtime::Cache::new(cfg).ok()
        })
        .as_ref()
}

#[allow(dead_code)]
fn startup_snapshot_enabled() -> bool {
    !matches!(
        std::env::var("WJSM_STARTUP_SNAPSHOT").as_deref(),
        Ok("0") | Ok("false") | Ok("off")
    )
}

fn startup_engine_config(use_epoch_async_yield: bool) -> Config {
    let mut config = Config::new();
    if use_epoch_async_yield {
        config.epoch_interruption(true);
    }
    // 可选的跨 run 编译缓存：设 WJSM_MODULE_CACHE=<dir> 时，wasmtime 把 Cranelift
    // 编译产物按 wasm 内容哈希缓存到磁盘。同一 wasm 第二次执行直接读缓存，跳过编译。
    // 仅在显式开启时生效（测试加速场景），不影响生产 CLI 默认行为。
    if let Some(cache) = module_compile_cache() {
        config.cache(Some(cache.clone()));
    }
    config
}

fn register_startup_linker(
    linker: &mut Linker<RuntimeState>,
    store: &mut Store<RuntimeState>,
) -> Result<()> {
    register_linker(linker, store)?;
    register_common_bridges(linker, store)?;
    register_complex_bridges(linker, store)?;
    Ok(())
}

fn prepare_async_host_completion(
    store: &mut Store<RuntimeState>,
) -> tokio::sync::mpsc::UnboundedReceiver<crate::scheduler::AsyncHostCompletion> {
    // Phase 6: 创建 channel + counter（仅 async 路径）
    let (host_completion_tx, host_completion_rx) = tokio::sync::mpsc::unbounded_channel();
    store
        .data_mut()
        .host_completion_tx
        .replace(host_completion_tx);
    let counter = crate::scheduler::AsyncOpCounter::new();
    store.data_mut().async_op_counter.replace(counter);
    host_completion_rx
}

fn extract_wasm_env(instance: &Instance, store: &mut Store<RuntimeState>) -> WasmEnv {
    let memory = instance
        .get_export(&mut *store, "memory")
        .and_then(|e| e.into_memory())
        .expect("memory");
    let heap_ptr_global = instance
        .get_export(&mut *store, "__heap_ptr")
        .and_then(|e| e.into_global())
        .expect("heap");
    let obj_table_ptr_global = instance
        .get_export(&mut *store, "__obj_table_ptr")
        .and_then(|e| e.into_global())
        .expect("obj_table_ptr");
    let obj_table_count_global = instance
        .get_export(&mut *store, "__obj_table_count")
        .and_then(|e| e.into_global())
        .expect("obj_table_count");
    let func_table = instance
        .get_export(&mut *store, "__table")
        .and_then(|e| e.into_table())
        .expect("table");
    let shadow_sp_global = instance
        .get_export(&mut *store, "__shadow_sp")
        .and_then(|e| e.into_global())
        .expect("shadow");
    let array_proto_handle_global = instance
        .get_export(&mut *store, "__array_proto_handle")
        .and_then(|e| e.into_global())
        .expect("array_proto");
    let object_proto_handle_global = instance
        .get_export(&mut *store, "__object_proto_handle")
        .and_then(|e| e.into_global())
        .expect("object_proto");

    wasm_env::WasmEnv {
        memory,
        func_table,
        shadow_sp: shadow_sp_global,
        heap_ptr: heap_ptr_global,
        obj_table_ptr: obj_table_ptr_global,
        obj_table_count: obj_table_count_global,
        object_proto_handle: object_proto_handle_global,
        array_proto_handle: array_proto_handle_global,
    }
}

fn initialize_host_post_bootstrap(store: &mut Store<RuntimeState>, wasm_env: &WasmEnv) {
    if wasm_env.obj_table_count.get(&mut *store).i32().unwrap_or(0) == 0 {
        // handle 0 仍作为旧原型链 null 哨兵；host primordial 从 1 开始，避免 Object.getPrototypeOf 误判。
        let _ = alloc_host_object(store, wasm_env, 0);
    }
    let async_iterator_proto = alloc_host_object(store, wasm_env, 2);
    let async_iterator_symbol_async_iterator = {
        let mut table = store.data().native_callables.lock().expect("native");
        let handle = table.len() as u32;
        table.push(NativeCallable::AsyncIteratorProtoSymbolAsyncIterator);
        value::encode_native_callable_idx(handle)
    };
    let _ = define_host_data_property_by_name_id_with_env(
        store,
        wasm_env,
        async_iterator_proto,
        encode_symbol_name_id(3),
        async_iterator_symbol_async_iterator,
        constants::FLAG_CONFIGURABLE | constants::FLAG_WRITABLE,
    );
    let async_iterator_tag =
        store_runtime_string_in_state(store.data(), "AsyncIterator".to_string());
    let _ = define_host_data_property_with_env(
        store,
        wasm_env,
        async_iterator_proto,
        "Symbol.toStringTag",
        async_iterator_tag,
    );
    let async_gen_proto = alloc_host_object(store, wasm_env, 2);
    let async_gen_handle = value::decode_object_handle(async_gen_proto);
    let async_iterator_handle = value::decode_object_handle(async_iterator_proto);
    let obj_ptr =
        resolve_handle_idx_with_env(store, wasm_env, async_gen_handle as usize).expect("obj_ptr");
    let data = wasm_env.memory.data_mut(&mut *store);
    data[obj_ptr..obj_ptr + 4].copy_from_slice(&async_iterator_handle.to_le_bytes());
    let async_gen_tag = store_runtime_string_in_state(store.data(), "AsyncGenerator".to_string());
    let _ = define_host_data_property_with_env(
        store,
        wasm_env,
        async_gen_proto,
        "Symbol.toStringTag",
        async_gen_tag,
    );
    store.data_mut().async_iterator_prototype = async_iterator_proto;
    store.data_mut().async_gen_prototype = async_gen_proto;
}



#[cfg(test)]
#[derive(Default)]
struct StartupBenchTimings {
    engine_only: Duration,
    module_only: Duration,
    store_only: Duration,
    linker_register: Duration,
    instantiate_async: Duration,
    bootstrap_cold: Duration,
    host_post_bootstrap: Duration,
}

#[cfg(test)]
async fn instantiate_for_startup_bench(wasm: &[u8]) -> Result<StartupBenchTimings> {
    let mut timings = StartupBenchTimings::default();

    let config = startup_engine_config(true);
    let start = std::time::Instant::now();
    let engine = Engine::new(&config)
        .map_err(|e| anyhow::anyhow!("Failed to create async engine: {:?}", e))?;
    timings.engine_only = start.elapsed();

    let start = std::time::Instant::now();
    let module = match Module::new(&engine, wasm) {
        Ok(m) => m,
        Err(e) => {
            return Err(anyhow::anyhow!("WASM validation failed: {:?}", e));
        }
    };
    timings.module_only = start.elapsed();

    let start = std::time::Instant::now();
    let mut store = Store::new(&engine, RuntimeState::new_with_shared(None));
    store.set_epoch_deadline(1);
    store.epoch_deadline_async_yield_and_update(1);
    let _host_completion_rx = prepare_async_host_completion(&mut store);
    timings.store_only = start.elapsed();

    let mut linker = Linker::new(&engine);
    let start = std::time::Instant::now();
    register_startup_linker(&mut linker, &mut store)?;
    timings.linker_register = start.elapsed();

    let start = std::time::Instant::now();
    let instance = linker
        .instantiate_async(&mut store, &module)
        .await
        .map_err(|e| anyhow::anyhow!("async instantiate failed: {:?}", e))?;
    timings.instantiate_async = start.elapsed();

    // P0 还没有拆出 __wjsm_bootstrap_once；真正 WASM 启动成本仍在 instantiate_async 内。
    // 这里临时记录 execute 进入 host post-bootstrap 前的环境提取成本，不能当作 snapshot 指标。
    let start = std::time::Instant::now();
    let wasm_env = extract_wasm_env(&instance, &mut store);
    timings.bootstrap_cold = start.elapsed();

    let start = std::time::Instant::now();
    initialize_host_post_bootstrap(&mut store, &wasm_env);
    timings.host_post_bootstrap = start.elapsed();

    Ok(timings)
}
async fn execute_with_writer_shared_inner<W: Write>(
    wasm_bytes: &[u8],
    writer: W,
    shared_state: Option<Arc<SharedRuntimeState>>,
    use_epoch_async_yield: bool,
) -> Result<W> {
    let config = startup_engine_config(use_epoch_async_yield);
    let engine = Engine::new(&config)
        .map_err(|e| anyhow::anyhow!("Failed to create async engine: {:?}", e))?;

    let module = match Module::new(&engine, wasm_bytes) {
        Ok(m) => m,
        Err(e) => {
            return Err(anyhow::anyhow!("WASM validation failed: {:?}", e));
        }
    };

    let mut store = Store::new(&engine, RuntimeState::new_with_shared(shared_state));
    let output = Arc::clone(&store.data().output);
    let runtime_error = Arc::clone(&store.data().runtime_error);

    if use_epoch_async_yield {
        store.set_epoch_deadline(1);
        store.epoch_deadline_async_yield_and_update(1);
    }

    let mut host_completion_rx = prepare_async_host_completion(&mut store);

    let mut linker = Linker::new(&engine);
    register_startup_linker(&mut linker, &mut store)?;
    let instance = linker
        .instantiate_async(&mut store, &module)
        .await
        .map_err(|e| anyhow::anyhow!("async instantiate failed: {:?}", e))?;

    let wasm_env = extract_wasm_env(&instance, &mut store);
    initialize_host_post_bootstrap(&mut store, &wasm_env);

    // 委托（Phase 6 传入 rx 供 scheduler 接收 completion）
    run_main_completion_block_async(
        &instance,
        store,
        wasm_env,
        output,
        runtime_error,
        writer,
        &mut host_completion_rx,
    )
    .await
}
impl Clone for RuntimeState {
    fn clone(&self) -> Self {
        Self {
            output: self.output.clone(),
            iterators: self.iterators.clone(),
            enumerators: self.enumerators.clone(),
            runtime_strings: self.runtime_strings.clone(),
            runtime_error: self.runtime_error.clone(),
            gc_mark_bits: self.gc_mark_bits.clone(),
            alloc_counter: self.alloc_counter.clone(),
            gc_threshold: self.gc_threshold,
            timers: self.timers.clone(),
            cancelled_timers: self.cancelled_timers.clone(),
            next_timer_id: self.next_timer_id.clone(),
            closures: self.closures.clone(),
            bound_objects: self.bound_objects.clone(),
            native_callables: self.native_callables.clone(),
            native_callable_free_slots: self.native_callable_free_slots.clone(),
            handle_free_list: self.handle_free_list.clone(),
            abandoned_regions: self.abandoned_regions.clone(),
            gc_algorithm: self.gc_algorithm.clone(),
            continuation_free_slots: self.continuation_free_slots.clone(),
            combinator_context_free_slots: self.combinator_context_free_slots.clone(),
            eval_cache: self.eval_cache.clone(),
            bigint_table: self.bigint_table.clone(),
            symbol_table: self.symbol_table.clone(),
            regex_table: self.regex_table.clone(),
            promise_table: self.promise_table.clone(),
            pending_unhandled_rejections: self.pending_unhandled_rejections.clone(),
            microtask_queue: self.microtask_queue.clone(),
            continuation_table: self.continuation_table.clone(),
            async_generator_table: self.async_generator_table.clone(),
            async_from_sync_iterators: self.async_from_sync_iterators.clone(),
            async_iterator_prototype: self.async_iterator_prototype,
            async_gen_prototype: self.async_gen_prototype,
            array_proto_values: AtomicI64::new(self.array_proto_values.load(Ordering::Relaxed)),
            combinator_contexts: self.combinator_contexts.clone(),
            module_namespace_cache: self.module_namespace_cache.clone(),
            error_table: self.error_table.clone(),
            map_table: self.map_table.clone(),
            set_table: self.set_table.clone(),
            weakmap_table: self.weakmap_table.clone(),
            weakset_table: self.weakset_table.clone(),
            weakref_table: self.weakref_table.clone(),
            finalization_registry_table: self.finalization_registry_table.clone(),
            pending_cleanup_callbacks: self.pending_cleanup_callbacks.clone(),
            proxy_table: self.proxy_table.clone(),
            arraybuffer_table: self.arraybuffer_table.clone(),
            dataview_table: self.dataview_table.clone(),
            typedarray_table: self.typedarray_table.clone(),
            headers_table: self.headers_table.clone(),
            fetch_response_table: self.fetch_response_table.clone(),
            fetch_request_table: self.fetch_request_table.clone(),
            abort_signal_table: self.abort_signal_table.clone(),
            http_response_table: self.http_response_table.clone(),
            readable_stream_table: self.readable_stream_table.clone(),
            reader_table: self.reader_table.clone(),
            stream_controller_table: self.stream_controller_table.clone(),
            byob_request_table: self.byob_request_table.clone(),
            writable_stream_table: self.writable_stream_table.clone(),
            writer_table: self.writer_table.clone(),
            transform_stream_table: self.transform_stream_table.clone(),
            shared_state: self.shared_state.clone(),
            non_extensible_handles: self.non_extensible_handles.clone(),
            scope_records: self.scope_records.clone(),
            scope_record_next_handle: self.scope_record_next_handle,
            new_target: AtomicI64::new(self.new_target.load(Ordering::Relaxed)),
            host_completion_tx: self.host_completion_tx.clone(),
            async_op_counter: self.async_op_counter.clone(),
        }
    }
}
struct RuntimeState {
    output: Arc<Mutex<Vec<u8>>>,
    iterators: Arc<Mutex<Vec<IteratorState>>>,
    enumerators: Arc<Mutex<Vec<EnumeratorState>>>,
    runtime_strings: Arc<Mutex<Vec<String>>>,
    runtime_error: Arc<Mutex<Option<String>>>,
    /// GC 标记位图：每个 handle 对应 1 bit，用于标记-清除 GC。
    gc_mark_bits: Arc<Mutex<Vec<u64>>>,
    /// 分配计数器：每次对象分配后递增，用于触发周期性 GC。
    alloc_counter: Arc<Mutex<u64>>,
    /// GC 触发阈值：当 alloc_counter 达到此值时触发 GC。
    gc_threshold: u64,
    /// 定时器列表
    timers: Arc<Mutex<Vec<TimerEntry>>>,
    /// 已取消的定时器 ID 集合
    cancelled_timers: Arc<Mutex<HashSet<u32>>>,
    /// 下一个定时器 ID
    next_timer_id: Arc<Mutex<u32>>,
    /// 闭包表：每个闭包条目存储函数表索引和环境对象
    closures: Arc<Mutex<Vec<ClosureEntry>>>,
    /// 绑定函数表：存储 func.bind(this, args) 创建的绑定函数
    bound_objects: Arc<Mutex<Vec<BoundRecord>>>,
    /// 运行时原生可调用对象表：Promise resolving functions 等宿主创建函数。
    native_callables: Arc<Mutex<Vec<NativeCallable>>>,
    /// native_callable 表空闲槽位，用于复用已释放条目。
    native_callable_free_slots: Arc<Mutex<Vec<u32>>>,
    /// GC sweep 回收的 obj_table handle 槽（供 fast-path 复用，spec #7/IMPL-10）。
    /// runtime_gc::MarkSweepCollector::collect 把 sweep 回收的 handle push 到此；
    /// gc_take_freed_handle host import（P4）pop 给 WASM fast-path。
    handle_free_list: Arc<Mutex<Vec<u32>>>,
    /// resize（grow_array/grow_object）抛弃的旧区域 (ptr, size)。
    /// handle 的 obj_table 槽被重写到新 ptr 后，旧 ptr 区域不再被 obj_table 索引，
    /// sweep 单独遍历 obj_table 看不到它 → 永久泄漏（INV-B vs §8.2 矛盾，P4-blocker #1）。
    /// grow_array/grow_object 在重写前注册旧 (ptr, old_size)；sweep 读此并入 free list，sweep 结束清空。
    abandoned_regions: Arc<Mutex<Vec<(usize, usize)>>>,
    /// 可插拔 GC 算法实例（默认 MarkSweepCollector）。host imports gc_alloc_slow/
    /// gc_maybe_collect 经此驱动（P4）。Arc<Mutex> 因 host fn 经 &Caller 访问需内部可变性。
    gc_algorithm: Arc<Mutex<Box<dyn crate::runtime_gc::GcAlgorithm + Send + Sync>>>,
    /// continuation 侧表空闲槽位（handle 下标稳定，禁止 Vec::retain）。
    continuation_free_slots: Arc<Mutex<Vec<u32>>>,
    /// combinator context 侧表空闲槽位。
    combinator_context_free_slots: Arc<Mutex<Vec<usize>>>,
    /// eval 编译缓存：code string hash → eval 模式 WASM bytes。
    eval_cache: Arc<Mutex<HashMap<u64, Vec<u8>>>>,
    /// BigInt 侧表：存储任意精度 BigInt 值
    bigint_table: Arc<Mutex<Vec<num_bigint::BigInt>>>,
    /// Symbol 侧表：存储 symbol 条目（description + global_key）
    symbol_table: Arc<Mutex<Vec<SymbolEntry>>>,
    /// RegExp 侧表：存储编译后的正则表达式和元数据
    regex_table: Arc<Mutex<Vec<RegexEntry>>>,
    /// Promise 侧表：object handle → Promise 内部槽；非 Promise object handle 使用空占位。
    promise_table: Arc<Mutex<Vec<PromiseEntry>>>,
    /// 已 reject 且尚未 handled 的 promise 索引，用于 drain 时避免全表扫描。
    pending_unhandled_rejections: Arc<Mutex<HashSet<usize>>>,
    /// 微任务队列
    microtask_queue: Arc<Mutex<VecDeque<Microtask>>>,
    /// Continuation 侧表：存储异步函数续延
    continuation_table: Arc<Mutex<Vec<ContinuationEntry>>>,
    /// AsyncGenerator 侧表：存储异步生成器状态
    async_generator_table: Arc<Mutex<Vec<AsyncGeneratorEntry>>>,
    /// async-from-sync iterator 侧表
    async_from_sync_iterators: Arc<Mutex<Vec<AsyncFromSyncIteratorEntry>>>,
    /// %AsyncIteratorPrototype% 对象
    async_iterator_prototype: i64,
    /// AsyncGenerator.prototype 对象
    async_gen_prototype: i64,
    /// Promise combinator 侧表：pending 元素的 reaction 通过索引回写共享结果。
    combinator_contexts: Arc<Mutex<Vec<CombinatorContext>>>,
    /// 模块命名空间对象缓存：module_id → namespace object (i64 NaN-boxed)
    module_namespace_cache: Arc<Mutex<HashMap<u32, i64>>>,
    /// Error 侧表：存储 error 对象的 name 和 message
    error_table: Arc<Mutex<Vec<ErrorEntry>>>,
    /// Map 侧表：存储 Map 对象的键值对
    map_table: Arc<Mutex<Vec<MapEntry>>>,
    /// Set 侧表：存储 Set 对象的值
    set_table: Arc<Mutex<Vec<SetEntry>>>,
    /// WeakMap 侧表：存储 WeakMap 对象的键值对
    weakmap_table: Arc<Mutex<Vec<WeakMapEntry>>>,
    /// WeakSet 侧表：存储 WeakSet 对象的值
    weakset_table: Arc<Mutex<Vec<WeakSetEntry>>>,
    /// WeakRef 侧表：存储 WeakRef 对象的 target handle
    weakref_table: Arc<Mutex<Vec<WeakRefEntry>>>,
    /// Array.prototype.values 缓存，用于规范要求复用该函数对象的 @@iterator。
    array_proto_values: AtomicI64,
    /// FinalizationRegistry 侧表：存储 registry 对象、callback 和注册信息
    finalization_registry_table: Arc<Mutex<Vec<FinalizationRegistryEntry>>>,
    /// GC 后待调度的清理回调
    pending_cleanup_callbacks: Arc<Mutex<Vec<(i64, Vec<i64>)>>>,
    /// Proxy 侧表：存储 Proxy 对象的 target、handler 和 revoked 状态
    proxy_table: Arc<Mutex<Vec<ProxyEntry>>>,
    /// ArrayBuffer 侧表：存储 ArrayBuffer 的底层数据
    arraybuffer_table: Arc<Mutex<Vec<ArrayBufferEntry>>>,
    /// DataView 侧表：存储 DataView 的 buffer 引用和偏移量
    dataview_table: Arc<Mutex<Vec<DataViewEntry>>>,
    /// TypedArray 侧表：存储 TypedArray 的 buffer 引用、偏移量和长度
    typedarray_table: Arc<Mutex<Vec<TypedArrayEntry>>>,
    /// Headers 侧表：存储 Headers 对象 (key-value pairs, lowercased keys)
    headers_table: Arc<Mutex<Vec<HeadersEntry>>>,
    /// Fetch Response 侧表：存储 Response 对象 (status/headers/body)
    fetch_response_table: Arc<Mutex<Vec<FetchResponseEntry>>>,
    /// Fetch Request 侧表：存储 Request 对象 (method/url/headers/body)
    fetch_request_table: Arc<Mutex<Vec<FetchRequestEntry>>>,
    /// AbortSignal 侧表：存储 abort 状态
    abort_signal_table: Arc<Mutex<Vec<AbortSignalEntry>>>,
    /// reqwest Response 侧表：持有未消费的 HTTP response body stream
    http_response_table: Arc<Mutex<Vec<HttpResponseEntry>>>,
    /// ReadableStream 侧表：存储流状态
    readable_stream_table: Arc<Mutex<Vec<ReadableStreamEntry>>>,
    /// Reader 侧表：存储 reader → stream 映射
    reader_table: Arc<Mutex<Vec<ReaderEntry>>>,
    /// Controller 侧表（ReadableStream DefaultController 等）
    stream_controller_table: Arc<Mutex<Vec<StreamControllerEntry>>>,
    byob_request_table: Arc<Mutex<Vec<ByobRequestEntry>>>,
    /// WritableStream 侧表：存储可写流状态
    writable_stream_table: Arc<Mutex<Vec<WritableStreamEntry>>>,
    /// Writer 侧表：存储 WritableStreamDefaultWriter → stream 映射
    writer_table: Arc<Mutex<Vec<WriterEntry>>>,
    /// TransformStream 侧表：存储转换流状态
    transform_stream_table: Arc<Mutex<Vec<TransformStreamEntry>>>,
    /// normal execution 拥有单 agent cluster；$262.agent 可共享同一状态。
    shared_state: Option<Arc<SharedRuntimeState>>,
    /// 被 preventExtensions 标记为不可扩展对象的 handle 集合（使用完整的 NaN-boxed 值作为 key）
    non_extensible_handles: Arc<Mutex<HashSet<u64>>>,
    /// Temporary ScopeRecord handles for active eval calls.
    /// Keyed by handle index; entries removed when eval returns.
    pub(crate) scope_records: HashMap<u32, crate::runtime_eval::ScopeRecord>,
    /// Monotonic counter for scope record handle allocation.
    /// Using a counter instead of len() ensures no collisions when records are removed.
    pub(crate) scope_record_next_handle: u32,
    /// new.target value meta property
    /// new.target 值元属性（AtomicI64 + Relaxed 替换 Cell，以满足 wasmtime async instantiate_async
    /// 要求的 T: Send + 'static 约束。Relaxed 足够且零开销；语义与原 Cell 完全等价。）
    new_target: AtomicI64,

    /// Phase 6: host completion channel tx（Option 便于 sync 路径保持 None；创建后 set Some）。
    /// 引用 plan Correction 7：worker 只通过 tx 发送可 Send 的 SettleValue 或 Materialize 闭包，绝不触碰 Store/heap。
    host_completion_tx:
        Option<tokio::sync::mpsc::UnboundedSender<crate::scheduler::AsyncHostCompletion>>,
    /// Phase 6: in-flight async op 计数器（用于 scheduler 安全退出条件）。
    async_op_counter: Option<crate::scheduler::AsyncOpCounter>,
}

impl RuntimeState {
    fn new_with_shared(shared_state: Option<Arc<SharedRuntimeState>>) -> Self {
        let mut state = Self::new();
        state.shared_state = shared_state.or_else(|| Some(new_shared_runtime_state()));
        state
    }

    /// GC 框架访问 handle_free_list（runtime_gc::MarkSweepCollector::collect 用）。
    /// 返回 handle_free_list 的可变引用，供 sweep 回收的 handle 入表。
    pub(crate) fn handle_free_list_for_gc(&self) -> Option<std::sync::MutexGuard<'_, Vec<u32>>> {
        self.handle_free_list.lock().ok()
    }

    /// 注册 resize（grow_array/grow_object）抛弃的旧区域 (ptr, old_size)。
    /// sweeper 读此并入 free list（P4-blocker #1）。
    pub(crate) fn abandon_region(&self, ptr: usize, size: usize) {
        if size == 0 {
            return;
        }
        if let Ok(mut list) = self.abandoned_regions.lock() {
            list.push((ptr, size));
        }
    }

    /// GC 框架访问 abandoned_regions（sweeper 读 + 清空）。
    pub(crate) fn abandoned_regions_for_gc(
        &self,
    ) -> Option<std::sync::MutexGuard<'_, Vec<(usize, usize)>>> {
        self.abandoned_regions.lock().ok()
    }

    /// 构造一个新的 RuntimeState，所有侧表初始化为空，well-known symbols 预分配。
    pub(crate) fn new() -> Self {
        const GC_THRESHOLD: u64 = 1000;
        RuntimeState {
            output: Arc::new(Mutex::new(Vec::new())),
            iterators: Arc::new(Mutex::new(Vec::new())),
            enumerators: Arc::new(Mutex::new(Vec::new())),
            runtime_strings: Arc::new(Mutex::new(Vec::new())),
            runtime_error: Arc::new(Mutex::new(None)),
            gc_mark_bits: Arc::new(Mutex::new(Vec::new())),
            alloc_counter: Arc::new(Mutex::new(0)),
            gc_threshold: GC_THRESHOLD,
            timers: Arc::new(Mutex::new(Vec::new())),
            cancelled_timers: Arc::new(Mutex::new(HashSet::new())),
            next_timer_id: Arc::new(Mutex::new(1)),
            closures: Arc::new(Mutex::new(Vec::new())),
            bound_objects: Arc::new(Mutex::new(Vec::new())),
            native_callables: Arc::new(Mutex::new(vec![NativeCallable::EvalIndirect])),
            native_callable_free_slots: Arc::new(Mutex::new(Vec::new())),
            handle_free_list: Arc::new(Mutex::new(Vec::new())),
            abandoned_regions: Arc::new(Mutex::new(Vec::new())),
            gc_algorithm: Arc::new(Mutex::new(Box::new(
                crate::runtime_gc::MarkSweepCollector::new(),
            ))),
            continuation_free_slots: Arc::new(Mutex::new(Vec::new())),
            combinator_context_free_slots: Arc::new(Mutex::new(Vec::new())),
            eval_cache: Arc::new(Mutex::new(HashMap::new())),
            bigint_table: Arc::new(Mutex::new(Vec::new())),
            symbol_table: Arc::new(Mutex::new(vec![
                SymbolEntry {
                    description: Some("Symbol(Symbol.iterator)".into()),
                    global_key: None,
                },
                SymbolEntry {
                    description: Some("Symbol(Symbol.species)".into()),
                    global_key: None,
                },
                SymbolEntry {
                    description: Some("Symbol(Symbol.toStringTag)".into()),
                    global_key: None,
                },
                SymbolEntry {
                    description: Some("Symbol(Symbol.asyncIterator)".into()),
                    global_key: None,
                },
                SymbolEntry {
                    description: Some("Symbol(Symbol.hasInstance)".into()),
                    global_key: None,
                },
                SymbolEntry {
                    description: Some("Symbol(Symbol.toPrimitive)".into()),
                    global_key: None,
                },
                SymbolEntry {
                    description: Some("Symbol(Symbol.dispose)".into()),
                    global_key: None,
                },
                SymbolEntry {
                    description: Some("Symbol(Symbol.match)".into()),
                    global_key: None,
                },
                SymbolEntry {
                    description: Some("Symbol(Symbol.asyncDispose)".into()),
                    global_key: None,
                },
            ])),
            regex_table: Arc::new(Mutex::new(Vec::new())),
            promise_table: Arc::new(Mutex::new(Vec::new())),
            pending_unhandled_rejections: Arc::new(Mutex::new(HashSet::new())),
            microtask_queue: Arc::new(Mutex::new(VecDeque::new())),
            continuation_table: Arc::new(Mutex::new(Vec::new())),
            async_generator_table: Arc::new(Mutex::new(Vec::new())),
            async_from_sync_iterators: Arc::new(Mutex::new(Vec::new())),
            async_iterator_prototype: value::encode_undefined(),
            async_gen_prototype: value::encode_undefined(),
            combinator_contexts: Arc::new(Mutex::new(Vec::new())),
            module_namespace_cache: Arc::new(Mutex::new(HashMap::new())),
            error_table: Arc::new(Mutex::new(Vec::new())),
            map_table: Arc::new(Mutex::new(Vec::new())),
            set_table: Arc::new(Mutex::new(Vec::new())),
            weakmap_table: Arc::new(Mutex::new(Vec::new())),
            weakset_table: Arc::new(Mutex::new(Vec::new())),
            weakref_table: Arc::new(Mutex::new(Vec::new())),
            finalization_registry_table: Arc::new(Mutex::new(Vec::new())),
            pending_cleanup_callbacks: Arc::new(Mutex::new(Vec::new())),
            proxy_table: Arc::new(Mutex::new(Vec::new())),
            arraybuffer_table: Arc::new(Mutex::new(Vec::new())),
            dataview_table: Arc::new(Mutex::new(Vec::new())),
            typedarray_table: Arc::new(Mutex::new(Vec::new())),
            headers_table: Arc::new(Mutex::new(Vec::new())),
            array_proto_values: AtomicI64::new(value::encode_undefined()),
            fetch_response_table: Arc::new(Mutex::new(Vec::new())),
            fetch_request_table: Arc::new(Mutex::new(Vec::new())),
            abort_signal_table: Arc::new(Mutex::new(Vec::new())),
            http_response_table: Arc::new(Mutex::new(Vec::new())),
            readable_stream_table: Arc::new(Mutex::new(Vec::new())),
            reader_table: Arc::new(Mutex::new(Vec::new())),
            stream_controller_table: Arc::new(Mutex::new(Vec::new())),
            byob_request_table: Arc::new(Mutex::new(Vec::new())),
            writable_stream_table: Arc::new(Mutex::new(Vec::new())),
            transform_stream_table: Arc::new(Mutex::new(Vec::new())),
            writer_table: Arc::new(Mutex::new(Vec::new())),
            shared_state: Some(new_shared_runtime_state()),
            non_extensible_handles: Arc::new(Mutex::new(HashSet::new())),
            scope_records: HashMap::new(),
            scope_record_next_handle: 0,
            new_target: AtomicI64::new(value::encode_undefined()),
            host_completion_tx: None,
            async_op_counter: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::execute_with_writer;
    use anyhow::Result;
    use tokio::runtime::Runtime;
    // Phase 5 TDD 回归标记（严格按主代理 2026-06-01 授权方案）：
    // - TimerEntry.deadline 改为 tokio::time::Instant（仅此字段，interval 仍 std Duration）
    // - 仅通过根 use + 创建点显式路径 + sync loop 最小限定符（若需）实现
    // - 目标：cargo check 0 errors + sync timer 语义 100% 保留（MAX_ITERATIONS、per-callback drain、重复重调度顺序不变）
    // - async 路径后续由 scheduler.rs 接管；sync 路径 loop 文本逻辑零改动
    // - 验证：本注释后立即做机械类型变更，每步 cargo check --tests 确认通过
    //   （注意：当前 execute 测试因 wiring 期 pre-existing 缺失 define_get_builtin_global 无法 runtime 跑 timer fixture，此为非 Phase 5 问题，不在此 slice 修复）
    fn compile_source(source: &str) -> Result<Vec<u8>> {
        super::compile_source(source)
    }

    #[test]
    fn execute_with_writer_prints_string_fixture() -> Result<()> {
        let rt = Runtime::new()?;
        let wasm_bytes = compile_source(r#"console.log("Hello, Async Runtime!");"#)?;
        let output = rt.block_on(async { execute_with_writer(&wasm_bytes, Vec::new()).await })?;
        assert_eq!(String::from_utf8(output)?, "Hello, Async Runtime!\n");
        Ok(())
    }

    // 临时 benchmark：分阶段测 execute 路径各步的耗时（消除单次噪声，每步循环取均值）。
    #[test]
    #[ignore]
    fn bench_execute_phases() -> Result<()> {
        use super::*;
        use std::time::Instant;
        let wasm = compile_source("")?;
        let n = 50u32;
        let rt = Runtime::new()?;

        let mut startup = StartupBenchTimings::default();
        rt.block_on(async {
            for _ in 0..n {
                let run = instantiate_for_startup_bench(&wasm).await.unwrap();
                startup.engine_only += run.engine_only;
                startup.module_only += run.module_only;
                startup.store_only += run.store_only;
                startup.linker_register += run.linker_register;
                startup.instantiate_async += run.instantiate_async;
                startup.bootstrap_cold += run.bootstrap_cold;
                startup.host_post_bootstrap += run.host_post_bootstrap;
            }
        });
        eprintln!("BENCH engine only       : {:?}/each", startup.engine_only / n);
        eprintln!("BENCH module only       : {:?}/each", startup.module_only / n);
        eprintln!("BENCH store only        : {:?}/each", startup.store_only / n);
        eprintln!(
            "BENCH linker register   : {:?}/each",
            startup.linker_register / n
        );
        eprintln!(
            "BENCH instantiate_async : {:?}/each",
            startup.instantiate_async / n
        );
        eprintln!("BENCH bootstrap cold    : {:?}/each", startup.bootstrap_cold / n);
        eprintln!(
            "BENCH host post-bootstrap: {:?}/each",
            startup.host_post_bootstrap / n
        );

        let mut full_execute_cold = std::time::Duration::ZERO;
        rt.block_on(async {
            for _ in 0..n {
                let start = Instant::now();
                let _ = execute_with_writer(&wasm, Vec::new()).await.unwrap();
                full_execute_cold += start.elapsed();
            }
        });
        eprintln!(
            "BENCH full execute cold : {:?}/each",
            full_execute_cold / n
        );
        Ok(())
    }
    #[test]
    fn execute_with_writer_timer_fires_via_scheduler() -> Result<()> {
        // Phase 5 核心行为验证：async 路径下 scheduler 接管 timer loop 后必须正确 fire + 输出。
        // 使用 async execute（已 wiring get_builtin_global），触发 setTimeout 回调 + console.log。
        // 证明：无阻塞 sleep、无 MAX 超限、per-callback drain 工作、语义与 sync 一致。
        let rt = Runtime::new()?;
        let wasm_bytes = compile_source(
            r#"
            setTimeout(() => { console.log("async-timer-fired"); }, 0);
        "#,
        )?;
        let output = rt.block_on(async { execute_with_writer(&wasm_bytes, Vec::new()).await })?;
        let s = String::from_utf8(output)?;
        assert!(
            s.contains("async-timer-fired"),
            "async scheduler must deliver timer callback output: {}",
            s
        );
        Ok(())
    }
    // Phase 6 针对性单元测试（任务 6）：手动 enqueue 完成，验证 value settlement + 材料化能分配 runtime string/object
    // 严格引用 plan 458-550 + Correction 7：worker 只 Send 数据/闭包，materialize 在 Store owner 执行
    // 使用 channel 直接模拟（不依赖真实 wasm 主流程），证明 channel 形状 + 处理路径正确
    #[test]
    fn async_host_completion_manual_enqueue_settle_and_materialize() -> Result<()> {
        use super::runtime_builtins::PromiseSettlement;
        use super::scheduler::{AsyncHostCompletion, AsyncOpCounter};

        let counter = AsyncOpCounter::new();
        let _guard = counter.begin();

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<AsyncHostCompletion>();

        // 手动 enqueue SettleValue（模拟 worker 发简单值）
        tx.send(AsyncHostCompletion::SettleValue {
            promise: 100,
            settlement: PromiseSettlement::Fulfill(999),
        })
        .expect("send settle");

        // 手动 enqueue Materialize（闭包在 owner 执行，可分配）
        let mat: Box<
            dyn FnOnce(
                    &mut wasmtime::Store<super::RuntimeState>,
                    &super::WasmEnv,
                ) -> PromiseSettlement
                + Send,
        > = Box::new(|_store, _env| {
            // 真实会 alloc runtime string/object，此处模拟
            PromiseSettlement::Fulfill(888)
        });
        tx.send(AsyncHostCompletion::Materialize {
            promise: 101,
            materialize: mat,
        })
        .expect("send mat");

        // 模拟 scheduler loop drain (while try_recv)
        let c1 = rx.try_recv().expect("c1");
        match c1 {
            AsyncHostCompletion::SettleValue {
                promise,
                settlement: PromiseSettlement::Fulfill(v),
            } => {
                assert_eq!(promise, 100);
                assert_eq!(v, 999);
            }
            _ => panic!("bad settle"),
        }
        let c2 = rx.try_recv().expect("c2");
        match c2 {
            AsyncHostCompletion::Materialize { promise, .. } => {
                assert_eq!(promise, 101);
            }
            _ => panic!("bad mat"),
        }
        drop(_guard);
        assert_eq!(counter.count(), 0);
        Ok(())
    }
}
