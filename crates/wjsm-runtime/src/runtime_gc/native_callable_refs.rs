//! NativeCallable 内部引用提取（单一权威定义）。
//!
//! 从 native_callable 表项提取其持有的对象引用（raw i64 值列表），
//! 供 GC marker 和 root 发现共用。新增 NativeCallable variant 时只需更新此处。

use crate::NativeCallable;
use wjsm_ir::value;

/// 从 native_callable 表项提取其内部持有的对象引用。
///
/// 返回 raw i64 值列表（由调用方经 resolve_value_handles / push_value_roots
/// 进一步解析为 obj_table handle）。
///
/// - PromiseResolvingFunction → promise
/// - PromiseCombinatorReaction → result_promise + result_array
/// - AsyncGeneratorMethod/Identity → generator
/// - EvalFunction → scope_env
/// - 其余变体不直接持 obj_table 引用（method dispatch 的 handle 是 side-table 索引，
///   由对应 side-table 的 fixed-point root 路径覆盖）。
pub(crate) fn collect_native_callable_refs(st: &mut crate::RuntimeState, idx: usize) -> Vec<i64> {
    let record = match st
        .native_callables
        .lock()
        .ok()
        .and_then(|g| g.get(idx).cloned())
    {
        Some(r) => r,
        None => return vec![],
    };
    match record {
        NativeCallable::PromiseResolvingFunction { promise, .. } => vec![promise],
        NativeCallable::PromiseCombinatorReaction { context, .. } => {
            let (rp, ra) = st
                .combinator_contexts
                .lock()
                .ok()
                .and_then(|g| g.get(context).map(|e| (e.result_promise, e.result_array)))
                .unwrap_or((value::encode_undefined(), value::encode_undefined()));
            vec![rp, ra]
        }
        NativeCallable::AsyncGeneratorMethod { generator, .. }
        | NativeCallable::AsyncGeneratorIdentity { generator } => vec![generator],
        NativeCallable::EvalFunction(function) => function.scope_env.into_iter().collect(),
        // 其余变体不直接持 obj_table 引用（method dispatch 的 handle 是 side-table 索引，
        // 由对应 side-table 的 fixed-point root 路径覆盖）。
        _ => vec![],
    }
}
