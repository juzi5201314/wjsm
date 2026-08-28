//! 迭代器协议 builtin 的宿主实现：from / next / value / done / close。
//!
//! `IteratorNext` 同时服务 async 迭代器（委托 `async_generator` 的
//! `is_managed_async_iterator` 判定）与普通迭代器（`runtime::iterator_next`），
//! 替代原 `dispatch_inline` 的兜底分支。

use wjsm_ir::Builtin;
use wjsm_native_abi::NativeVmContext;

use super::async_generator;
use super::runtime::{iterator_close, iterator_done, iterator_from, iterator_next, iterator_value};
use crate::NativeAgentState;

pub(super) fn dispatch_iterator(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    builtin: Builtin,
    args: &[i64],
) -> Option<i64> {
    Some(match builtin {
        Builtin::IteratorNext if async_generator::is_managed_async_iterator(state, args) => {
            async_generator::iterator_next_async(ctx, state, args)
        }
        Builtin::IteratorNext => iterator_next(ctx, state, args),
        // StringIterator 与 IteratorFrom 共享实现：仅 JS 可见 name 不同
        // （String.prototype[Symbol.iterator] 的固有 name 为 "[Symbol.iterator]"，
        // Array.prototype.values 为 "values"，见 builtin_metadata）。
        Builtin::IteratorFrom | Builtin::StringIterator => iterator_from(ctx, state, args),
        Builtin::IteratorDone => iterator_done(ctx, state, args),
        Builtin::IteratorValue => iterator_value(ctx, state, args, false),
        Builtin::IteratorStepValue => iterator_value(ctx, state, args, true),
        Builtin::IteratorClose => iterator_close(ctx, state, args),
        _ => return None,
    })
}
