use std::collections::VecDeque;

use wjsm_ir::{Builtin, value};
use wjsm_native_abi::NativeVmContext;

use super::promise::{new_promise, settle_promise};
use super::runtime::{
    create_iterator_result, fail_dispatch, get_property, iterator_from, object_handle,
    property_key, type_error,
};
use crate::NativeAgentState;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AsyncGeneratorStatus {
    SuspendedStart,
    SuspendedYield,
    Executing,
    Completed,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum RequestKind {
    Next,
    Return,
    Throw,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct AsyncGeneratorRequest {
    pub(crate) kind: RequestKind,
    pub(crate) value: i64,
    pub(crate) promise: i64,
}

#[derive(Debug)]
pub(crate) struct NativeAsyncGenerator {
    pub(crate) continuation: i64,
    status: AsyncGeneratorStatus,
    pub(crate) active: Option<AsyncGeneratorRequest>,
    pub(crate) queue: VecDeque<AsyncGeneratorRequest>,
    pub(crate) resume_promise: Option<i64>,
}

pub(super) fn dispatch_async_generator(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    builtin: Builtin,
    args: &[i64],
) -> Option<i64> {
    Some(match builtin {
        Builtin::AsyncGeneratorStart => start(ctx, state, args),
        Builtin::AsyncGeneratorNext => request_or_yield(ctx, state, args),
        Builtin::AsyncGeneratorReturn => request_or_return(ctx, state, args),
        Builtin::AsyncGeneratorThrow => request_or_throw(ctx, state, args),
        Builtin::AsyncIteratorFrom => async_iterator_from(ctx, state, args),
        Builtin::IteratorNext if is_managed_async_iterator(state, args) => {
            iterator_next(ctx, state, args)
        }
        Builtin::IteratorNext => return None,
        _ => return None,
    })
}

pub(crate) fn method(state: &NativeAgentState, receiver: i64, key: &str) -> Option<Builtin> {
    is_async_generator(state, receiver).then(|| match key {
        "next" => Some(Builtin::AsyncGeneratorNext),
        "return" => Some(Builtin::AsyncGeneratorReturn),
        "throw" => Some(Builtin::AsyncGeneratorThrow),
        _ => None,
    })?
}

pub(crate) fn is_async_generator(state: &NativeAgentState, receiver: i64) -> bool {
    value::is_object(receiver)
        && state
            .async_generators
            .contains_key(&value::decode_object_handle(receiver))
}

fn is_managed_async_iterator(state: &NativeAgentState, args: &[i64]) -> bool {
    args.first().is_some_and(|source| {
        is_async_generator(state, *source)
            || state.async_iterator_objects.contains(source)
            || object_handle(*source)
                .is_some_and(|handle| state.async_from_sync_iterators.contains_key(&handle))
    })
}

fn wrap_sync_iterator(ctx: &mut NativeVmContext, state: &mut NativeAgentState, source: i64) -> i64 {
    let iterator = iterator_from(ctx, state, &[source]);
    if value::is_exception(iterator) {
        return iterator;
    }
    let Ok(wrapper) = state.allocate_object(0, false) else {
        return fail_dispatch(ctx);
    };
    state
        .async_from_sync_iterators
        .insert(value::decode_handle(wrapper), iterator);
    wrapper
}

fn ensure_prototypes(ctx: &mut NativeVmContext, state: &mut NativeAgentState) -> Option<i64> {
    if let Some(prototype) = state.async_generator_prototype {
        return Some(prototype);
    }
    let async_iterator_prototype = state.allocate_object(2, false).ok()?;
    let async_generator_prototype = state.allocate_object(0, false).ok()?;
    if state
        .heap
        .set_prototype(
            value::decode_handle(async_generator_prototype),
            value::decode_handle(async_iterator_prototype),
        )
        .is_err()
    {
        let _ = fail_dispatch(ctx);
        return None;
    }
    let name = state.intern_text("Symbol.toStringTag".into(), value::TAG_STRING)?;
    let tag = state.intern_text("AsyncIterator".into(), value::TAG_STRING)?;
    if state
        .heap
        .set_property(
            value::decode_handle(async_iterator_prototype),
            value::decode_handle(name),
            tag as u64,
        )
        .is_err()
    {
        let _ = fail_dispatch(ctx);
        return None;
    }
    let async_iterator =
        value::encode_handle(value::TAG_SYMBOL, wjsm_ir::wk_symbol::ASYNC_ITERATOR);
    let key = property_key(state, async_iterator)?;
    let callable = state.native_callable(crate::NativeCallableKind::Builtin(
        Builtin::ObjectProtoValueOf,
        true,
    ))?;
    if state
        .heap
        .set_property(
            value::decode_handle(async_iterator_prototype),
            key,
            callable as u64,
        )
        .is_err()
    {
        let _ = fail_dispatch(ctx);
        return None;
    }
    state.async_iterator_prototype = Some(async_iterator_prototype);
    state.async_generator_prototype = Some(async_generator_prototype);
    Some(async_generator_prototype)
}
fn async_iterator_from(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let source = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    if is_async_generator(state, source) {
        return source;
    }
    let async_iterator =
        value::encode_handle(value::TAG_SYMBOL, wjsm_ir::wk_symbol::ASYNC_ITERATOR);
    let method = match get_property(ctx, state, source, async_iterator) {
        Ok(method) => method,
        Err(()) => return wrap_sync_iterator(ctx, state, source),
    };
    if value::is_exception(method) {
        return method;
    }
    if value::is_undefined(method) {
        return wrap_sync_iterator(ctx, state, source);
    }
    if !value::is_callable(method) {
        return type_error(ctx, state, "async iterator method is not callable");
    }
    let iterator = state
        .invoke_callable(ctx, method, source, &[])
        .unwrap_or_else(|| fail_dispatch(ctx));
    if value::is_exception(iterator) {
        iterator
    } else if value::is_js_object(iterator) {
        state.async_iterator_objects.insert(iterator);
        iterator
    } else {
        type_error(ctx, state, "async iterator method did not return an object")
    }
}

fn iterator_next(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let source = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    if value::is_exception(source) {
        return source;
    }
    if let Some(iterator) = object_handle(source)
        .and_then(|handle| state.async_from_sync_iterators.get(&handle).copied())
    {
        return super::runtime::iterator_next_result(ctx, state, value::decode_handle(iterator));
    }
    if is_async_generator(state, source) {
        return enqueue(
            ctx,
            state,
            source,
            RequestKind::Next,
            value::encode_undefined(),
        );
    }
    let Some(next_key) = state.intern_text("next".into(), value::TAG_STRING) else {
        return fail_dispatch(ctx);
    };
    let next = match get_property(ctx, state, source, next_key) {
        Ok(next) if value::is_callable(next) => next,
        Ok(_) => return type_error(ctx, state, "iterator.next is not callable"),
        Err(()) => return type_error(ctx, state, "iterator.next is not callable"),
    };
    let result = state
        .invoke_callable(ctx, next, source, &args[1..])
        .unwrap_or_else(|| fail_dispatch(ctx));
    if value::is_exception(result) || value::is_js_object(result) {
        result
    } else {
        type_error(ctx, state, "iterator result is not an object")
    }
}

fn start(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(continuation) = args.first().copied() else {
        return fail_dispatch(ctx);
    };
    let continuation_handle = value::decode_handle(continuation);
    let Some(record) = state.continuations.get_mut(&continuation_handle) else {
        return fail_dispatch(ctx);
    };
    let Some([state_slot, completion_slot, ..]) = record.vars.get_mut(..) else {
        return fail_dispatch(ctx);
    };
    *state_slot = value::encode_f64(0.0);
    *completion_slot = value::encode_f64(0.0);
    let Ok(generator) = state.allocate_object(0, false) else {
        return fail_dispatch(ctx);
    };
    let Some(prototype) = ensure_prototypes(ctx, state) else {
        return fail_dispatch(ctx);
    };
    if state
        .heap
        .set_prototype(
            value::decode_handle(generator),
            value::decode_handle(prototype),
        )
        .is_err()
    {
        return fail_dispatch(ctx);
    }
    state.async_generators.insert(
        value::decode_object_handle(generator),
        NativeAsyncGenerator {
            continuation,
            status: AsyncGeneratorStatus::SuspendedStart,
            active: None,
            queue: VecDeque::new(),
            resume_promise: None,
        },
    );
    generator
}

fn request_or_yield(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(generator) = generator_argument(state, args) else {
        return type_error(
            ctx,
            state,
            "AsyncGenerator.prototype.next called on incompatible receiver",
        );
    };
    let supplied = args.get(1).copied().unwrap_or_else(value::encode_undefined);
    if is_internal(state, generator) {
        yield_value(ctx, state, generator, supplied)
    } else {
        enqueue(
            ctx,
            state,
            value::encode_object_handle(generator),
            RequestKind::Next,
            supplied,
        )
    }
}

fn request_or_return(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(generator) = generator_argument(state, args) else {
        return type_error(
            ctx,
            state,
            "AsyncGenerator.prototype.return called on incompatible receiver",
        );
    };
    let supplied = args.get(1).copied().unwrap_or_else(value::encode_undefined);
    if is_internal(state, generator) {
        complete(ctx, state, generator, supplied, false)
    } else {
        enqueue(
            ctx,
            state,
            value::encode_object_handle(generator),
            RequestKind::Return,
            supplied,
        )
    }
}

fn request_or_throw(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(generator) = generator_argument(state, args) else {
        return type_error(
            ctx,
            state,
            "AsyncGenerator.prototype.throw called on incompatible receiver",
        );
    };
    let supplied = args.get(1).copied().unwrap_or_else(value::encode_undefined);
    if is_internal(state, generator) {
        complete(ctx, state, generator, supplied, true)
    } else {
        enqueue(
            ctx,
            state,
            value::encode_object_handle(generator),
            RequestKind::Throw,
            supplied,
        )
    }
}

fn generator_argument(state: &NativeAgentState, args: &[i64]) -> Option<u32> {
    let receiver = args.first().copied()?;
    is_async_generator(state, receiver).then(|| value::decode_object_handle(receiver))
}

fn is_internal(state: &NativeAgentState, generator: u32) -> bool {
    state.call_environment()
        == state
            .async_generators
            .get(&generator)
            .map(|generator| generator.continuation)
}

fn enqueue(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    receiver: i64,
    kind: RequestKind,
    supplied: i64,
) -> i64 {
    let generator = value::decode_object_handle(receiver);
    let Some(promise) = new_promise(ctx, state) else {
        return fail_dispatch(ctx);
    };
    let request = AsyncGeneratorRequest {
        kind,
        value: supplied,
        promise,
    };
    state
        .async_generators
        .get_mut(&generator)
        .expect("receiver was validated")
        .queue
        .push_back(request);
    process_queue(ctx, state, generator);
    promise
}

fn process_queue(ctx: &mut NativeVmContext, state: &mut NativeAgentState, generator: u32) {
    let status = state.async_generators[&generator].status;
    match status {
        AsyncGeneratorStatus::Executing => {}
        AsyncGeneratorStatus::Completed => drain_completed(ctx, state, generator),
        AsyncGeneratorStatus::SuspendedStart => process_start(ctx, state, generator),
        AsyncGeneratorStatus::SuspendedYield => process_suspended_yield(state, generator),
    }
}

fn process_start(ctx: &mut NativeVmContext, state: &mut NativeAgentState, generator: u32) {
    let Some(request) = state
        .async_generators
        .get_mut(&generator)
        .expect("generator exists")
        .queue
        .pop_front()
    else {
        return;
    };
    match request.kind {
        RequestKind::Next => {
            let continuation = state.async_generators[&generator].continuation;
            state
                .async_generators
                .get_mut(&generator)
                .expect("generator exists")
                .active = Some(request);
            state
                .async_generators
                .get_mut(&generator)
                .expect("generator exists")
                .status = AsyncGeneratorStatus::Executing;
            let continuation_handle = value::decode_handle(continuation);
            let Some(record) = state.continuations.get_mut(&continuation_handle) else {
                let reason = fail_dispatch(ctx);
                reject_active(ctx, state, generator, reason);
                return;
            };
            let Some([state_slot, completion_slot, ..]) = record.vars.get_mut(..) else {
                let reason = fail_dispatch(ctx);
                reject_active(ctx, state, generator, reason);
                return;
            };
            *state_slot = value::encode_f64(0.0);
            *completion_slot = value::encode_f64(0.0);
            let function = record.function;
            let result = state
                .invoke_callable_with_environment(ctx, function, continuation, request.value, &[])
                .unwrap_or_else(|| fail_dispatch(ctx));
            if value::is_exception(result) {
                reject_active(ctx, state, generator, result);
            }
        }
        RequestKind::Return => {
            settle_request(ctx, state, request, request.value, true, false);
            state
                .async_generators
                .get_mut(&generator)
                .expect("generator exists")
                .status = AsyncGeneratorStatus::Completed;
            drain_completed(ctx, state, generator);
        }
        RequestKind::Throw => {
            settle_request(ctx, state, request, request.value, true, true);
            state
                .async_generators
                .get_mut(&generator)
                .expect("generator exists")
                .status = AsyncGeneratorStatus::Completed;
            drain_completed(ctx, state, generator);
        }
    }
}

fn process_suspended_yield(state: &mut NativeAgentState, generator: u32) {
    let generator_state = state
        .async_generators
        .get_mut(&generator)
        .expect("generator exists");
    let Some(request) = generator_state.queue.pop_front() else {
        return;
    };
    let Some(resume_promise) = generator_state.resume_promise.take() else {
        return;
    };
    let continuation = generator_state.continuation;
    generator_state.active = Some(request);
    generator_state.status = AsyncGeneratorStatus::Executing;
    if matches!(request.kind, RequestKind::Return) {
        state
            .async_generator_resume_completions
            .insert(value::decode_handle(continuation), 2.0);
    }
    settle_promise(
        state,
        value::decode_handle(resume_promise),
        request.value,
        matches!(request.kind, RequestKind::Throw),
    );
}

fn yield_value(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    generator: u32,
    yielded: i64,
) -> i64 {
    let Some(request) = state
        .async_generators
        .get_mut(&generator)
        .expect("generator exists")
        .active
        .take()
    else {
        return fail_dispatch(ctx);
    };
    settle_request(ctx, state, request, yielded, false, false);
    let Some(resume_promise) = new_promise(ctx, state) else {
        return fail_dispatch(ctx);
    };
    let generator_state = state
        .async_generators
        .get_mut(&generator)
        .expect("generator exists");
    generator_state.status = AsyncGeneratorStatus::SuspendedYield;
    generator_state.resume_promise = Some(resume_promise);
    process_queue(ctx, state, generator);
    resume_promise
}

fn complete(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    generator: u32,
    completion: i64,
    rejected: bool,
) -> i64 {
    let Some(request) = state
        .async_generators
        .get_mut(&generator)
        .expect("generator exists")
        .active
        .take()
    else {
        return fail_dispatch(ctx);
    };
    settle_request(ctx, state, request, completion, true, rejected);
    let generator_state = state
        .async_generators
        .get_mut(&generator)
        .expect("generator exists");
    generator_state.status = AsyncGeneratorStatus::Completed;
    generator_state.resume_promise = None;
    drain_completed(ctx, state, generator);
    value::encode_undefined()
}

fn reject_active(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    generator: u32,
    reason: i64,
) {
    if let Some(request) = state
        .async_generators
        .get_mut(&generator)
        .expect("generator exists")
        .active
        .take()
    {
        settle_request(ctx, state, request, reason, true, true);
    }
    state
        .async_generators
        .get_mut(&generator)
        .expect("generator exists")
        .status = AsyncGeneratorStatus::Completed;
    drain_completed(ctx, state, generator);
}

fn drain_completed(ctx: &mut NativeVmContext, state: &mut NativeAgentState, generator: u32) {
    loop {
        let request = state
            .async_generators
            .get_mut(&generator)
            .expect("generator exists")
            .queue
            .pop_front();
        let Some(request) = request else {
            break;
        };
        match request.kind {
            RequestKind::Next => {
                settle_request(ctx, state, request, value::encode_undefined(), true, false)
            }
            RequestKind::Return => settle_request(ctx, state, request, request.value, true, false),
            RequestKind::Throw => settle_request(ctx, state, request, request.value, true, true),
        }
    }
}

fn settle_request(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    request: AsyncGeneratorRequest,
    result: i64,
    done: bool,
    rejected: bool,
) {
    let settled = if rejected {
        result
    } else {
        create_iterator_result(ctx, state, result, done)
    };
    settle_promise(
        state,
        value::decode_handle(request.promise),
        settled,
        rejected,
    );
}
