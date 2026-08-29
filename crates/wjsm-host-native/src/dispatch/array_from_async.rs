//! `Array.fromAsync`（proposal-array-from-async Stage 3，Node v22 已发布）
//! 的宿主异步状态机。
//!
//! 整个「fromAsyncClosure」是跨微任务的状态机：Await 点以
//! `NativePromiseReaction::ArrayFromAsync` 续点挂到 promise 上，操作状态存
//! `NativeAgentState::array_from_async` 侧表（GC 根见 `extend_gc_roots`）。
//! tick 序与 V8 的 array-from-async.tq 逐点对齐：
//! - 首个 next / Get 在调用当轮同步执行，其后每个 Await 恰好 1 tick；
//! - 同步迭代器经 CreateAsyncFromSyncIterator 包裹（§27.1.6），每元素
//!   2 tick（unwrap 闭包 + fromAsync 续点）；
//! - 可迭代路径上任何错误（含 next 结果 promise 拒绝）先走
//!   AsyncIteratorClose 再以原错误拒绝（V8 形态，比提案文本激进）；
//! - close 期间 return 侧自身出错时按 AsyncIteratorClose（§7.4.10）吞掉
//!   并以原错误拒绝，不复刻 V8 让结果 promise 永不结算的缺陷（v8:13321）。

use std::collections::{HashMap, VecDeque};

use wjsm_ir::value;
use wjsm_native_abi::NativeVmContext;

use super::promise::{
    NativePromiseReaction, enqueue_reaction_microtask, new_promise, observe_reaction,
    promise_resolve_value, settle_promise,
};
use super::runtime::{
    allocate_object_or_out_of_memory, default_call_site, fail_dispatch, get_on_nullish_base,
    get_property, is_truthy, range_error, render_value, to_number_coerced, type_error,
};
use crate::NativeAgentState;

/// 迭代来源：GetMethod 协商结果（步骤 c–g）。
#[derive(Clone, Copy)]
enum FromAsyncSource {
    /// 异步迭代器记录（GetIteratorFromMethod on @@asyncIterator）。
    Async { iterator: i64, next: i64 },
    /// CreateAsyncFromSyncIterator 包裹的同步迭代器记录（next 建包时取定）。
    Sync { iterator: i64, next: i64 },
    /// array-like 回退（步骤 i）：对象 + LengthOfArrayLike。
    ArrayLike { object: i64, length: u32 },
}

/// 在飞 fromAsync 操作：一个结果 promise 对应一条记录，结算即移除。
pub(crate) struct FromAsyncOperation {
    /// 结果 promise（编码值，兼作 GC 根）。
    result: i64,
    source: FromAsyncSource,
    /// mapfn（入口已验证可调用）。
    map: Option<i64>,
    this_arg: i64,
    /// 结果数组 A（迭代器路径追加式，array-like 路径预填洞）。
    array: i64,
    /// 当前下标 k。
    index: u32,
}

/// Await 续点相位：reaction 触发后从这里恢复状态机。
#[derive(Clone, Copy)]
pub(crate) enum FromAsyncPhase {
    /// 异步迭代器路径 Await(nextResult)（步骤 h.iv.4）。
    NextResult,
    /// 同步包裹：valueWrapper 的 unwrap 闭包 tick（§27.1.6.4 步骤 9–12）。
    SyncUnwrap { done: bool },
    /// 同步包裹：wrapper promise 结算送达 fromAsync 续点（值已解包）。
    SyncNextValue { done: bool },
    /// Await(mappedValue)（步骤 h.iv.9.c，拒绝须 close）。
    Mapped,
    /// array-like 路径 Await(kValue)（步骤 i.vii.3）。
    ArrayLikeValue,
    /// array-like 路径 Await(mappedValue)（步骤 i.vii.4.b）。
    ArrayLikeMapped,
    /// 同步包裹 close：模拟 wrapper.return() 的 unwrap tick，随后以原错误拒绝。
    SyncCloseUnwrap { error: i64 },
    /// AsyncIteratorClose 的 Await(innerResult)（§7.4.10 步骤 4.d）后以原错误拒绝。
    CloseThenReject { error: i64 },
}

