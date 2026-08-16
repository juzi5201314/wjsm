//! console 系列 builtin 的宿主实现：把渲染后的参数写入 `state.output`。

use wjsm_ir::{Builtin, value};
use wjsm_native_abi::NativeVmContext;

use super::runtime::render_value;
use crate::NativeAgentState;

pub(super) fn dispatch_console(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    builtin: Builtin,
    args: &[i64],
) -> Option<i64> {
    let _ = ctx;
    Some(render_to_output(state, builtin, args))
}

/// 按 builtin 前缀 + 空格分隔参数 + 换行写入输出。
fn render_to_output(state: &mut NativeAgentState, builtin: Builtin, args: &[i64]) -> i64 {
    let mut line = Vec::new();
    match builtin {
        Builtin::ConsoleInfo => line.extend_from_slice(b"[info] "),
        Builtin::ConsoleDebug => line.extend_from_slice(b"[debug] "),
        Builtin::ConsoleWarn => line.extend_from_slice(b"[warn] "),
        Builtin::ConsoleError => line.extend_from_slice(b"[error] "),
        Builtin::ConsoleTrace => line.extend_from_slice(b"[trace] "),
        Builtin::ConsoleLog => {}
        _ => unreachable!("console builtin match is exhaustive"),
    }
    for (index, argument) in args.iter().enumerate() {
        if index != 0 {
            line.push(b' ');
        }
        line.extend_from_slice(render_value(state, *argument).as_bytes());
    }
    line.push(b'\n');
    state.emit_output(&line, false);
    value::encode_undefined()
}
