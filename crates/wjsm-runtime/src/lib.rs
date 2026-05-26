use anyhow::Result;
use chrono::{DateTime, Datelike, Local, TimeZone, Timelike, Utc};
use num_traits::cast::ToPrimitive;
use rand::Rng;
use std::cell::Cell;
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::io::{self, Write};
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};
use swc_core::ecma::ast as swc_ast;
use wasmtime::*;
/// 影子栈大小（必须与后端保持一致）
const SHADOW_STACK_SIZE: u32 = 65536;

use wjsm_ir::{constants, value};
mod runtime_builtins;
mod runtime_eval;
mod runtime_heap;
mod runtime_host_helpers;
mod runtime_arguments;
mod runtime_promises;
mod runtime_render;
mod runtime_values;
mod wasm_env;
pub(crate) use wasm_env::WasmEnv;

use runtime_builtins::*;
use runtime_arguments::*;
use runtime_eval::*;
use runtime_heap::*;
use runtime_host_helpers::*;
use runtime_promises::*;
use runtime_render::*;
use runtime_values::*;

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
    let output = Arc::new(Mutex::new(Vec::new()));

    // Iterator/enumerator side tables
    let iterators: Arc<Mutex<Vec<IteratorState>>> = Arc::new(Mutex::new(Vec::new()));
    let enumerators: Arc<Mutex<Vec<EnumeratorState>>> = Arc::new(Mutex::new(Vec::new()));
    let runtime_strings: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let runtime_error: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let timers: Arc<Mutex<Vec<TimerEntry>>> = Arc::new(Mutex::new(Vec::new()));
    let cancelled_timers: Arc<Mutex<HashSet<u32>>> = Arc::new(Mutex::new(HashSet::new()));
    let next_timer_id: Arc<Mutex<u32>> = Arc::new(Mutex::new(1));
    let closures: Arc<Mutex<Vec<ClosureEntry>>> = Arc::new(Mutex::new(Vec::new()));
    let bound_objects: Arc<Mutex<Vec<BoundRecord>>> = Arc::new(Mutex::new(Vec::new()));
    let native_callables: Arc<Mutex<Vec<NativeCallable>>> =
        Arc::new(Mutex::new(vec![NativeCallable::EvalIndirect]));
    let eval_cache: Arc<Mutex<HashMap<u64, Vec<u8>>>> = Arc::new(Mutex::new(HashMap::new()));

    let bigint_table: Arc<Mutex<Vec<num_bigint::BigInt>>> = Arc::new(Mutex::new(Vec::new()));
    let symbol_table: Arc<Mutex<Vec<SymbolEntry>>> = Arc::new(Mutex::new(vec![
        // 预分配 well-known symbols（id=0..7，对应 ECMAScript § 6.1.5.1）
        // 这些 symbol 不属于全局注册表（global_key = None），仅通过 Symbol.wellKnown 访问
        SymbolEntry {
            description: Some("Symbol(Symbol.iterator)".into()),
            global_key: None,
        }, // 0 = @@iterator
        SymbolEntry {
            description: Some("Symbol(Symbol.species)".into()),
            global_key: None,
        }, // 1 = @@species
        SymbolEntry {
            description: Some("Symbol(Symbol.toStringTag)".into()),
            global_key: None,
        }, // 2 = @@toStringTag
        SymbolEntry {
            description: Some("Symbol(Symbol.asyncIterator)".into()),
            global_key: None,
        }, // 3 = @@asyncIterator
        SymbolEntry {
            description: Some("Symbol(Symbol.hasInstance)".into()),
            global_key: None,
        }, // 4 = @@hasInstance
        SymbolEntry {
            description: Some("Symbol(Symbol.toPrimitive)".into()),
            global_key: None,
        }, // 5 = @@toPrimitive
        SymbolEntry {
            description: Some("Symbol(Symbol.dispose)".into()),
            global_key: None,
        }, // 6 = @@dispose
        SymbolEntry {
            description: Some("Symbol(Symbol.match)".into()),
            global_key: None,
        }, // 7 = @@match
        SymbolEntry {
            description: Some("Symbol(Symbol.asyncDispose)".into()),
            global_key: None,
        }, // 8 = @@asyncDispose
    ]));
    let regex_table: Arc<Mutex<Vec<RegexEntry>>> = Arc::new(Mutex::new(Vec::new()));

    let promise_table: Arc<Mutex<Vec<PromiseEntry>>> = Arc::new(Mutex::new(Vec::new()));
    let pending_unhandled_rejections: Arc<Mutex<HashSet<usize>>> =
        Arc::new(Mutex::new(HashSet::new()));
    let native_callable_free_slots: Arc<Mutex<Vec<u32>>> = Arc::new(Mutex::new(Vec::new()));
    let non_extensible_handles: Arc<Mutex<HashSet<u64>>> = Arc::new(Mutex::new(HashSet::new()));
    let microtask_queue: Arc<Mutex<VecDeque<Microtask>>> = Arc::new(Mutex::new(VecDeque::new()));
    let continuation_table: Arc<Mutex<Vec<ContinuationEntry>>> = Arc::new(Mutex::new(Vec::new()));
    let async_generator_table: Arc<Mutex<Vec<AsyncGeneratorEntry>>> =
        Arc::new(Mutex::new(Vec::new()));
    let async_from_sync_iterators: Arc<Mutex<Vec<AsyncFromSyncIteratorEntry>>> =
        Arc::new(Mutex::new(Vec::new()));
    let combinator_contexts: Arc<Mutex<Vec<CombinatorContext>>> = Arc::new(Mutex::new(Vec::new()));
    let module_namespace_cache: Arc<Mutex<HashMap<u32, i64>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let error_table: Arc<Mutex<Vec<ErrorEntry>>> = Arc::new(Mutex::new(Vec::new()));
    let map_table: Arc<Mutex<Vec<MapEntry>>> = Arc::new(Mutex::new(Vec::new()));
    let set_table: Arc<Mutex<Vec<SetEntry>>> = Arc::new(Mutex::new(Vec::new()));
    let weakmap_table: Arc<Mutex<Vec<WeakMapEntry>>> = Arc::new(Mutex::new(Vec::new()));
    let weakset_table: Arc<Mutex<Vec<WeakSetEntry>>> = Arc::new(Mutex::new(Vec::new()));
    let proxy_table: Arc<Mutex<Vec<ProxyEntry>>> = Arc::new(Mutex::new(Vec::new()));
    let arraybuffer_table: Arc<Mutex<Vec<ArrayBufferEntry>>> = Arc::new(Mutex::new(Vec::new()));
    let dataview_table: Arc<Mutex<Vec<DataViewEntry>>> = Arc::new(Mutex::new(Vec::new()));
    let typedarray_table: Arc<Mutex<Vec<TypedArrayEntry>>> = Arc::new(Mutex::new(Vec::new()));

    // GC 相关状态
    let gc_mark_bits: Arc<Mutex<Vec<u64>>> = Arc::new(Mutex::new(Vec::new()));
    let alloc_counter: Arc<Mutex<u64>> = Arc::new(Mutex::new(0));
    const GC_THRESHOLD: u64 = 1000; // 每 1000 次分配触发一次 GC 检查
    let weakref_table: Arc<Mutex<Vec<WeakRefEntry>>> = Arc::new(Mutex::new(Vec::new()));
    let finalization_registry_table: Arc<Mutex<Vec<FinalizationRegistryEntry>>> = Arc::new(Mutex::new(Vec::new()));
    let pending_cleanup_callbacks: Arc<Mutex<Vec<(i64, Vec<i64>)>>> = Arc::new(Mutex::new(Vec::new()));
    let mut store = Store::new(
        &engine,
        RuntimeState {
            output: Arc::clone(&output),
            iterators: Arc::clone(&iterators),
            enumerators: Arc::clone(&enumerators),
            runtime_strings: Arc::clone(&runtime_strings),
            runtime_error: Arc::clone(&runtime_error),
            timers: Arc::clone(&timers),
            cancelled_timers: Arc::clone(&cancelled_timers),
            next_timer_id: Arc::clone(&next_timer_id),
            gc_mark_bits: Arc::clone(&gc_mark_bits),
            alloc_counter: Arc::clone(&alloc_counter),
            gc_threshold: GC_THRESHOLD,
            closures: Arc::clone(&closures),
            bound_objects: Arc::clone(&bound_objects),
            native_callables: Arc::clone(&native_callables),
            native_callable_free_slots: Arc::clone(&native_callable_free_slots),
            eval_cache: Arc::clone(&eval_cache),
            bigint_table: Arc::clone(&bigint_table),
            symbol_table: Arc::clone(&symbol_table),
            regex_table: Arc::clone(&regex_table),
            promise_table: Arc::clone(&promise_table),
            pending_unhandled_rejections: Arc::clone(&pending_unhandled_rejections),
            microtask_queue: Arc::clone(&microtask_queue),
            continuation_table: Arc::clone(&continuation_table),
            async_generator_table: Arc::clone(&async_generator_table),
            async_from_sync_iterators: Arc::clone(&async_from_sync_iterators),
            async_iterator_prototype: value::encode_undefined(), // set after store creation
            async_gen_prototype: value::encode_undefined(),      // set after store creation
            combinator_contexts: Arc::clone(&combinator_contexts),
            module_namespace_cache: Arc::clone(&module_namespace_cache),
            error_table: Arc::clone(&error_table),
            map_table: Arc::clone(&map_table),
            set_table: Arc::clone(&set_table),
            weakmap_table: Arc::clone(&weakmap_table),
            weakset_table: Arc::clone(&weakset_table),
            weakref_table: Arc::clone(&weakref_table),
            finalization_registry_table: Arc::clone(&finalization_registry_table),
            pending_cleanup_callbacks: Arc::clone(&pending_cleanup_callbacks),
            proxy_table: Arc::clone(&proxy_table),
            arraybuffer_table: Arc::clone(&arraybuffer_table),
            dataview_table: Arc::clone(&dataview_table),
            typedarray_table: Arc::clone(&typedarray_table),
            shared_state: Some(Arc::new(SharedRuntimeState {
                sab_table: Arc::new(Mutex::new(Vec::new())),
                agent_state: Arc::new(AgentState {
                    reports: Arc::new(Mutex::new(Vec::new())),
                    waiters: Arc::new(Mutex::new(HashMap::new())),
                }),
            })),
            non_extensible_handles: Arc::clone(&non_extensible_handles),
            scope_records: HashMap::new(),
            scope_record_next_handle: 0,
            new_target: Cell::new(value::encode_undefined()),
        },
    );

    // ── Import 0: console_log(i64) → () ─────────────────────────────────
    let mut imports: Vec<Extern> = Vec::with_capacity(381);
    imports.extend(include!("host_imports/core.rs"));
    imports.extend(include!("host_imports/timers_arrays.rs"));
    imports.extend(include!("host_imports/array_object.rs"));
    imports.extend(include!("host_imports/primitive_core.rs"));
    imports.extend(include!("host_imports/promise_async.rs"));
    imports.extend(include!("host_imports/string_methods.rs"));
    imports.extend(include!("host_imports/math_number_error.rs"));
    imports.extend(include!("host_imports/collections_buffers.rs"));
    // ── Proxy traps (imports 318-320) ──
    imports.extend(include!("host_imports/proxy_traps.rs"));
    // Import 321: get_builtin_global
    imports.push(include!("host_imports/get_builtin_global_entry.rs"));
    // Import 322: new_target
    let new_target_fn = Func::wrap(
        &mut store,
        |caller: Caller<'_, RuntimeState>, _dummy: i64| -> i64 { caller.data().new_target.get() },
    );
    imports.push(new_target_fn.into());
    // Import 323: new_target_set
    let new_target_set_fn = Func::wrap(
        &mut store,
        |caller: Caller<'_, RuntimeState>, new_target: i64| -> i64 {
            let previous = caller.data().new_target.get();
            caller.data().new_target.set(new_target);
            previous
        },
    );
    imports.push(new_target_set_fn.into());
    // Import 324: create_unmapped_arguments_object: (i64, i64) -> i64
    let create_unmapped_arguments_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, args_array: i64, param_count: i64| -> i64 {
            create_unmapped_arguments_object(&mut caller, args_array, param_count)
        },
    );
    imports.push(create_unmapped_arguments_fn.into());
    // Import 325: create_mapped_arguments_object: (i64, i64, i64) -> i64
    let create_mapped_arguments_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, args_array: i64, param_count: i64, func_ref: i64| -> i64 {
            create_mapped_arguments_object(&mut caller, args_array, param_count, func_ref)
        },
    );
    imports.push(create_mapped_arguments_fn.into());
    // ── TypedArray extra methods (imports 326-347) ──
    imports.extend(include!("host_imports/typedarray_new_methods.rs"));
    // ── ScopeRecord eval bridge (imports 348-355) ──
    let scope_record_create_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, capacity: i64| -> i64 {
            scope_record_create(caller, capacity)
        },
    );
    imports.push(scope_record_create_fn.into());
    let scope_record_add_binding_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, record: i64, name: i64, val: i64, is_tdz: i64, is_const: i64| {
            scope_record_add_binding(caller, record, name, val, is_tdz, is_const)
        },
    );
    imports.push(scope_record_add_binding_fn.into());
    let eval_get_binding_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, record: i64, name: i64| -> i64 {
            eval_get_binding(caller, record, name)
        },
    );
    imports.push(eval_get_binding_fn.into());
    let eval_set_binding_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, record: i64, name: i64, val: i64| -> i64 {
            eval_set_binding(caller, record, name, val)
        },
    );
    imports.push(eval_set_binding_fn.into());
    let eval_has_binding_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, record: i64, name: i64| -> i64 {
            eval_has_binding(caller, record, name)
        },
    );
    imports.push(eval_has_binding_fn.into());
    let eval_super_base_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, record: i64| -> i64 {
            eval_super_base(caller, record)
        },
    );
    imports.push(eval_super_base_fn.into());
    let scope_record_set_meta_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, record: i64, key: i64, val: i64| {
            scope_record_set_meta(caller, record, key, val)
        },
    );
    imports.push(scope_record_set_meta_fn.into());
    let scope_record_destroy_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, record: i64| {
            scope_record_destroy(caller, record)
        },
    );
    imports.push(scope_record_destroy_fn.into());
    // ── WeakRef / FinalizationRegistry (imports 356-360) ──
    imports.extend(include!("host_imports/weakref_finalization.rs"));
    // ── SharedArrayBuffer + Atomics (imports 361-377) ──
    imports.extend(include!("host_imports/atomics.rs"));
    // ── Import 378: async_iterator_from(i64) -> i64 ────────────────────────
    let async_iterator_from_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, iterable: i64| -> i64 {
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
                        let mut iters = caller.data().iterators.lock()
                            .expect("iterators mutex");
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
                        call_native_callable_with_args_from_caller(&mut caller, method, iterable, vec![])
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
                            let next =
                                read_object_property_by_name(&mut caller, iter_ptr, "next")
                                    .filter(|n| value::is_callable(*n));
                            if let Some(next_fn) = next {
                                let return_method =
                                    read_object_property_by_name(
                                        &mut caller,
                                        iter_ptr,
                                        "return",
                                    )
                                    .filter(|c| value::is_callable(*c));
                                let mut iters = caller
                                    .data()
                                    .iterators
                                    .lock()
                                    .expect("iterators mutex");
                                let handle = iters.len() as u32;
                                iters.push(IteratorState::ObjectIter {
                                    next: next_fn,
                                    return_method,
                                    current_value: value::encode_undefined(),
                                    has_current: false,
                                    done: false,
                                });
                                return value::encode_handle(
                                    value::TAG_ITERATOR,
                                    handle,
                                );
                            }
                        }
                    }
                } else if !value::is_undefined(method) && !value::is_null(method) {
                    // C4: @@asyncIterator 存在但不可调用 → TypeError（ES §7.3.10）
                    return create_error_object(
                        &mut caller,
                        "TypeError",
                        value::encode_undefined(),
                    );
                }
            }

            // 回退到 @@iterator
            if let Some(method) =
                read_object_property_by_name(&mut caller, ptr, "Symbol.iterator")
            {
                if value::is_callable(method) {
                    let sync_iter = if value::is_native_callable(method) {
                        call_native_callable_with_args_from_caller(&mut caller, method, iterable, vec![])
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
                                    read_object_property_by_name(
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
                                        next: next_fn,
                                        return_method,
                                        current_value: value::encode_undefined(),
                                        has_current: false,
                                        done: false,
                                    });
                                    value::encode_handle(
                                        value::TAG_ITERATOR,
                                        sync_handle,
                                    )
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

            // 没有 @@asyncIterator 也没有 @@iterator，创建 TypeError
            create_error_object(&mut caller, "TypeError", value::encode_undefined())
        },
    );
    imports.push(async_iterator_from_fn.into());
    // ── Import 379-380: Object.groupBy / Map.groupBy ─────────────────────
    let object_group_by_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, items: i64, callbackfn: i64| -> i64 {
            // Check items is not null/undefined
            if value::is_null(items) || value::is_undefined(items) {
                *caller
                    .data()
                    .runtime_error
                    .lock()
                    .expect("runtime error mutex") =
                    Some("TypeError: Cannot group null or undefined".to_string());
                return value::encode_undefined();
            }

            // Check callback is callable
            if !value::is_callable(callbackfn) {
                *caller
                    .data()
                    .runtime_error
                    .lock()
                    .expect("runtime error mutex") =
                    Some("TypeError: callbackfn is not callable".to_string());
                return value::encode_undefined();
            }

            // Create null-prototype result object
            let result = alloc_object(&mut caller, 0);

            // Use HashMap to collect groups, avoiding mid-loop object property manipulation
            let mut groups: HashMap<String, Vec<i64>> = HashMap::new();

            // Array fast path
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

                        // ToPropertyKey: convert key to string for object property
                        let key_str = to_property_key(&mut caller, key);
                        if caller.data().runtime_error.lock().expect("mutex").is_some() {
                            return value::encode_undefined();
                        }

                        groups.entry(key_str).or_default().push(elem);
                        index += 1;
                    }

                    // After collection, create arrays and define properties on result object
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
            } else {
                // Not an array or failed to resolve — fall through to return result
            }

            // TODO: General iterable support (non-array iterables)
            result
        },
    );
    imports.push(object_group_by_fn.into());

    let map_group_by_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, items: i64, callbackfn: i64| -> i64 {
            // Check items is not null/undefined
            if value::is_null(items) || value::is_undefined(items) {
                *caller
                    .data()
                    .runtime_error
                    .lock()
                    .expect("runtime error mutex") =
                    Some("TypeError: Cannot group null or undefined".to_string());
                return value::encode_undefined();
            }

            // Check callback is callable
            if !value::is_callable(callbackfn) {
                *caller
                    .data()
                    .runtime_error
                    .lock()
                    .expect("runtime error mutex") =
                    Some("TypeError: callbackfn is not callable".to_string());
                return value::encode_undefined();
            }

            // Create Map internal state
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

            // Use Vec of (key, elements) pairs to collect groups, avoiding mid-loop WASM manipulation
            // Also use HashMap for O(1) key lookup while maintaining insertion order
            let mut groups: Vec<(i64, Vec<i64>)> = Vec::new();
            let mut key_to_index: HashMap<i64, usize> = HashMap::new();

            // Array fast path
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

                        // Look up key in groups using HashMap for O(1) average case
                        // Fall back to linear search for SameValueZero edge cases (e.g., NaN)
                        let group_index = if let Some(&idx) = key_to_index.get(&key) {
                            // Verify with SameValueZero to handle edge cases
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
                            // Linear search for edge cases or if HashMap miss
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

                    // After collection, create arrays and populate Map
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
                            .expect("map table mutex");
                        table[map_handle].keys.push(*group_key);
                        table[map_handle].values.push(arr);
                    }
                }
            }

            map_result
        },
    );
    imports.push(map_group_by_fn.into());
    // ── Import 381: symbol_property_key(i64) -> i32 ───────────────────
    let symbol_property_key_fn = Func::wrap(
        &mut store,
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
    imports.push(symbol_property_key_fn.into());
    // ── Import 382: array.from ──
    let array_from_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env_obj: i64, _this_val: i64, args_base: i32, args_count: i32|
         -> i64 {
            if args_count < 1 { return value::encode_undefined(); }
            let memory = caller.get_export("memory").and_then(|e| e.into_memory()).unwrap();
            let mut buf = [0u8; 8];
            let _ = memory.read(&mut caller, args_base as usize, &mut buf);
            let source = i64::from_le_bytes(buf);
            if value::is_iterator(source) {
                let handle_idx = value::decode_handle(source) as usize;
                let mut values = Vec::new();
                loop {
                    let mut done = true;
                    { let mut iters = caller.data().iterators.lock().expect("iters");
                      if let Some(iter) = iters.get_mut(handle_idx) {
                        match iter {
                            IteratorState::MapKeyIter { keys, index } => {
                                if (*index as usize) < keys.len() { values.push(keys[*index as usize]); *index += 1; done = false; }
                            }
                            IteratorState::MapValueIter { values: v, index } => {
                                if (*index as usize) < v.len() { values.push(v[*index as usize]); *index += 1; done = false; }
                            }
                            _ => {}
                        }
                    } }
                    if done { break; }
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
            if value::is_array(source) { return source; }
            value::encode_undefined()
        },
    );
    imports.push(array_from_fn.into());
    // ── Import 383: obj_get_by_index(i64, i32) -> i64 ────────────────────
    let obj_get_by_index_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, boxed: i64, index: i32| -> i64 {
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
    imports.push(obj_get_by_index_fn.into());
    let instance = Instance::new(&mut store, &module, &imports)?;
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
    let obj_ptr = resolve_handle_idx_with_env(
        &mut store,
        &wasm_env,
        async_gen_handle as usize,
    )
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
                    let mut buffer = store
                        .data()
                        .output
                        .lock()
                        .expect("output mutex");
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
                ).ok();
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

                // Call the callback via WASM function table call_indirect
                let raw_idx = value::decode_function_idx(callback) as u64;
                if let Some(Extern::Table(tbl)) = instance.get_export(&mut store, "__table") {
                    if let Some(Ref::Func(Some(func))) = tbl.get(&mut store, raw_idx)
                        && let Ok(typed) = func.typed::<(i64, i32, i32), i64>(&store)
                    {
                        match typed.call(&mut store, (value::encode_undefined(), 0i32, 0i32)) {
                            Ok(_) => {}
                            Err(e) => {
                                let msg = format!("timer callback error: {}", e);
                                let mut error_lock = store
                                    .data()
                                    .runtime_error
                                    .lock()
                                    .expect("runtime_error mutex");
                                if error_lock.is_none() {
                                    *error_lock = Some(msg);
                                }
                                break;
                            }
                        }
                    }
                    // Drain microtasks after timer callback
                    drain_microtasks(&mut store, &wasm_env);
                }

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
    /// new.target 值元属性
    new_target: Cell<i64>,
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
struct TypedArrayEntry {
    buffer_handle: u32,
    byte_offset: u32,
    length: u32,
    element_size: u8,
    /// 0=Int, 1=Uint, 2=Clamped, 3=Float
    element_kind: u8,
    is_shared: bool,
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
    use anyhow::Result;

    fn compile_source(source: &str) -> Result<Vec<u8>> {
        let module = wjsm_parser::parse_module(source)?;
        let program = wjsm_semantic::lower_module(module, false)?;
        wjsm_backend_wasm::compile(&program)
    }

    #[test]
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
    fn execute_direct_eval_updates_shadow_backed_var() -> Result<()> {
        let wasm_bytes = compile_source(
            r#"
            var x = 1;
            console.log(eval("x = 4"));
            console.log(x);
            "#,
        )?;
        let output = execute_with_writer(&wasm_bytes, Vec::new())?;

        assert_eq!(String::from_utf8(output)?, "4\n4\n");
        Ok(())
    }
}