/// `Array.fromAsync(asyncItems[, mapfn[, thisArg]])` 宿主入口：同步段
/// （mapfn 检查、迭代器协商、首个 next / Get）在本调用内执行，其余由
/// promise reaction 驱动；闭包内任何异常拒绝结果 promise 而非同步抛出。
pub(super) fn from_async(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let items = args.first().copied().unwrap_or_else(value::encode_undefined);
    let map = args.get(1).copied().filter(|map| !value::is_undefined(*map));
    let this_arg = args.get(2).copied().unwrap_or_else(value::encode_undefined);
    let Some(result) = new_promise(ctx, state) else {
        return fail_dispatch(ctx);
    };
    let roots_base = state.temporary_roots.len();
    state.temporary_roots.push(result);
    state.temporary_roots.push(items);
    state.temporary_roots.extend(map);
    state.temporary_roots.push(this_arg);
    if let Err(exception) = start(ctx, state, result, items, map, this_arg) {
        settle_promise(state, value::decode_handle(result), exception, true);
    }
    state.temporary_roots.truncate(roots_base);
    result
}

/// fromAsyncClosure 同步段（步骤 2–3.h/i 的首步）。
fn start(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    result: i64,
    items: i64,
    map: Option<i64>,
    this_arg: i64,
) -> Result<(), i64> {
    // 2.i：IsCallable(mapfn) 为 false 抛 TypeError（V8 kCalledNonCallable，
    // 先于任何 items 属性读取）。
    if let Some(map) = map
        && !state.is_callable_value(map)
    {
        let rendered = super::iterator_helpers::render_receiver(state, map);
        return Err(type_error(ctx, state, &format!("{rendered} is not a function")));
    }
    let source = match get_iteration_method(ctx, state, items, wjsm_ir::wk_symbol::ASYNC_ITERATOR)?
    {
        Some(method) => iterator_record(ctx, state, items, method, true)?,
        None => match get_iteration_method(ctx, state, items, wjsm_ir::wk_symbol::ITERATOR)? {
            Some(method) => iterator_record(ctx, state, items, method, false)?,
            None => return start_array_like(ctx, state, result, items, map, this_arg),
        },
    };
    // h.i–iii：A = ArrayCreate(0)，k = 0，踏出首个 kGetIteratorStep。
    let Ok(array) = state.allocate_array_values_with_gc_retry(ctx, &[]) else {
        return Err(fail_dispatch(ctx));
    };
    state.temporary_roots.push(array);
    let id = register(state, FromAsyncOperation { result, source, map, this_arg, array, index: 0 });
    step_iterator(ctx, state, id);
    Ok(())
}

/// GetMethod(items, @@asyncIterator / @@iterator)（步骤 c–d）：nullish 基座
/// 与非可调用错误文案对齐 V8（kFirstArgument*IteratorSymbolNonCallable）。
fn get_iteration_method(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    items: i64,
    symbol: u32,
) -> Result<Option<i64>, i64> {
    let key = value::encode_handle(value::TAG_SYMBOL, symbol);
    if let Some(exception) = get_on_nullish_base(ctx, state, items, key) {
        return Err(exception);
    }
    let method = match get_property(ctx, state, items, key) {
        Ok(method) if value::is_exception(method) => return Err(method),
        Ok(method) => method,
        Err(()) => return Err(fail_dispatch(ctx)),
    };
    if value::is_undefined(method) || value::is_null(method) {
        return Ok(None);
    }
    if !state.is_callable_value(method) {
        let name = if symbol == wjsm_ir::wk_symbol::ASYNC_ITERATOR {
            "asyncIterator"
        } else {
            "iterator"
        };
        return Err(type_error(
            ctx,
            state,
            &format!(
                "Array.fromAsync requires that the property of the first argument, \
                 items[Symbol.{name}], when exists, be a function"
            ),
        ));
    }
    Ok(Some(method))
}

