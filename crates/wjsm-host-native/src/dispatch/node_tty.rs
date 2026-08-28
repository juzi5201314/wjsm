// node:tty 宿主桥：isatty 对真实 fd 做终端探测。
// 标准流（0/1/2）经 std::io::IsTerminal 跨平台查询；其余 fd 在 unix 上走
// libc::isatty，非 unix 平台上不存在可查询的任意 fd 概念，按非终端处理。

use std::io::IsTerminal;

use wjsm_ir::value;

use super::modules;
use crate::{NativeAgentState, NativeCallableKind};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum NodeTtyMethod {
    Isatty,
}

pub(crate) fn ensure_bridge(state: &mut NativeAgentState) -> Option<i64> {
    if let Some(bridge) = state.node_tty_bridge {
        return Some(bridge);
    }
    let bridge = state.allocate_object(1, false).ok()?;
    let callable = state.native_callable(NativeCallableKind::NodeTty(NodeTtyMethod::Isatty))?;
    modules::set_named_property(state, bridge, "isatty", callable).ok()?;
    state.node_tty_bridge = Some(bridge);
    Some(bridge)
}

pub(crate) fn call(method: NodeTtyMethod, args: &[i64]) -> i64 {
    match method {
        NodeTtyMethod::Isatty => value::encode_bool(fd_is_tty(args.first().copied())),
    }
}

fn fd_is_tty(stored: Option<i64>) -> bool {
    let Some(stored) = stored else {
        return false;
    };
    if !value::is_f64(stored) {
        return false;
    }
    let number = value::decode_f64(stored);
    if number.fract() != 0.0 || !(0.0..=f64::from(i32::MAX)).contains(&number) {
        return false;
    }
    match number as i32 {
        0 => std::io::stdin().is_terminal(),
        1 => std::io::stdout().is_terminal(),
        2 => std::io::stderr().is_terminal(),
        other => descriptor_is_tty(other),
    }
}

#[cfg(unix)]
fn descriptor_is_tty(fd: i32) -> bool {
    // SAFETY: isatty 只读查询 fd 属性；无效 fd 时返回 0 并置 errno，无内存前置条件。
    unsafe { libc::isatty(fd) == 1 }
}

#[cfg(not(unix))]
fn descriptor_is_tty(_fd: i32) -> bool {
    false
}
