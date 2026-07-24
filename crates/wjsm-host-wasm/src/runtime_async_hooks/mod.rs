//! Host-core async_hooks / AsyncLocalStorage 状态与 scope 辅助。
//!
//! Node API 外形在 `node_async_hooks.js`；本模块拥有 id 栈、context frame、
//! CapturedScope 与 hook 发射（Phase 0–1 先落地 ids/ALS/scope；createHook 同步可用）。

mod context_frame;
pub(crate) mod emit;
mod state;

pub use context_frame::FrameId;
pub use state::{
    AsyncHooksFlags, AsyncHooksState, CapturedScope, HookRecord, PendingPromiseHookEvent,
    ResourceMeta,
};

/// 顶层 executionAsyncResource 使用的哨兵：无用户资源时返回的空对象语义由 JS 层持有。
pub const TOP_LEVEL_RESOURCE_SENTINEL: i64 = 0;

/// 从 Caller 捕获调度 scope（微任务入队用）。
pub(crate) fn capture_from_caller(
    caller: &wasmtime::Caller<'_, crate::RuntimeState>,
) -> Option<CapturedScope> {
    let mut hooks = caller
        .data()
        .async_hooks
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    hooks.capture_for_scheduled_callback(0, false)
}