/// GetIteratorFromMethod（§7.4.4）：Call(method) 结果必须为对象（V8 对同步 /
/// 异步统一渲染 Symbol.iterator），next 取一次、不做急可调用检查。
fn iterator_record(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    items: i64,
    method: i64,
    asynchronous: bool,
) -> Result<FromAsyncSource, i64> {
    let iterator = state
        .invoke_callable(ctx, method, items, &[])
        .unwrap_or_else(|| fail_dispatch(ctx));
    if value::is_exception(iterator) {
        return Err(iterator);
    }
    if !value::is_js_object(iterator) {
        return Err(type_error(
            ctx,
            state,
            "Result of the Symbol.iterator method is not an object",
        ));
    }
    state.temporary_roots.push(iterator);
    let next = get_named(ctx, state, iterator, "next")?;
    state.temporary_roots.push(next);
    Ok(if asynchronous {
        FromAsyncSource::Async { iterator, next }
    } else {
        FromAsyncSource::Sync { iterator, next }
    })
}

/// array-like 回退（步骤 i）：LengthOfArrayLike 后按 Construct(%Array%, len)
/// 语义预建洞数组（长度超上限按 V8 报 RangeError）。
fn start_array_like(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    result: i64,
    items: i64,
    map: Option<i64>,
    this_arg: i64,
) -> Result<(), i64> {
    let raw = get_named(ctx, state, items, "length")?;
    let number = to_number_coerced(ctx, state, raw)?;
    // ToLength（§7.1.20）：NaN / 负值归 0，上截 2^53-1。
    let length = if number.is_nan() || number <= 0.0 {
        0.0
    } else {
        number.trunc().min(9_007_199_254_740_991.0)
    };
    if length > f64::from(u32::MAX) {
        return Err(range_error(ctx, state, "Invalid array length"));
    }
    let array = allocate_hole_array(ctx, state, length as u32)?;
    state.temporary_roots.push(array);
    let source = FromAsyncSource::ArrayLike { object: items, length: length as u32 };
    let id = register(state, FromAsyncOperation { result, source, map, this_arg, array, index: 0 });
    array_like_step(ctx, state, id);
    Ok(())
}

/// `new Array(len)` 语义的洞数组（与 `array.allocate` 一致：显式填洞）。
fn allocate_hole_array(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    length: u32,
) -> Result<i64, i64> {
    let array = allocate_object_or_out_of_memory(ctx, state, length, true);
    if value::is_exception(array) {
        return Err(array);
    }
    let roots_base = state.temporary_roots.len();
    state.temporary_roots.push(array);
    let handle = value::decode_handle(array);
    for index in 0..length {
        if super::array_callbacks::set_element_with_gc_retry(
            ctx,
            state,
            handle,
            index,
            value::encode_array_hole() as u64,
        )
        .is_err()
        {
            state.temporary_roots.truncate(roots_base);
            return Err(fail_dispatch(ctx));
        }
    }
    state.temporary_roots.truncate(roots_base);
    Ok(array)
}

fn register(state: &mut NativeAgentState, operation: FromAsyncOperation) -> u32 {
    let id = state.array_from_async_next_id;
    state.array_from_async_next_id = state.array_from_async_next_id.wrapping_add(1);
    state.array_from_async.insert(id, operation);
    id
}

/// Await(value)（§6.2.9.3）：PromiseResolve + 宿主续点版 PerformPromiseThen。
fn await_with(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    id: u32,
    phase: FromAsyncPhase,
    awaited: i64,
) {
    let roots_base = state.temporary_roots.len();
    state.temporary_roots.push(awaited);
    let promise = promise_resolve_value(ctx, state, awaited);
    state.temporary_roots.truncate(roots_base);
    if value::is_exception(promise) {
        // 引擎分配失败（fail_dispatch 已置致命标记）：直接拒绝收尾。
        reject_operation(state, id, promise);
        return;
    }
    observe_reaction(
        state,
        value::decode_handle(promise),
        NativePromiseReaction::ArrayFromAsync { operation: id, phase },
    );
}

