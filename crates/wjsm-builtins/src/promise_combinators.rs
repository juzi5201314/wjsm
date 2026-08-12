//! Promise.all / Promise.race / Promise.allSettled / Promise.any
//! 后端无关算法实现（§27.2.4.1-4）。
//!
//! 算法逻辑通过 `<E: ExecContext>` 泛型单态化，零 vtable 开销。
//! native host 注册层仅做上下文适配并委托调用。

use wjsm_host::{
    ExecContext, PromiseCombinatorReactionKind, PromiseEntry, PromiseReaction, PromiseSettlement,
    PromiseState, ReactionType, Value,
};
use wjsm_ir::value;

/// §27.2.4.1-4：把非原生-promise 的 thenable 元素转为中间 promise（adopt 其状态），
/// 返回该中间 promise 的值，使后续逻辑按 pending 原生 promise 统一处理。
/// 原生 promise 与普通（非 thenable）值返回 None，调用方按既有路径处理。
fn thenable_to_intermediate_promise<E: ExecContext>(ctx: &mut E, elem: Value) -> Option<Value> {
    if ctx.is_promise_value(elem) {
        return None;
    }
    if !ctx.is_thenable(elem) {
        return None;
    }
    let inter = ctx.alloc_promise();
    ctx.resolve_promise(inter, elem);
    Some(inter)
}

/// 分配 pending 结果 promise；`constructor` 非 undefined/null 时记录 species constructor。
fn alloc_result_promise<E: ExecContext>(ctx: &mut E, constructor: Value) -> Value {
    let mut entry = PromiseEntry::pending();
    if !value::is_undefined(constructor) && !value::is_null(constructor) {
        entry.constructor_handle = Some(constructor);
    }
    ctx.alloc_promise_with_entry(entry)
}

/// 为 pending 元素挂接 combinator reaction handler（fulfill + reject 各一）。
fn attach_combinator_reactions<E: ExecContext>(
    ctx: &mut E,
    elem: Value,
    context: u32,
    index: usize,
    result_handle: Value,
    fulfill_kind: PromiseCombinatorReactionKind,
    reject_kind: PromiseCombinatorReactionKind,
) {
    let fulfill_handler = ctx.create_combinator_reaction_handler(context, index, fulfill_kind);
    let reject_handler = ctx.create_combinator_reaction_handler(context, index, reject_kind);
    ctx.push_promise_reaction(
        elem,
        PromiseReaction::new(fulfill_handler, result_handle, ReactionType::Fulfill),
        true,
    );
    ctx.push_promise_reaction(
        elem,
        PromiseReaction::new(reject_handler, result_handle, ReactionType::Reject),
        false,
    );
}

/// §27.2.4.1 `Promise.all(C, iterable)`（元素已物化为数组）。
pub fn promise_all_impl<E: ExecContext>(ctx: &mut E, constructor: Value, arr: Value) -> Value {
    let Some(len) = ctx.array_read_length(arr) else {
        let mut entry = PromiseEntry::rejected(value::encode_undefined());
        if !value::is_undefined(constructor) && !value::is_null(constructor) {
            entry.constructor_handle = Some(constructor);
        }
        return ctx.alloc_promise_with_entry(entry);
    };
    let result_promise = alloc_result_promise(ctx, constructor);

    if len == 0 {
        let empty_arr = ctx.alloc_array(0);
        ctx.array_write_length(empty_arr, 0);
        ctx.settle_promise(result_promise, PromiseSettlement::Fulfill(empty_arr));
        return result_promise;
    }

    let result_array = ctx.alloc_array(len);
    ctx.array_write_length(result_array, len);
    let context = ctx.create_combinator_context(result_promise, result_array);
    let result_handle = ctx.raw_promise_handle(result_promise) as i64;
    let elems: Vec<Value> = (0..len)
        .map(|i| {
            ctx.array_elem_at(arr, i)
                .unwrap_or_else(value::encode_undefined)
        })
        .collect();
    let mut remaining = 0usize;
    let mut rejected = None;

    for (index, elem) in elems.iter().copied().enumerate() {
        if rejected.is_some() {
            break;
        }
        let elem = thenable_to_intermediate_promise(ctx, elem).unwrap_or(elem);
        let mut fulfilled = None;
        let mut rejected_elem = None;
        let mut pending = false;

        if value::is_object(elem) && ctx.is_promise_value(elem) {
            // §27.2.4.1.1 — 标记所有已知 promise 为已处理
            ctx.mark_promise_handled(elem);
            match ctx.promise_state(elem) {
                PromiseState::Fulfilled(value) => fulfilled = Some(value),
                PromiseState::Rejected(reason) => rejected_elem = Some(reason),
                PromiseState::Pending => {
                    pending = true;
                    attach_combinator_reactions(
                        ctx,
                        elem,
                        context,
                        index,
                        result_handle,
                        PromiseCombinatorReactionKind::AllFulfill,
                        PromiseCombinatorReactionKind::AllReject,
                    );
                }
            }
        }

        if pending {
            remaining += 1;
            ctx.increment_combinator_outstanding_settlements(context);
        } else if let Some(reason) = rejected_elem {
            rejected.get_or_insert(reason);
        } else {
            let value = fulfilled.unwrap_or(elem);
            ctx.array_write_elem(result_array, index as u32, value);
        }
    }

    ctx.set_combinator_remaining(context, remaining);
    if let Some(reason) = rejected {
        ctx.mark_combinator_settled(context);
        ctx.settle_promise(result_promise, PromiseSettlement::Reject(reason));
        ctx.try_recycle_combinator_context(context);
    } else if remaining == 0 {
        ctx.mark_combinator_settled(context);
        ctx.settle_promise(result_promise, PromiseSettlement::Fulfill(result_array));
        ctx.try_recycle_combinator_context(context);
    }

    result_promise
}

