//! wasmtime 后端的 [`wjsm_host::ExecContext`] 实现。
//!
//! 惰性构造：持有 `&mut Caller`，仅在需要导出句柄时提取并缓存 `WasmEnv`。
//! 所有方法委托到现有 host-wasm helper（heap_access_v2 / runtime primitive 等）。

use crate::runtime_string::RuntimeString;
use crate::types::EnumeratorState;
use crate::{RuntimeState, WasmEnv, value};
use wasmtime::Caller;
use wjsm_host::{
    AsyncHookEvent, AtomicsRmwOp, BoundEntry, ClosureEntry, ExecContext, ExecFuture, GcOutcome,
    Handle, HeapContext, IteratorNextStep, PromiseSettlement, ProxyEntry, RegExpMatchInfo,
    TypedArrayView, Value,
};
use wjsm_ir::constants;

/// Wasmtime 后端的 [`ExecContext`] / [`HeapContext`] 实现。
///
/// `env` 惰性提取并 memo（`WasmEnv: Copy`）：`WasmEnv::from_caller` 需要约 28 次
/// `get_export` 名称查找，热路径（数值比较、`typeof` 等）不得为其付费；
/// 仅在首个需要线性内存/导出句柄的方法处解析一次。
pub(crate) struct WasmExecContext<'a, 'b> {
    caller: &'a mut Caller<'b, RuntimeState>,
    env: Option<Option<WasmEnv>>,
}

impl<'a, 'b> WasmExecContext<'a, 'b> {
    #[inline]
    pub(crate) fn new(caller: &'a mut Caller<'b, RuntimeState>) -> Self {
        Self { caller, env: None }
    }

    /// 解析并缓存 `WasmEnv`；同一上下文内只解析一次。
    #[inline]
    fn env(&mut self) -> Option<WasmEnv> {
        *self
            .env
            .get_or_insert_with(|| WasmEnv::from_caller(self.caller))
    }

    fn property_key(&mut self, key: &str) -> u32 {
        let index = crate::property_key::intern_runtime_property_key(
            self.caller.data(),
            RuntimeString::from_utf8_str(key),
        );
        crate::property_key::encode_runtime_string_name_id(index)
    }

    fn heap_access(&mut self) -> Option<&crate::runtime_gc::HeapAccessV2> {
        self.caller.data().heap_access_v2.as_deref()
    }
}

pub(crate) fn to_number(caller: &mut Caller<'_, RuntimeState>, value: Value) -> Value {
    let mut context = WasmExecContext::new(caller);
    wjsm_builtins::core::to_number(&mut context, value)
}

mod heap;

include!("heap_methods.rs");
include!("strings.rs");
include!("property.rs");
include!("promise.rs");
include!("collections.rs");
include!("atomics.rs");
include!("streams.rs");
include!("fetch.rs");
include!("modules.rs");
include!("iterator.rs");
include!("async_gen.rs");
include!("call.rs");
include!("typedarray.rs");
include!("render.rs");
include!("error.rs");
include!("global.rs");

impl ExecContext for WasmExecContext<'_, '_> {
    exec_ctx_heap!();
    exec_ctx_strings!();
    exec_ctx_property!();
    exec_ctx_promise!();
    exec_ctx_collections!();
    exec_ctx_atomics!();
    exec_ctx_streams!();
    exec_ctx_fetch!();
    exec_ctx_modules!();
    exec_ctx_iterator!();
    exec_ctx_async_gen!();
    exec_ctx_call!();
    exec_ctx_typedarray!();
    exec_ctx_render!();
    exec_ctx_error!();
    exec_ctx_global!();
}

// ── Promise 类型转换 helper（wjsm-host ↔ host-wasm 内部类型）──

fn convert_promise_entry(entry: wjsm_host::PromiseEntry) -> crate::PromiseEntry {
    let state = match entry.state {
        wjsm_host::PromiseState::Pending => crate::PromiseState::Pending,
        wjsm_host::PromiseState::Fulfilled(v) => crate::PromiseState::Fulfilled(v),
        wjsm_host::PromiseState::Rejected(r) => crate::PromiseState::Rejected(r),
    };
    crate::PromiseEntry {
        state,
        fulfill_reactions: entry
            .fulfill_reactions
            .into_iter()
            .map(convert_promise_reaction)
            .collect(),
        reject_reactions: entry
            .reject_reactions
            .into_iter()
            .map(convert_promise_reaction)
            .collect(),
        handled: entry.handled,
        constructor_resolver: entry.constructor_resolver,
        constructor_handle: entry.constructor_handle,
        is_promise: entry.is_promise,
        capture_scope: entry.capture_scope.map(convert_captured_scope),
    }
}

fn convert_promise_reaction(reaction: wjsm_host::PromiseReaction) -> crate::PromiseReaction {
    let rt = match reaction.reaction_type {
        wjsm_host::ReactionType::Fulfill => crate::ReactionType::Fulfill,
        wjsm_host::ReactionType::Reject => crate::ReactionType::Reject,
        wjsm_host::ReactionType::FinallyFulfill => crate::ReactionType::FinallyFulfill,
        wjsm_host::ReactionType::FinallyReject => crate::ReactionType::FinallyReject,
    };
    crate::PromiseReaction::new(reaction.handler, reaction.target_promise, rt)
}

fn convert_captured_scope(scope: wjsm_host::CapturedScope) -> crate::CapturedScope {
    crate::CapturedScope {
        async_id: scope.async_id,
        trigger_async_id: scope.trigger_async_id,
        resource: scope.resource,
        frame_id: scope.frame_id.map(crate::runtime_async_hooks::FrameId),
    }
}

fn convert_captured_scope_back(scope: crate::CapturedScope) -> wjsm_host::CapturedScope {
    wjsm_host::CapturedScope {
        async_id: scope.async_id,
        trigger_async_id: scope.trigger_async_id,
        resource: scope.resource,
        frame_id: scope.frame_id.map(|f| f.0),
    }
}
