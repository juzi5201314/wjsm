//! `yield*` 委托收到 received throw/return 时的向内迭代器转发
//! （§27.5.3.7 YieldExpression : yield * AssignmentExpression 步骤 7.b/7.c）。
//!
//! sync 形态在宿主内完成 GetMethod → Call → 结果校验 → IteratorComplete，
//! 并把结果对象写回 `array_iterators` 条目（done/current），委托循环 header
//! 的 IteratorDone/IteratorValue 据此续走；async 形态只执行同步段（方法解析
//! 与调用），Await 必须在协程内挂起，故以 `{k, v}` 标记对象交回语义层：
//! k=0 方法调用结果（Await 后按 done 分支）、k=1 throw 缺失时 return() 的
//! close 结果（§7.4.10，Await + 对象校验后抛缺方法 TypeError）、k=2 方法
//! 缺失（Await(received) 后 ReturnCompletion，步骤 7.c.iii）。

use wjsm_ir::{Builtin, value};
use wjsm_native_abi::NativeVmContext;

use super::async_generator::{self, RequestKind};
use super::runtime::{
    create_iterator_result, default_call_site, fail_dispatch, get_property, is_truthy,
    object_handle, render_value, type_error,
};
use crate::NativeAgentState;

pub(super) fn dispatch_yield_delegate(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    builtin: Builtin,
    args: &[i64],
) -> Option<i64> {
    Some(match builtin {
        Builtin::IteratorDelegateThrow => sync_delegate(ctx, state, args, ForwardKind::Throw),
        Builtin::IteratorDelegateReturn => sync_delegate(ctx, state, args, ForwardKind::Return),
        Builtin::AsyncIteratorDelegateThrow => {
            async_delegate(ctx, state, args, ForwardKind::Throw)
        }
        Builtin::AsyncIteratorDelegateReturn => {
            async_delegate(ctx, state, args, ForwardKind::Return)
        }
        Builtin::IteratorResultRequireObject => require_result_object(ctx, state, args),
        Builtin::IteratorThrowMethodMissingError => throw_method_missing_error(ctx, state),
        _ => return None,
    })
}

#[derive(Clone, Copy)]
enum ForwardKind {
    Throw,
    Return,
}

impl ForwardKind {
    fn method_name(self) -> &'static str {
        match self {
            Self::Throw => "throw",
            Self::Return => "return",
        }
    }
}

/// sync `yield*` 的 received throw/return 转发（步骤 7.b/7.c 的方法调用段）。
///
/// throw：方法缺失时按步骤 7.b.iii 先以 normal completion 关闭迭代器（close
/// 错误传播）再返回缺方法 TypeError 异常；有方法则调用并把结果缓存进条目，
/// 返回结果对象（语义层跳回 header 按 done 续走）。
/// return：方法缺失返回 undefined 哨兵（语义层按步骤 7.c.iii 直接
/// ReturnCompletion(received)）；有方法则调用并缓存，返回结果对象。
fn sync_delegate(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
    kind: ForwardKind,
) -> i64 {
    let [iterator, received] = args else {
        return fail_dispatch(ctx);
    };
    if value::is_exception(*iterator) {
        return *iterator;
    }
    let handle = value::decode_handle(*iterator);
    let Some(entry) = state.array_iterators.get(&handle).copied() else {
        return fail_dispatch(ctx);
    };
    // 内建家族迭代器（数组/字符串/集合等）原型上无 throw/return 方法，
    // 统一按方法缺失路径处理（与 iterator_close 的 pristine 假设一致）。
    let object = match entry.source {
        super::super::NativeIteratorSource::Custom(object) => Some(object),
        _ => None,
    };
    match call_forward_method(ctx, state, object, *received, kind) {
        Err(exception) => exception,
        Ok(None) => match kind {
            ForwardKind::Return => value::encode_undefined(),
            ForwardKind::Throw => {
                if let Err(exception) = close_for_missing_throw(ctx, state, object) {
                    return exception;
                }
                throw_method_missing(ctx, state)
            }
        },
        Ok(Some(result)) => {
            let done = match iterator_complete(ctx, state, result) {
                Ok(done) => done,
                Err(exception) => return exception,
            };
            let Some(entry) = state.array_iterators.get_mut(&handle) else {
                return fail_dispatch(ctx);
            };
            entry.done = done;
            entry.current = Some(result);
            result
        }
    }
}

