use anyhow::Result;
use wasmtime::Store;
use wasmtime::{Caller, Linker};

use crate::*;

/// 是否为已撤销的代理。供返回 bool/Result（无法回传 TAG_EXCEPTION）的内部方法在其
/// Reflect 入口处提前判定，从而返回可捕获的 TypeError。
pub(crate) fn proxy_is_revoked(caller: &mut Caller<'_, RuntimeState>, value: i64) -> bool {
    if !value::is_proxy(value) {
        return false;
    }
    let handle = value::decode_proxy_handle(value) as usize;
    caller
        .data()
        .proxy_table.lock().unwrap_or_else(|e| e.into_inner())
        .get(handle)
        .map(|entry| entry.revoked)
        .unwrap_or(false)
}

/// 解析 proxy 的 (target, handler)。撤销代理或非代理 → 返回 `Err(TAG_EXCEPTION)`，
/// 调用方（返回 i64 的 get/delete 等）应直接返回该异常值，从而经语义层 IsException
/// 分叉被 try/catch 同步捕获。返回 void 的 set 路径无法回传异常值，由其自行降级处理。
pub(crate) fn proxy_trap_proxy_entry(
    caller: &mut Caller<'_, RuntimeState>,
    proxy: i64,
    op: &str,
) -> Result<(i64, i64), i64> {
    if !value::is_proxy(proxy) {
        let exc = make_type_error_exception(
            caller,
            &format!("TypeError: Proxy internal method {op} called on non-proxy"),
        );
        return Err(exc);
    }
    let handle = value::decode_proxy_handle(proxy) as usize;
    let entry = {
        let table = caller.data().proxy_table.lock().unwrap_or_else(|e| e.into_inner());
        table.get(handle).cloned()
    };
    let entry = match entry {
        Some(entry) => entry,
        None => {
            let exc = make_type_error_exception(
                caller,
                &format!("TypeError: Proxy internal method {op} called on non-proxy"),
            );
            return Err(exc);
        }
    };
    if entry.revoked {
        let exc = make_type_error_exception(
            caller,
            &format!("TypeError: Cannot perform '{op}' on a proxy that has been revoked"),
        );
        return Err(exc);
    }
    Ok((entry.target, entry.handler))
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
    if let Some(symbol_key) = name_id_to_property_key_value(name_id as u32) {
        return symbol_key;
    }
    let name = read_string(caller, name_id as u32).unwrap_or_default();
    store_runtime_string(caller, name)
}

pub(crate) fn define_proxy_traps(
    _linker: &mut Linker<RuntimeState>,
    _store: &mut Store<RuntimeState>,
) -> Result<()> {
    Ok(())
}
