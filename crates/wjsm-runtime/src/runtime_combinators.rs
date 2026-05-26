use super::*;

pub(crate) fn create_combinator_context(
    state: &RuntimeState,
    result_promise: i64,
    result_array: i64,
) -> usize {
    let mut contexts = state
        .combinator_contexts
        .lock()
        .expect("combinator context mutex");
    let idx = contexts.len();
    contexts.push(CombinatorContext {
        result_promise,
        result_array,
        remaining: 0,
        settled: false,
    });
    idx
}

pub(crate) fn set_combinator_remaining(state: &RuntimeState, context: usize, remaining: usize) {
    if let Some(entry) = state
        .combinator_contexts
        .lock()
        .expect("combinator context mutex")
        .get_mut(context)
    {
        entry.remaining = remaining;
    }
}

pub(crate) fn mark_combinator_settled(state: &RuntimeState, context: usize) {
    if let Some(entry) = state
        .combinator_contexts
        .lock()
        .expect("combinator context mutex")
        .get_mut(context)
    {
        entry.settled = true;
    }
}

pub(crate) fn create_combinator_reaction_handler(
    state: &RuntimeState,
    context: usize,
    index: usize,
    kind: PromiseCombinatorReactionKind,
) -> i64 {
    create_native_callable(
        state,
        NativeCallable::PromiseCombinatorReaction {
            context,
            index,
            kind,
        },
    )
}

pub(crate) fn combinator_reaction_record(
    state: &RuntimeState,
    handler: i64,
) -> Option<(usize, usize, PromiseCombinatorReactionKind)> {
    if !value::is_native_callable(handler) {
        return None;
    }
    let idx = value::decode_native_callable_idx(handler) as usize;
    let record = state
        .native_callables
        .lock()
        .expect("native callable table mutex")
        .get(idx)
        .cloned()?;
    match record {
        NativeCallable::PromiseCombinatorReaction {
            context,
            index,
            kind,
        } => Some((context, index, kind)),
        _ => None,
    }
}

pub(crate) fn open_combinator_context(state: &RuntimeState, context: usize) -> Option<(i64, i64)> {
    let contexts = state
        .combinator_contexts
        .lock()
        .expect("combinator context mutex");
    let entry = contexts.get(context)?;
    if entry.settled {
        None
    } else {
        Some((entry.result_promise, entry.result_array))
    }
}

pub(crate) fn decrement_combinator_remaining(
    state: &RuntimeState,
    context: usize,
) -> Option<(i64, i64)> {
    let mut contexts = state
        .combinator_contexts
        .lock()
        .expect("combinator context mutex");
    let entry = contexts.get_mut(context)?;
    if entry.settled {
        return None;
    }
    entry.remaining = entry.remaining.saturating_sub(1);
    if entry.remaining == 0 {
        entry.settled = true;
        Some((entry.result_promise, entry.result_array))
    } else {
        None
    }
}

pub(crate) fn handle_combinator_reaction<C: AsContextMut<Data = RuntimeState> + RuntimeStateAccess>(
    ctx: &mut C,
    env: &WasmEnv,
    handler: i64,
    argument: i64,
) -> bool {
    let (context, index, kind) = match {
        let state = ctx.state_mut();
        combinator_reaction_record(state, handler)
    } {
        Some(record) => record,
        None => return false,
    };
    let state = ctx.state_mut();
    recycle_native_callable(state, handler);
    let (_, result_array) = match {
        let state = ctx.state_mut();
        open_combinator_context(state, context)
    } {
        Some(record) => record,
        None => return true,
    };

    match kind {
        PromiseCombinatorReactionKind::AllFulfill => {
            if let Some(result_ptr) = resolve_array_ptr_with_env(ctx, env, result_array) {
                write_array_elem_with_env(ctx, env, result_ptr, index as u32, argument);
            }
            if let Some((result_promise, result_array)) = {
                let state = ctx.state_mut();
                decrement_combinator_remaining(state, context)
            } {
                settle_promise(
                    ctx.state_mut(),
                    result_promise,
                    PromiseSettlement::Fulfill(result_array),
                );
            }
        }
        PromiseCombinatorReactionKind::AllSettledFulfill
        | PromiseCombinatorReactionKind::AllSettledReject => {
            let (status, value_name) = match kind {
                PromiseCombinatorReactionKind::AllSettledFulfill => ("fulfilled", "value"),
                PromiseCombinatorReactionKind::AllSettledReject => ("rejected", "reason"),
                _ => unreachable!(),
            };
            let record =
                crate::runtime_heap::alloc_all_settled_result(ctx, env, status, value_name, argument);
            if let Some(result_ptr) = resolve_array_ptr_with_env(ctx, env, result_array) {
                write_array_elem_with_env(ctx, env, result_ptr, index as u32, record);
            }
            if let Some((result_promise, result_array)) = {
                let state = ctx.state_mut();
                decrement_combinator_remaining(state, context)
            } {
                settle_promise(
                    ctx.state_mut(),
                    result_promise,
                    PromiseSettlement::Fulfill(result_array),
                );
            }
        }
        PromiseCombinatorReactionKind::AnyReject => {
            if let Some(errors_ptr) = resolve_array_ptr_with_env(ctx, env, result_array) {
                write_array_elem_with_env(ctx, env, errors_ptr, index as u32, argument);
            }
            if let Some((result_promise, errors_array)) = {
                let state = ctx.state_mut();
                decrement_combinator_remaining(state, context)
            } {
                let aggregate =
                    crate::runtime_heap::alloc_heap_aggregate_error(ctx, env, errors_array);
                settle_promise(
                    ctx.state_mut(),
                    result_promise,
                    PromiseSettlement::Reject(aggregate),
                );
            }
        }
    }
    true
}