/// promise reaction 续点入口：按相位恢复状态机。
pub(crate) fn run_reaction(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    id: u32,
    phase: FromAsyncPhase,
    settled: i64,
    rejected: bool,
) -> i64 {
    if !state.array_from_async.contains_key(&id) {
        return value::encode_undefined();
    }
    match phase {
        FromAsyncPhase::NextResult if rejected => close_and_reject(ctx, state, id, settled),
        FromAsyncPhase::NextResult => handle_next_result(ctx, state, id, settled),
        // unwrap 闭包 tick：解包值经 wrapper promise 结算再走一跳送达续点。
        FromAsyncPhase::SyncUnwrap { done } => enqueue_reaction_microtask(
            state,
            NativePromiseReaction::ArrayFromAsync {
                operation: id,
                phase: FromAsyncPhase::SyncNextValue { done },
            },
            settled,
            rejected,
        ),
        FromAsyncPhase::SyncNextValue { .. } if rejected => {
            close_and_reject(ctx, state, id, settled)
        }
        FromAsyncPhase::SyncNextValue { done: true } => resolve_operation(state, id),
        FromAsyncPhase::SyncNextValue { done: false } => {
            continue_with_value(ctx, state, id, settled)
        }
        FromAsyncPhase::Mapped if rejected => close_and_reject(ctx, state, id, settled),
        FromAsyncPhase::Mapped => define_and_advance(ctx, state, id, settled),
        FromAsyncPhase::ArrayLikeValue if rejected => reject_operation(state, id, settled),
        FromAsyncPhase::ArrayLikeValue => array_like_continue(ctx, state, id, settled),
        FromAsyncPhase::ArrayLikeMapped if rejected => reject_operation(state, id, settled),
        FromAsyncPhase::ArrayLikeMapped => array_like_define(ctx, state, id, settled),
        // wrapper.return() 的 unwrap tick：结算形态无关，再一跳后以原错误拒绝。
        FromAsyncPhase::SyncCloseUnwrap { error } => enqueue_reaction_microtask(
            state,
            NativePromiseReaction::ArrayFromAsync {
                operation: id,
                phase: FromAsyncPhase::CloseThenReject { error },
            },
            settled,
            rejected,
        ),
        FromAsyncPhase::CloseThenReject { error } => reject_operation(state, id, error),
    }
    value::encode_undefined()
}

/// kGetIteratorStep：Call(iteratorRecord.[[NextMethod]]) 后 Await 结果；
/// 同步包裹改走 wrapper.next() 模拟。next 非可调用按 Call 语义抛（→ close）。
fn step_iterator(ctx: &mut NativeVmContext, state: &mut NativeAgentState, id: u32) {
    let Some(operation) = state.array_from_async.get(&id) else {
        return;
    };
    let (iterator, next, asynchronous) = match operation.source {
        FromAsyncSource::Async { iterator, next } => (iterator, next, true),
        FromAsyncSource::Sync { iterator, next } => (iterator, next, false),
        FromAsyncSource::ArrayLike { .. } => return,
    };
    if !asynchronous {
        match sync_wrapper_next(ctx, state, iterator, next) {
            // wrapper 内部错误使 wrapper promise 当即拒绝，fromAsync 续点
            // 下一 tick 观察到拒绝再走 close。
            Err(error) => enqueue_reaction_microtask(
                state,
                NativePromiseReaction::ArrayFromAsync {
                    operation: id,
                    phase: FromAsyncPhase::SyncNextValue { done: false },
                },
                error,
                true,
            ),
            Ok((unwrapped, done)) => {
                await_with(ctx, state, id, FromAsyncPhase::SyncUnwrap { done }, unwrapped)
            }
        }
        return;
    }
    if !state.is_callable_value(next) {
        let callsite = default_call_site(state, next);
        let error = type_error(ctx, state, &format!("{callsite} is not a function"));
        close_and_reject(ctx, state, id, error);
        return;
    }
    let result = state
        .invoke_callable(ctx, next, iterator, &[])
        .unwrap_or_else(|| fail_dispatch(ctx));
    if value::is_exception(result) {
        close_and_reject(ctx, state, id, result);
        return;
    }
    await_with(ctx, state, id, FromAsyncPhase::NextResult, result);
}

