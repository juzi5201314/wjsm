use anyhow::Result;
use wasmtime::Store;
use wasmtime::{Caller, Func, Linker};

use crate::*;

pub(crate) fn define_proxy_traps(
    linker: &mut Linker<RuntimeState>,
    mut store: &mut Store<RuntimeState>,
) -> Result<()> {
    fn proxy_entry(
        caller: &mut Caller<'_, RuntimeState>,
        proxy: i64,
        op: &str,
    ) -> Option<(i64, i64)> {
        if !value::is_proxy(proxy) {
            set_runtime_error(
                caller.data(),
                format!("TypeError: Proxy internal method {op} called on non-proxy"),
            );
            return None;
        }
        let handle = value::decode_proxy_handle(proxy) as usize;
        let entry = {
            let table = caller.data().proxy_table.lock().expect("proxy_table mutex");
            table.get(handle).cloned()
        }?;
        if entry.revoked {
            set_runtime_error(
                caller.data(),
                format!("TypeError: Cannot perform '{op}' on a proxy that has been revoked"),
            );
            return None;
        }
        Some((entry.target, entry.handler))
    }

    fn handler_trap(
        caller: &mut Caller<'_, RuntimeState>,
        handler: i64,
        trap_name: &str,
    ) -> Option<i64> {
        let ptr = resolve_handle(caller, handler)?;
        let trap = read_object_property_by_name(caller, ptr, trap_name)
            .unwrap_or_else(value::encode_undefined);
        if value::is_undefined(trap) || value::is_null(trap) {
            None
        } else if value::is_callable(trap) {
            Some(trap)
        } else {
            set_runtime_error(
                caller.data(),
                format!("TypeError: Proxy handler trap '{trap_name}' is not callable"),
            );
            None
        }
    }

    fn property_key_value(caller: &mut Caller<'_, RuntimeState>, name_id: i32) -> i64 {
        let name = read_string(caller, name_id as u32).unwrap_or_default();
        store_runtime_string(caller, name)
    }

    fn ordinary_get_by_name_id(
        caller: &mut Caller<'_, RuntimeState>,
        target: i64,
        name_id: i32,
    ) -> i64 {
        if value::is_proxy(target) {
            return proxy_internal_get(caller, target, name_id);
        }
        let Some(ptr) = resolve_handle(caller, target) else {
            return value::encode_undefined();
        };
        read_object_property_by_name_id(caller, ptr, name_id as u32)
            .unwrap_or_else(value::encode_undefined)
    }

    fn ordinary_set_by_name_id(
        caller: &mut Caller<'_, RuntimeState>,
        target: i64,
        name_id: i32,
        val: i64,
    ) -> bool {
        if value::is_proxy(target) {
            proxy_internal_set(caller, target, name_id, val);
            return true;
        }
        let Some(ptr) = resolve_handle(caller, target) else {
            return false;
        };
        write_object_property_by_name_id(
            caller,
            ptr,
            target,
            name_id as u32,
            val,
            constants::FLAG_CONFIGURABLE | constants::FLAG_ENUMERABLE | constants::FLAG_WRITABLE,
        );
        true
    }

    fn call_trap_with_args(
        caller: &mut Caller<'_, RuntimeState>,
        trap: i64,
        this_val: i64,
        args: &[i64],
    ) -> i64 {
        let memory = caller
            .get_export("memory")
            .and_then(|e| e.into_memory())
            .unwrap();
        let shadow_sp_global = caller
            .get_export("__shadow_sp")
            .and_then(|e| e.into_global())
            .unwrap();
        let saved_sp = shadow_sp_global.get(&mut *caller).i32().unwrap();
        let total_size = (args.len() * 8) as i32;
        let new_sp = saved_sp + total_size;
        for (i, &arg) in args.iter().enumerate() {
            memory
                .write(
                    &mut *caller,
                    (saved_sp + i as i32 * 8) as usize,
                    &arg.to_le_bytes(),
                )
                .unwrap();
        }
        shadow_sp_global
            .set(&mut *caller, Val::I32(new_sp))
            .unwrap();
        let result = resolve_and_call(caller, trap, this_val, saved_sp, args.len() as i32);
        shadow_sp_global
            .set(&mut *caller, Val::I32(saved_sp))
            .unwrap();
        result
    }

    fn proxy_internal_get(caller: &mut Caller<'_, RuntimeState>, proxy: i64, name_id: i32) -> i64 {
        let Some((target, handler)) = proxy_entry(caller, proxy, "get") else {
            return value::encode_undefined();
        };
        if let Some(trap) = handler_trap(caller, handler, "get") {
            let prop = property_key_value(caller, name_id);
            return call_trap_with_args(caller, trap, handler, &[target, prop, proxy]);
        }
        ordinary_get_by_name_id(caller, target, name_id)
    }

    fn proxy_internal_set(
        caller: &mut Caller<'_, RuntimeState>,
        proxy: i64,
        name_id: i32,
        val: i64,
    ) {
        let Some((target, handler)) = proxy_entry(caller, proxy, "set") else {
            return;
        };
        if let Some(trap) = handler_trap(caller, handler, "set") {
            let prop = property_key_value(caller, name_id);
            let result = call_trap_with_args(caller, trap, handler, &[target, prop, val, proxy]);
            if !nanbox_to_bool(result) {
                set_runtime_error(
                    caller.data(),
                    "TypeError: Proxy set trap returned falsy".to_string(),
                );
            }
            return;
        }
        let _ = ordinary_set_by_name_id(caller, target, name_id, val);
    }

    fn proxy_internal_delete(
        caller: &mut Caller<'_, RuntimeState>,
        proxy: i64,
        name_id: i32,
    ) -> i64 {
        let Some((target, handler)) = proxy_entry(caller, proxy, "deleteProperty") else {
            return value::encode_bool(false);
        };
        if let Some(trap) = handler_trap(caller, handler, "deleteProperty") {
            let prop = property_key_value(caller, name_id);
            let result = call_trap_with_args(caller, trap, handler, &[target, prop]);
            return value::encode_bool(nanbox_to_bool(result));
        }
        value::encode_bool(true)
    }

    let proxy_trap_get = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, proxy: i64, name_id: i32| -> i64 {
            proxy_internal_get(&mut caller, proxy, name_id)
        },
    );
    linker.define(&mut store, "env", "proxy_trap_get", proxy_trap_get)?;

    let proxy_trap_set = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, proxy: i64, name_id: i32, val: i64| {
            proxy_internal_set(&mut caller, proxy, name_id, val);
        },
    );
    linker.define(&mut store, "env", "proxy_trap_set", proxy_trap_set)?;

    let proxy_trap_delete = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, proxy: i64, name_id: i32| -> i64 {
            proxy_internal_delete(&mut caller, proxy, name_id)
        },
    );
    linker.define(&mut store, "env", "proxy_trap_delete", proxy_trap_delete)?;

    Ok(())
}
