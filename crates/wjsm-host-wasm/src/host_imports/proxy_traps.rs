//! Proxy trap 基础解析（薄包装层）。
//!
//! 算法在 `wjsm_builtins::proxy_traps`；本文件仅保留 `pub(crate)` 薄包装
//! 供未迁移的 host_imports 文件（core.rs / reentrant_proxy_async.rs）调用。

use anyhow::Result;
use wasmtime::Store;
use wasmtime::{Caller, Linker};

use crate::exec_context_impl::WasmExecContext;
use crate::*;

/// 解析 proxy 的 (target, handler)（薄包装）。
pub(crate) fn proxy_trap_proxy_entry(
    caller: &mut Caller<'_, RuntimeState>,
    proxy: i64,
    op: &str,
) -> Result<(i64, i64), i64> {
    let mut ctx = WasmExecContext::new(caller);
    wjsm_builtins::proxy_traps::proxy_trap_proxy_entry(&mut ctx, proxy, op)
}

/// 从 handler 读取 trap 方法（薄包装）。
pub(crate) fn proxy_trap_handler_trap(
    caller: &mut Caller<'_, RuntimeState>,
    handler: i64,
    trap_name: &str,
) -> Option<i64> {
    let mut ctx = WasmExecContext::new(caller);
    wjsm_builtins::proxy_traps::proxy_trap_handler_trap(&mut ctx, handler, trap_name)
}

/// 将 name_id 转为属性键值（薄包装）。
pub(crate) fn proxy_trap_property_key_value(
    caller: &mut Caller<'_, RuntimeState>,
    name_id: i32,
) -> i64 {
    let mut ctx = WasmExecContext::new(caller);
    wjsm_builtins::proxy_traps::proxy_trap_property_key_value(&mut ctx, name_id)
}

pub(crate) fn define_proxy_traps(
    _linker: &mut Linker<RuntimeState>,
    _store: &mut Store<RuntimeState>,
) -> Result<()> {
    Ok(())
}