/// async `yield*` 的 received throw/return 转发同步段（步骤 7.b/7.c 的
/// async 形态）：按内层迭代器种类分派，返回 `{k, v}` 标记对象或异常。
fn async_delegate(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
    kind: ForwardKind,
) -> i64 {
    let [iterator, received] = args else {
        return fail_dispatch(ctx);
    };
    if value::is_exception(*iterator) {
        return *iterator;
    }
    // 宿主管理的 async generator：next/return/throw 方法恒存在，入队等价于
    // GetMethod + Call（与 iterator_next_async 的管理分支一致）。
    if async_generator::is_async_generator(state, *iterator) {
        let request = match kind {
            ForwardKind::Throw => RequestKind::Throw,
            ForwardKind::Return => RequestKind::Return,
        };
        let promise = async_generator::enqueue(ctx, state, *iterator, request, *received);
        return make_marker(ctx, state, 0.0, promise);
    }
    // sync 迭代器包装（§7.4.3 步骤 1.b 的 CreateAsyncFromSyncIterator）：
    // %AsyncFromSyncIteratorPrototype% 的 throw/return 语义（§27.1.4.4.3/.4）
    // 在包装内部对底层 sync 迭代器做 GetMethod + Call + 结果重包装。
    if let Some(sync_iterator) = object_handle(*iterator)
        .and_then(|handle| state.async_from_sync_iterators.get(&handle).copied())
    {
        return async_from_sync_delegate(ctx, state, sync_iterator, *received, kind);
    }
    // 用户 async 迭代器对象：只做 GetMethod + Call，结果对象校验与
    // IteratorComplete 必须等 Await 之后（§27.5.3.7 步骤 7.b.ii.2/7.c.v）。
    match call_forward_method(ctx, state, Some(*iterator), *received, kind) {
        Err(exception) => exception,
        Ok(Some(result)) => make_marker(ctx, state, 0.0, result),
        Ok(None) => match kind {
            ForwardKind::Return => make_marker(ctx, state, 2.0, *received),
            ForwardKind::Throw => {
                // AsyncIteratorClose（§7.4.10）的同步段：return() 调用结果的
                // Await 由语义层完成（k=1），throw/return 皆缺时直接抛缺方法
                // TypeError（close 为空操作）。
                match call_close_return(ctx, state, *iterator) {
                    Err(exception) => exception,
                    Ok(Some(close_result)) => make_marker(ctx, state, 1.0, close_result),
                    Ok(None) => throw_method_missing(ctx, state),
                }
            }
        },
    }
}

