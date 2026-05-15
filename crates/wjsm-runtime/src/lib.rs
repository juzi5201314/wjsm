use anyhow::Result;
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{self, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use wasmtime::*;

use wjsm_ir::value;

mod types;
mod runtime;
mod host;
mod entry;

pub fn execute(wasm_bytes: &[u8]) -> Result<()> {
    let stdout = io::stdout();
    let _ = execute_with_writer(wasm_bytes, stdout.lock())?;
    Ok(())
}

pub fn execute_with_writer<W: Write>(wasm_bytes: &[u8], writer: W) -> Result<W> {
    use types::*;
    use runtime::*;

    let engine = Engine::default();
    let module = match Module::new(&engine, wasm_bytes) {
        Ok(m) => m,
        Err(e) => {
            return Err(anyhow::anyhow!("WASM validation failed: {:?}", e));
        }
    };
    let output = Arc::new(Mutex::new(Vec::new()));

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
    ]));
    let regex_table: Arc<Mutex<Vec<RegexEntry>>> = Arc::new(Mutex::new(Vec::new()));

    let promise_table: Arc<Mutex<Vec<PromiseEntry>>> = Arc::new(Mutex::new(Vec::new()));
    let microtask_queue: Arc<Mutex<VecDeque<Microtask>>> = Arc::new(Mutex::new(VecDeque::new()));
    let continuation_table: Arc<Mutex<Vec<ContinuationEntry>>> = Arc::new(Mutex::new(Vec::new()));
    let async_generator_table: Arc<Mutex<Vec<AsyncGeneratorEntry>>> =
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

    let gc_mark_bits: Arc<Mutex<Vec<u64>>> = Arc::new(Mutex::new(Vec::new()));
    let alloc_counter: Arc<Mutex<u64>> = Arc::new(Mutex::new(0));
    const GC_THRESHOLD: u64 = 1000;
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
            eval_cache: Arc::clone(&eval_cache),
            bigint_table: Arc::clone(&bigint_table),
            symbol_table: Arc::clone(&symbol_table),
            regex_table: Arc::clone(&regex_table),
            promise_table: Arc::clone(&promise_table),
            microtask_queue: Arc::clone(&microtask_queue),
            continuation_table: Arc::clone(&continuation_table),
            async_generator_table: Arc::clone(&async_generator_table),
            combinator_contexts: Arc::clone(&combinator_contexts),
            module_namespace_cache: Arc::clone(&module_namespace_cache),
            error_table: Arc::clone(&error_table),
            map_table: Arc::clone(&map_table),
            set_table: Arc::clone(&set_table),
            weakmap_table: Arc::clone(&weakmap_table),
            weakset_table: Arc::clone(&weakset_table),
            proxy_table: Arc::clone(&proxy_table),
            arraybuffer_table: Arc::clone(&arraybuffer_table),
            dataview_table: Arc::clone(&dataview_table),
            typedarray_table: Arc::clone(&typedarray_table),
        },
    );

    let imports = entry::build_imports(&mut store);

    let instance = Instance::new(&mut store, &module, &imports)?;

    let main = instance.get_typed_func::<(), ()>(&mut store, "main")?;
    let main_result = main.call(&mut store, ());

    if main_result.is_ok() {
        if let Some(Extern::Table(func_table)) = instance.get_export(&mut store, "__table") {
            if let (
                Some(Extern::Memory(memory)),
                Some(Extern::Global(shadow_sp_global)),
                Some(Extern::Global(heap_ptr_global)),
                Some(Extern::Global(obj_table_ptr_global)),
                Some(Extern::Global(obj_table_count_global)),
            ) = (
                instance.get_export(&mut store, "memory"),
                instance.get_export(&mut store, "__shadow_sp"),
                instance.get_export(&mut store, "__heap_ptr"),
                instance.get_export(&mut store, "__obj_table_ptr"),
                instance.get_export(&mut store, "__obj_table_count"),
            ) {
                drain_microtasks_from_store(
                    &mut store,
                    &func_table,
                    &memory,
                    &shadow_sp_global,
                    &heap_ptr_global,
                    &obj_table_ptr_global,
                    &obj_table_count_global,
                );
            }
        }
    }

    if main_result.is_ok() {
        loop {
            let now = Instant::now();
            let mut _entry_to_fire: Option<TimerEntry> = None;

            {
                let mut timers = store.data().timers.lock().expect("timers mutex");
                let mut cancelled = store
                    .data()
                    .cancelled_timers
                    .lock()
                    .expect("cancelled_timers mutex");

                timers.retain(|t| !cancelled.contains(&t.id));
                cancelled.clear();

                if timers.is_empty() {
                    break;
                }

                if let Some(idx) = timers.iter().position(|t| t.deadline <= now) {
                    _entry_to_fire = Some(timers.remove(idx));
                } else {
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

                let raw_idx = value::decode_function_idx(callback) as u64;
                if let Some(Extern::Table(tbl)) = instance.get_export(&mut store, "__table") {
                    if let Some(Ref::Func(Some(func))) = tbl.get(&mut store, raw_idx) {
                        if let Ok(typed) = func.typed::<(i64, i32, i32), i64>(&store) {
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
                    }
                    if let (
                        Some(Extern::Memory(mem)),
                        Some(Extern::Global(sp_global)),
                        Some(Extern::Global(heap_ptr_global)),
                        Some(Extern::Global(obj_table_ptr_global)),
                        Some(Extern::Global(obj_table_count_global)),
                    ) = (
                        instance.get_export(&mut store, "memory"),
                        instance.get_export(&mut store, "__shadow_sp"),
                        instance.get_export(&mut store, "__heap_ptr"),
                        instance.get_export(&mut store, "__obj_table_ptr"),
                        instance.get_export(&mut store, "__obj_table_count"),
                    ) {
                        drain_microtasks_from_store(
                            &mut store,
                            &tbl,
                            &mem,
                            &sp_global,
                            &heap_ptr_global,
                            &obj_table_ptr_global,
                            &obj_table_count_global,
                        );
                    }
                }

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

    let bytes = output
        .lock()
        .expect("runtime output buffer mutex should not be poisoned")
        .clone();
    drop(store);

    let mut writer = writer;
    writer.write_all(&bytes)?;

    if let Some(message) = runtime_error.lock().expect("runtime error mutex").clone() {
        anyhow::bail!(message);
    }

    main_result?;

    Ok(writer)
}

#[cfg(test)]
mod tests {
    use super::execute_with_writer;
    use anyhow::Result;

    fn compile_source(source: &str) -> Result<Vec<u8>> {
        let module = wjsm_parser::parse_module(source)?;
        let program = wjsm_semantic::lower_module(module)?;
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
