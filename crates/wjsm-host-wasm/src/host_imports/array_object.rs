use anyhow::Result;
use wasmtime::Store;
use wasmtime::{Caller, Func, Linker};
use wjsm_host::{ExecContext, HeapContext};

use crate::exec_context_impl::WasmExecContext;
use crate::*;


pub(crate) fn define_array_object(
    linker: &mut Linker<RuntimeState>,
    mut store: &mut Store<RuntimeState>,
) -> Result<()> {
    macro_rules! wrap_arr {
        ($name:expr, $f:path) => {{
            let func = Func::wrap(
                &mut store,
                |mut caller: Caller<'_, RuntimeState>,
                 _env: i64,
                 this_val: i64,
                 args_base: i32,
                 args_count: i32|
                 -> i64 {
                    let mut ctx = WasmExecContext::new(&mut caller);
                    $f(&mut ctx, this_val, args_base, args_count)
                },
            );
            linker.define(&mut store, "env", $name, func)?;
        }};
    }
    macro_rules! wrap_arr1 {
        ($name:expr, $f:path) => {{
            let func = Func::wrap(
                &mut store,
                |mut caller: Caller<'_, RuntimeState>,
                 _env: i64,
                 this_val: i64,
                 _args_base: i32,
                 _args_count: i32|
                 -> i64 {
                    let mut ctx = WasmExecContext::new(&mut caller);
                    $f(&mut ctx, this_val)
                },
            );
            linker.define(&mut store, "env", $name, func)?;
        }};
    }

    wrap_arr!(
        "arr_proto_push",
        wjsm_builtins::array_object::arr_proto_push
    );
    wrap_arr1!("arr_proto_pop", wjsm_builtins::array_object::arr_proto_pop);
    wrap_arr!(
        "arr_proto_includes",
        wjsm_builtins::array_object::arr_proto_includes
    );
    wrap_arr!(
        "arr_proto_index_of",
        wjsm_builtins::array_object::arr_proto_index_of_args
    );
    wrap_arr!(
        "arr_proto_join",
        wjsm_builtins::array_object::arr_proto_join_args
    );
    wrap_arr!(
        "arr_proto_concat",
        wjsm_builtins::array_object::arr_proto_concat_args
    );
    wrap_arr!(
        "arr_proto_slice",
        wjsm_builtins::array_object::arr_proto_slice_args
    );
    wrap_arr!(
        "arr_proto_fill",
        wjsm_builtins::array_object::arr_proto_fill_args
    );
    wrap_arr1!(
        "arr_proto_reverse",
        wjsm_builtins::array_object::arr_proto_reverse
    );
    wrap_arr!(
        "arr_proto_flat",
        wjsm_builtins::array_object::arr_proto_flat_args
    );
    wrap_arr1!(
        "arr_proto_shift",
        wjsm_builtins::array_object::arr_proto_shift
    );
    wrap_arr!(
        "arr_proto_unshift",
        wjsm_builtins::array_object::arr_proto_unshift
    );
    wrap_arr!("arr_proto_at", wjsm_builtins::array_object::arr_proto_at);
    wrap_arr!(
        "arr_proto_copy_within",
        wjsm_builtins::array_object::arr_proto_copy_within
    );
    wrap_arr!(
        "arr_proto_splice",
        wjsm_builtins::array_object::arr_proto_splice
    );
    wrap_arr!(
        "arr_proto_last_index_of",
        wjsm_builtins::array_object::arr_proto_last_index_of
    );
    wrap_arr1!(
        "arr_proto_to_reversed",
        wjsm_builtins::array_object::arr_proto_to_reversed
    );
    wrap_arr!(
        "arr_proto_to_spliced",
        wjsm_builtins::array_object::arr_proto_to_spliced
    );
    wrap_arr!(
        "arr_proto_with",
        wjsm_builtins::array_object::arr_proto_with
    );

    let arr_static_of_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env: i64,
         _this: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::array_object::array_of(&mut ctx, args_base, args_count)
        },
    );
    linker.define(&mut store, "env", "arr_static_of", arr_static_of_fn)?;

    let arr_static_is_array_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env: i64,
         _this: i64,
         args_base: i32,
         _args_count: i32|
         -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            let val = ctx.read_shadow_arg(args_base, 0);
            wjsm_builtins::array_object::array_is_array(&mut ctx, val)
        },
    );
    linker.define(
        &mut store,
        "env",
        "arr_static_is_array",
        arr_static_is_array_fn,
    )?;

    // ── ensure_shadow_stack_capacity ──────────────────────────────────
    let ensure_shadow_stack_capacity_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         shadow_sp: i32,
         needed_bytes: i32,
         _stack_end: i32|
         -> i32 {
            let Some(env) = crate::wasm_env::WasmEnv::from_caller(&mut caller) else {
                *caller
                    .data()
                    .runtime_error
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()) =
                    Some("RangeError: Maximum call stack size exceeded (no WasmEnv)".into());
                return 0;
            };
            if crate::runtime_heap::ensure_shadow_stack_capacity(
                &mut caller,
                &env,
                shadow_sp,
                needed_bytes,
            ) {
                1
            } else {
                0
            }
        },
    );
    linker.define(
        &mut store,
        "env",
        "ensure_shadow_stack_capacity",
        ensure_shadow_stack_capacity_fn,
    )?;

    let func_bind_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         func: i64,
         this_val: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::array_object::func_bind(&mut ctx, func, this_val, args_base, args_count)
        },
    );
    linker.define(&mut store, "env", "func_bind", func_bind_fn)?;

    let object_rest_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, source: i64, excluded_keys: i64| -> i64 {
            crate::runtime_values::object_rest_impl(&mut caller, source, excluded_keys)
        },
    );
    linker.define(&mut store, "env", "object_rest", object_rest_fn)?;

    let obj_spread_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, target: i64, source: i64| {
            crate::runtime_values::obj_spread_impl(&mut caller, target, source);
        },
    );
    linker.define(&mut store, "env", "obj_spread", obj_spread_fn)?;

    let has_own_property_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, obj: i64, key_ptr: i32| -> i64 {
            if !value::is_object(obj) && !value::is_function(obj) && !value::is_array(obj) {
                *caller
                    .data()
                    .runtime_error
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()) =
                    Some("TypeError: hasOwnProperty called on non-object".to_string());
                return value::encode_undefined();
            }
            let Some(ptr) = resolve_handle(&mut caller, obj) else {
                return value::encode_bool(false);
            };
            let found = find_property_slot_by_name_id(&mut caller, ptr, key_ptr as u32);
            value::encode_bool(found.is_some())
        },
    );
    linker.define(&mut store, "env", "has_own_property", has_own_property_fn)?;

    let obj_create_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, proto: i64, properties: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::object_builtins::object_create(&mut ctx, proto, properties)
        },
    );
    linker.define(&mut store, "env", "obj_create", obj_create_fn)?;

    let obj_set_proto_of_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, obj: i64, proto: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::object_builtins::object_set_prototype_of(&mut ctx, obj, proto)
        },
    );
    linker.define(&mut store, "env", "obj_set_proto_of", obj_set_proto_of_fn)?;

    let obj_is_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::object_builtins::object_is(&mut ctx, a, b)
        },
    );
    linker.define(&mut store, "env", "obj_is", obj_is_fn)?;

    let obj_proto_to_string_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            ctx.obj_proto_to_string(this_val)
        },
    );
    linker.define(
        &mut store,
        "env",
        "obj_proto_to_string",
        obj_proto_to_string_fn,
    )?;

    let obj_proto_value_of_fn = Func::wrap(
        &mut store,
        |_caller: Caller<'_, RuntimeState>, this_val: i64| -> i64 { this_val },
    );
    linker.define(
        &mut store,
        "env",
        "obj_proto_value_of",
        obj_proto_value_of_fn,
    )?;

    let obj_proto_init_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, obj: i64| -> i64 {
            let to_string =
                crate::create_native_callable(caller.data(), NativeCallable::ObjectProtoToString);
            let value_of =
                crate::create_native_callable(caller.data(), NativeCallable::ObjectProtoValueOf);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "toString", to_string);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "valueOf", value_of);
            obj
        },
    );
    linker.define(&mut store, "env", "obj_proto_init", obj_proto_init_fn)?;

    Ok(())
}
