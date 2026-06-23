use anyhow::Result;
use wasmtime::Store;
use wasmtime::{Caller, Func, Linker};

use crate::*;

pub(crate) fn define_weakref_finalization(
    linker: &mut Linker<RuntimeState>,
    mut store: &mut Store<RuntimeState>,
) -> Result<()> {
    // ── Method factory functions ──
    fn create_weakref_deref_method(state: &RuntimeState) -> i64 {
        let mut table = state
            .native_callables
            .lock()
            .expect("native callable table mutex");
        let handle = table.len() as u32;
        table.push(NativeCallable::WeakRefDerefMethod);
        value::encode_native_callable_idx(handle)
    }

    fn create_fr_register_method(state: &RuntimeState) -> i64 {
        let mut table = state
            .native_callables
            .lock()
            .expect("native callable table mutex");
        let handle = table.len() as u32;
        table.push(NativeCallable::FinalizationRegistryRegisterMethod);
        value::encode_native_callable_idx(handle)
    }

    fn create_fr_unregister_method(state: &RuntimeState) -> i64 {
        let mut table = state
            .native_callables
            .lock()
            .expect("native callable table mutex");
        let handle = table.len() as u32;
        table.push(NativeCallable::FinalizationRegistryUnregisterMethod);
        value::encode_native_callable_idx(handle)
    }

    // ── 1. WeakRef constructor (Type 12: env, this, args_base, args_count) ──
    let weakref_constructor_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env: i64,
         _this: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            if args_count < 1 {
                let msg_val = store_runtime_string(
                    &caller,
                    "TypeError: WeakRef constructor requires a target argument".to_string(),
                );
                let error_obj = create_error_object(&mut caller, "TypeError", msg_val);
                return value::encode_exception(value::decode_object_handle(error_obj));
            }
            let target = read_shadow_arg(&mut caller, args_base, 0);
            // Validate: target must be a JS object (per spec, Type(target) must be Object)
            if !value::is_js_object(target) {
                let msg_val = store_runtime_string(
                    &caller,
                    "TypeError: WeakRef: target must be an object".to_string(),
                );
                let error_obj = create_error_object(&mut caller, "TypeError", msg_val);
                return value::encode_exception(value::decode_object_handle(error_obj));
            }
            // 记录目标在 obj_table 中的 handle 索引（非堆指针）
            let target_handle = match weak_target_handle_index_of(&mut caller, target) {
                Some(h) => h,
                None => {
                    let msg_val = store_runtime_string(
                        &caller,
                        "TypeError: WeakRef: cannot resolve target handle".to_string(),
                    );
                    let error_obj = create_error_object(&mut caller, "TypeError", msg_val);
                    return value::encode_exception(value::decode_object_handle(error_obj));
                }
            };
            // Push a new WeakRef entry and get its index
            let handle;
            {
                let mut table = caller
                    .data()
                    .weakref_table
                    .lock()
                    .expect("weakref table mutex");
                let weakref_index = table.len() as u32;
                table.push(WeakRefEntry { target_handle });
                handle = weakref_index;
            }
            // Create the deref method NativeCallable
            let deref_fn = {
                let state = caller.data();
                create_weakref_deref_method(state)
            };
            // Allocate host object and set the internal handle + deref method
            let obj = {
                let _wjsm_env = WasmEnv::from_caller(&mut caller).expect("WasmEnv");
                alloc_host_object(&mut caller, &_wjsm_env, 2)
            };
            let handle_val = value::encode_f64(handle as f64);
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "__weakref_handle__",
                handle_val,
            );
            let _ = define_host_data_property_from_caller(&mut caller, obj, "deref", deref_fn);
            obj
        },
    );
    linker.define(
        &mut store,
        "env",
        "weakref_constructor",
        weakref_constructor_fn,
    )?;

    // ── 2. WeakRef.prototype.deref (direct: this_val → i64) ──
    let weakref_proto_deref_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64| -> i64 {
            weakref_deref_impl(&mut caller, this_val)
        },
    );
    linker.define(
        &mut store,
        "env",
        "weakref_proto_deref",
        weakref_proto_deref_fn,
    )?;

    // ── 3. FinalizationRegistry constructor (Type 12) ──
    let finalization_registry_constructor_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env: i64,
         _this: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            if args_count < 1 {
                *caller.data().runtime_error.lock().expect("error mutex") = Some(
                    "TypeError: FinalizationRegistry constructor requires a callback argument"
                        .to_string(),
                );
                return value::encode_undefined();
            }
            let callback = read_shadow_arg(&mut caller, args_base, 0);
            // Validate callable
            if !is_callable_in_runtime(&mut caller, callback) {
                *caller.data().runtime_error.lock().expect("error mutex") =
                    Some("TypeError: FinalizationRegistry: callback must be callable".to_string());
                return value::encode_undefined();
            }
            // Allocate host object first to get its VM handle
            let obj = {
                let _wjsm_env = WasmEnv::from_caller(&mut caller).expect("WasmEnv");
                alloc_host_object(&mut caller, &_wjsm_env, 3)
            };
            let object_handle = value::decode_object_handle(obj);
            // Push a new FinalizationRegistry entry, storing the callback alongside
            let handle;
            {
                let mut table = caller
                    .data()
                    .finalization_registry_table
                    .lock()
                    .expect("finalization registry table mutex");
                let registry_index = table.len() as u32;
                table.push(FinalizationRegistryEntry {
                    object_handle,
                    callback,
                    registrations: Vec::new(),
                });
                handle = registry_index;
            }
            // Create method NativeCallables for register/unregister
            let (register_fn, unregister_fn) = {
                let state = caller.data();
                (
                    create_fr_register_method(state),
                    create_fr_unregister_method(state),
                )
            };
            // Store handle + methods on the host object
            let handle_val = value::encode_f64(handle as f64);
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "__finalization_registry_handle__",
                handle_val,
            );
            let _ =
                define_host_data_property_from_caller(&mut caller, obj, "register", register_fn);
            let _ = define_host_data_property_from_caller(
                &mut caller,
                obj,
                "unregister",
                unregister_fn,
            );
            obj
        },
    );
    linker.define(
        &mut store,
        "env",
        "finalization_registry_constructor",
        finalization_registry_constructor_fn,
    )?;

    // ── 4. FinalizationRegistry.prototype.register (Type 12) ──
    let finalization_registry_proto_register_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         _env: i64,
         this_val: i64,
         args_base: i32,
         args_count: i32|
         -> i64 {
            // Need target, heldValue, and optional unregisterToken
            if args_count < 2 {
                return value::encode_undefined();
            }
            let target = read_shadow_arg(&mut caller, args_base, 0);
            let held_value = read_shadow_arg(&mut caller, args_base, 1);
            let unregister_token = if args_count >= 3 {
                let token = read_shadow_arg(&mut caller, args_base, 2);
                // Per spec, unregisterToken must be an object or symbol
                if value::is_js_object(token) || value::is_symbol(token) {
                    Some(token)
                } else {
                    None
                }
            } else {
                None
            };
            // Validate target is a JS object
            if !value::is_js_object(target) {
                return value::encode_undefined();
            }
            // Resolve target handle
            let target_handle = match weak_target_handle_index_of(&mut caller, target) {
                Some(h) => h,
                None => return value::encode_undefined(),
            };
            if !value::is_object(this_val) {
                return value::encode_undefined();
            }
            let obj_ptr =
                resolve_handle_idx(&mut caller, value::decode_object_handle(this_val) as usize);
            let handle_val = obj_ptr.and_then(|p| {
                read_object_property_by_name(&mut caller, p, "__finalization_registry_handle__")
            });
            let Some(handle) = handle_val.map(|v| value::decode_f64(v) as usize) else {
                return value::encode_undefined();
            };
            // Push the registration record
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
        },
    );
    linker.define(
        &mut store,
        "env",
        "finalization_registry_proto_register",
        finalization_registry_proto_register_fn,
    )?;

    // ── 5. FinalizationRegistry.prototype.unregister (direct: this_val, token → i64) ──
    let finalization_registry_proto_unregister_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, token: i64| -> i64 {
            fr_unregister_impl(&mut caller, this_val, token)
        },
    );
    linker.define(
        &mut store,
        "env",
        "finalization_registry_proto_unregister",
        finalization_registry_proto_unregister_fn,
    )?;

    // ── Exports ──
    Ok(())
}
