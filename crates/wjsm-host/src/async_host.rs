//! Async hooks 宿主能力。
//!
//! 对应 Node.js `node:async_hooks` 的生命周期回调。本 trait 定义"何时派发"，
//! 真正的"读 hooks + 调 JS 回调"经 [`HeapContext`] 落到后端。

use crate::heap_context::{AsyncHookEvent, HeapContext};

/// Async hooks 生命周期能力。方法接收后端上下文 `ctx`。
pub trait AsyncHost {
    /// 资源初始化（`init` 回调）。
    ///
    /// 默认实现经 `ctx` 查询启用的 init 回调；实际 JS 调用由后端在
    /// `HeapContext`/执行层完成（本 crate 不抽象 JS 调用域）。
    fn async_hook_init(
        &mut self,
        ctx: &mut dyn HeapContext,
        async_id: u32,
        resource_type: &str,
        trigger_id: u32,
    ) {
        let _ = (async_id, resource_type, trigger_id);
        let _ = ctx.async_hook_callbacks(AsyncHookEvent::Init, false);
    }

    /// `before` 回调。
    fn async_hook_before(&mut self, ctx: &mut dyn HeapContext, async_id: u32) {
        let _ = async_id;
        let _ = ctx.async_hook_callbacks(AsyncHookEvent::Before, false);
    }

    /// `after` 回调。
    fn async_hook_after(&mut self, ctx: &mut dyn HeapContext, async_id: u32) {
        let _ = async_id;
        let _ = ctx.async_hook_callbacks(AsyncHookEvent::After, false);
    }

    /// 资源销毁（`destroy` 回调）。
    fn async_hook_destroy(&mut self, ctx: &mut dyn HeapContext, async_id: u32) {
        let _ = async_id;
        let _ = ctx.async_hook_callbacks(AsyncHookEvent::Destroy, false);
    }

    /// promise resolve（`promiseResolve` 回调）。
    fn async_hook_promise_resolve(&mut self, ctx: &mut dyn HeapContext, async_id: u32) {
        let _ = async_id;
        let _ = ctx.async_hook_callbacks(AsyncHookEvent::PromiseResolve, true);
    }
}
