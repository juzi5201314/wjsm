//! AbortController / AbortSignal 的宿主实现（WHATWG DOM §3.2 的已实现子集）。
//!
//! 控制器与 signal 是普通堆对象，身份经 fetch 侧表登记；`signal` / `abort`
//! 是 `AbortController.prototype` 的自有属性，`aborted` / `reason` /
//! `throwIfAborted` / `onabort` 是 `AbortSignal.prototype` 的自有属性（均按
//! 实际 this 分派）。signal 的 [[Prototype]] 链为 `AbortSignal.prototype →
//! EventTarget.prototype → Object.prototype`，监听器登记表内嵌在
//! [`AbortSignalState`]，EventTarget 行为由 `dispatch::events` 提供。abort
//! 后 signal 进入 aborted 状态、在 signal 上一次性派发 `abort` 事件，重复
//! abort 无操作；reason 缺省合成 name 为 `AbortError` 的错误对象（对应
//! Node 的 `AbortError` DOMException 可观察字段）。

use wjsm_ir::value;
use wjsm_native_abi::NativeVmContext;

use super::FetchObjectKind;
use crate::NativeAgentState;
use crate::dispatch::events::EventTargetData;

pub(super) struct AbortSignalState {
    pub(super) object: i64,
    pub(super) aborted: bool,
    pub(super) reason: i64,
    /// abort 事件的监听器登记表 + onabort 处理器槽（EventTarget 身份）。
    pub(super) events: EventTargetData,
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
    // signal 分配与其原型链创建可触发 GC，此刻 controller/signal 仅由局部值
    // 持有，必须钉扎到登记完成。
    let initial_temp_roots = state.temporary_roots.len();
    state.temporary_roots.push(controller);
    let signal = state.allocate_object_with_gc_retry(ctx, 2, false);
    let Ok(signal) = signal else {
        state.temporary_roots.truncate(initial_temp_roots);
        return super::super::fail_dispatch(ctx);
    };
    state.temporary_roots.push(signal);
    let prototype_attached = state
        .set_web_instance_prototype(signal, wjsm_ir::Builtin::AbortSignalConstructor)
        .is_ok();
    state.temporary_roots.truncate(initial_temp_roots);
    if !prototype_attached {
        return super::super::fail_dispatch(ctx);
    }
    let Some(handle) = state.fetch.abort_signals.insert(AbortSignalState {
        object: signal,
        aborted: false,
        reason: value::encode_undefined(),
        events: EventTargetData::default(),
    }) else {
        return super::super::fail_dispatch(ctx);
    };
    super::register_object(state, controller, FetchObjectKind::AbortController(handle));
    super::register_object(state, signal, FetchObjectKind::AbortSignal(handle));
    controller
}

/// `AbortController.prototype.abort(reason)`：已 aborted 时无操作；reason 为
/// 缺省或 undefined 时合成 AbortError 形态的错误对象；随后在 signal 上派发
/// 一次 `abort` 事件（WHATWG DOM "signal abort" 步骤 1-2、5）。
pub(super) fn abort(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    handle: u32,
    args: &[i64],
) -> i64 {
    if state
        .fetch
        .abort_signals
        .get(handle)
        .is_none_or(|signal| signal.aborted)
    {
        return value::encode_undefined();
    }
    let reason = match args
        .first()
        .copied()
        .filter(|raw| !value::is_undefined(*raw))
    {
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
    let Some(signal) = state.fetch.abort_signals.get_mut(handle) else {
        return super::super::fail_dispatch(ctx);
    };
    signal.aborted = true;
    signal.reason = reason;
    // 状态置位后才派发（监听器内观察 signal.aborted === true；监听器里再
    // abort 命中顶部 aborted 检查成为无操作，事件不会重复派发）。
    super::super::events::fire_abort_event(ctx, state, handle)
}
