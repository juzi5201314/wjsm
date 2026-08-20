//! JSX 元素创建 builtin 的宿主实现：`<div/>` 降级为
//! `CallBuiltin(JsxCreateElement, [tag, props, children])`，返回
//! `{ type, props, children }` 对象（与 wjsm-builtins 参考实现一致）。

use wjsm_ir::{Builtin, value};
use wjsm_native_abi::NativeVmContext;

use super::runtime::fail_dispatch;
use crate::NativeAgentState;

pub(super) fn dispatch_jsx(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    builtin: Builtin,
    args: &[i64],
) -> Option<i64> {
    let _ = builtin;
    Some(create_element(ctx, state, args))
}

/// 分配 `{ type, props, children }` 对象并返回其句柄。
fn create_element(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let [tag, props, children] = args else {
        return fail_dispatch(ctx);
    };
    let Ok(object) = state.allocate_object(3, false) else {
        return fail_dispatch(ctx);
    };
    let handle = value::decode_handle(object);
    for (name, stored) in [("type", *tag), ("props", *props), ("children", *children)] {
        let Some(key) = state.intern_property_string(name.into()) else {
            return fail_dispatch(ctx);
        };
        if state
            .gc
            .heap()
            .set_property(handle, key, stored as u64)
            .is_err()
        {
            return fail_dispatch(ctx);
        }
    }
    object
}