/// %AsyncFromSyncIteratorPrototype%.next（§27.1.6.2.1）同步段：调 sync next、
/// 校验结果对象、按 done → value 序读出（V8 LoadIteratorResult 慢路径序），
/// 返回待 PromiseResolve 的 value 与 ToBoolean(done)。
fn sync_wrapper_next(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    iterator: i64,
    next: i64,
) -> Result<(i64, bool), i64> {
    if !state.is_callable_value(next) {
        let callsite = default_call_site(state, next);
        return Err(type_error(ctx, state, &format!("{callsite} is not a function")));
    }
    let result = state
        .invoke_callable(ctx, next, iterator, &[])
        .unwrap_or_else(|| fail_dispatch(ctx));
    if value::is_exception(result) {
        return Err(result);
    }
    if !value::is_js_object(result) {
        let rendered = render_value(state, result);
        return Err(type_error(
            ctx,
            state,
            &format!("Iterator result {rendered} is not an object"),
        ));
    }
    let done = get_named(ctx, state, result, "done")?;
    let done = is_truthy(state, done);
    let unwrapped = get_named(ctx, state, result, "value")?;
    Ok((unwrapped, done))
}

/// kCheckIteratorValueAndMapping（异步路径）：非对象结果按 V8 实现怪癖以
/// 方法名渲染（kIteratorResultNotAnObject 传 'Array.fromAsync'）；done 为真
/// 直接结算，否则读 value 继续。
fn handle_next_result(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    id: u32,
    result: i64,
) {
    if !value::is_js_object(result) {
        let error = type_error(ctx, state, "Iterator result Array.fromAsync is not an object");
        close_and_reject(ctx, state, id, error);
        return;
    }
    let roots_base = state.temporary_roots.len();
    state.temporary_roots.push(result);
    let outcome = (|| -> Result<Option<i64>, i64> {
        let done = get_named(ctx, state, result, "done")?;
        if is_truthy(state, done) {
            return Ok(None);
        }
        Ok(Some(get_named(ctx, state, result, "value")?))
    })();
    state.temporary_roots.truncate(roots_base);
    match outcome {
        Err(error) => close_and_reject(ctx, state, id, error),
        Ok(None) => resolve_operation(state, id),
        Ok(Some(next_value)) => continue_with_value(ctx, state, id, next_value),
    }
}

/// 步骤 h.iv.9–10：mapping 时 Call(mapfn) 后 Await（异常 close），否则直接定值。
fn continue_with_value(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    id: u32,
    next_value: i64,
) {
    let Some(operation) = state.array_from_async.get(&id) else {
        return;
    };
    let (map, this_arg, index) = (operation.map, operation.this_arg, operation.index);
    let Some(map) = map else {
        define_and_advance(ctx, state, id, next_value);
        return;
    };
    let mapped = state
        .invoke_callable(ctx, map, this_arg, &[next_value, value::encode_f64(f64::from(index))])
        .unwrap_or_else(|| fail_dispatch(ctx));
    if value::is_exception(mapped) {
        close_and_reject(ctx, state, id, mapped);
        return;
    }
    await_with(ctx, state, id, FromAsyncPhase::Mapped, mapped);
}

/// 步骤 h.iv.11–13：CreateDataPropertyOrThrow(A, Pk)（宿主追加式数组不可失败，
/// 分配失败除外），k++ 后回到 kGetIteratorStep。
fn define_and_advance(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    id: u32,
    mapped: i64,
) {
    let Some(operation) = state.array_from_async.get(&id) else {
        return;
    };
    let handle = value::decode_handle(operation.array);
    if super::array_callbacks::push_element_with_gc_retry(ctx, state, handle, mapped as u64)
        .is_err()
    {
        let failure = fail_dispatch(ctx);
        reject_operation(state, id, failure);
        return;
    }
    if let Some(operation) = state.array_from_async.get_mut(&id) {
        operation.index += 1;
    }
    step_iterator(ctx, state, id);
}

/// kGetArrayLikeValue：k < len 时 Get(arrayLike, Pk) 后 Await，否则结算。
fn array_like_step(ctx: &mut NativeVmContext, state: &mut NativeAgentState, id: u32) {
    let Some(operation) = state.array_from_async.get(&id) else {
        return;
    };
    let FromAsyncSource::ArrayLike { object, length } = operation.source else {
        return;
    };
    let index = operation.index;
    if index >= length {
        resolve_operation(state, id);
        return;
    }
    let Some(key) = state.intern_text(index.to_string(), value::TAG_STRING) else {
        let failure = fail_dispatch(ctx);
        reject_operation(state, id, failure);
        return;
    };
    match get_property(ctx, state, object, key) {
        Ok(stored) if value::is_exception(stored) => reject_operation(state, id, stored),
        Ok(stored) => await_with(ctx, state, id, FromAsyncPhase::ArrayLikeValue, stored),
        Err(()) => {
            let failure = fail_dispatch(ctx);
            reject_operation(state, id, failure);
        }
    }
}