/// sync 迭代器包装的 throw/return 转发（§27.1.4.4.3/.4 +
/// AsyncFromSyncIteratorContinuation §27.1.4.5 的同步段）：对底层迭代器
/// GetMethod + Call 后做对象校验与 done/value 读取，重包装为 fresh result
/// 交语义层 Await（value 为 promise 时的解包由 Await 完成）。
fn async_from_sync_delegate(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    sync_iterator: i64,
    received: i64,
    kind: ForwardKind,
) -> i64 {
    let handle = value::decode_handle(sync_iterator);
    let Some(entry) = state.array_iterators.get(&handle).copied() else {
        return fail_dispatch(ctx);
    };
    let object = match entry.source {
        super::super::NativeIteratorSource::Custom(object) => Some(object),
        _ => None,
    };
    match call_forward_method(ctx, state, object, received, kind) {
        Err(exception) => exception,
        Ok(None) => match kind {
            // 包装层 return 缺失：§27.1.4.4.4 步骤 7 resolve {done:true,
            // value:received}，与步骤 7.c.iii 的 Await + ReturnCompletion 等价。
            ForwardKind::Return => make_marker(ctx, state, 2.0, received),
            ForwardKind::Throw => {
                if let Err(exception) = close_for_missing_throw(ctx, state, object) {
                    return exception;
                }
                throw_method_missing(ctx, state)
            }
        },
        Ok(Some(result)) => {
            let done = match iterator_complete(ctx, state, result) {
                Ok(done) => done,
                Err(exception) => return exception,
            };
            // IteratorValue（§27.1.4.5 步骤 6）：value 读取抛出同样拒绝。
            let stored = match get_string_property(ctx, state, result, "value") {
                Ok(stored) => stored,
                Err(exception) => return exception,
            };
            // 条目只推进 done：async 循环从 result 对象读值，后续 next()
            // 经 ensure_custom_current 再调底层迭代器，不缓存本结果。
            if let Some(entry) = state.array_iterators.get_mut(&handle) {
                entry.done = done;
                entry.current = None;
            }
            let fresh = create_iterator_result(ctx, state, stored, done);
            make_marker(ctx, state, 0.0, fresh)
        }
    }
}

/// GetMethod(object, kind) + Call(method, object, «received»)（步骤 7.b.i–ii.1
/// / 7.c.ii–iv）：`Ok(None)` 为方法缺失（undefined/null），异常（GetMethod
/// 的 getter 抛出、非可调用 TypeError、调用抛出）以 `Err` 传播。
fn call_forward_method(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    object: Option<i64>,
    received: i64,
    kind: ForwardKind,
) -> Result<Option<i64>, i64> {
    let Some(object) = object else {
        return Ok(None);
    };
    let Some(method) = get_method(ctx, state, object, kind.method_name())? else {
        return Ok(None);
    };
    let Some(result) = state.invoke_callable(ctx, method, object, &[received]) else {
        return Err(fail_dispatch(ctx));
    };
    if value::is_exception(result) {
        return Err(result);
    }
    Ok(Some(result))
}

/// throw 方法缺失时的 IteratorClose（步骤 7.b.iii.3，completion 为 normal，
/// §7.4.11：close 各段错误传播——GetMethod 抛出、return 调用抛出、结果非
/// 对象皆以 `Err` 上浮，方法缺失为空操作）。
fn close_for_missing_throw(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    object: Option<i64>,
) -> Result<(), i64> {
    let Some(object) = object else {
        return Ok(());
    };
    let Some(method) = get_method(ctx, state, object, "return")? else {
        return Ok(());
    };
    // IteratorClose 步骤 5：Call(return, iterator) 不传实参。
    let Some(result) = state.invoke_callable(ctx, method, object, &[]) else {
        return Err(fail_dispatch(ctx));
    };
    if value::is_exception(result) {
        return Err(result);
    }
    if !value::is_js_object(result) {
        return Err(iterator_result_type_error(ctx, state, result));
    }
    Ok(())
}

/// AsyncIteratorClose（§7.4.10）的同步段：解析并调用 return 方法，结果的
/// Await 与对象校验由语义层完成。`Ok(None)` 为方法缺失。
fn call_close_return(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    iterator: i64,
) -> Result<Option<i64>, i64> {
    let Some(method) = get_method(ctx, state, iterator, "return")? else {
        return Ok(None);
    };
    let Some(result) = state.invoke_callable(ctx, method, iterator, &[]) else {
        return Err(fail_dispatch(ctx));
    };
    if value::is_exception(result) {
        return Err(result);
    }
    Ok(Some(result))
}

