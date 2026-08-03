use anyhow::Result;
use wasmtime::Store;
use wasmtime::{Caller, Func, Linker};

use crate::exec_context_impl::WasmExecContext;
use crate::*;

pub(crate) fn define_collections_buffers(
    linker: &mut Linker<RuntimeState>,
    mut store: &mut Store<RuntimeState>,
) -> Result<()> {
    // ── Map / Set / WeakMap / WeakSet / ArrayBuffer / Date：builtins 算法 ──
    linker.func_wrap_async(
        "env",
        "map_constructor",
        |mut caller: Caller<'_, RuntimeState>, (arg,): (i64,)| {
            Box::new(async move {
                let mut ctx = WasmExecContext::new(&mut caller);
                wjsm_builtins::collections::map_constructor(&mut ctx, arg).await
            })
        },
    )?;
    linker.func_wrap_async(
        "env",
        "set_constructor",
        |mut caller: Caller<'_, RuntimeState>, (arg,): (i64,)| {
            Box::new(async move {
                let mut ctx = WasmExecContext::new(&mut caller);
                wjsm_builtins::collections::set_constructor(&mut ctx, arg).await
            })
        },
    )?;

    macro_rules! wrap2 {
        ($name:expr, $f:path) => {{
            let f = Func::wrap(
                &mut store,
                |mut caller: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
                    let mut ctx = WasmExecContext::new(&mut caller);
                    $f(&mut ctx, a, b)
                },
            );
            linker.define(&mut store, "env", $name, f)?;
        }};
    }
    macro_rules! wrap3 {
        ($name:expr, $f:path) => {{
            let f = Func::wrap(
                &mut store,
                |mut caller: Caller<'_, RuntimeState>, a: i64, b: i64, c: i64| -> i64 {
                    let mut ctx = WasmExecContext::new(&mut caller);
                    $f(&mut ctx, a, b, c)
                },
            );
            linker.define(&mut store, "env", $name, f)?;
        }};
    }
    macro_rules! wrap1 {
        ($name:expr, $f:path) => {{
            let f = Func::wrap(
                &mut store,
                |mut caller: Caller<'_, RuntimeState>, a: i64| -> i64 {
                    let mut ctx = WasmExecContext::new(&mut caller);
                    $f(&mut ctx, a)
                },
            );
            linker.define(&mut store, "env", $name, f)?;
        }};
    }

    wrap3!("map_proto_set", wjsm_builtins::collections::map_proto_set);
    wrap2!("map_proto_get", wjsm_builtins::collections::map_proto_get);
    wrap2!("set_proto_add", wjsm_builtins::collections::set_proto_add);
    wrap2!("set_proto_has", wjsm_builtins::collections::set_proto_has);
    wrap2!("set_proto_delete", wjsm_builtins::collections::set_proto_delete);
    wrap2!("map_set_has", wjsm_builtins::collections::map_set_has);
    wrap2!("map_set_delete", wjsm_builtins::collections::map_set_delete);
    wrap1!("map_set_clear", wjsm_builtins::collections::map_set_clear);
    wrap1!(
        "map_set_get_size",
        wjsm_builtins::collections::map_set_get_size
    );
    linker.func_wrap_async(
        "env",
        "map_set_for_each",
        |mut caller: Caller<'_, RuntimeState>,
         (_env, this_val, args_base, args_count): (i64, i64, i32, i32)| {
            Box::new(async move {
                let mut ctx = WasmExecContext::new(&mut caller);
                wjsm_builtins::collections::map_set_for_each(
                    &mut ctx, this_val, args_base, args_count,
                )
                .await
            })
        },
    )?;
    wrap1!("map_set_keys", wjsm_builtins::collections::map_set_keys);
    wrap1!("map_set_first_key", wjsm_builtins::collections::map_set_first_key);
    wrap1!("map_set_values", wjsm_builtins::collections::map_set_values);
    wrap1!(
        "map_set_entries",
        wjsm_builtins::collections::map_set_entries
    );

    let weakmap_ctor = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, _arg: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::collections::weakmap_constructor(&mut ctx)
        },
    );
    linker.define(&mut store, "env", "weakmap_constructor", weakmap_ctor)?;
    wrap3!(
        "weakmap_proto_set",
        wjsm_builtins::collections::weakmap_proto_set
    );
    wrap2!(
        "weakmap_proto_get",
        wjsm_builtins::collections::weakmap_proto_get
    );
    wrap2!(
        "weakmap_proto_has",
        wjsm_builtins::collections::weakmap_proto_has
    );
    wrap2!(
        "weakmap_proto_delete",
        wjsm_builtins::collections::weakmap_proto_delete
    );

    let weakset_ctor = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, _arg: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::collections::weakset_constructor(&mut ctx)
        },
    );
    linker.define(&mut store, "env", "weakset_constructor", weakset_ctor)?;
    wrap2!(
        "weakset_proto_add",
        wjsm_builtins::collections::weakset_proto_add
    );
    wrap2!(
        "weakset_proto_has",
        wjsm_builtins::collections::weakset_proto_has
    );
    wrap2!(
        "weakset_proto_delete",
        wjsm_builtins::collections::weakset_proto_delete
    );

    wrap1!(
        "arraybuffer_constructor",
        wjsm_builtins::collections::arraybuffer_constructor
    );
    wrap1!(
        "arraybuffer_proto_byte_length",
        wjsm_builtins::collections::arraybuffer_proto_byte_length
    );
    wrap3!(
        "arraybuffer_proto_slice",
        wjsm_builtins::collections::arraybuffer_proto_slice
    );

    let dataview_constructor_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         buffer: i64,
         byte_offset: i64,
         byte_length: i64|
         -> i64 {
            wjsm_builtins::collections_buffers::dataview_constructor(
                &mut WasmExecContext::new(&mut caller),
                buffer,
                byte_offset,
                byte_length,
            )
        },
    );
    linker.define(
        &mut store,
        "env",
        "dataview_constructor",
        dataview_constructor_fn,
    )?;

    macro_rules! dataview_get_fn {
        ($name:ident, $import:literal, $kind:ident) => {
            let $name = Func::wrap(
                &mut store,
                |mut caller: Caller<'_, RuntimeState>, this_value: i64, byte_offset: i64| -> i64 {
                    wjsm_builtins::collections_buffers::dataview_get(
                        &mut WasmExecContext::new(&mut caller),
                        this_value,
                        byte_offset,
                        wjsm_builtins::collections_buffers::DataViewValueKind::$kind,
                    )
                },
            );
            linker.define(&mut store, "env", $import, $name)?;
        };
    }

    dataview_get_fn!(dataview_proto_get_int8_fn, "dataview_proto_get_int8", Int8);
    dataview_get_fn!(dataview_proto_get_uint8_fn, "dataview_proto_get_uint8", Uint8);
    dataview_get_fn!(dataview_proto_get_int16_fn, "dataview_proto_get_int16", Int16);
    dataview_get_fn!(dataview_proto_get_uint16_fn, "dataview_proto_get_uint16", Uint16);
    dataview_get_fn!(dataview_proto_get_int32_fn, "dataview_proto_get_int32", Int32);
    dataview_get_fn!(dataview_proto_get_uint32_fn, "dataview_proto_get_uint32", Uint32);
    dataview_get_fn!(dataview_proto_get_float32_fn, "dataview_proto_get_float32", Float32);
    dataview_get_fn!(dataview_proto_get_float64_fn, "dataview_proto_get_float64", Float64);

    macro_rules! dataview_set_fn {
        ($name:ident, $import:literal, $kind:ident) => {
            let $name = Func::wrap(
                &mut store,
                |mut caller: Caller<'_, RuntimeState>,
                 this_value: i64,
                 byte_offset: i64,
                 raw: i64|
                 -> i64 {
                    wjsm_builtins::collections_buffers::dataview_set(
                        &mut WasmExecContext::new(&mut caller),
                        this_value,
                        byte_offset,
                        raw,
                        wjsm_builtins::collections_buffers::DataViewValueKind::$kind,
                    )
                },
            );
            linker.define(&mut store, "env", $import, $name)?;
        };
    }

    dataview_set_fn!(dataview_proto_set_int8_fn, "dataview_proto_set_int8", Int8);
    dataview_set_fn!(dataview_proto_set_uint8_fn, "dataview_proto_set_uint8", Uint8);
    dataview_set_fn!(dataview_proto_set_int16_fn, "dataview_proto_set_int16", Int16);
    dataview_set_fn!(dataview_proto_set_uint16_fn, "dataview_proto_set_uint16", Uint16);
    dataview_set_fn!(dataview_proto_set_int32_fn, "dataview_proto_set_int32", Int32);
    dataview_set_fn!(dataview_proto_set_uint32_fn, "dataview_proto_set_uint32", Uint32);
    dataview_set_fn!(dataview_proto_set_float32_fn, "dataview_proto_set_float32", Float32);
    dataview_set_fn!(dataview_proto_set_float64_fn, "dataview_proto_set_float64", Float64);
    macro_rules! typedarray_constructor {
        ($name:ident, $import:literal, $size:expr, $kind:expr) => {
            let $name = Func::wrap(
                &mut store,
                |mut caller: Caller<'_, RuntimeState>,
                 buffer: i64,
                 byte_offset: i64,
                 length: i64|
                 -> i64 {
                    typedarray_construct(
                        &mut caller,
                        buffer,
                        byte_offset,
                        length,
                        $size,
                        $kind,
                        None,
                    )
                },
            );
            linker.define(&mut store, "env", $import, $name)?;
        };
    }

    typedarray_constructor!(int8array_constructor_fn, "int8array_constructor", 1, 0);
    typedarray_constructor!(uint8array_constructor_fn, "uint8array_constructor", 1, 1);
    typedarray_constructor!(
        uint8clampedarray_constructor_fn,
        "uint8clampedarray_constructor",
        1,
        2
    );
    typedarray_constructor!(int16array_constructor_fn, "int16array_constructor", 2, 0);
    typedarray_constructor!(uint16array_constructor_fn, "uint16array_constructor", 2, 1);
    typedarray_constructor!(int32array_constructor_fn, "int32array_constructor", 4, 0);
    typedarray_constructor!(uint32array_constructor_fn, "uint32array_constructor", 4, 1);
    typedarray_constructor!(
        float32array_constructor_fn,
        "float32array_constructor",
        4,
        3
    );
    typedarray_constructor!(
        float64array_constructor_fn,
        "float64array_constructor",
        8,
        3
    );
    typedarray_constructor!(
        bigint64array_constructor_fn,
        "bigint64array_constructor",
        8,
        4
    );
    typedarray_constructor!(
        biguint64array_constructor_fn,
        "biguint64array_constructor",
        8,
        5
    );
    macro_rules! typedarray_property {
        ($name:ident, $import:literal, $property:literal) => {
            let $name = Func::wrap(
                &mut store,
                |mut caller: Caller<'_, RuntimeState>, this_value: i64| -> i64 {
                    wjsm_builtins::collections_buffers::typedarray_property(
                        &mut WasmExecContext::new(&mut caller),
                        this_value,
                        $property,
                    )
                },
            );
            linker.define(&mut store, "env", $import, $name)?;
        };
    }
    typedarray_property!(typedarray_proto_length_fn, "typedarray_proto_length", "length");
    typedarray_property!(
        typedarray_proto_byte_length_fn,
        "typedarray_proto_byte_length",
        "byteLength"
    );
    typedarray_property!(
        typedarray_proto_byte_offset_fn,
        "typedarray_proto_byte_offset",
        "byteOffset"
    );

    let typedarray_proto_set_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         this_value: i64,
         source: i64,
         offset: i64|
         -> i64 {
            wjsm_builtins::collections_buffers::typedarray_set(
                &mut WasmExecContext::new(&mut caller),
                this_value,
                source,
                offset,
            )
        },
    );
    linker.define(
        &mut store,
        "env",
        "typedarray_proto_set",
        typedarray_proto_set_fn,
    )?;

    let typedarray_proto_slice_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_value: i64, begin: i64, end: i64| -> i64 {
            wjsm_builtins::collections_buffers::typedarray_slice(
                &mut WasmExecContext::new(&mut caller),
                this_value,
                begin,
                end,
            )
        },
    );
    linker.define(
        &mut store,
        "env",
        "typedarray_proto_slice",
        typedarray_proto_slice_fn,
    )?;

    let typedarray_proto_subarray_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_value: i64, begin: i64, end: i64| -> i64 {
            wjsm_builtins::collections_buffers::typedarray_subarray(
                &mut WasmExecContext::new(&mut caller),
                this_value,
                begin,
                end,
            )
        },
    );
    linker.define(
        &mut store,
        "env",
        "typedarray_proto_subarray",
        typedarray_proto_subarray_fn,
    )?;

    let create_global_object_fn =
        Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>| -> i64 {
            // 单例：嵌套函数入口会再次调用 create_global_object 填充 `$0.$global` local。
            // 若每次新建，globalThis 上的 JS 属性（如 `__wjsm_cluster`）在函数间不可见。
            let existing = caller
                .data()
                .js_global_object
                .load(std::sync::atomic::Ordering::Relaxed);
            if !value::is_undefined(existing) {
                return existing;
            }
            let obj = {
                let _wjsm_env = WasmEnv::from_caller(&mut caller).expect("WasmEnv");
                alloc_host_object(&mut caller, &_wjsm_env, 128)
            };
            let temp_root_len = caller.data().push_host_temp_roots([obj]);
            let builtin_pairs: &[(&str, NativeCallable)] = &[
                ("Array", NativeCallable::ArrayConstructor),
                ("Object", NativeCallable::ObjectConstructor),
                ("Function", NativeCallable::FunctionConstructor),
                ("String", NativeCallable::StringConstructor),
                ("Boolean", NativeCallable::BooleanConstructor),
                ("Number", NativeCallable::NumberConstructor),
                ("Symbol", NativeCallable::SymbolConstructor),
                ("BigInt", NativeCallable::BigIntConstructor),
                ("RegExp", NativeCallable::RegExpConstructor),
                ("Error", NativeCallable::ErrorConstructor),
                ("TypeError", NativeCallable::TypeErrorConstructor),
                ("RangeError", NativeCallable::RangeErrorConstructor),
                ("SyntaxError", NativeCallable::SyntaxErrorConstructor),
                ("ReferenceError", NativeCallable::ReferenceErrorConstructor),
                ("URIError", NativeCallable::URIErrorConstructor),
                ("EvalError", NativeCallable::EvalErrorConstructor),
                ("AggregateError", NativeCallable::AggregateErrorConstructor),
                ("Map", NativeCallable::MapConstructor),
                ("Set", NativeCallable::SetConstructor),
                ("WeakMap", NativeCallable::WeakMapConstructor),
                ("WeakSet", NativeCallable::WeakSetConstructor),
                ("WeakRef", NativeCallable::WeakRefConstructor),
                (
                    "FinalizationRegistry",
                    NativeCallable::FinalizationRegistryConstructor,
                ),
                ("Date", NativeCallable::DateConstructorGlobal),
                ("Promise", NativeCallable::PromiseConstructor),
                ("Headers", NativeCallable::HeadersConstructor),
                ("Request", NativeCallable::RequestConstructor),
                ("Response", NativeCallable::ResponseConstructor),
                ("ReadableStream", NativeCallable::ReadableStreamConstructor),
                ("WritableStream", NativeCallable::WritableStreamConstructor),
                (
                    "TransformStream",
                    NativeCallable::TransformStreamConstructor,
                ),
                (
                    "CountQueuingStrategy",
                    NativeCallable::CountQueuingStrategyConstructor,
                ),
                (
                    "ByteLengthQueuingStrategy",
                    NativeCallable::ByteLengthQueuingStrategyConstructor,
                ),
                (
                    "AbortController",
                    NativeCallable::AbortControllerConstructor,
                ),
                ("ArrayBuffer", NativeCallable::ArrayBufferConstructorGlobal),
                (
                    "SharedArrayBuffer",
                    NativeCallable::SharedArrayBufferConstructor,
                ),
                ("Atomics", NativeCallable::AtomicsGlobal),
                ("DataView", NativeCallable::DataViewConstructorGlobal),
                (
                    "Int8Array",
                    NativeCallable::TypedArrayConstructor(TypedArrayConstructorKind::Int8),
                ),
                (
                    "Uint8Array",
                    NativeCallable::TypedArrayConstructor(TypedArrayConstructorKind::Uint8),
                ),
                (
                    "Uint8ClampedArray",
                    NativeCallable::TypedArrayConstructor(TypedArrayConstructorKind::Uint8Clamped),
                ),
                (
                    "Int16Array",
                    NativeCallable::TypedArrayConstructor(TypedArrayConstructorKind::Int16),
                ),
                (
                    "Uint16Array",
                    NativeCallable::TypedArrayConstructor(TypedArrayConstructorKind::Uint16),
                ),
                (
                    "Int32Array",
                    NativeCallable::TypedArrayConstructor(TypedArrayConstructorKind::Int32),
                ),
                (
                    "Uint32Array",
                    NativeCallable::TypedArrayConstructor(TypedArrayConstructorKind::Uint32),
                ),
                (
                    "Float32Array",
                    NativeCallable::TypedArrayConstructor(TypedArrayConstructorKind::Float32),
                ),
                (
                    "Float64Array",
                    NativeCallable::TypedArrayConstructor(TypedArrayConstructorKind::Float64),
                ),
                (
                    "BigInt64Array",
                    NativeCallable::TypedArrayConstructor(TypedArrayConstructorKind::BigInt64),
                ),
                (
                    "BigUint64Array",
                    NativeCallable::TypedArrayConstructor(TypedArrayConstructorKind::BigUint64),
                ),
                ("Proxy", NativeCallable::ProxyConstructor),
                ("gc", NativeCallable::GcCollect),
            ];

            for (name, callable) in builtin_pairs {
                let mut native_callables = caller
                    .data()
                    .native_callables
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                let idx = native_callables.len() as u32;
                native_callables.push(callable.clone());
                let val = value::encode_native_callable_idx(idx);
                drop(native_callables);
                let _ = define_host_data_property_from_caller(&mut caller, obj, name, val);
                if *name == "Symbol" {
                    crate::symbol_well_known::install_well_known_symbols_on_symbol_constructor(
                        &mut caller,
                        val,
                    );
                }
            }

            let _ = define_host_data_property_from_caller(&mut caller, obj, "globalThis", obj);
            let _ = install_process_global_from_caller(&mut caller, obj);
            let _ =
                crate::runtime_node_globals::install_node_web_globals_from_caller(&mut caller, obj);

            // test262 harness: global `$262` with `.agent` methods
            let agent_obj = {
                let _wjsm_env = WasmEnv::from_caller(&mut caller).expect("WasmEnv");
                alloc_host_object(&mut caller, &_wjsm_env, 7)
            };
            let _ = caller.data().push_host_temp_roots([agent_obj]);
            let agent_methods: &[(&str, NativeCallable)] = &[
                ("start", NativeCallable::AgentStart),
                ("broadcast", NativeCallable::AgentBroadcast),
                ("receiveBroadcast", NativeCallable::AgentReceiveBroadcast),
                ("getReport", NativeCallable::AgentGetReport),
                ("report", NativeCallable::AgentReport),
                ("sleep", NativeCallable::AgentSleep),
                ("monotonicNow", NativeCallable::AgentMonotonicNow),
            ];
            for (name, callable) in agent_methods {
                let mut nc = caller
                    .data()
                    .native_callables
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                let idx = nc.len() as u32;
                nc.push(callable.clone());
                let val = value::encode_native_callable_idx(idx);
                drop(nc);
                let _ = define_host_data_property_from_caller(&mut caller, agent_obj, name, val);
            }
            let harness_obj = {
                let _wjsm_env = WasmEnv::from_caller(&mut caller).expect("WasmEnv");
                alloc_host_object(&mut caller, &_wjsm_env, 1)
            };
            let _ = caller.data().push_host_temp_roots([harness_obj]);
            let _ =
                define_host_data_property_from_caller(&mut caller, harness_obj, "agent", agent_obj);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "$262", harness_obj);

            caller.data().truncate_host_temp_roots(temp_root_len);
            // 永久 root：事件循环回调之间 main local 可能不在栈上。
            caller
                .data()
                .js_global_object
                .store(obj, std::sync::atomic::Ordering::Relaxed);
            obj
        });
    linker.define(
        &mut store,
        "env",
        "create_global_object",
        create_global_object_fn,
    )?;

    let create_exception_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, thrown_value: i64| -> i64 {
            wjsm_builtins::core::create_exception(
                &mut WasmExecContext::new(&mut caller),
                thrown_value,
            )
        },
    );
    linker.define(&mut store, "env", "create_exception", create_exception_fn)?;

    let exception_value_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, exception: i64| -> i64 {
            wjsm_builtins::core::exception_value(
                &mut WasmExecContext::new(&mut caller),
                exception,
            )
        },
    );
    linker.define(&mut store, "env", "exception_value", exception_value_fn)?;

    let date_constructor_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _environment: i64,
         _this_value: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            wjsm_builtins::date::constructor(
                &mut WasmExecContext::new(&mut caller),
                args_base,
                args_count,
            )
        },
    );
    linker.define(&mut store, "env", "date_constructor", date_constructor_fn)?;

    let date_now_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>| -> i64 {
            wjsm_builtins::date::now(&mut WasmExecContext::new(&mut caller))
        },
    );
    linker.define(&mut store, "env", "date_now", date_now_fn)?;

    let performance_now_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>| -> i64 {
            crate::runtime_node_globals::call_performance_now(&mut caller)
        },
    );
    linker.define(&mut store, "env", "performance_now", performance_now_fn)?;

    let date_parse_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, argument: i64| -> i64 {
            wjsm_builtins::date::parse(
                &mut WasmExecContext::new(&mut caller),
                argument,
            )
        },
    );
    linker.define(&mut store, "env", "date_parse", date_parse_fn)?;

    let date_utc_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, args_base: i32, args_count: i32| -> i64 {
            wjsm_builtins::date::utc(
                &mut WasmExecContext::new(&mut caller),
                args_base,
                args_count,
            )
        },
    );
    linker.define(&mut store, "env", "date_utc", date_utc_fn)?;

    super::private_fields::define_private_fields(linker, store)?;
    Ok(())
}
