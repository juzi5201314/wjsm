use wjsm_ir::value;
use wjsm_native_abi::NativeVmContext;

use super::{
    NativeMicrotask, NativePromiseCombinator, NativePromiseReaction, NativeScheduledReaction,
    PromiseCombinatorId, PromiseCombinatorKind, PromiseState, enqueue_microtask_with_context,
    new_promise, resolve_into, settle_promise,
};
use crate::NativeAgentState;
use crate::dispatch::runtime::fail_dispatch;

pub(super) fn run(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
    kind: PromiseCombinatorKind,
) -> i64 {
    let Some((source, length)) = input_array(ctx, state, args) else {
        return fail_dispatch(ctx);
    };
    let Some(target) = new_promise(ctx, state) else {
        return fail_dispatch(ctx);
    };
    let target_handle = value::decode_handle(target);
    if length == 0 {
        match kind {
            PromiseCombinatorKind::All | PromiseCombinatorKind::AllSettled => {
                let Ok(values) = state.allocate_array_values(&[]) else {
                    return fail_dispatch(ctx);
                };
                settle_promise(state, target_handle, values, false);
            }
            PromiseCombinatorKind::Any => {
                let Some(reason) =
                    super::aggregate_error_object(state, &[], "All promises were rejected".into())
                else {
                    return fail_dispatch(ctx);
                };
                settle_promise(state, target_handle, reason, true);
            }
        }
        return target;
    }

    let Ok(combinator) = u32::try_from(state.promise_combinators.len()).map(PromiseCombinatorId)
    else {
        return fail_dispatch(ctx);
    };
    state.promise_combinators.push(NativePromiseCombinator {
        kind,
        target_promise: target_handle,
        values: vec![value::encode_undefined(); length as usize],
        remaining: length,
        settled: false,
    });
    for index in 0..length {
        let element = array_element(state, source, index);
        let Some(source_promise) = promise_for_value(ctx, state, element) else {
            return fail_dispatch(ctx);
        };
        attach_reaction(
            state,
            source_promise,
            NativePromiseReaction::CombinatorElement { combinator, index },
        );
    }
    target
}

pub(super) fn race(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some((source, length)) = input_array(ctx, state, args) else {
        return fail_dispatch(ctx);
    };
    let Some(target) = new_promise(ctx, state) else {
        return fail_dispatch(ctx);
    };
    let target_handle = value::decode_handle(target);
    for index in 0..length {
        let element = array_element(state, source, index);
        let Some(source_promise) = promise_for_value(ctx, state, element) else {
            return fail_dispatch(ctx);
        };
        attach_reaction(
            state,
            source_promise,
            NativePromiseReaction::Handler {
                on_fulfilled: value::encode_undefined(),
                on_rejected: value::encode_undefined(),
                target_promise: target_handle,
            },
        );
    }
    target
}

pub(super) fn settle_element(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    combinator: PromiseCombinatorId,
    index: u32,
    result: i64,
    rejected: bool,
) -> i64 {
    let Some(entry) = state.promise_combinators.get(combinator.0 as usize) else {
        return fail_dispatch(ctx);
    };
    if entry.settled {
        return value::encode_undefined();
    }
    match entry.kind {
        PromiseCombinatorKind::All if rejected => {
            let target = entry.target_promise;
            state.promise_combinators[combinator.0 as usize].settled = true;
            settle_promise(state, target, result, true);
            value::encode_undefined()
        }
        PromiseCombinatorKind::All => {
            settle_value_slot(ctx, state, combinator, index, result, false)
        }
        PromiseCombinatorKind::AllSettled => {
            let Some(record) = all_settled_record(state, result, rejected) else {
                return fail_dispatch(ctx);
            };
            settle_value_slot(ctx, state, combinator, index, record, false)
        }
        PromiseCombinatorKind::Any if !rejected => {
            let target = entry.target_promise;
            state.promise_combinators[combinator.0 as usize].settled = true;
            settle_promise(state, target, result, false);
            value::encode_undefined()
        }
        PromiseCombinatorKind::Any => {
            settle_value_slot(ctx, state, combinator, index, result, true)
        }
    }
}