/// GetMethod（§7.3.10）：undefined/null 归约为 `None`，非可调用抛 TypeError
/// （V8 kCalledNonCallable 按候选方法值渲染）。
fn get_method(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    object: i64,
    name: &str,
) -> Result<Option<i64>, i64> {
    let Some(key) = state.intern_text(name.into(), value::TAG_STRING) else {
        return Err(fail_dispatch(ctx));
    };
    let method = get_property(ctx, state, object, key).map_err(|()| fail_dispatch(ctx))?;
    if value::is_exception(method) {
        return Err(method);
    }
    if value::is_undefined(method) || value::is_null(method) {
        return Ok(None);
    }
    if !value::is_callable(method) {
        let callsite = default_call_site(state, method);
        return Err(type_error(
            ctx,
            state,
            &format!("{callsite} is not a function"),
        ));
    }
    Ok(Some(method))
}

/// 结果对象校验 + IteratorComplete（步骤 7.b.ii.4–5 / 7.c.vi–vii）：非对象
/// 抛 kIteratorResultNotAnObject，done 读取（getter）抛出传播。
fn iterator_complete(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    result: i64,
) -> Result<bool, i64> {
    if !value::is_js_object(result) {
        return Err(iterator_result_type_error(ctx, state, result));
    }
    let done = get_string_property(ctx, state, result, "done")?;
    Ok(is_truthy(state, done))
}

fn get_string_property(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    object: i64,
    name: &str,
) -> Result<i64, i64> {
    let Some(key) = state.intern_text(name.into(), value::TAG_STRING) else {
        return Err(fail_dispatch(ctx));
    };
    let stored = get_property(ctx, state, object, key).map_err(|()| fail_dispatch(ctx))?;
    if value::is_exception(stored) {
        return Err(stored);
    }
    Ok(stored)
}

/// async 转发的 `{k, v}` 标记对象（语义层协议，见模块头注释）。
fn make_marker(ctx: &mut NativeVmContext, state: &mut NativeAgentState, k: f64, v: i64) -> i64 {
    let Ok(marker) = state.allocate_object_with_gc_retry(ctx, 2, false) else {
        return fail_dispatch(ctx);
    };
    for (name, stored) in [("k", value::encode_f64(k)), ("v", v)] {
        let Some(key) = state.intern_property_string(name.into()) else {
            return fail_dispatch(ctx);
        };
        if state
            .gc
            .heap()
            .set_property(value::decode_handle(marker), key, stored as u64)
            .is_err()
        {
            return fail_dispatch(ctx);
        }
    }
    marker
}

/// Await 后的结果对象校验（§27.5.3.7 步骤 7.b.ii.4 / 7.c.vi 的 async 形态，
/// §7.4.10 步骤 6）：对象原样返回，非对象抛 kIteratorResultNotAnObject。
fn require_result_object(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let Some(result) = args.first().copied() else {
        return fail_dispatch(ctx);
    };
    if value::is_exception(result) {
        return result;
    }
    if value::is_js_object(result) {
        result
    } else {
        iterator_result_type_error(ctx, state, result)
    }
}

fn iterator_result_type_error(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    result: i64,
) -> i64 {
    let rendered = render_value(state, result);
    type_error(
        ctx,
        state,
        &format!("Iterator result {rendered} is not an object"),
    )
}

fn throw_method_missing(ctx: &mut NativeVmContext, state: &mut NativeAgentState) -> i64 {
    type_error(
        ctx,
        state,
        "The iterator does not provide a 'throw' method.",
    )
}

/// 缺方法 TypeError 的错误对象（未包装为异常哨兵）：async close 的 Await
/// 完成后由语义层 emit_throw_value 抛出（步骤 7.b.iii.6）。
fn throw_method_missing_error(ctx: &mut NativeVmContext, state: &mut NativeAgentState) -> i64 {
    super::modules::named_error_object(
        state,
        "TypeError",
        "The iterator does not provide a 'throw' method.".into(),
    )
    .unwrap_or_else(|| fail_dispatch(ctx))
}
