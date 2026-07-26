//! Proxy trap 基础解析（后端无关）。
//!
//! 算法在此，host-wasm 仅保留薄包装供未迁移文件（core.rs / reentrant_proxy_async.rs）调用。

use wjsm_host::{ExecContext, Value};
use wjsm_ir::value;

/// 是否为已撤销的代理。供返回 bool 的内部方法在其 Reflect 入口处提前判定，
/// 从而返回可捕获的 TypeError。
pub fn proxy_is_revoked<E: ExecContext>(ctx: &mut E, val: Value) -> bool {
    if !value::is_proxy(val) {
        return false;
    }
    let handle = value::decode_proxy_handle(val) as usize;
    ctx.proxy_is_revoked(handle as u32)
}

/// 解析 proxy 的 (target, handler)。撤销代理或非代理 → 返回 `Err(exception)`，
/// 调用方（返回 i64 的 get/delete 等）应直接返回该异常值，从而经语义层 IsException
/// 分叉被 try/catch 同步捕获。返回 void 的 set 路径无法回传异常值，由其自行降级处理。
pub fn proxy_trap_proxy_entry<E: ExecContext>(
    ctx: &mut E,
    proxy: Value,
    op: &str,
) -> Result<(Value, Value), Value> {
    if !value::is_proxy(proxy) {
        let exc = ctx.make_type_error(&format!(
            "TypeError: Proxy internal method {op} called on non-proxy"
        ));
        return Err(exc);
    }
    let handle = value::decode_proxy_handle(proxy) as usize;
    let Some(entry) = ctx.proxy_entry(handle as u32) else {
        let exc = ctx.make_type_error(&format!(
            "TypeError: Proxy internal method {op} called on non-proxy"
        ));
        return Err(exc);
    };
    if ctx.proxy_is_revoked(handle as u32) {
        let exc = ctx.make_type_error(&format!(
            "TypeError: Cannot perform '{op}' on a proxy that has been revoked"
        ));
        return Err(exc);
    }
    Ok((entry.target, entry.handler))
}

/// 从 handler 对象读取指定 trap 方法。undefined/null → None（无 trap，走默认路径）；
/// 非函数 → 设置 runtime_error 并返回 None。
pub fn proxy_trap_handler_trap<E: ExecContext>(
    ctx: &mut E,
    handler: Value,
    trap_name: &str,
) -> Option<Value> {
    let trap = ctx.read_data_property(handler, trap_name);
    if value::is_undefined(trap) || value::is_null(trap) {
        None
    } else if ctx.is_callable(trap) {
        Some(trap)
    } else {
        ctx.set_last_error(format!(
            "TypeError: Proxy handler trap '{trap_name}' is not callable"
        ));
        None
    }
}

/// 将 name_id 转为属性键值（Symbol / RuntimeString / MemoryString）。
pub fn proxy_trap_property_key_value<E: ExecContext>(ctx: &mut E, name_id: i32) -> Value {
    // Symbol 与 MemoryString 直接可用的 prop 值。
    if let Some(key) = ctx.name_id_to_property_key_value(name_id as u32) {
        return key;
    }
    // RuntimeString 必须从 runtime_property_keys 表查真实字符串，
    // 不能复用 runtime_strings 表（两表 index 空间互不相干）。
    if let Some(string) = ctx.property_key_string(name_id as u32) {
        return ctx.store_string_owned(string);
    }
    // MemoryString fallback：读主存 c-string。
    let name = ctx.read_memory_string(name_id as u32, None);
    ctx.store_string_owned(name)
}