/// §27.2.4.5 `Promise.race(C, iterable)`（元素已物化为数组）。
pub fn promise_race_impl<E: ExecContext>(ctx: &mut E, constructor: Value, arr: Value) -> Value {
    let result_promise = alloc_result_promise(ctx, constructor);
    let result_handle = ctx.raw_promise_handle(result_promise) as i64;
    let Some(len) = ctx.array_read_length(arr) else {
        ctx.settle_promise(
            result_promise,
            PromiseSettlement::Reject(value::encode_undefined()),
        );
        return result_promise;
    };

    for index in 0..len {
        let elem = ctx
            .array_elem_at(arr, index)
            .unwrap_or_else(value::encode_undefined);
        let elem = thenable_to_intermediate_promise(ctx, elem).unwrap_or(elem);
        if value::is_object(elem) {
            if ctx.is_promise_value(elem) {
                // 标记所有已知 promise 为已处理
                ctx.mark_promise_handled(elem);
                match ctx.promise_state(elem) {
                    PromiseState::Fulfilled(value) => {
                        ctx.settle_promise(result_promise, PromiseSettlement::Fulfill(value));
                        return result_promise;
                    }
                    PromiseState::Rejected(reason) => {
                        ctx.settle_promise(result_promise, PromiseSettlement::Reject(reason));
                        return result_promise;
                    }
                    PromiseState::Pending => {
                        ctx.push_promise_reaction(
                            elem,
                            PromiseReaction::new(
                                value::encode_undefined(),
                                result_handle,
                                ReactionType::Fulfill,
                            ),
                            true,
                        );
                        ctx.push_promise_reaction(
                            elem,
                            PromiseReaction::new(
                                value::encode_undefined(),
                                result_handle,
                                ReactionType::Reject,
                            ),
                            false,
                        );
                    }
                }
            } else {
                // #166: 对象但非原生 promise — 走 Promise.resolve(C, x) 语义，
                // 由 resolve_promise 统一处理 thenable adopt / 普通对象 fulfill，
                // 不再把原始对象当作立即值。
                ctx.resolve_promise(result_promise, elem);
                return result_promise;
            }
        } else {
            ctx.resolve_promise(result_promise, elem);
            return result_promise;
        }
    }
    result_promise
}

