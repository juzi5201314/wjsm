//! 定时器 builtin 的宿主实现：setTimeout / setInterval 及其清除。

use wjsm_ir::{Builtin, value};
use wjsm_native_abi::NativeVmContext;

use super::node_async_hooks;
use super::promise;
use super::runtime::{to_number, type_error};
use crate::NativeAgentState;

pub(super) fn dispatch_timer(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    builtin: Builtin,
    args: &[i64],
) -> Option<i64> {
    Some(match builtin {
        Builtin::SetTimeout | Builtin::SetInterval => schedule(ctx, state, builtin, args),
        Builtin::ClearTimeout | Builtin::ClearInterval => clear(ctx, state, args),
        _ => return None,
    })
}

fn schedule(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    builtin: Builtin,
    args: &[i64],
) -> i64 {
    let Some(callback) = args
        .first()
        .copied()
        .filter(|callback| value::is_callable(*callback))
    else {
        return type_error(ctx, state, "TypeError: timer callback must be callable");
    };
    let delay = args
        .get(1)
        .and_then(|delay| to_number(state, *delay))
        .filter(|delay| delay.is_finite() && *delay > 0.0)
        .map_or(0, |delay| delay.trunc() as u64);
    promise::enqueue_timer(
        ctx,
        state,
        callback,
        args.get(2..).unwrap_or_default().to_vec(),
        "Timeout",
        delay,
        builtin == Builtin::SetInterval,
    )
}

fn clear(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    if let Some(timer) = args.first() {
        state.cancelled_timers.insert(value::decode_handle(*timer));
        if let Some(exception) = node_async_hooks::destroy_scheduled_resource(ctx, state, *timer) {
            return exception;
        }
    }
    value::encode_undefined()
}
