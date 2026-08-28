//! 惰性 Iterator Helper（map / filter / take / drop / flatMap）的生成器式
//! 步进与关闭（§27.1.2 Iterator Helper 对象按生成器语义规范化）。

use wjsm_ir::value;
use wjsm_native_abi::NativeVmContext;

use super::super::runtime::{create_iterator_result, fail_dispatch, is_truthy, type_error};
use super::{
    HelperKind, HelperRunState, IteratorHelper, IteratorRecord, PrimitiveHandling, close_iterator,
    ensure_helper_prototype, get_iterator_flattenable, render_receiver, step_has_value, step_value,
};
use crate::NativeAgentState;

/// 创建 helper 对象：[[Prototype]] 为 %IteratorHelperPrototype%，内部槽入
/// 宿主侧表，初始 suspended-start。
pub(crate) fn create_helper(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    kind: HelperKind,
    underlying: IteratorRecord,
) -> i64 {
    let Some(prototype) = ensure_helper_prototype(state) else {
        return fail_dispatch(ctx);
    };
    // 分配可触发 GC：record 里的迭代器 / next 可能只被本地持有（用户在
    // next getter 里删除自有属性的病态场景），锚根到分配完成。
    let initial_temp_roots = state.temporary_roots.len();
    state.temporary_roots.push(underlying.iterator);
    state.temporary_roots.push(underlying.next);
    if let HelperKind::Map(callback) | HelperKind::Filter(callback) | HelperKind::FlatMap(callback) =
        kind
    {
        state.temporary_roots.push(callback);
    }
    let allocated = state.allocate_object_with_gc_retry(ctx, 0, false);
    state.temporary_roots.truncate(initial_temp_roots);
    let Ok(object) = allocated else {
        return fail_dispatch(ctx);
    };
    if state
        .gc
        .heap()
        .set_prototype(
            value::decode_handle(object),
            value::decode_handle(prototype),
        )
        .is_err()
    {
        return fail_dispatch(ctx);
    }
    state.iterator_helpers.helpers.insert(
        value::decode_handle(object),
        IteratorHelper {
            kind,
            underlying,
            counter: 0,
            run: HelperRunState::SuspendedStart,
            inner: None,
        },
    );
    object
}

fn helper_entry(state: &NativeAgentState, receiver: i64) -> Option<IteratorHelper> {
    if !value::is_js_object(receiver) {
        return None;
    }
    state
        .iterator_helpers
        .helpers
        .get(&value::decode_handle(receiver))
        .copied()
}

fn incompatible_receiver(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    receiver: i64,
    method: &str,
) -> i64 {
    let message = format!(
        "Method Iterator Helper.prototype.{method} called on incompatible receiver {}",
        render_receiver(state, receiver)
    );
    type_error(ctx, state, &message)
}

fn set_run(state: &mut NativeAgentState, handle: u32, run: HelperRunState) {
    if let Some(entry) = state.iterator_helpers.helpers.get_mut(&handle) {
        entry.run = run;
    }
}

/// %IteratorHelperPrototype%.next（§27.1.2.1.1）。
pub(crate) fn helper_next(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    receiver: i64,
) -> i64 {
    let Some(entry) = helper_entry(state, receiver) else {
        return incompatible_receiver(ctx, state, receiver, "next");
    };
    match entry.run {
        HelperRunState::Completed => {
            create_iterator_result(ctx, state, value::encode_undefined(), true)
        }
        HelperRunState::Executing => type_error(ctx, state, "Generator is already running"),
        HelperRunState::SuspendedStart | HelperRunState::SuspendedYield => {
            let handle = value::decode_handle(receiver);
            set_run(state, handle, HelperRunState::Executing);
            step(ctx, state, handle)
        }
    }
}

