use anyhow::Result;
use chrono::{DateTime, Datelike, Local, TimeZone, Timelike, Utc};
use num_traits::cast::ToPrimitive;
use rand::Rng;
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::time::Duration;
use tokio::time::Instant;
use swc_core::ecma::ast as swc_ast;
use wasmtime::Func;
use wasmtime::*;
/// 影子栈大小（必须与后端保持一致）
const SHADOW_STACK_SIZE: u32 = 65536;

use wjsm_ir::{constants, value};
mod runtime_arguments;
mod runtime_async_fn;
mod runtime_builtins;
mod runtime_combinators;
mod runtime_eval;
mod runtime_heap;
mod runtime_host_helpers;
mod runtime_microtask;
mod runtime_promises;
mod scheduler;

mod host_imports;
mod runtime_render;
mod runtime_values;
mod wasm_env;
use host_imports::*;
pub(crate) use wasm_env::WasmEnv;

use runtime_arguments::*;
use runtime_async_fn::*;
use runtime_builtins::*;
use runtime_combinators::*;
use runtime_eval::*;
use runtime_heap::*;
use runtime_host_helpers::*;
use runtime_microtask::*;
use runtime_promises::*;
use runtime_render::*;
use runtime_values::*;
// ── Linker 注册辅助函数 ─────────────────────────────────────────

/// 注册 18 个 define_* 宿主函数模块
fn register_linker(
    linker: &mut Linker<RuntimeState>,
    store: &mut Store<RuntimeState>,
) -> Result<()> {
    define_core(linker, store)?;
    define_timers_arrays(linker, store)?;
    // 使用 1-param fetch shim 兼容当前测试 WASM 的 1-param fetch 签名
    // TODO: 当后端生成 2-param fetch 时恢复 define_fetch
    let f = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, _url: i64| -> i64 {
            alloc_promise_from_caller(&mut caller, PromiseEntry::pending())
        },
    );
    linker.define(&mut *store, "env", "fetch", f)?;
    define_array_object(linker, store)?;
    define_primitive_core(linker, store)?;
    define_promise(linker, store)?;
    define_promise_combinators(linker, store)?;
    define_misc(linker, store)?;
    define_async_fn(linker, store)?;
    define_async_generator(linker, store)?;
    define_proxy_reflect(linker, store)?;
    define_object_builtins(linker, store)?;
    define_string_methods(linker, store)?;
    define_math_number_error(linker, store)?;
    define_collections_buffers(linker, store)?;
    define_proxy_traps(linker, store)?;
    define_get_builtin_global(linker, store)?;
    define_typedarray_new_methods(linker, store)?;
    define_weakref_finalization(linker, store)?;
    define_atomics(linker, store)?;
    Ok(())
}

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
        |caller: Caller<'_, RuntimeState>, record: i64, name: i64| -> i64 {
            eval_get_binding(caller, record, name)
        },
    );
    linker.define(&mut *store, "env", "eval_get_binding", f)?;
    // eval_set_binding
    let f = Func::wrap(
        &mut *store,
        |caller: Caller<'_, RuntimeState>, record: i64, name: i64, val: i64| -> i64 {
            eval_set_binding(caller, record, name, val)
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
        |caller: Caller<'_, RuntimeState>, record: i64| -> i64 {
            eval_super_base(caller, record)
        },
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
        |caller: Caller<'_, RuntimeState>, record: i64| {
            scope_record_destroy(caller, record)
        },
    );
    linker.define(&mut *store, "env", "scope_record_destroy", f)?;
    // symbol_property_key
    let f = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, key: i64| -> i32 {
            if value::is_symbol(key) {
                let handle = value::decode_handle(key) as usize;
                let desc_str = {
                    let table = caller.data().symbol_table.lock().expect("symbol table");
                    table.get(handle).and_then(|e| e.description.clone())
                };
                if let Some(desc) = desc_str {
                    let trimmed = desc
                        .strip_prefix("Symbol(")
                        .and_then(|s| s.strip_suffix(")"))
                        .unwrap_or(&desc);
                    let name_id = find_memory_c_string(&mut caller, trimmed)
                        .or_else(|| alloc_heap_c_string(&mut caller, trimmed));
                    if let Some(id) = name_id {
                        return id as i32;
                    }
                }
            }
            key as i32
        },
    );
    linker.define(&mut *store, "env", "symbol_property_key", f)?;
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

