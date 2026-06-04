use anyhow::Result;
use wasmtime::Store;
use wasmtime::{Caller, Linker};

use crate::*;

pub(crate) fn proxy_trap_proxy_entry(
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

pub(crate) fn proxy_trap_handler_trap(
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

pub(crate) fn proxy_trap_property_key_value(
    caller: &mut Caller<'_, RuntimeState>,
    name_id: i32,
) -> i64 {
    let name = read_string(caller, name_id as u32).unwrap_or_default();
    store_runtime_string(caller, name)
}

pub(crate) fn define_proxy_traps(
    _linker: &mut Linker<RuntimeState>,
    _store: &mut Store<RuntimeState>,
) -> Result<()> {
    Ok(())
}
