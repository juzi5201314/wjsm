//! AbortController / AbortSignal 的宿主实现（WHATWG DOM §3.2 的已实现子集）。
//!
//! 控制器与 signal 是普通堆对象，身份经 fetch 侧表登记；`signal` / `abort` /
//! `aborted` / `reason` 经虚拟属性解析。abort 后 signal 进入 aborted 状态，
//! reason 缺省合成 name 为 `AbortError` 的错误对象（对应 Node 的
//! `AbortError` DOMException 可观察字段）。

use wjsm_ir::value;
use wjsm_native_abi::NativeVmContext;

use super::{FetchCallable, FetchObjectKind, FetchProperty};
use crate::NativeAgentState;

pub(super) struct AbortSignalState {
    pub(super) object: i64,
    pub(super) aborted: bool,
    pub(super) reason: i64,
}

pub(super) fn construct(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    _args: &[i64],
) -> i64 {
    let Ok(controller) = state.allocate_object_with_gc_retry(ctx, 1, false) else {
        return super::super::fail_dispatch(ctx);
    };
    if state
        .set_web_instance_prototype(controller, wjsm_ir::Builtin::AbortControllerConstructor)
        .is_err()
    {
        return super::super::fail_dispatch(ctx);
    }
    let Ok(signal) = state.allocate_object_with_gc_retry(ctx, 2, false) else {
        return super::super::fail_dispatch(ctx);
    };
    let Ok(handle) = u32::try_from(state.fetch.abort_signals.len()) else {
        return super::super::fail_dispatch(ctx);
    };
    state.fetch.abort_signals.push(AbortSignalState {
        object: signal,
        aborted: false,
        reason: value::encode_undefined(),
    });
    super::register_object(state, controller, FetchObjectKind::AbortController(handle));
    super::register_object(state, signal, FetchObjectKind::AbortSignal(handle));
    controller
}

pub(super) fn controller_property(
    state: &NativeAgentState,
    handle: u32,
    key: &str,
) -> Option<FetchProperty> {
    let signal = state.fetch.abort_signals.get(handle as usize)?;
    match key {
        "signal" => Some(FetchProperty::Value(signal.object)),
        "abort" => Some(FetchProperty::Callable(FetchCallable::AbortControllerAbort(
            handle,
        ))),
        _ => None,
    }
}

pub(super) fn signal_property(
    state: &NativeAgentState,
    handle: u32,
    key: &str,
) -> Option<FetchProperty> {
    let signal = state.fetch.abort_signals.get(handle as usize)?;
    match key {
        "aborted" => Some(FetchProperty::Value(value::encode_bool(signal.aborted))),
        "reason" => Some(FetchProperty::Value(signal.reason)),
        _ => None,
    }
}

/// `AbortController.prototype.abort(reason)`：已 aborted 时无操作；reason 为
/// 缺省或 undefined 时合成 AbortError 形态的错误对象（WHATWG DOM "signal
/// abort" 步骤 1-2）。
pub(super) fn abort(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    handle: u32,
    args: &[i64],
) -> i64 {
    if state
        .fetch
        .abort_signals
        .get(handle as usize)
        .is_none_or(|signal| signal.aborted)
    {
        return value::encode_undefined();
    }
    let reason = match args.first().copied().filter(|raw| !value::is_undefined(*raw)) {
        Some(reason) => reason,
        None => {
            let Some(reason) = super::super::modules::named_error_object(
                state,
                "AbortError",
                "This operation was aborted".into(),
            ) else {
                return super::super::fail_dispatch(ctx);
            };
            reason
        }
    };
    let Some(signal) = state.fetch.abort_signals.get_mut(handle as usize) else {
        return super::super::fail_dispatch(ctx);
    };
    signal.aborted = true;
    signal.reason = reason;
    value::encode_undefined()
}

/// 把 abort signal 侧表持有的 JS 值（signal 对象与 reason）并入 GC 根队列：
/// 它们经虚拟属性暴露，堆对象图上没有对应 slot，不钉扎会被误回收。
pub(crate) fn extend_gc_roots(
    fetch: &super::NativeFetchState,
    roots: &mut std::collections::VecDeque<i64>,
) {
    for signal in &fetch.abort_signals {
        roots.push_back(signal.object);
        if !value::is_undefined(signal.reason) {
            roots.push_back(signal.reason);
        }
    }
}