/// %IteratorHelperPrototype%.return（§27.1.2.1.2）：挂起态关闭底层迭代器
/// （flatMap 先关内层再关外层），恒返回 { value: undefined, done: true }。
pub(crate) fn helper_return(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    receiver: i64,
) -> i64 {
    let Some(entry) = helper_entry(state, receiver) else {
        return incompatible_receiver(ctx, state, receiver, "return");
    };
    match entry.run {
        HelperRunState::Completed => {
            create_iterator_result(ctx, state, value::encode_undefined(), true)
        }
        HelperRunState::Executing => type_error(ctx, state, "Generator is already running"),
        HelperRunState::SuspendedStart | HelperRunState::SuspendedYield => {
            let handle = value::decode_handle(receiver);
            set_run(state, handle, HelperRunState::Completed);
            // flatMap 内层 yield 被 return 完成打断（§27.1.4.8 步骤 viii.4.b）：
            // 先 IteratorClose(inner, completion)，异常再对外层做 throw 关闭。
            if let Some(inner) = entry.inner {
                let closed = close_iterator(
                    ctx,
                    state,
                    inner.iterator,
                    value::encode_undefined(),
                    false,
                );
                if value::is_exception(closed) {
                    return close_iterator(
                        ctx,
                        state,
                        entry.underlying.iterator,
                        closed,
                        true,
                    );
                }
            }
            let closed = close_iterator(
                ctx,
                state,
                entry.underlying.iterator,
                value::encode_undefined(),
                false,
            );
            if value::is_exception(closed) {
                return closed;
            }
            create_iterator_result(ctx, state, value::encode_undefined(), true)
        }
    }
}

/// 底层迭代器步进异常（next 抛出 / 结果非对象 / done、value 读取抛出）：
/// 生成器进入 completed 后原样传播（§7.4.7–7.4.8，不做 IteratorClose）。
fn finish_throw(state: &mut NativeAgentState, handle: u32, exception: i64) -> i64 {
    set_run(state, handle, HelperRunState::Completed);
    exception
}

/// 底层迭代完成：生成器 completed，返回 done 结果对象。
fn finish_done(ctx: &mut NativeVmContext, state: &mut NativeAgentState, handle: u32) -> i64 {
    set_run(state, handle, HelperRunState::Completed);
    create_iterator_result(ctx, state, value::encode_undefined(), true)
}

/// 用户回调抛出：IfAbruptCloseIterator——throw 完成关闭底层迭代器后传播
/// （§27.1.4 各 helper 闭包）。
fn close_underlying_throw(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    handle: u32,
    underlying: i64,
    exception: i64,
) -> i64 {
    set_run(state, handle, HelperRunState::Completed);
    close_iterator(ctx, state, underlying, exception, true)
}

/// yield：生成器挂起（suspended-yield），产出 { value, done: false }。
fn yield_value(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    handle: u32,
    stepped: i64,
) -> i64 {
    set_run(state, handle, HelperRunState::SuspendedYield);
    create_iterator_result(ctx, state, stepped, false)
}

fn call_callback(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    callback: i64,
    stepped: i64,
    counter: u64,
) -> Result<i64, i64> {
    let arguments = [stepped, value::encode_f64(counter as f64)];
    let result = state
        .invoke_callable(ctx, callback, value::encode_undefined(), &arguments)
        .unwrap_or_else(|| fail_dispatch(ctx));
    if value::is_exception(result) {
        Err(result)
    } else {
        Ok(result)
    }
}

fn bump_counter(state: &mut NativeAgentState, handle: u32) {
    if let Some(entry) = state.iterator_helpers.helpers.get_mut(&handle) {
        entry.counter += 1;
    }
}