/// 步骤 i.vii.4：mapping 时 Call(mapfn) 后 Await（array-like 路径无 close）。
fn array_like_continue(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    id: u32,
    stored: i64,
) {
    let Some(operation) = state.array_from_async.get(&id) else {
        return;
    };
    let (map, this_arg, index) = (operation.map, operation.this_arg, operation.index);
    let Some(map) = map else {
        array_like_define(ctx, state, id, stored);
        return;
    };
    let mapped = state
        .invoke_callable(ctx, map, this_arg, &[stored, value::encode_f64(f64::from(index))])
        .unwrap_or_else(|| fail_dispatch(ctx));
    if value::is_exception(mapped) {
        reject_operation(state, id, mapped);
        return;
    }
    await_with(ctx, state, id, FromAsyncPhase::ArrayLikeMapped, mapped);
}

/// 步骤 i.vii.6–7：CreateDataPropertyOrThrow(A, Pk) 写入预填洞数组后推进。
fn array_like_define(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    id: u32,
    mapped: i64,
) {
    let Some(operation) = state.array_from_async.get(&id) else {
        return;
    };
    let handle = value::decode_handle(operation.array);
    let index = operation.index;
    if super::array_callbacks::set_element_with_gc_retry(ctx, state, handle, index, mapped as u64)
        .is_err()
    {
        let failure = fail_dispatch(ctx);
        reject_operation(state, id, failure);
        return;
    }
    if let Some(operation) = state.array_from_async.get_mut(&id) {
        operation.index += 1;
    }
    array_like_step(ctx, state, id);
}

/// kCloseAsyncIterator：可迭代路径任何错误先关闭迭代器再以原错误拒绝。
/// return 侧自身的异常按 AsyncIteratorClose（§7.4.10 步骤 5）吞掉。
fn close_and_reject(ctx: &mut NativeVmContext, state: &mut NativeAgentState, id: u32, error: i64) {
    let Some(operation) = state.array_from_async.get(&id) else {
        return;
    };
    let roots_base = state.temporary_roots.len();
    state.temporary_roots.push(error);
    match operation.source {
        FromAsyncSource::ArrayLike { .. } => reject_operation(state, id, error),
        FromAsyncSource::Async { iterator, .. } => match async_close_step(ctx, state, iterator) {
            // return 缺失 / 出错：立即以原错误拒绝（无额外 tick）。
            None => reject_operation(state, id, error),
            // Await(innerResult) 一跳后拒绝。
            Some(inner) => await_with(ctx, state, id, FromAsyncPhase::CloseThenReject { error }, inner),
        },
        FromAsyncSource::Sync { iterator, .. } => match sync_close_step(ctx, state, iterator) {
            // wrapper 内部异常：按规范吞掉立即拒绝（V8 在此挂起，不复刻）。
            SyncClose::Failed => reject_operation(state, id, error),
            // sync return 缺失：wrapper promise 当即解决，1 tick 后拒绝。
            SyncClose::Immediate => enqueue_reaction_microtask(
                state,
                NativePromiseReaction::ArrayFromAsync {
                    operation: id,
                    phase: FromAsyncPhase::CloseThenReject { error },
                },
                value::encode_undefined(),
                false,
            ),
            // sync return 结果的 value 经 unwrap（1 tick）+ 续点（1 tick）后拒绝。
            SyncClose::Unwrap(unwrapped) => {
                await_with(ctx, state, id, FromAsyncPhase::SyncCloseUnwrap { error }, unwrapped)
            }
        },
    }
    state.temporary_roots.truncate(roots_base);
}

/// 异步迭代器 close 首段：GetProperty(iterator, "return") 并调用。任何异常
/// 或缺失返回 None（原错误胜出）；成功返回待 Await 的 innerResult。
fn async_close_step(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    iterator: i64,
) -> Option<i64> {
    let method = get_named(ctx, state, iterator, "return").ok()?;
    if value::is_undefined(method) || value::is_null(method) || !state.is_callable_value(method) {
        return None;
    }
    let inner = state
        .invoke_callable(ctx, method, iterator, &[])
        .unwrap_or_else(|| fail_dispatch(ctx));
    if value::is_exception(inner) {
        return None;
    }
    Some(inner)
}

