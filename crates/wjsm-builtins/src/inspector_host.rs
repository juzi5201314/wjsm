//! Inspector debug_break（后端经 ExecContext::debug_break 实现暂停循环）。

use wjsm_host::ExecContext;

/// `env.debug_break(line, col, flags)`。
pub async fn debug_break<E: ExecContext>(ctx: &mut E, line: i32, col: i32, flags: i32) {
    ctx.debug_break(line, col, flags).await
}