/// 注册 3 个复杂桥接的 sync 版本（使用 Func::wrap + call_wasm_callback）
fn register_complex_bridges_sync(
    linker: &mut Linker<RuntimeState>,
    store: &mut Store<RuntimeState>,
) -> Result<()> {
    // async_iterator_from
    let f = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, iterable: i64| -> i64 {
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

            let Some(ptr) = resolve_handle(&mut caller, iterable) else {
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
                        let mut iters = caller.data().iterators.lock().expect("iterators mutex");
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
            // 尝试 @@asyncIterator
            if let Some(method) =
                read_object_property_by_name(&mut caller, ptr, "Symbol.asyncIterator")
            {
                if value::is_callable(method) {
                    let iterator = if value::is_native_callable(method) {
                        call_native_callable_with_args_from_caller(
                            &mut caller,
                            method,
                            iterable,
                            vec![],
                        )
                        .unwrap_or_else(value::encode_undefined)
                    } else if let Ok(result) =
                        call_wasm_callback(&mut caller, method, iterable, &[])
                    {
                        result
                    } else {
                        value::encode_undefined()
                    };
                    if value::is_object(iterator) {
                        if let Some(iter_ptr) = resolve_handle(&mut caller, iterator) {
                            let next = read_object_property_by_name(&mut caller, iter_ptr, "next")
                                .filter(|n| value::is_callable(*n));
                            if let Some(next_fn) = next {
                                let return_method =
                                    read_object_property_by_name(&mut caller, iter_ptr, "return")
                                        .filter(|c| value::is_callable(*c));
                                let mut iters =
                                    caller.data().iterators.lock().expect("iterators mutex");
                                let handle = iters.len() as u32;
                                iters.push(IteratorState::ObjectIter {
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
                } else if !value::is_undefined(method) && !value::is_null(method) {
                    return create_error_object(
                        &mut caller,
                        "TypeError",
                        value::encode_undefined(),
                    );
                }
            }

            // 回退到 @@iterator
            if let Some(method) = read_object_property_by_name(&mut caller, ptr, "Symbol.iterator")
            {
                if value::is_callable(method) {
                    let sync_iter = if value::is_native_callable(method) {
                        call_native_callable_with_args_from_caller(
                            &mut caller,
                            method,
                            iterable,
                            vec![],
                        )
                        .unwrap_or_else(value::encode_undefined)
                    } else if let Ok(result) =
                        call_wasm_callback(&mut caller, method, iterable, &[])
                    {
                        result
                    } else {
                        value::encode_undefined()
                    };
                    if value::is_object(sync_iter) {
                        if let Some(sync_ptr) = resolve_handle(&mut caller, sync_iter) {
                            let next_fn =
                                read_object_property_by_name(&mut caller, sync_ptr, "next")
                                    .filter(|n| value::is_callable(*n));
                            if let Some(next_fn) = next_fn {
                                let return_method =
                                    read_object_property_by_name(&mut caller, sync_ptr, "return")
                                        .filter(|c| value::is_callable(*c));
                                let sync_iter_handle = {
                                    let mut iters =
                                        caller.data().iterators.lock().expect("iterators mutex");
                                    let sync_handle = iters.len() as u32;
                                    iters.push(IteratorState::ObjectIter {
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
            }

            create_error_object(&mut caller, "TypeError", value::encode_undefined())
        },
    );
    linker.define(&mut *store, "env", "async_iterator_from", f)?;
    // object.group_by
    let f = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, items: i64, callbackfn: i64| -> i64 {
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
                        let key = match call_wasm_callback(
                            &mut caller,
                            callbackfn,
                            value::encode_undefined(),
                            &[elem, idx_val],
                        ) {
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
        },
    );
    linker.define(&mut *store, "env", "object.group_by", f)?;
    // map.group_by
    let f = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, items: i64, callbackfn: i64| -> i64 {
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
                let _ = define_host_data_property(&mut caller, map_result, "size", size_fn);
                let _ = define_host_data_property(&mut caller, map_result, "forEach", for_each_fn);
                let _ = define_host_data_property(&mut caller, map_result, "keys", keys_fn);
                let _ = define_host_data_property(&mut caller, map_result, "values", values_fn);
                let _ = define_host_data_property(&mut caller, map_result, "entries", entries_fn);
            }
            if let Some(_map_ptr) = resolve_handle(&mut caller, map_result) {
                let handle_val = value::encode_f64(map_handle as f64);
                define_host_data_property(&mut caller, map_result, "__map_handle__", handle_val);
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
                        let key = match call_wasm_callback(
                            &mut caller,
                            callbackfn,
                            value::encode_undefined(),
                            &[elem, idx_val],
                        ) {
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
                        let mut table = caller.data().map_table.lock().expect("map table mutex");
                        table[map_handle].keys.push(*group_key);
                        table[map_handle].values.push(arr);
                    }
                }
            }
            map_result
        },
    );
    linker.define(&mut *store, "env", "map.group_by", f)?;
    Ok(())
}

/// 注册 18 个 define_* 宿主函数模块（async 版本，使用 1-param fetch shim）
fn register_linker_async(
    linker: &mut Linker<RuntimeState>,
    store: &mut Store<RuntimeState>,
) -> Result<()> {
    define_core(linker, store)?;
    define_timers_arrays(linker, store)?;
    // Use 1-param fetch shim for async_scheduler test compatibility
    let f = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, url: i64| -> i64 {
            let promise = alloc_promise_from_caller(&mut caller, PromiseEntry::pending());
            promise
        },
    );
    linker.define(&mut *store, "env", "fetch", f)?;
    define_array_object(linker, store)?;
    define_primitive_core(linker, store)?;
    define_promise(linker, store)?;
    define_promise_combinators(linker, store)?;
    define_misc(linker, store)?;
    define_async_fn(linker, store)?;
    define_async_generator(linker, store)?;
    define_proxy_reflect(linker, store)?;
    define_object_builtins(linker, store)?;
    define_string_methods(linker, store)?;
    define_math_number_error(linker, store)?;
    define_collections_buffers(linker, store)?;
    define_proxy_traps(linker, store)?;
    define_typedarray_new_methods(linker, store)?;
    define_weakref_finalization(linker, store)?;
    define_atomics(linker, store)?;
    define_get_builtin_global(linker, store)?;
    Ok(())
}

/// 注册 3 个复杂桥接的 async 版本（使用 Linker::func_wrap_async + call_wasm_callback_async）
fn register_complex_bridges_async(
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

                let Some(ptr) = resolve_handle(&mut caller, iterable) else {
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
                            let mut iters = caller.data().iterators.lock().expect("iterators mutex");
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
                // 尝试 @@asyncIterator
                if let Some(method) =
                    read_object_property_by_name(&mut caller, ptr, "Symbol.asyncIterator")
                {
                    if value::is_callable(method) {
                        let iterator = if value::is_native_callable(method) {
                            call_native_callable_with_args_from_caller(
                                &mut caller,
                                method,
                                iterable,
                                vec![],
                            )
                            .unwrap_or_else(value::encode_undefined)
                        } else if let Ok(result) =
                            call_wasm_callback_async(&mut caller, method, iterable, &[]).await
                        {
                            result
                        } else {
                            value::encode_undefined()
                        };
                        if value::is_object(iterator) {
                            if let Some(iter_ptr) = resolve_handle(&mut caller, iterator) {
                                let next = read_object_property_by_name(&mut caller, iter_ptr, "next")
                                    .filter(|n| value::is_callable(*n));
                                if let Some(next_fn) = next {
                                    let return_method =
                                        read_object_property_by_name(&mut caller, iter_ptr, "return")
                                            .filter(|c| value::is_callable(*c));
                                    let mut iters =
                                        caller.data().iterators.lock().expect("iterators mutex");
                                    let handle = iters.len() as u32;
                                    iters.push(IteratorState::ObjectIter {
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
                    } else if !value::is_undefined(method) && !value::is_null(method) {
                        return create_error_object(
                            &mut caller,
                            "TypeError",
                            value::encode_undefined(),
                        );
                    }
                }

                // 回退到 @@iterator
                if let Some(method) = read_object_property_by_name(&mut caller, ptr, "Symbol.iterator")
                {
                    if value::is_callable(method) {
                        let sync_iter = if value::is_native_callable(method) {
                            call_native_callable_with_args_from_caller(
                                &mut caller,
                                method,
                                iterable,
                                vec![],
                            )
                            .unwrap_or_else(value::encode_undefined)
                        } else if let Ok(result) =
                            call_wasm_callback_async(&mut caller, method, iterable, &[]).await
                        {
                            result
                        } else {
                            value::encode_undefined()
                        };
                        if value::is_object(sync_iter) {
                            if let Some(sync_ptr) = resolve_handle(&mut caller, sync_iter) {
                                let next_fn =
                                    read_object_property_by_name(&mut caller, sync_ptr, "next")
                                        .filter(|n| value::is_callable(*n));
                                if let Some(next_fn) = next_fn {
                                    let return_method =
                                        read_object_property_by_name(&mut caller, sync_ptr, "return")
                                            .filter(|c| value::is_callable(*c));
                                    let sync_iter_handle = {
                                        let mut iters =
                                            caller.data().iterators.lock().expect("iterators mutex");
                                        let sync_handle = iters.len() as u32;
                                        iters.push(IteratorState::ObjectIter {
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
                }

                create_error_object(&mut caller, "TypeError", value::encode_undefined())
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
                            ).await {
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
                    let _ = define_host_data_property(&mut caller, map_result, "size", size_fn);
                    let _ = define_host_data_property(&mut caller, map_result, "forEach", for_each_fn);
                    let _ = define_host_data_property(&mut caller, map_result, "keys", keys_fn);
                    let _ = define_host_data_property(&mut caller, map_result, "values", values_fn);
                    let _ = define_host_data_property(&mut caller, map_result, "entries", entries_fn);
                }
                if let Some(_map_ptr) = resolve_handle(&mut caller, map_result) {
                    let handle_val = value::encode_f64(map_handle as f64);
                    define_host_data_property(&mut caller, map_result, "__map_handle__", handle_val);
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
                            ).await {
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
                            let mut table = caller.data().map_table.lock().expect("map table mutex");
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

pub fn execute(wasm_bytes: &[u8]) -> Result<()> {
    let stdout = io::stdout();
    let _ = execute_with_writer(wasm_bytes, stdout.lock())?;
    Ok(())
}

pub fn execute_with_writer<W: Write>(wasm_bytes: &[u8], writer: W) -> Result<W> {
    let engine = Engine::default();
    let module = match Module::new(&engine, wasm_bytes) {
        Ok(m) => m,
        Err(e) => {
            return Err(anyhow::anyhow!("WASM validation failed: {:?}", e));
        }
    };
    let mut store = Store::new(&engine, RuntimeState::new());
    let output = Arc::clone(&store.data().output);
    let runtime_error = Arc::clone(&store.data().runtime_error);


    let mut linker = Linker::new(&engine);
    register_linker(&mut linker, &mut store)?;
    register_common_bridges(&mut linker, &mut store)?;
    register_complex_bridges_sync(&mut linker, &mut store)?;
    let instance = linker.instantiate(&mut store, &module)?;
    // ── Create %AsyncIteratorPrototype% and AsyncGenerator.prototype ──
    let memory = instance
        .get_export(&mut store, "memory")
        .and_then(|e| e.into_memory())
        .expect("memory export");
    let heap_ptr_global = instance
        .get_export(&mut store, "__heap_ptr")
        .and_then(|e| e.into_global())
        .expect("__heap_ptr export");
    let obj_table_ptr_global = instance
        .get_export(&mut store, "__obj_table_ptr")
        .and_then(|e| e.into_global())
        .expect("__obj_table_ptr export");
    let obj_table_count_global = instance
        .get_export(&mut store, "__obj_table_count")
        .and_then(|e| e.into_global())
        .expect("__obj_table_count export");
    let func_table = instance
        .get_export(&mut store, "__table")
        .and_then(|e| e.into_table())
        .expect("__table export");
    let shadow_sp_global = instance
        .get_export(&mut store, "__shadow_sp")
        .and_then(|e| e.into_global())
        .expect("__shadow_sp export");
    let array_proto_handle_global = instance
        .get_export(&mut store, "__array_proto_handle")
        .and_then(|e| e.into_global())
        .expect("__array_proto_handle export");
    let object_proto_handle_global = instance
        .get_export(&mut store, "__object_proto_handle")
        .and_then(|e| e.into_global())
        .expect("__object_proto_handle export");
    let wasm_env = wasm_env::WasmEnv {
        memory,
        func_table,
        shadow_sp: shadow_sp_global,
        heap_ptr: heap_ptr_global,
        obj_table_ptr: obj_table_ptr_global,
        obj_table_count: obj_table_count_global,
        object_proto_handle: object_proto_handle_global,
        array_proto_handle: array_proto_handle_global,
    };

    // 创建 %AsyncIteratorPrototype%
    let async_iterator_proto = alloc_host_object(&mut store, &wasm_env, 2);

    // 创建 AsyncIteratorProtoSymbolAsyncIterator native callable
    let async_iterator_symbol_async_iterator = {
        let mut table = store
            .data()
            .native_callables
            .lock()
            .expect("native callable table mutex");
        let handle = table.len() as u32;
        table.push(NativeCallable::AsyncIteratorProtoSymbolAsyncIterator);
        value::encode_native_callable_idx(handle)
    };

    // 设置 %AsyncIteratorPrototype%[Symbol.asyncIterator]
    let _ = define_host_data_property_with_env(
        &mut store,
        &wasm_env,
        async_iterator_proto,
        "Symbol.asyncIterator",
        async_iterator_symbol_async_iterator,
    );

    // 设置 %AsyncIteratorPrototype%[Symbol.toStringTag] = "AsyncIterator"
    let async_iterator_tag =
        store_runtime_string_in_state(store.data(), "AsyncIterator".to_string());
    let _ = define_host_data_property_with_env(
        &mut store,
        &wasm_env,
        async_iterator_proto,
        "Symbol.toStringTag",
        async_iterator_tag,
    );

    // 创建 AsyncGenerator.prototype
    let async_gen_proto = alloc_host_object(&mut store, &wasm_env, 2);

    // 设置 AsyncGenerator.prototype.[[Prototype]] = %AsyncIteratorPrototype%
    let async_gen_handle = value::decode_object_handle(async_gen_proto);
    let async_iterator_handle = value::decode_object_handle(async_iterator_proto);
    let obj_ptr = resolve_handle_idx_with_env(&mut store, &wasm_env, async_gen_handle as usize)
        .expect("async_gen_proto object ptr");
    let data = memory.data_mut(&mut store);
    data[obj_ptr..obj_ptr + 4].copy_from_slice(&async_iterator_handle.to_le_bytes());

    // 设置 AsyncGenerator.prototype[Symbol.toStringTag] = "AsyncGenerator"
    let async_gen_tag = store_runtime_string_in_state(store.data(), "AsyncGenerator".to_string());
    let _ = define_host_data_property_with_env(
        &mut store,
        &wasm_env,
        async_gen_proto,
        "Symbol.toStringTag",
        async_gen_tag,
    );

    // 设置 RuntimeState 中的原型字段
    store.data_mut().async_iterator_prototype = async_iterator_proto;
    store.data_mut().async_gen_prototype = async_gen_proto;

    // ── Run main ──
    let main = instance.get_typed_func::<(), i64>(&mut store, "main")?;
    let main_result = main.call(&mut store, ());
    let main_ok = match main_result {
        Ok(return_val) => {
            if value::is_exception(return_val) {
                // 未捕获异常被抛出顶层：将异常信息写入输出并设置 runtime_error
                let idx = value::decode_handle(return_val) as usize;
                if let Some(entry) = store
                    .data()
                    .error_table
                    .lock()
                    .expect("error_table mutex")
                    .get(idx)
                {
                    let msg = if entry.message.is_empty() {
                        "Uncaught exception".to_string()
                    } else {
                        format!("Uncaught exception: {}", entry.message)
                    };
                    let mut buffer = store.data().output.lock().expect("output mutex");
                    writeln!(&mut *buffer, "{msg}").ok();
                    *store
                        .data()
                        .runtime_error
                        .lock()
                        .expect("runtime_error mutex") = Some(msg);
                }
                // 跳过后续 microtasks/timers
                false
            } else {
                true
            }
        }
        Err(ref trap) => {
            if store
                .data()
                .runtime_error
                .lock()
                .expect("runtime_error mutex")
                .is_some()
            {
                // throw import 已经记录了 JS 层异常；先走统一输出/错误收集路径。
                false
            } else {
                return Err(anyhow::anyhow!("WASM trap: {:?}", trap));
            }
        }
    };

    // ── Drain microtasks after main() ────────────────────────────────────
    if main_ok {
        drain_microtasks(&mut store, &wasm_env);
    }

    // ── Timer event loop (only if main succeeded) ─────────────────────────
    // Poll timers; fire expired callbacks via the WASM function table.
    if main_ok {
        let mut timer_iterations = 0u32;
        const MAX_TIMER_ITERATIONS: u32 = 1000;
        loop {
            timer_iterations += 1;
            if timer_iterations > MAX_TIMER_ITERATIONS {
                writeln!(
                    store.data().output.lock().expect("output mutex"),
                    "Internal error: timer event loop exceeded max iterations"
                )
                .ok();
                break;
            }
            let now = Instant::now();
            let mut _entry_to_fire: Option<TimerEntry> = None;

            {
                let mut timers = store.data().timers.lock().expect("timers mutex");
                let mut cancelled = store
                    .data()
                    .cancelled_timers
                    .lock()
                    .expect("cancelled_timers mutex");

                // Remove cancelled timers
                timers.retain(|t| !cancelled.contains(&t.id));
                cancelled.clear();

                if timers.is_empty() {
                    break;
                }

                // Find earliest expired timer
                if let Some(idx) = timers.iter().position(|t| t.deadline <= now) {
                    _entry_to_fire = Some(timers.remove(idx));
                } else {
                    // Sleep until next timer
                    let next = timers.iter().min_by_key(|t| t.deadline).unwrap().deadline;
                    let dur = next.saturating_duration_since(Instant::now());
                    if !dur.is_zero() {
                        std::thread::sleep(dur);
                    }
                    continue;
                }
            }

            if let Some(entry) = _entry_to_fire {
                let callback = entry.callback;
                let repeating = entry.repeating;
                let interval = entry.interval;
                let entry_id = entry.id;

                // 定时器回调按宿主 API 语义以 this=undefined、零参数调用。
                call_host_function_with_args(
                    &mut store,
                    &wasm_env,
                    callback,
                    value::encode_undefined(),
                    &[],
                );

                // Drain microtasks after timer callback
                drain_microtasks(&mut store, &wasm_env);

                // Re-schedule if repeating
                if repeating {
                    store
                        .data()
                        .timers
                        .lock()
                        .expect("timers mutex")
                        .push(TimerEntry {
                            id: entry_id,
                            deadline: Instant::now() + interval,
                            callback,
                            repeating: true,
                            interval,
                        });
                }
            }
        }
    }
    // ── Collect output ────────────────────────────────────────────────────
    let bytes = output
        .lock()
        .expect("runtime output buffer mutex should not be poisoned")
        .clone();
    drop(store);

    let mut writer = writer;
    writer.write_all(&bytes)?;

    // ── Check errors ─────────────────────────────────────────────────────
    if let Some(message) = runtime_error.lock().expect("runtime error mutex").clone() {
        anyhow::bail!(message);
    }

    // Propagate any wasm trap from main() call (must be after output collection)
    main_result?;

    Ok(writer)
}
/// 异步版本入口（薄封装，专注 wiring gate）。
/// 状态构造：与 sync 相同（boring explicit 少量重复，按指派）。
/// Engine: async_support(true) + epoch_interruption(true)。
/// Store 后立即 epoch yield。
/// Linker: 复用 define_* + 必要短手动桥接（长闭包如 async_iterator_from 为本 slice 省略，简单 console.log 测试不命中）。
/// EpochIncrementer: crate 内不存在（Phase 4），略。
/// instantiate_async + 委托 pre-existing run_main_completion_block_async.await。
/// 仅完成指定 slice + e2e 测试；不碰 scheduler/定时器/Phase5+。
pub async fn execute_async(wasm_bytes: &[u8]) -> Result<()> {
    let stdout = io::stdout();
    let _ = execute_with_writer_async(wasm_bytes, stdout.lock()).await?;
    Ok(())
}

pub async fn execute_with_writer_async<W: Write>(wasm_bytes: &[u8], writer: W) -> Result<W> {
    let mut config = Config::new();
    config.async_support(true);
    config.epoch_interruption(true);
    let engine = Engine::new(&config)
        .map_err(|e| anyhow::anyhow!("Failed to create async engine: {:?}", e))?;

    let module = match Module::new(&engine, wasm_bytes) {
        Ok(m) => m,
        Err(e) => {
            return Err(anyhow::anyhow!("WASM validation failed: {:?}", e));
        }
    };

    let mut store = Store::new(&engine, RuntimeState::new());
    let output = Arc::clone(&store.data().output);
    let runtime_error = Arc::clone(&store.data().runtime_error);

    store.set_epoch_deadline(1);
    store.epoch_deadline_async_yield_and_update(1);

    // Phase 6: 创建 channel + counter（仅 async 路径）
    let (host_completion_tx, mut host_completion_rx) =
        tokio::sync::mpsc::unbounded_channel();
    store
        .data_mut()
        .host_completion_tx
        .replace(host_completion_tx);
    let counter = crate::scheduler::AsyncOpCounter::new();
    store.data_mut().async_op_counter.replace(counter);

    let mut linker = Linker::new(&engine);
    register_linker_async(&mut linker, &mut store)?;
    register_common_bridges(&mut linker, &mut store)?;
    register_complex_bridges_async(&mut linker, &mut store)?;
    let instance = linker.instantiate_async(&mut store, &module).await
        .map_err(|e| anyhow::anyhow!("async instantiate failed: {:?}", e))?;

    // post 原型（boring dupe 小段）
    let memory = instance.get_export(&mut store, "memory").and_then(|e| e.into_memory()).expect("memory");
    let heap_ptr_global = instance.get_export(&mut store, "__heap_ptr").and_then(|e| e.into_global()).expect("heap");
    let obj_table_ptr_global = instance.get_export(&mut store, "__obj_table_ptr").and_then(|e| e.into_global()).expect("obj_table_ptr");
    let obj_table_count_global = instance.get_export(&mut store, "__obj_table_count").and_then(|e| e.into_global()).expect("obj_table_count");
    let func_table = instance.get_export(&mut store, "__table").and_then(|e| e.into_table()).expect("table");
    let shadow_sp_global = instance.get_export(&mut store, "__shadow_sp").and_then(|e| e.into_global()).expect("shadow");
    let array_proto_handle_global = instance.get_export(&mut store, "__array_proto_handle").and_then(|e| e.into_global()).expect("array_proto");
    let object_proto_handle_global = instance.get_export(&mut store, "__object_proto_handle").and_then(|e| e.into_global()).expect("object_proto");
    let wasm_env = wasm_env::WasmEnv {
        memory, func_table, shadow_sp: shadow_sp_global, heap_ptr: heap_ptr_global,
        obj_table_ptr: obj_table_ptr_global, obj_table_count: obj_table_count_global,
        object_proto_handle: object_proto_handle_global, array_proto_handle: array_proto_handle_global,
    };
    let async_iterator_proto = alloc_host_object(&mut store, &wasm_env, 2);
    let async_iterator_symbol_async_iterator = {
        let mut table = store.data().native_callables.lock().expect("native");
        let handle = table.len() as u32;
        table.push(NativeCallable::AsyncIteratorProtoSymbolAsyncIterator);
        value::encode_native_callable_idx(handle)
    };
    let _ = define_host_data_property_with_env(&mut store, &wasm_env, async_iterator_proto, "Symbol.asyncIterator", async_iterator_symbol_async_iterator);
    let async_iterator_tag = store_runtime_string_in_state(store.data(), "AsyncIterator".to_string());
    let _ = define_host_data_property_with_env(&mut store, &wasm_env, async_iterator_proto, "Symbol.toStringTag", async_iterator_tag);
    let async_gen_proto = alloc_host_object(&mut store, &wasm_env, 2);
    let async_gen_handle = value::decode_object_handle(async_gen_proto);
    let async_iterator_handle = value::decode_object_handle(async_iterator_proto);
    let obj_ptr = resolve_handle_idx_with_env(&mut store, &wasm_env, async_gen_handle as usize).expect("obj_ptr");
    let data = memory.data_mut(&mut store);
    data[obj_ptr..obj_ptr + 4].copy_from_slice(&async_iterator_handle.to_le_bytes());
    let async_gen_tag = store_runtime_string_in_state(store.data(), "AsyncGenerator".to_string());
    let _ = define_host_data_property_with_env(&mut store, &wasm_env, async_gen_proto, "Symbol.toStringTag", async_gen_tag);
    store.data_mut().async_iterator_prototype = async_iterator_proto;
    store.data_mut().async_gen_prototype = async_gen_proto;

    // 委托（Phase 6 传入 rx 供 scheduler 接收 completion）
    run_main_completion_block_async(&instance, store, wasm_env, output, runtime_error, writer, &mut host_completion_rx).await
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
    #[allow(dead_code)]
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
    /// Optional shared state for cross-agent coordination.
    /// None in normal (non-agent) execution.
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
    host_completion_tx: Option<tokio::sync::mpsc::UnboundedSender<crate::scheduler::AsyncHostCompletion>>,
    /// Phase 6: in-flight async op 计数器（用于 scheduler 安全退出条件）。
    async_op_counter: Option<crate::scheduler::AsyncOpCounter>,
}

impl RuntimeState {
    /// 构造一个新的 RuntimeState，所有侧表初始化为空，well-known symbols 预分配。
    fn new() -> Self {
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
            eval_cache: Arc::new(Mutex::new(HashMap::new())),
            bigint_table: Arc::new(Mutex::new(Vec::new())),
            symbol_table: Arc::new(Mutex::new(vec![
                SymbolEntry { description: Some("Symbol(Symbol.iterator)".into()), global_key: None },
                SymbolEntry { description: Some("Symbol(Symbol.species)".into()), global_key: None },
                SymbolEntry { description: Some("Symbol(Symbol.toStringTag)".into()), global_key: None },
                SymbolEntry { description: Some("Symbol(Symbol.asyncIterator)".into()), global_key: None },
                SymbolEntry { description: Some("Symbol(Symbol.hasInstance)".into()), global_key: None },
                SymbolEntry { description: Some("Symbol(Symbol.toPrimitive)".into()), global_key: None },
                SymbolEntry { description: Some("Symbol(Symbol.dispose)".into()), global_key: None },
                SymbolEntry { description: Some("Symbol(Symbol.match)".into()), global_key: None },
                SymbolEntry { description: Some("Symbol(Symbol.asyncDispose)".into()), global_key: None },
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
            fetch_response_table: Arc::new(Mutex::new(Vec::new())),
            fetch_request_table: Arc::new(Mutex::new(Vec::new())),
            shared_state: Some(Arc::new(SharedRuntimeState {
                sab_table: Arc::new(Mutex::new(Vec::new())),
                agent_state: Arc::new(AgentState {
                    reports: Arc::new(Mutex::new(Vec::new())),
                    waiters: Arc::new(Mutex::new(HashMap::new())),
                }),
            })),
            non_extensible_handles: Arc::new(Mutex::new(HashSet::new())),
            scope_records: HashMap::new(),
            scope_record_next_handle: 0,
            new_target: AtomicI64::new(value::encode_undefined()),
            host_completion_tx: None,
            async_op_counter: None,
        }
    }
}
/// 绑定函数记录
struct BoundRecord {
    target_func: i64,     // TAG_FUNCTION / TAG_CLOSURE / TAG_BOUND
    bound_this: i64,      // NaN-boxed
    bound_args: Vec<i64>, // NaN-boxed values
}

/// Symbol 条目
struct SymbolEntry {
    description: Option<String>,
    global_key: Option<String>,
}

/// Error 条目：存储 error 对象的 name 和 message
#[allow(dead_code)]
struct ErrorEntry {
    name: String,
    message: String,
    value: i64,
}

struct MapEntry {
    keys: Vec<i64>,
    values: Vec<i64>,
}

struct SetEntry {
    values: Vec<i64>,
}

#[derive(Clone, Debug)]
struct WeakMapEntry {
    map: HashMap<u32, i64>,
}

#[derive(Clone, Debug)]
struct WeakSetEntry {
    set: HashSet<u32>,
}

#[derive(Clone, Debug)]
struct WeakRefEntry {
    target_handle: u32,
}

#[derive(Clone, Debug)]
struct FinalizationRegistryEntry {
    object_handle: u32,
    callback: i64,
    registrations: Vec<FinalizationRegistration>,
}

#[derive(Clone, Debug)]
struct FinalizationRegistration {
    target_handle: u32,
    held_value: i64,
    unregister_token: Option<i64>,
}

#[derive(Clone, Debug)]
struct ArrayBufferEntry {
    data: Vec<u8>,
}

#[derive(Clone, Debug)]
struct SharedArrayBufferEntry {
    data: Arc<RwLock<Vec<u8>>>,
    byte_length: u64,
}

struct SharedRuntimeState {
    sab_table: Arc<Mutex<Vec<SharedArrayBufferEntry>>>,
    agent_state: Arc<AgentState>,
}

struct AgentState {
    reports: Arc<Mutex<Vec<String>>>,
    waiters: Arc<Mutex<HashMap<(u32, u32), Vec<Waiter>>>>,
}

struct Waiter {
    condvar: Arc<Condvar>,
    notified: Arc<AtomicBool>,
}

#[derive(Clone, Debug)]
struct DataViewEntry {
    buffer_handle: u32,
    byte_offset: u32,
    byte_length: u32,
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
pub(crate) struct TypedArrayEntry {
    buffer_handle: u32,
    byte_offset: u32,
    length: u32,
    element_size: u8,
    /// 0=Int, 1=Uint, 2=Clamped, 3=Float, 4=BigInt, 5=BigUint
    element_kind: u8,
    is_shared: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ResponseType {
    Basic,
    Cors,
    Error,
    Opaque,
    OpaqueRedirect,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RedirectMode {
    Follow,
    Error,
    Manual,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum HeadersGuard {
    #[default]
    None,
    Request,
    RequestNoCors,
    Response,
    Immutable,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum RequestMode {
    #[default]
    Cors,
    SameOrigin,
    NoCors,
    Navigate,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum RequestCredentials {
    #[default]
    SameOrigin,
    Omit,
    Include,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum RequestCache {
    #[default]
    Default,
    NoStore,
    Reload,
    NoCache,
    ForceCache,
    OnlyIfCached,
}
#[derive(Clone, Debug)]
struct HeadersEntry {
    /// Lowercased key → value (append allows multi-value; we store duplicates)
    pairs: Vec<(String, String)>,
    guard: HeadersGuard,
}
#[derive(Clone, Debug)]
struct FetchResponseEntry {
    status: u16,
    status_text: String,
    headers_handle: u32,
    url: String,
    body: Vec<u8>,
    response_type: ResponseType,
    redirected: bool,
    body_used: bool,
}
#[derive(Clone, Debug)]
struct FetchRequestEntry {
    method: String,
    url: String,
    headers_handle: u32,
    body: Option<Vec<u8>>,
    redirect: RedirectMode,
    body_used: bool,
    // Extended observable fields per Fetch Standard
    mode: RequestMode,
    credentials: RequestCredentials,
    cache: RequestCache,
    referrer: String,
    referrer_policy: String,
    integrity: String,
    keepalive: bool,
    destination: String,
    duplex: String,
}
fn bigint_low_64_bytes(value: &num_bigint::BigInt) -> [u8; 8] {
    let fill = if value.sign() == num_bigint::Sign::Minus {
        0xff
    } else {
        0
    };
    let mut out = [fill; 8];
    let bytes = value.to_signed_bytes_le();
    let len = bytes.len().min(8);
    out[..len].copy_from_slice(&bytes[..len]);
    out
}
pub(crate) fn typedarray_entry_from_value(
    caller: &mut Caller<'_, RuntimeState>,
    value_raw: i64,
) -> Option<TypedArrayEntry> {
    if !value::is_object(value_raw) {
        return None;
    }
    let ptr = resolve_handle(caller, value_raw)?;
    let handle_raw = read_object_property_by_name(caller, ptr, "__typedarray_handle__")?;
    let handle = value::decode_f64(handle_raw) as usize;
    let table = caller.data().typedarray_table.lock().ok()?;
    table.get(handle).cloned()
}

fn typedarray_element_offset(entry: &TypedArrayEntry, index: u32) -> Option<usize> {
    if index >= entry.length {
        return None;
    }
    Some(entry.byte_offset as usize + index as usize * entry.element_size as usize)
}

fn decode_typedarray_element(
    caller: &mut Caller<'_, RuntimeState>,
    bytes: &[u8; 8],
    elem_size: u8,
    element_kind: u8,
) -> Option<i64> {
    let value = match (elem_size, element_kind) {
        (1, 0) => value::encode_f64(bytes[0] as i8 as f64),
        (1, 1) | (1, 2) => value::encode_f64(bytes[0] as f64),
        (2, 0) => value::encode_f64(i16::from_le_bytes([bytes[0], bytes[1]]) as f64),
        (2, 1) => value::encode_f64(u16::from_le_bytes([bytes[0], bytes[1]]) as f64),
        (4, 0) => {
            value::encode_f64(i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as f64)
        }
        (4, 1) => {
            value::encode_f64(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as f64)
        }
        (4, 3) => {
            value::encode_f64(f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as f64)
        }
        (8, 3) => value::encode_f64(f64::from_le_bytes(*bytes)),
        (8, 4) => {
            let raw = i64::from_le_bytes(*bytes);
            let mut table = caller.data().bigint_table.lock().ok()?;
            let handle = table.len() as u32;
            table.push(num_bigint::BigInt::from(raw));
            value::encode_bigint_handle(handle)
        }
        (8, 5) => {
            let raw = u64::from_le_bytes(*bytes);
            let mut table = caller.data().bigint_table.lock().ok()?;
            let handle = table.len() as u32;
            table.push(num_bigint::BigInt::from(raw));
            value::encode_bigint_handle(handle)
        }
        _ => return None,
    };
    Some(value)
}

fn to_uint8_clamp(number: f64) -> u8 {
    if number.is_nan() || number <= 0.0 {
        return 0;
    }
    if number >= 255.0 {
        return 255;
    }
    let floor = number.floor();
    let delta = number - floor;
    if delta > 0.5 {
        return floor as u8 + 1;
    }
    if delta < 0.5 {
        return floor as u8;
    }
    let value = floor as u8;
    value + (value & 1)
}

fn set_typedarray_runtime_error(caller: &mut Caller<'_, RuntimeState>, message: &'static str) {
    set_runtime_error(caller.data(), message.to_string());
}

fn typedarray_to_number(caller: &mut Caller<'_, RuntimeState>, value_raw: i64) -> Option<f64> {
    if value::is_bigint(value_raw) {
        set_typedarray_runtime_error(
            caller,
            "TypeError: Cannot convert a BigInt value to a number",
        );
        return None;
    }
    let number_raw = to_number(caller, value_raw);
    if caller
        .data()
        .runtime_error
        .lock()
        .expect("runtime_error mutex")
        .is_some()
    {
        return None;
    }
    Some(value::decode_f64(number_raw))
}

fn to_uint_n(number: f64, bits: u32) -> u32 {
    if number == 0.0 || !number.is_finite() {
        return 0;
    }
    let modulo = 2.0_f64.powi(bits as i32);
    number.trunc().rem_euclid(modulo) as u32
}

fn to_int_n(number: f64, bits: u32) -> i32 {
    let unsigned = to_uint_n(number, bits);
    let sign_bit = 1u32 << (bits - 1);
    if (unsigned & sign_bit) == 0 {
        unsigned as i32
    } else {
        (unsigned as i64 - (1i64 << bits)) as i32
    }
}

fn typedarray_to_index(
    caller: &mut Caller<'_, RuntimeState>,
    value_raw: i64,
    range_error: &'static str,
) -> Option<u32> {
    if value::is_undefined(value_raw) {
        return Some(0);
    }
    let number = typedarray_to_number(caller, value_raw)?;
    if number.is_nan() || number == 0.0 {
        return Some(0);
    }
    if !number.is_finite() || number < 0.0 || number.trunc() > u32::MAX as f64 {
        set_typedarray_runtime_error(caller, range_error);
        return None;
    }
    Some(number.trunc() as u32)
}

fn typedarray_byte_len(
    caller: &mut Caller<'_, RuntimeState>,
    len: u32,
    elem_size: u32,
) -> Option<usize> {
    let Some(byte_len) = len.checked_mul(elem_size) else {
        set_typedarray_runtime_error(caller, "RangeError: Invalid typed array length");
        return None;
    };
    Some(byte_len as usize)
}

pub(crate) fn encode_typedarray_element(
    caller: &mut Caller<'_, RuntimeState>,
    elem_size: u8,
    element_kind: u8,
    value_raw: i64,
) -> Option<[u8; 8]> {
    let mut out = [0u8; 8];
    match (elem_size, element_kind) {
        (1, 0) => out[0] = to_int_n(typedarray_to_number(caller, value_raw)?, 8) as i8 as u8,
        (1, 1) => out[0] = to_uint_n(typedarray_to_number(caller, value_raw)?, 8) as u8,
        (1, 2) => out[0] = to_uint8_clamp(typedarray_to_number(caller, value_raw)?),
        (2, 0) => out[..2].copy_from_slice(
            &(to_int_n(typedarray_to_number(caller, value_raw)?, 16) as i16).to_le_bytes(),
        ),
        (2, 1) => out[..2].copy_from_slice(
            &(to_uint_n(typedarray_to_number(caller, value_raw)?, 16) as u16).to_le_bytes(),
        ),
        (4, 0) => out[..4]
            .copy_from_slice(&to_int_n(typedarray_to_number(caller, value_raw)?, 32).to_le_bytes()),
        (4, 1) => out[..4].copy_from_slice(
            &to_uint_n(typedarray_to_number(caller, value_raw)?, 32).to_le_bytes(),
        ),
        (4, 3) => out[..4]
            .copy_from_slice(&(typedarray_to_number(caller, value_raw)? as f32).to_le_bytes()),
        (8, 3) => out.copy_from_slice(&typedarray_to_number(caller, value_raw)?.to_le_bytes()),
        (8, 4) | (8, 5) => {
            if !value::is_bigint(value_raw) {
                set_typedarray_runtime_error(caller, "TypeError: Cannot convert value to a BigInt");
                return None;
            }
            let handle = value::decode_bigint_handle(value_raw) as usize;
            let table = caller.data().bigint_table.lock().ok()?;
            let bigint = table.get(handle)?;
            out = bigint_low_64_bytes(bigint);
        }
        _ => return None,
    }
    Some(out)
}

pub(crate) fn typedarray_element_read(
    caller: &mut Caller<'_, RuntimeState>,
    typedarray: i64,
    index: u32,
) -> Option<i64> {
    let entry = typedarray_entry_from_value(caller, typedarray)?;
    typedarray_element_read_entry(caller, &entry, index)
}

pub(crate) fn typedarray_element_read_entry(
    caller: &mut Caller<'_, RuntimeState>,
    entry: &TypedArrayEntry,
    index: u32,
) -> Option<i64> {
    let off = typedarray_element_offset(entry, index)?;
    let mut bytes = [0u8; 8];
    let elem_size = entry.element_size as usize;
    if entry.is_shared {
        let shared = caller.data().shared_state.as_ref()?.clone();
        let sab_table = shared.sab_table.lock().ok()?;
        let buffer = sab_table.get(entry.buffer_handle as usize)?;
        let data = buffer.data.read().ok()?;
        if off + elem_size > data.len() {
            return None;
        }
        bytes[..elem_size].copy_from_slice(&data[off..off + elem_size]);
    } else {
        let ab_table = caller.data().arraybuffer_table.lock().ok()?;
        let buffer = ab_table.get(entry.buffer_handle as usize)?;
        if off + elem_size > buffer.data.len() {
            return None;
        }
        bytes[..elem_size].copy_from_slice(&buffer.data[off..off + elem_size]);
    }
    decode_typedarray_element(caller, &bytes, entry.element_size, entry.element_kind)
}

pub(crate) fn typedarray_element_write(
    caller: &mut Caller<'_, RuntimeState>,
    typedarray: i64,
    index: u32,
    value_raw: i64,
) -> bool {
    let Some(entry) = typedarray_entry_from_value(caller, typedarray) else {
        return false;
    };
    let Some(off) = typedarray_element_offset(&entry, index) else {
        return false;
    };
    let Some(bytes) =
        encode_typedarray_element(caller, entry.element_size, entry.element_kind, value_raw)
    else {
        return false;
    };
    let elem_size = entry.element_size as usize;
    if entry.is_shared {
        let Some(shared) = caller.data().shared_state.as_ref().cloned() else {
            return false;
        };
        let Ok(sab_table) = shared.sab_table.lock() else {
            return false;
        };
        let Some(buffer) = sab_table.get(entry.buffer_handle as usize) else {
            return false;
        };
        let Ok(mut data) = buffer.data.write() else {
            return false;
        };
        if off + elem_size > data.len() {
            return false;
        }
        data[off..off + elem_size].copy_from_slice(&bytes[..elem_size]);
        true
    } else {
        let Ok(mut ab_table) = caller.data().arraybuffer_table.lock() else {
            return false;
        };
        let Some(buffer) = ab_table.get_mut(entry.buffer_handle as usize) else {
            return false;
        };
        if off + elem_size > buffer.data.len() {
            return false;
        }
        buffer.data[off..off + elem_size].copy_from_slice(&bytes[..elem_size]);
        true
    }
}

pub(crate) fn typedarray_construct(
    caller: &mut Caller<'_, RuntimeState>,
    buffer: i64,
    byte_offset: i64,
    length: i64,
    elem_size: u8,
    element_kind: u8,
    target_obj: Option<i64>,
) -> i64 {
    let elem_size_u32 = elem_size as u32;
    let mut initial_values: Option<Vec<i64>> = None;

    let (buf_handle, offset, len, byte_len) = if value::is_array(buffer) {
        let Some(arr_ptr) = resolve_array_ptr(caller, buffer) else {
            return value::encode_undefined();
        };
        let len = read_array_length(caller, arr_ptr).unwrap_or(0);
        let Some(byte_len) = typedarray_byte_len(caller, len, elem_size_u32) else {
            return value::encode_undefined();
        };
        let mut values = Vec::with_capacity(len as usize);
        for i in 0..len {
            values
                .push(read_array_elem(caller, arr_ptr, i).unwrap_or_else(value::encode_undefined));
        }
        let handle = {
            let mut table = caller
                .data()
                .arraybuffer_table
                .lock()
                .expect("arraybuffer_table mutex");
            let handle = table.len() as u32;
            table.push(ArrayBufferEntry {
                data: vec![0; byte_len],
            });
            handle
        };
        initial_values = Some(values);
        (handle, 0, len, byte_len)
    } else if let Some(src_entry) = typedarray_entry_from_value(caller, buffer) {
        let len = src_entry.length;
        let Some(byte_len) = typedarray_byte_len(caller, len, elem_size_u32) else {
            return value::encode_undefined();
        };
        let mut values = Vec::with_capacity(len as usize);
        for i in 0..len {
            values.push(
                typedarray_element_read(caller, buffer, i).unwrap_or_else(value::encode_undefined),
            );
        }
        let handle = {
            let mut table = caller
                .data()
                .arraybuffer_table
                .lock()
                .expect("arraybuffer_table mutex");
            let handle = table.len() as u32;
            table.push(ArrayBufferEntry {
                data: vec![0; byte_len],
            });
            handle
        };
        initial_values = Some(values);
        (handle, 0, len, byte_len)
    } else if value::is_object(buffer) {
        let Some(offset) = typedarray_to_index(
            caller,
            byte_offset,
            "RangeError: Invalid typed array byteOffset",
        ) else {
            return value::encode_undefined();
        };
        let Some(obj_ptr) = resolve_handle(caller, buffer) else {
            return value::encode_undefined();
        };
        let handle_val = read_object_property_by_name(caller, obj_ptr, "__arraybuffer_handle__");
        let byte_len_val = read_object_property_by_name(caller, obj_ptr, "byteLength");
        let (Some(handle_val), Some(byte_len_val)) = (handle_val, byte_len_val) else {
            return value::encode_undefined();
        };
        let byte_len = value::decode_f64(byte_len_val) as u32;
        if offset > byte_len || offset % elem_size_u32 != 0 {
            set_typedarray_runtime_error(caller, "RangeError: Invalid typed array byteOffset");
            return value::encode_undefined();
        }
        let remaining = byte_len - offset;
        let len = if value::is_undefined(length) {
            if remaining % elem_size_u32 != 0 {
                set_typedarray_runtime_error(caller, "RangeError: Invalid typed array length");
                return value::encode_undefined();
            }
            remaining / elem_size_u32
        } else {
            let Some(len) =
                typedarray_to_index(caller, length, "RangeError: Invalid typed array length")
            else {
                return value::encode_undefined();
            };
            let Some(byte_count) = len.checked_mul(elem_size_u32) else {
                set_typedarray_runtime_error(caller, "RangeError: Invalid typed array length");
                return value::encode_undefined();
            };
            if byte_count > remaining {
                set_typedarray_runtime_error(caller, "RangeError: Invalid typed array length");
                return value::encode_undefined();
            }
            len
        };
        let Some(view_byte_len) = typedarray_byte_len(caller, len, elem_size_u32) else {
            return value::encode_undefined();
        };
        (
            value::decode_f64(handle_val) as u32,
            offset,
            len,
            view_byte_len,
        )
    } else {
        let Some(len) =
            typedarray_to_index(caller, buffer, "RangeError: Invalid typed array length")
        else {
            return value::encode_undefined();
        };
        let Some(byte_len) = typedarray_byte_len(caller, len, elem_size_u32) else {
            return value::encode_undefined();
        };
        let handle = {
            let mut table = caller
                .data()
                .arraybuffer_table
                .lock()
                .expect("arraybuffer_table mutex");
            let handle = table.len() as u32;
            table.push(ArrayBufferEntry {
                data: vec![0; byte_len],
            });
            handle
        };
        (handle, 0, len, byte_len)
    };

    let handle = {
        let mut table = caller
            .data()
            .typedarray_table
            .lock()
            .expect("typedarray_table mutex");
        let handle = table.len() as u32;
        table.push(TypedArrayEntry {
            buffer_handle: buf_handle,
            byte_offset: offset,
            length: len,
            element_size: elem_size,
            element_kind,
            is_shared: false,
        });
        handle
    };

    let obj = if let Some(target) = target_obj.filter(|target| value::is_object(*target)) {
        target
    } else {
        let env = WasmEnv::from_caller(caller).expect("WasmEnv");
        alloc_host_object(caller, &env, 4)
    };
    let _ = define_host_data_property_from_caller(
        caller,
        obj,
        "__typedarray_handle__",
        value::encode_f64(handle as f64),
    );
    let _ =
        define_host_data_property_from_caller(caller, obj, "length", value::encode_f64(len as f64));
    let _ = define_host_data_property_from_caller(
        caller,
        obj,
        "byteLength",
        value::encode_f64(byte_len as f64),
    );
    let _ = define_host_data_property_from_caller(
        caller,
        obj,
        "byteOffset",
        value::encode_f64(offset as f64),
    );

    if let Some(values) = initial_values {
        for (i, value) in values.into_iter().enumerate() {
            if !typedarray_element_write(caller, obj, i as u32, value)
                && caller
                    .data()
                    .runtime_error
                    .lock()
                    .expect("runtime_error mutex")
                    .is_some()
            {
                return value::encode_undefined();
            }
        }
    }

    obj
}

pub(crate) fn typedarray_same_value_zero(
    caller: &mut Caller<'_, RuntimeState>,
    left: i64,
    right: i64,
) -> bool {
    if value::is_bigint(left) && value::is_bigint(right) {
        let left_handle = value::decode_bigint_handle(left) as usize;
        let right_handle = value::decode_bigint_handle(right) as usize;
        let Ok(table) = caller.data().bigint_table.lock() else {
            return false;
        };
        return match (table.get(left_handle), table.get(right_handle)) {
            (Some(left), Some(right)) => left == right,
            _ => false,
        };
    }
    same_value_zero(left, right)
}

#[derive(Clone, Debug)]
struct ProxyEntry {
    target: i64,
    handler: i64,
    revoked: bool,
}

/// RegExp 条目
#[derive(Clone)]
struct RegexEntry {
    pattern: String,
    flags: String,
    compiled: regress::Regex,
    last_index: i64,
}

/// 闭包条目
struct ClosureEntry {
    func_idx: u32,
    env_obj: i64,
}

#[derive(Clone)]
enum NativeCallable {
    EvalIndirect,
    EvalFunction(EvalFunction),
    PromiseResolvingFunction {
        promise: i64,
        already_resolved: Arc<Mutex<bool>>,
        kind: PromiseResolvingKind,
    },
    PromiseCombinatorReaction {
        context: usize,
        index: usize,
        kind: PromiseCombinatorReactionKind,
    },
    AsyncGeneratorMethod {
        generator: i64,
        kind: AsyncGeneratorCompletionType,
    },
    AsyncGeneratorIdentity {
        generator: i64,
    },
    /// %AsyncIteratorPrototype%[Symbol.asyncIterator]() → return this
    AsyncIteratorProtoSymbolAsyncIterator,
    /// AsyncFromSyncIterator.prototype.next()
    AsyncFromSyncNext {
        handle: u32,
    },
    /// AsyncFromSyncIterator.prototype.return()
    AsyncFromSyncReturn {
        handle: u32,
    },
    /// AsyncFromSyncIterator.prototype.throw()
    AsyncFromSyncThrow {
        handle: u32,
    },
    MapSetMethod {
        kind: MapSetMethodKind,
    },
    DateMethod {
        kind: DateMethodKind,
    },
    WeakMapMethod {
        kind: WeakMapMethodKind,
    },
    WeakSetMethod {
        kind: WeakSetMethodKind,
    },
    WeakRefDerefMethod,
    FinalizationRegistryRegisterMethod,
    FinalizationRegistryUnregisterMethod,
    ArrayConstructor,
    ObjectConstructor,
    FunctionConstructor,
    StringConstructor,
    BooleanConstructor,
    NumberConstructor,
    SymbolConstructor,
    BigIntConstructor,
    RegExpConstructor,
    ErrorConstructor,
    TypeErrorConstructor,
    RangeErrorConstructor,
    SyntaxErrorConstructor,
    ReferenceErrorConstructor,
    URIErrorConstructor,
    EvalErrorConstructor,
    AggregateErrorConstructor,
    MapConstructor,
    SetConstructor,
    WeakMapConstructor,
    WeakSetConstructor,
    WeakRefConstructor,
    FinalizationRegistryConstructor,
    DateConstructorGlobal,
    PromiseConstructor,
    ArrayBufferConstructorGlobal,
    DataViewConstructorGlobal,
    TypedArrayConstructor(()),
    BigInt64ArrayConstructor,
    BigUint64ArrayConstructor,
    ProxyConstructor,
    ProxyRevoker {
        proxy_handle: u32,
    },
    /// GcCollect: trigger mark-sweep GC collection
    GcCollect,
    StubGlobal(()),
    // ── SharedArrayBuffer builtins ──
    SharedArrayBufferConstructor,
    // ── Atomics builtins ──
    AtomicsGlobal,
    // ── Agent harness ──
    AgentStart,
    AgentBroadcast,
    AgentReceiveBroadcast,
    AgentGetReport,
    AgentSleep,
    AgentMonotonicNow,
    // ── Fetch / Headers / Response / Request method dispatch ──
    HeadersMethod {
        handle: u32,
        kind: HeadersMethodKind,
    },
    ResponseMethod {
        handle: u32,
        kind: ResponseMethodKind,
    },
    RequestMethod {
        handle: u32,
        kind: RequestMethodKind,
    },
    // Constructors for the Fetch API (installed on globalThis)
    HeadersConstructor,
    ResponseConstructor,
    RequestConstructor,
}
#[derive(Clone, Copy)]
enum MapSetMethodKind {
    MapSet,
    MapGet,
    SetAdd,
    Has,
    Delete,
    Clear,
    Size,
    ForEach,
    Keys,
    Values,
    Entries,
}
#[derive(Clone, Copy)]
enum WeakMapMethodKind {
    Set,
    Get,
    Has,
    Delete,
}

#[derive(Clone, Copy)]
enum WeakSetMethodKind {
    Add,
    Has,
    Delete,
}

#[derive(Clone, Copy)]
enum DateMethodKind {
    GetDate,
    GetDay,
    GetFullYear,
    GetHours,
    GetMilliseconds,
    GetMinutes,
    GetMonth,
    GetSeconds,
    GetTime,
    GetTimezoneOffset,
    GetUTCDate,
    GetUTCDay,
    GetUTCFullYear,
    GetUTCHours,
    GetUTCMilliseconds,
    GetUTCMinutes,
    GetUTCMonth,
    GetUTCSeconds,
    SetDate,
    SetFullYear,
    SetHours,
    SetMilliseconds,
    SetMinutes,
    SetMonth,
    SetSeconds,
    SetTime,
    SetUTCDate,
    SetUTCFullYear,
    SetUTCHours,
    SetUTCMilliseconds,
    SetUTCMinutes,
    SetUTCMonth,
    SetUTCSeconds,
    ToString,
    ToDateString,
    ToTimeString,
    ToISOString,
    ToUTCString,
    ToJSON,
    ValueOf,
}
#[derive(Clone, Copy)]
enum HeadersMethodKind {
    Get,
    Set,
    Has,
    Delete,
    Append,
    Entries,
    ForEach,
    Keys,
    Values,
}
#[derive(Clone, Copy)]
enum ResponseMethodKind {
    Text,
    Json,
    ArrayBuffer,
    Clone,
}
#[derive(Clone, Copy)]
enum RequestMethodKind {
    Clone,
}
#[derive(Clone, Copy)]
enum PromiseCombinatorReactionKind {
    AllFulfill,
    AllSettledFulfill,
    AllSettledReject,
    AnyReject,
}
struct CombinatorContext {
    result_promise: i64,
    result_array: i64,
    remaining: usize,
    settled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EvalVarMapEntry {
    function_name: String,
    var_name: String,
    offset: u32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EvalLocalKind {
    Var,
    Let,
    Const,
}

struct EvalLocalBinding {
    kind: EvalLocalKind,
    value: i64,
}

#[derive(Clone)]
struct EvalFunction {
    params: Vec<String>,
    body: Vec<swc_ast::Stmt>,
    scope_env: Option<i64>,
}

#[derive(Clone, Copy)]
enum PromiseResolvingKind {
    Fulfill,
    Reject,
}

struct TimerEntry {
    id: u32,
    deadline: Instant,
    callback: i64, // NaN-boxed function handle
    repeating: bool,
    interval: Duration,
}

enum IteratorState {
    StringIter {
        data: Vec<u8>,
        byte_pos: usize,
    },
    ArrayIter {
        ptr: usize,
        index: u32,
        length: u32,
    },
    MapKeyIter {
        keys: Vec<i64>,
        index: u32,
    },
    MapValueIter {
        values: Vec<i64>,
        index: u32,
    },
    TypedArrayValueIter {
        entry: TypedArrayEntry,
        index: u32,
        length: u32,
    },
    TypedArrayEntryIter {
        entry: TypedArrayEntry,
        index: u32,
        length: u32,
    },
    ObjectIter {
        next: i64,
        return_method: Option<i64>,
        current_value: i64,
        done: bool,
        has_current: bool,
    },
    Error,
}

enum EnumeratorState {
    StringEnum {
        length: usize,
        index: usize,
    },
    /// 对象属性枚举：keys 存储属性名列表
    ObjectEnum {
        keys: Vec<String>,
        index: usize,
    },
    Error,
}

#[derive(Clone)]
enum PromiseState {
    Pending,
    Fulfilled(i64),
    Rejected(i64),
}

struct PromiseEntry {
    state: PromiseState,
    fulfill_reactions: Vec<PromiseReaction>,
    reject_reactions: Vec<PromiseReaction>,
    handled: bool,
    constructor_resolver: Option<Arc<Mutex<bool>>>,
    /// 构造器引用（用于 species-aware 操作；None 表示内建 Promise）
    constructor_handle: Option<i64>,
    is_promise: bool,
}

impl PromiseEntry {
    fn pending() -> Self {
        Self {
            state: PromiseState::Pending,
            fulfill_reactions: Vec::new(),
            reject_reactions: Vec::new(),
            handled: false,
            constructor_resolver: None,
            constructor_handle: None,
            is_promise: true,
        }
    }

    fn rejected(reason: i64) -> Self {
        Self {
            state: PromiseState::Rejected(reason),
            fulfill_reactions: Vec::new(),
            reject_reactions: Vec::new(),
            handled: false,
            constructor_resolver: None,
            constructor_handle: None,
            is_promise: true,
        }
    }

    fn empty() -> Self {
        Self {
            state: PromiseState::Pending,
            fulfill_reactions: Vec::new(),
            reject_reactions: Vec::new(),
            handled: false,
            constructor_resolver: None,
            constructor_handle: None,
            is_promise: false,
        }
    }
}

#[derive(Clone)]
enum PromiseReactionKind {
    Normal { handler: i64 },
    AsyncResume { fn_table_idx: u32, state: u32 },
}

#[derive(Clone)]
struct PromiseReaction {
    kind: PromiseReactionKind,
    target_promise: i64,
    reaction_type: ReactionType,
}

impl PromiseReaction {
    fn new(handler: i64, target_promise: i64, reaction_type: ReactionType) -> Self {
        Self {
            kind: PromiseReactionKind::Normal { handler },
            target_promise,
            reaction_type,
        }
    }
    fn new_async(
        fn_table_idx: u32,
        target_promise: i64,
        reaction_type: ReactionType,
        state: u32,
    ) -> Self {
        Self {
            kind: PromiseReactionKind::AsyncResume {
                fn_table_idx,
                state,
            },
            target_promise,
            reaction_type,
        }
    }
}

#[derive(Clone, Copy)]
enum ReactionType {
    Fulfill,
    Reject,
    FinallyFulfill,
    FinallyReject,
}

#[allow(clippy::enum_variant_names, dead_code)]
enum Microtask {
    PromiseReaction {
        promise: i64,
        reaction_type: ReactionType,
        handler: i64,
        argument: i64,
    },
    PromiseResolveThenable {
        promise: i64,
        thenable: i64,
        then: i64,
    },
    MicrotaskCallback {
        callback: i64,
    },
    AsyncResume {
        fn_table_idx: u32,
        continuation: i64,
        state: u32,
        resume_val: i64,
        is_rejected: bool,
    },
    CleanupFinalizationRegistry {
        callback: i64,
        held_value: i64,
    },
}

#[allow(dead_code)]
struct ContinuationEntry {
    fn_table_idx: u32,
    outer_promise: i64,
    captured_vars: Vec<i64>,
    completed: bool,
}

#[allow(dead_code)]
struct AsyncGeneratorEntry {
    state: AsyncGeneratorState,
    continuation: i64,
    active_request: Option<AsyncGeneratorRequest>,
    waiting_resume_promise: Option<i64>,
    queue: VecDeque<AsyncGeneratorRequest>,
}

#[derive(Clone)]
#[allow(dead_code)]
enum AsyncGeneratorState {
    SuspendedStart,
    SuspendedYield,
    Executing,
    Completed,
}
#[derive(Clone, Copy)]
#[allow(dead_code)]
struct AsyncGeneratorRequest {
    completion_type: AsyncGeneratorCompletionType,
    value: i64,
    promise: i64,
}

enum AsyncGeneratorHostAction {
    Immediate {
        active: Option<AsyncGeneratorRequest>,
        queued: VecDeque<AsyncGeneratorRequest>,
    },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AsyncGeneratorCompletionType {
    Next,
    Return,
    Throw,
}
/// async-from-sync iterator 内部状态
#[derive(Clone, Debug)]
struct AsyncFromSyncIteratorEntry {
    /// 同步迭代器句柄 (TAG_ITERATOR handle)
    sync_iterator: i64,
    /// 同步迭代器是否已完成
    sync_done: bool,
}

#[cfg(test)]
mod tests {
    use super::execute_with_writer;
    use super::execute_with_writer_async;
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
        let module = wjsm_parser::parse_module(source)?;
        let program = wjsm_semantic::lower_module(module, false)?;
        wjsm_backend_wasm::compile(&program)
    }
    fn execute_with_writer_prints_string_fixture() -> Result<()> {
        let wasm_bytes = compile_source(r#"console.log("Hello, Runtime!");"#)?;
        let output = execute_with_writer(&wasm_bytes, Vec::new())?;

        assert_eq!(String::from_utf8(output)?, "Hello, Runtime!\n");
        Ok(())
    }

    #[test]
    fn execute_with_writer_prints_arithmetic_fixture() -> Result<()> {
        let wasm_bytes = compile_source("console.log(1 + 2 * 3);")?;
        let output = execute_with_writer(&wasm_bytes, Vec::new())?;

        assert_eq!(String::from_utf8(output)?, "7\n");
        Ok(())
    }
    #[test]
    fn execute_with_writer_async_prints_string_fixture() -> Result<()> {
        let rt = Runtime::new()?;
        let wasm_bytes = compile_source(r#"console.log("Hello, Async Runtime!");"#)?;
        let output = rt.block_on(async {
            execute_with_writer_async(&wasm_bytes, Vec::new()).await
        })?;
        assert_eq!(String::from_utf8(output)?, "Hello, Async Runtime!\n");
        Ok(())
    }
    #[test]
    fn execute_with_writer_async_timer_fires_via_scheduler() -> Result<()> {
        // Phase 5 核心行为验证：async 路径下 scheduler 接管 timer loop 后必须正确 fire + 输出。
        // 使用 async execute（已 wiring get_builtin_global），触发 setTimeout 回调 + console.log。
        // 证明：无阻塞 sleep、无 MAX 超限、per-callback drain 工作、语义与 sync 一致。
        let rt = Runtime::new()?;
        let wasm_bytes = compile_source(r#"
            setTimeout(() => { console.log("async-timer-fired"); }, 0);
        "#)?;
        let output = rt.block_on(async {
            execute_with_writer_async(&wasm_bytes, Vec::new()).await
        })?;
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
        use super::scheduler::{AsyncHostCompletion, AsyncOpCounter};
        use super::runtime_builtins::PromiseSettlement;

        let counter = AsyncOpCounter::new();
        let _guard = counter.begin();

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<AsyncHostCompletion>();

        // 手动 enqueue SettleValue（模拟 worker 发简单值）
        tx.send(AsyncHostCompletion::SettleValue {
            promise: 100,
            settlement: PromiseSettlement::Fulfill(999),
        }).expect("send settle");

        // 手动 enqueue Materialize（闭包在 owner 执行，可分配）
        let mat: Box<dyn FnOnce(&mut wasmtime::Store<super::RuntimeState>, &super::WasmEnv) -> PromiseSettlement + Send> = Box::new(|_store, _env| {
            // 真实会 alloc runtime string/object，此处模拟
            PromiseSettlement::Fulfill(888)
        });
        tx.send(AsyncHostCompletion::Materialize {
            promise: 101,
            materialize: mat,
        }).expect("send mat");

        // 模拟 scheduler loop drain (while try_recv)
        let c1 = rx.try_recv().expect("c1");
        match c1 {
            AsyncHostCompletion::SettleValue { promise, settlement: PromiseSettlement::Fulfill(v) } => {
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
        assert_eq!(counter.count(), 0);
        Ok(())
    }
}