fn settle_value_slot(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    combinator: PromiseCombinatorId,
    index: u32,
    result: i64,
    aggregate_error_on_completion: bool,
) -> i64 {
    let (target, values) = {
        let Some(entry) = state.promise_combinators.get_mut(combinator.0 as usize) else {
            return fail_dispatch(ctx);
        };
        let Some(slot) = entry.values.get_mut(index as usize) else {
            return fail_dispatch(ctx);
        };
        *slot = result;
        entry.remaining = entry.remaining.saturating_sub(1);
        if entry.remaining != 0 {
            return value::encode_undefined();
        }
        entry.settled = true;
        (entry.target_promise, entry.values.clone())
    };
    if aggregate_error_on_completion {
        let Some(reason) =
            super::aggregate_error_object(state, &values, "All promises were rejected".into())
        else {
            return fail_dispatch(ctx);
        };
        settle_promise(state, target, reason, true);
    } else {
        let Ok(values) = state.allocate_array_values(&values) else {
            return fail_dispatch(ctx);
        };
        settle_promise(state, target, values, false);
    }
    value::encode_undefined()
}

fn input_array(
    _ctx: &mut NativeVmContext,
    state: &NativeAgentState,
    args: &[i64],
) -> Option<(u32, u32)> {
    let iterable = args.get(1).or_else(|| args.first()).copied()?;
    let source = value::is_array(iterable).then(|| value::decode_handle(iterable))?;
    let length = state.heap.array_length(source).ok()?;
    Some((source, length))
}

fn array_element(state: &NativeAgentState, source: u32, index: u32) -> i64 {
    state
        .heap
        .get_element(source, index)
        .ok()
        .flatten()
        .map(|element| element as i64)
        .filter(|element| !value::is_array_hole(*element))
        .unwrap_or_else(value::encode_undefined)
}

fn promise_for_value(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    input: i64,
) -> Option<u32> {
    let handle = value::decode_handle(input);
    if state.promises.contains_key(&handle) {
        return Some(handle);
    }
    let promise = new_promise(ctx, state)?;
    let handle = value::decode_handle(promise);
    resolve_into(ctx, state, handle, input);
    Some(handle)
}

fn attach_reaction(state: &mut NativeAgentState, promise: u32, reaction: NativePromiseReaction) {
    let Some(promise_state) = state.promises.get(&promise).map(|entry| entry.state) else {
        return;
    };
    let context = super::super::node_async_hooks::promise_context(state, promise)
        .unwrap_or_else(|| super::super::node_async_hooks::capture_context(state));
    super::mark_promise_handled(state, promise);
    match promise_state {
        PromiseState::Pending => state
            .promise_reactions
            .entry(promise)
            .or_default()
            .push(NativeScheduledReaction { reaction, context }),
        PromiseState::Fulfilled(value) => enqueue_microtask_with_context(
            state,
            NativeMicrotask::PromiseReaction {
                reaction,
                value,
                rejected: false,
            },
            context,
        ),
        PromiseState::Rejected(reason) => enqueue_microtask_with_context(
            state,
            NativeMicrotask::PromiseReaction {
                reaction,
                value: reason,
                rejected: true,
            },
            context,
        ),
    }
}

fn all_settled_record(state: &mut NativeAgentState, result: i64, rejected: bool) -> Option<i64> {
    let record = state.allocate_object(2, false).ok()?;
    let status = if rejected { "rejected" } else { "fulfilled" };
    let value_name = if rejected { "reason" } else { "value" };
    let status = state.intern_text(status.into(), value::TAG_STRING)?;
    set_named_property(state, record, "status", status)?;
    set_named_property(state, record, value_name, result)?;
    Some(record)
}

fn set_named_property(
    state: &mut NativeAgentState,
    object: i64,
    name: &str,
    stored: i64,
) -> Option<()> {
    let key = state.intern_text(name.into(), value::TAG_STRING)?;
    state
        .heap
        .set_property(
            value::decode_handle(object),
            value::decode_handle(key),
            stored as u64,
        )
        .ok()
}