/// §27.2.4.2 `Promise.allSettled(C, iterable)`（元素已物化为数组）。
pub fn promise_all_settled_impl<E: ExecContext>(
    ctx: &mut E,
    constructor: Value,
    arr: Value,
) -> Value {
    let result_promise = alloc_result_promise(ctx, constructor);
    let Some(len) = ctx.array_read_length(arr) else {
        ctx.settle_promise(
            result_promise,
            PromiseSettlement::Reject(value::encode_undefined()),
        );
        return result_promise;
    };
    let result_array = ctx.alloc_array(len);
    ctx.array_write_length(result_array, len);
    let context = ctx.create_combinator_context(result_promise, result_array);
    let result_handle = ctx.raw_promise_handle(result_promise) as i64;
    let elems: Vec<Value> = (0..len)
        .map(|i| {
            ctx.array_elem_at(arr, i)
                .unwrap_or_else(value::encode_undefined)
        })
        .collect();
    let mut remaining = 0usize;

    for (index, elem) in elems.iter().copied().enumerate() {
        let elem = thenable_to_intermediate_promise(ctx, elem).unwrap_or(elem);
        let mut outcome = Some(("fulfilled", "value", elem));
        let mut pending = false;

        if value::is_object(elem) && ctx.is_promise_value(elem) {
            // 标记所有已知 promise 为已处理
            ctx.mark_promise_handled(elem);
            match ctx.promise_state(elem) {
                PromiseState::Fulfilled(value) => outcome = Some(("fulfilled", "value", value)),
                PromiseState::Rejected(reason) => outcome = Some(("rejected", "reason", reason)),
                PromiseState::Pending => {
                    pending = true;
                    outcome = None;
                    attach_combinator_reactions(
                        ctx,
                        elem,
                        context,
                        index,
                        result_handle,
                        PromiseCombinatorReactionKind::AllSettledFulfill,
                        PromiseCombinatorReactionKind::AllSettledReject,
                    );
                }
            }
        }

        if pending {
            remaining += 1;
            ctx.increment_combinator_outstanding_settlements(context);
            continue;
        }

        if let Some((status, value_name, value)) = outcome {
            // GC：同步 allSettled 路径可能在输入 promise 已结算时直接分配
            // result record。先把 value/reason 暂存到已 root 的结果数组，
            // 避免分配期间 Rust 栈上的 JS handle 丢失。
            ctx.array_write_elem(result_array, index as u32, value);
            let record = ctx.alloc_all_settled_result(status, value_name, value);
            ctx.array_write_elem(result_array, index as u32, record);
        }
    }

    ctx.set_combinator_remaining(context, remaining);
    if remaining == 0 {
        ctx.settle_promise(result_promise, PromiseSettlement::Fulfill(result_array));
        ctx.mark_combinator_settled(context);
        ctx.try_recycle_combinator_context(context);
    }
    result_promise
}

/// §27.2.4.3 `Promise.any(C, iterable)`（元素已物化为数组）。
pub fn promise_any_impl<E: ExecContext>(ctx: &mut E, constructor: Value, arr: Value) -> Value {
    let result_promise = alloc_result_promise(ctx, constructor);
    let result_handle = ctx.raw_promise_handle(result_promise) as i64;
    let Some(len) = ctx.array_read_length(arr) else {
        ctx.settle_promise(
            result_promise,
            PromiseSettlement::Reject(value::encode_undefined()),
        );
        return result_promise;
    };
    let errors_array = ctx.alloc_array(len);
    ctx.array_write_length(errors_array, len);
    if len == 0 {
        let aggregate = ctx.alloc_aggregate_error(errors_array);
        ctx.settle_promise(result_promise, PromiseSettlement::Reject(aggregate));
        return result_promise;
    }

    let context = ctx.create_combinator_context(result_promise, errors_array);
    let elems: Vec<Value> = (0..len)
        .map(|i| {
            ctx.array_elem_at(arr, i)
                .unwrap_or_else(value::encode_undefined)
        })
        .collect();
    let mut remaining = len as usize;
    let mut fulfilled = None;

    for (index, elem) in elems.iter().copied().enumerate() {
        let elem = thenable_to_intermediate_promise(ctx, elem).unwrap_or(elem);
        let mut rejected_reason = None;
        let mut pending = false;
        let mut known_promise = false;

        if value::is_object(elem) && ctx.is_promise_value(elem) {
            known_promise = true;
            // 标记所有已知 promise 为已处理
            ctx.mark_promise_handled(elem);
            match ctx.promise_state(elem) {
                PromiseState::Fulfilled(value) => fulfilled = Some(value),
                PromiseState::Rejected(reason) => rejected_reason = Some(reason),
                PromiseState::Pending => {
                    pending = true;
                    attach_combinator_reactions(
                        ctx,
                        elem,
                        context,
                        index,
                        result_handle,
                        PromiseCombinatorReactionKind::AnyFulfill,
                        PromiseCombinatorReactionKind::AnyReject,
                    );
                }
            }
        }

        if fulfilled.is_some() {
            break;
        }
        if pending {
            ctx.increment_combinator_outstanding_settlements(context);
            continue;
        }
        if let Some(reason) = rejected_reason {
            ctx.array_write_elem(errors_array, index as u32, reason);
            remaining = remaining.saturating_sub(1);
        } else if !known_promise {
            fulfilled = Some(elem);
            break;
        }
    }

    ctx.set_combinator_remaining(context, remaining);
    if let Some(value) = fulfilled {
        ctx.mark_combinator_settled(context);
        ctx.settle_promise(result_promise, PromiseSettlement::Fulfill(value));
        ctx.try_recycle_combinator_context(context);
    } else if remaining == 0 {
        ctx.mark_combinator_settled(context);
        let aggregate = ctx.alloc_aggregate_error(errors_array);
        ctx.settle_promise(result_promise, PromiseSettlement::Reject(aggregate));
        ctx.try_recycle_combinator_context(context);
    }
    result_promise
}