/// 执行一次生成器步进（进入前 run 已置 executing）。回调可再入 JS，每次
/// 调用后重新读取侧表条目，不跨调用持有可变引用。
fn step(ctx: &mut NativeVmContext, state: &mut NativeAgentState, handle: u32) -> i64 {
    loop {
        let Some(entry) = state.iterator_helpers.helpers.get(&handle).copied() else {
            return fail_dispatch(ctx);
        };
        let underlying = entry.underlying;
        match entry.kind {
            HelperKind::Map(callback) => {
                let stepped = match step_value(ctx, state, &underlying) {
                    Err(exception) => return finish_throw(state, handle, exception),
                    Ok(None) => return finish_done(ctx, state, handle),
                    Ok(Some(stepped)) => stepped,
                };
                let counter = entry.counter;
                bump_counter(state, handle);
                let mapped = match call_callback(ctx, state, callback, stepped, counter) {
                    Err(exception) => {
                        return close_underlying_throw(
                            ctx,
                            state,
                            handle,
                            underlying.iterator,
                            exception,
                        );
                    }
                    Ok(mapped) => mapped,
                };
                return yield_value(ctx, state, handle, mapped);
            }
            HelperKind::Filter(callback) => {
                let stepped = match step_value(ctx, state, &underlying) {
                    Err(exception) => return finish_throw(state, handle, exception),
                    Ok(None) => return finish_done(ctx, state, handle),
                    Ok(Some(stepped)) => stepped,
                };
                let counter = entry.counter;
                bump_counter(state, handle);
                let selected = match call_callback(ctx, state, callback, stepped, counter) {
                    Err(exception) => {
                        return close_underlying_throw(
                            ctx,
                            state,
                            handle,
                            underlying.iterator,
                            exception,
                        );
                    }
                    Ok(selected) => selected,
                };
                if is_truthy(state, selected) {
                    return yield_value(ctx, state, handle, stepped);
                }
            }
            HelperKind::Take(remaining) => {
                if remaining == 0.0 {
                    // §27.1.4.11 步骤 a：remaining 归零即正常关闭底层迭代器。
                    set_run(state, handle, HelperRunState::Completed);
                    let closed = close_iterator(
                        ctx,
                        state,
                        underlying.iterator,
                        value::encode_undefined(),
                        false,
                    );
                    if value::is_exception(closed) {
                        return closed;
                    }
                    return create_iterator_result(ctx, state, value::encode_undefined(), true);
                }
                if remaining.is_finite()
                    && let Some(slot) = state.iterator_helpers.helpers.get_mut(&handle)
                {
                    slot.kind = HelperKind::Take(remaining - 1.0);
                }
                let stepped = match step_value(ctx, state, &underlying) {
                    Err(exception) => return finish_throw(state, handle, exception),
                    Ok(None) => return finish_done(ctx, state, handle),
                    Ok(Some(stepped)) => stepped,
                };
                return yield_value(ctx, state, handle, stepped);
            }
            HelperKind::Drop(remaining) => {
                if remaining > 0.0 {
                    if remaining.is_finite()
                        && let Some(slot) = state.iterator_helpers.helpers.get_mut(&handle)
                    {
                        slot.kind = HelperKind::Drop(remaining - 1.0);
                    }
                    // 跳过阶段按 IteratorStep 只读 done：不触发 value getter。
                    match step_has_value(ctx, state, &underlying) {
                        Err(exception) => return finish_throw(state, handle, exception),
                        Ok(false) => return finish_done(ctx, state, handle),
                        Ok(true) => {}
                    }
                    continue;
                }
                let stepped = match step_value(ctx, state, &underlying) {
                    Err(exception) => return finish_throw(state, handle, exception),
                    Ok(None) => return finish_done(ctx, state, handle),
                    Ok(Some(stepped)) => stepped,
                };
                return yield_value(ctx, state, handle, stepped);
            }
            HelperKind::FlatMap(callback) => {
                if let Some(inner) = entry.inner {
                    // 内层步进异常按 IfAbruptCloseIterator 关闭外层（§27.1.4.8
                    // 步骤 viii.2）。
                    match step_value(ctx, state, &inner) {
                        Err(exception) => {
                            return close_underlying_throw(
                                ctx,
                                state,
                                handle,
                                underlying.iterator,
                                exception,
                            );
                        }
                        Ok(None) => {
                            if let Some(slot) = state.iterator_helpers.helpers.get_mut(&handle) {
                                slot.inner = None;
                            }
                            continue;
                        }
                        Ok(Some(stepped)) => return yield_value(ctx, state, handle, stepped),
                    }
                }
                let stepped = match step_value(ctx, state, &underlying) {
                    Err(exception) => return finish_throw(state, handle, exception),
                    Ok(None) => return finish_done(ctx, state, handle),
                    Ok(Some(stepped)) => stepped,
                };
                let counter = entry.counter;
                bump_counter(state, handle);
                let mapped = match call_callback(ctx, state, callback, stepped, counter) {
                    Err(exception) => {
                        return close_underlying_throw(
                            ctx,
                            state,
                            handle,
                            underlying.iterator,
                            exception,
                        );
                    }
                    Ok(mapped) => mapped,
                };
                let inner = match get_iterator_flattenable(
                    ctx,
                    state,
                    mapped,
                    PrimitiveHandling::RejectPrimitives,
                    "Iterator.prototype.flatMap called on non-object",
                ) {
                    Err(exception) => {
                        return close_underlying_throw(
                            ctx,
                            state,
                            handle,
                            underlying.iterator,
                            exception,
                        );
                    }
                    Ok(record) => record,
                };
                if let Some(slot) = state.iterator_helpers.helpers.get_mut(&handle) {
                    slot.inner = Some(inner);
                }
            }
        }
    }
}