enum SyncClose {
    /// wrapper 内部异常（规范吞掉，原错误立即拒绝）。
    Failed,
    /// sync return 缺失：wrapper promise 当即解决。
    Immediate,
    /// sync return 结果的 value 待 unwrap。
    Unwrap(i64),
}

/// 模拟 %AsyncFromSyncIteratorPrototype%.return（§27.1.6.2.2）同步段：
/// 取 sync return、调用、校验结果对象并按 done → value 序读出。
fn sync_close_step(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    iterator: i64,
) -> SyncClose {
    let Ok(method) = get_named(ctx, state, iterator, "return") else {
        return SyncClose::Failed;
    };
    if value::is_undefined(method) || value::is_null(method) {
        return SyncClose::Immediate;
    }
    if !state.is_callable_value(method) {
        return SyncClose::Failed;
    }
    let result = state
        .invoke_callable(ctx, method, iterator, &[])
        .unwrap_or_else(|| fail_dispatch(ctx));
    if value::is_exception(result) || !value::is_js_object(result) {
        return SyncClose::Failed;
    }
    let Ok(_done) = get_named(ctx, state, result, "done") else {
        return SyncClose::Failed;
    };
    let Ok(unwrapped) = get_named(ctx, state, result, "value") else {
        return SyncClose::Failed;
    };
    SyncClose::Unwrap(unwrapped)
}

/// kDoneAndResolvePromise：追加式 / 预填数组的 length 已就位，直接结算 A。
fn resolve_operation(state: &mut NativeAgentState, id: u32) {
    let Some(operation) = state.array_from_async.remove(&id) else {
        return;
    };
    settle_promise(state, value::decode_handle(operation.result), operation.array, false);
}

/// kRejectPromise：结果 promise 以 error 拒绝并移除操作记录。
fn reject_operation(state: &mut NativeAgentState, id: u32, error: i64) {
    let Some(operation) = state.array_from_async.remove(&id) else {
        return;
    };
    settle_promise(state, value::decode_handle(operation.result), error, true);
}

fn get_named(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    object: i64,
    name: &str,
) -> Result<i64, i64> {
    let Some(key) = state.intern_text(name.into(), value::TAG_STRING) else {
        return Err(fail_dispatch(ctx));
    };
    match get_property(ctx, state, object, key) {
        Ok(result) if value::is_exception(result) => Err(result),
        Ok(result) => Ok(result),
        Err(()) => Err(fail_dispatch(ctx)),
    }
}

/// 在飞操作持有的 JS 值并入 GC 根：结果 promise、结果数组、迭代来源、
/// mapfn / thisArg。续点相位携带的原错误由 reaction 根扫描负责。
pub(crate) fn extend_gc_roots(
    operations: &HashMap<u32, FromAsyncOperation>,
    queue: &mut VecDeque<i64>,
) {
    for operation in operations.values() {
        queue.push_back(operation.result);
        queue.push_back(operation.array);
        queue.push_back(operation.this_arg);
        queue.extend(operation.map);
        match operation.source {
            FromAsyncSource::Async { iterator, next } | FromAsyncSource::Sync { iterator, next } => {
                queue.push_back(iterator);
                queue.push_back(next);
            }
            FromAsyncSource::ArrayLike { object, .. } => queue.push_back(object),
        }
    }
}

/// 续点相位携带的 JS 值（close 路径的原错误）并入 GC 根。
pub(crate) fn extend_phase_roots(phase: &FromAsyncPhase, queue: &mut VecDeque<i64>) {
    match phase {
        FromAsyncPhase::SyncCloseUnwrap { error } | FromAsyncPhase::CloseThenReject { error } => {
            queue.push_back(*error);
        }
        FromAsyncPhase::NextResult
        | FromAsyncPhase::SyncUnwrap { .. }
        | FromAsyncPhase::SyncNextValue { .. }
        | FromAsyncPhase::Mapped
        | FromAsyncPhase::ArrayLikeValue
        | FromAsyncPhase::ArrayLikeMapped => {}
    }
}
