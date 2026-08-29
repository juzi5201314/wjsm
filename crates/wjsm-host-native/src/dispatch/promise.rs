use std::cmp::Ordering;
use std::path::PathBuf;
use std::time::Duration;

use num_traits::ToPrimitive;
use wjsm_ir::{Builtin, value};
use wjsm_native_abi::NativeVmContext;

use super::runtime::{fail_dispatch, iterator_close, iterator_done, iterator_from, iterator_value};
use crate::{NativeAgentState, NativeCallableKind};

mod combinator;

#[derive(Clone, Copy)]
pub(crate) enum PromiseState {
    Pending,
    Fulfilled(i64),
    Rejected(i64),
}

#[derive(Clone, Copy)]
pub(crate) struct NativePromise {
    pub(crate) state: PromiseState,
    pub(crate) async_id: u64,
    handled: bool,
}

/// 微任务队列排空后是否执行 unhandled rejection 检查点。
/// 事件循环驱动处 `Check`（Node 语义：每轮微任务排空后处理未处理 rejection）；
/// 嵌套 drain（运行时模块加载、node:vm）必须 `Defer`，把报告留给外层事件循环，
/// 避免在宿主中途误判 handler 挂载时机。
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum RejectionCheckpoint {
    Check,
    Defer,
}

/// unhandled rejection 致命报告的退出码，与 CLI 的 EXIT_RUNTIME_ERROR 对齐：
/// 未处理 rejection 就是未捕获的运行时错误（Node 以非零码退出，wjsm 运行时错误约定为 2）。
const UNHANDLED_REJECTION_EXIT_CODE: i32 = 2;

#[derive(Clone, Copy)]
pub(crate) enum NativePromiseReaction {
    Handler {
        on_fulfilled: i64,
        on_rejected: i64,
        target_promise: u32,
    },
    AsyncResume {
        continuation: i64,
        state: i64,
    },
    CombinatorElement {
        combinator: PromiseCombinatorId,
        index: u32,
    },
    Finally {
        callback: i64,
        target_promise: u32,
    },
    FinallyResult {
        target_promise: u32,
        original: i64,
        original_rejected: bool,
    },
    Stream(super::streams::StreamReaction),
    /// `Array.fromAsync` 状态机的 Await 续点（§23.1.2.1 / §6.2.9.3）。
    ArrayFromAsync {
        operation: u32,
        phase: super::array_from_async::FromAsyncPhase,
    },
}

#[derive(Clone)]
pub(crate) struct NativeScheduledReaction {
    pub(crate) reaction: NativePromiseReaction,
    pub(crate) context: super::node_async_hooks::AsyncContextSnapshot,
}

#[derive(Clone, Copy)]
pub(crate) enum PromiseCombinatorKind {
    All,
    AllSettled,
    Any,
}

#[derive(Clone, Copy)]
pub(crate) struct PromiseCombinatorId(u32);

#[derive(Clone)]
pub(crate) struct NativePromiseCombinator {
    kind: PromiseCombinatorKind,
    target_promise: u32,
    pub(crate) values: Vec<i64>,
    remaining: u32,
    settled: bool,
}

#[derive(Clone)]
pub(crate) enum NativeMicrotask {
    Callback {
        callback: i64,
        arguments: Vec<i64>,
        resource: Option<i64>,
        repeat: bool,
    },
    PromiseReaction {
        reaction: NativePromiseReaction,
        value: i64,
        rejected: bool,
    },
    DynamicImport {
        specifier: String,
        referrer: PathBuf,
        promise: u32,
    },
    AsyncResume {
        continuation: i64,
        state: i64,
        value: i64,
        rejected: bool,
    },
    ResolveThenable {
        promise: u32,
        thenable: i64,
        then: i64,
    },
    Stream(super::streams::StreamTask),
}

#[derive(Clone)]
pub(crate) struct NativeScheduledMicrotask {
    pub(crate) task: NativeMicrotask,
    pub(crate) context: super::node_async_hooks::AsyncContextSnapshot,
}

#[derive(Clone)]
pub(crate) struct NativeTimer {
    pub(crate) scheduled: NativeScheduledMicrotask,
    due_ms: u64,
    interval_ms: Option<u64>,
    sequence: u64,
}

impl PartialEq for NativeTimer {
    fn eq(&self, other: &Self) -> bool {
        (self.due_ms, self.sequence) == (other.due_ms, other.sequence)
    }
}

impl Eq for NativeTimer {}

impl PartialOrd for NativeTimer {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for NativeTimer {
    fn cmp(&self, other: &Self) -> Ordering {
        (other.due_ms, other.sequence).cmp(&(self.due_ms, self.sequence))
    }
}

pub(crate) fn enqueue_microtask(state: &mut NativeAgentState, task: NativeMicrotask) {
    let context = super::node_async_hooks::capture_context(state);
    enqueue_microtask_with_context(state, task, context);
}

pub(crate) fn enqueue_stream_task(state: &mut NativeAgentState, task: super::streams::StreamTask) {
    enqueue_microtask(state, NativeMicrotask::Stream(task));
}

pub(crate) fn mark_promise_handled(state: &mut NativeAgentState, promise: u32) {
    if let Some(p) = state.promises.get_mut(&promise) {
        p.handled = true;
    }
    // 从待报告列表中移除（如果存在）
    state
        .pending_unhandled_rejections
        .retain(|(h, _)| *h != promise);
}
pub(crate) fn observe(
    state: &mut NativeAgentState,
    promise: u32,
    reaction: super::streams::StreamReaction,
) {
    observe_reaction(state, promise, NativePromiseReaction::Stream(reaction));
}

/// 宿主原生 reaction 版 PerformPromiseThen：pending 挂 reaction 表，已结算
/// 直接入微任务队列；宿主消费者自身即 handler，promise 记为已处理。
pub(crate) fn observe_reaction(
    state: &mut NativeAgentState,
    promise: u32,
    reaction: NativePromiseReaction,
) {
    let context = super::node_async_hooks::capture_context(state);
    let reaction = NativeScheduledReaction { reaction, context };
    mark_promise_handled(state, promise);
    match state.promises.get(&promise).map(|promise| promise.state) {
        Some(PromiseState::Pending) => state
            .promise_reactions
            .entry(promise)
            .or_default()
            .push(reaction),
        Some(PromiseState::Fulfilled(value)) => enqueue_microtask_with_context(
            state,
            NativeMicrotask::PromiseReaction {
                reaction: reaction.reaction,
                value,
                rejected: false,
            },
            reaction.context,
        ),
        Some(PromiseState::Rejected(value)) => enqueue_microtask_with_context(
            state,
            NativeMicrotask::PromiseReaction {
                reaction: reaction.reaction,
                value,
                rejected: true,
            },
            reaction.context,
        ),
        None => {}
    }
}

fn enqueue_microtask_with_context(
    state: &mut NativeAgentState,
    task: NativeMicrotask,
    context: super::node_async_hooks::AsyncContextSnapshot,
) {
    state
        .microtasks
        .push_back(NativeScheduledMicrotask { task, context });
}

/// 宿主状态机把原生续点连同结算值直接排入微任务队列（等价于对已结算
/// promise 的 PerformPromiseThen，用于模拟中间 promise 的既定 1-tick 跳）。
pub(crate) fn enqueue_reaction_microtask(
    state: &mut NativeAgentState,
    reaction: NativePromiseReaction,
    value: i64,
    rejected: bool,
) {
    let context = super::node_async_hooks::capture_context(state);
    enqueue_microtask_with_context(
        state,
        NativeMicrotask::PromiseReaction {
            reaction,
            value,
            rejected,
        },
        context,
    );
}
#[derive(Clone)]
pub(crate) struct NativeContinuation {
    pub(crate) function: i64,
    pub(crate) outer_promise: i64,
    pub(crate) vars: Vec<i64>,
}

pub(super) fn dispatch_promise(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    builtin: Builtin,
    args: &[i64],
) -> Option<i64> {
    Some(match builtin {
        Builtin::PromiseCreate => create(ctx, state, args),
        Builtin::PromiseCreateResolveFunction => create_resolver(ctx, state, args, false),
        Builtin::PromiseCreateRejectFunction => create_resolver(ctx, state, args, true),
        Builtin::PromiseInstanceResolve => settle(ctx, state, args, false),
        Builtin::PromiseInstanceReject => settle(ctx, state, args, true),
        Builtin::PromiseThen => then(ctx, state, args, false),
        Builtin::PromiseCatch => then(ctx, state, args, true),
        Builtin::PromiseFinally => finally(ctx, state, args),
        Builtin::PromiseResolveStatic => static_resolve(ctx, state, args),
        Builtin::PromiseRejectStatic => static_reject(ctx, state, args),
        Builtin::PromiseAll => combinator::run(ctx, state, args, PromiseCombinatorKind::All),
        Builtin::PromiseRace => combinator::race(ctx, state, args),
        Builtin::PromiseAllSettled => {
            combinator::run(ctx, state, args, PromiseCombinatorKind::AllSettled)
        }
        Builtin::PromiseAny => combinator::run(ctx, state, args, PromiseCombinatorKind::Any),
        Builtin::IsPromise => {
            value::encode_bool(args.first().is_some_and(|promise| {
                state.promises.contains_key(&value::decode_handle(*promise))
            }))
        }
        Builtin::QueueMicrotask => queue_microtask(ctx, state, args),
        Builtin::DrainMicrotasks => drain_microtasks(ctx, state, RejectionCheckpoint::Defer),
        Builtin::ContinuationCreate => continuation_create(ctx, state, args),
        Builtin::ContinuationSaveVar => continuation_save_var(ctx, state, args),
        Builtin::ContinuationLoadVar => continuation_load_var(ctx, state, args),
        Builtin::AsyncFunctionResume => async_function_resume(ctx, state, args),
        Builtin::AsyncFunctionStart => async_function_resume(ctx, state, args),
        Builtin::AsyncFunctionSuspend => async_function_suspend(ctx, state, args),
        Builtin::PromiseWithResolvers => with_resolvers(ctx, state),
        _ => return None,
    })
}

pub(crate) fn promise_builtin(
    state: &NativeAgentState,
    receiver: i64,
    key: &str,
) -> Option<Builtin> {
    if !state.promises.contains_key(&value::decode_handle(receiver)) {
        return None;
    }
    Some(match key {
        "then" => Builtin::PromiseThen,
        "catch" => Builtin::PromiseCatch,
        "finally" => Builtin::PromiseFinally,
        _ => return None,
    })
}

pub(crate) fn settle_resolver(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    promise_handle: u32,
    argument: Option<i64>,
    rejected: bool,
) -> i64 {
    let result = argument.unwrap_or_else(value::encode_undefined);
    if rejected {
        settle_promise(state, promise_handle, result, true);
    } else {
        resolve_into(ctx, state, promise_handle, result);
    }
    value::encode_undefined()
}

fn create(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(promise) = new_promise(ctx, state) else {
        return fail_dispatch(ctx);
    };
    if let Some(executor) = args
        .first()
        .copied()
        .filter(|value| value::is_callable(*value))
        && !run_executor(ctx, state, promise, executor)
    {
        return fail_dispatch(ctx);
    }
    promise
}

pub(crate) fn construct(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    receiver: i64,
    args: &[i64],
) -> i64 {
    if !value::is_js_object(receiver) {
        return promise_constructor_type_error(
            ctx,
            state,
            "Promise constructor cannot be invoked without 'new'",
        );
    }
    let Some(executor) = args
        .first()
        .copied()
        .filter(|executor| value::is_callable(*executor))
    else {
        return promise_constructor_type_error(ctx, state, "Promise resolver is not a function");
    };
    if !initialize_promise(ctx, state, receiver, None) {
        return fail_dispatch(ctx);
    }
    if !run_executor(ctx, state, receiver, executor) {
        return fail_dispatch(ctx);
    }
    receiver
}

fn run_executor(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    promise: i64,
    executor: i64,
) -> bool {
    let handle = value::decode_handle(promise);
    let Some(resolve) = state.native_callable(NativeCallableKind::PromiseResolve(handle)) else {
        return false;
    };
    let Some(reject) = state.native_callable(NativeCallableKind::PromiseReject(handle)) else {
        return false;
    };
    let result = state
        .invoke_callable(ctx, executor, value::encode_undefined(), &[resolve, reject])
        .unwrap_or_else(|| fail_dispatch(ctx));
    if value::is_exception(result) {
        settle_promise(state, handle, result, true);
    }
    true
}

fn promise_constructor_type_error(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    message: &str,
) -> i64 {
    super::modules::named_error_object(state, "TypeError", message.to_string())
        .and_then(|error| state.create_exception(error))
        .unwrap_or_else(|| fail_dispatch(ctx))
}

pub(super) fn aggregate_error_object(
    state: &mut NativeAgentState,
    errors: &[i64],
    message: String,
) -> Option<i64> {
    let errors = state.allocate_array_values(errors).ok()?;
    let error = super::modules::named_error_object(state, "AggregateError", message)?;
    super::modules::set_named_property(state, error, "errors", errors).ok()?;
    Some(error)
}

pub(crate) fn construct_aggregate_error(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let message = match args.get(1).copied() {
        None => String::new(),
        Some(message) if value::is_undefined(message) => String::new(),
        Some(message) => match super::to_string_coerced(ctx, state, message) {
            Ok(message) => message,
            Err(exception) => return exception,
        },
    };
    let iterable = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let iterator = iterator_from(ctx, state, &[iterable]);
    if value::is_exception(iterator) {
        return iterator;
    }
    let mut errors = Vec::new();
    loop {
        let done = iterator_done(ctx, state, &[iterator]);
        if value::is_exception(done) {
            return iterator_close(ctx, state, &[iterator, done], true);
        }
        if super::runtime::is_truthy(state, done) {
            break;
        }
        let error = iterator_value(ctx, state, &[iterator], true);
        if value::is_exception(error) {
            return iterator_close(ctx, state, &[iterator, error], true);
        }
        errors.push(error);
    }
    aggregate_error_object(state, &errors, message).unwrap_or_else(|| fail_dispatch(ctx))
}

fn create_resolver(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
    rejected: bool,
) -> i64 {
    let Some(promise) = args.first().copied() else {
        return fail_dispatch(ctx);
    };
    let handle = value::decode_handle(promise);
    if !state.promises.contains_key(&handle) {
        return fail_dispatch(ctx);
    }
    let kind = if rejected {
        NativeCallableKind::PromiseReject(handle)
    } else {
        NativeCallableKind::PromiseResolve(handle)
    };
    state
        .native_callable(kind)
        .unwrap_or_else(|| fail_dispatch(ctx))
}

pub(crate) fn new_promise(ctx: &mut NativeVmContext, state: &mut NativeAgentState) -> Option<i64> {
    new_promise_with_trigger(ctx, state, None)
}

/// 在已分配对象上安装 Promise 原型与内部状态（配合 JIT TLAB 分配）。
pub(crate) fn init_allocated_promise(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    object: i64,
) -> Option<i64> {
    if !value::is_object(object) {
        return None;
    }
    let Some(constructor) =
        state.native_callable(NativeCallableKind::Builtin(Builtin::PromiseCreate, false))
    else {
        return None;
    };
    let prototype_key = state.intern_property_string("prototype".into())?;
    let Some(prototype) = state.callable_property(constructor, prototype_key) else {
        return None;
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
        return None;
    }
    initialize_promise(ctx, state, object, None).then_some(object)
}

pub(crate) fn resolved_promise(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    stored: i64,
) -> i64 {
    let Some(promise) = new_promise(ctx, state) else {
        return fail_dispatch(ctx);
    };
    resolve_into(ctx, state, value::decode_handle(promise), stored);
    promise
}

pub(crate) fn rejected_promise(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    reason: i64,
) -> i64 {
    let Some(promise) = new_promise(ctx, state) else {
        return fail_dispatch(ctx);
    };
    settle_promise(state, value::decode_handle(promise), reason, true);
    promise
}

fn new_promise_with_trigger(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    trigger: Option<u64>,
) -> Option<i64> {
    let Ok(object) = state.allocate_object_with_gc_retry(ctx, 2, false) else {
        fail_dispatch(ctx);
        return None;
    };
    let Some(constructor) =
        state.native_callable(NativeCallableKind::Builtin(Builtin::PromiseCreate, false))
    else {
        fail_dispatch(ctx);
        return None;
    };
    let Some(prototype_key) = state.intern_property_string("prototype".into()) else {
        fail_dispatch(ctx);
        return None;
    };
    let Some(prototype) = state.callable_property(constructor, prototype_key) else {
        fail_dispatch(ctx);
        return None;
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
        fail_dispatch(ctx);
        return None;
    }
    initialize_promise(ctx, state, object, trigger).then_some(object)
}

fn initialize_promise(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    object: i64,
    trigger: Option<u64>,
) -> bool {
    let handle = value::decode_handle(object);
    if state.promises.contains_key(&handle) {
        fail_dispatch(ctx);
        return false;
    }
    let async_id = super::node_async_hooks::promise_created(ctx, state, object, trigger);
    state.promises.insert(
        handle,
        NativePromise {
            state: PromiseState::Pending,
            async_id,
            handled: false,
        },
    );
    true
}

fn settle(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
    rejected: bool,
) -> i64 {
    let Some(promise) = args.first().copied() else {
        return fail_dispatch(ctx);
    };
    if !state.promises.contains_key(&value::decode_handle(promise)) {
        return fail_dispatch(ctx);
    }
    let handle = value::decode_handle(promise);
    let result = args.get(1).copied().unwrap_or_else(value::encode_undefined);
    if rejected {
        settle_promise(state, handle, result, true);
    } else {
        resolve_into(ctx, state, handle, result);
    }
    promise
}

pub(crate) fn settle_promise(
    state: &mut NativeAgentState,
    handle: u32,
    value: i64,
    rejected: bool,
) {
    let value = if value::is_exception(value) {
        state
            .exceptions
            .get(value::decode_handle(value) as usize)
            .and_then(|stored| *stored)
            .unwrap_or(value)
    } else {
        value
    };
    let Some(promise) = state.promises.get_mut(&handle) else {
        return;
    };
    if !matches!(promise.state, PromiseState::Pending) {
        return;
    }
    promise.state = if rejected {
        PromiseState::Rejected(value)
    } else {
        PromiseState::Fulfilled(value)
    };
    state.gc.record_host_write(
        value::encode_handle(value::TAG_OBJECT, handle),
        None,
        Some(value),
    );
    // 如果 promise rejected 且尚无 handler，立即格式化 reason 并记录到待报告列表
    // 必须立即格式化，因为后续 GC 可能回收 reason 对应的对象（如 Error）
    if rejected && !promise.handled {
        let reason_text = super::modules::exception_text(state, value);
        state
            .pending_unhandled_rejections
            .push((handle, reason_text));
    }
    let reactions = state.promise_reactions.remove(&handle).unwrap_or_default();
    super::node_async_hooks::promise_settled(state, handle);
    for scheduled in reactions {
        enqueue_microtask_with_context(
            state,
            NativeMicrotask::PromiseReaction {
                reaction: scheduled.reaction,
                value,
                rejected,
            },
            scheduled.context,
        );
    }
}

fn then(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
    catch_only: bool,
) -> i64 {
    let Some(source) = args.first().copied() else {
        return fail_dispatch(ctx);
    };
    let source_handle = value::decode_handle(source);
    let Some((source_state, source_async_id)) = state
        .promises
        .get(&source_handle)
        .map(|promise| (promise.state, promise.async_id))
    else {
        return fail_dispatch(ctx);
    };
    let Some(result_promise) = new_promise_with_trigger(ctx, state, Some(source_async_id)) else {
        return fail_dispatch(ctx);
    };
    super::node_async_hooks::inherit_promise_stores(
        state,
        value::decode_handle(result_promise),
        source_handle,
    );
    let on_fulfilled = if catch_only {
        value::encode_undefined()
    } else {
        args.get(1).copied().unwrap_or_else(value::encode_undefined)
    };
    let on_rejected = if catch_only {
        args.get(1).copied().unwrap_or_else(value::encode_undefined)
    } else {
        args.get(2).copied().unwrap_or_else(value::encode_undefined)
    };
    let context =
        super::node_async_hooks::promise_context(state, value::decode_handle(result_promise))
            .unwrap_or_else(|| super::node_async_hooks::capture_context(state));
    schedule_reaction(
        state,
        source_handle,
        source_state,
        NativePromiseReaction::Handler {
            on_fulfilled,
            on_rejected,
            target_promise: value::decode_handle(result_promise),
        },
        context,
    );
    result_promise
}

fn schedule_reaction(
    state: &mut NativeAgentState,
    source: u32,
    source_state: PromiseState,
    reaction: NativePromiseReaction,
    context: super::node_async_hooks::AsyncContextSnapshot,
) {
    mark_promise_handled(state, source);
    match source_state {
        PromiseState::Pending => state
            .promise_reactions
            .entry(source)
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

fn invoke_handler(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    callback: i64,
    input: i64,
) -> i64 {
    if value::is_callable(callback) {
        state
            .invoke_callable(ctx, callback, value::encode_undefined(), &[input])
            .unwrap_or_else(|| fail_dispatch(ctx))
    } else {
        input
    }
}

fn finally(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(source) = args.first().copied() else {
        return fail_dispatch(ctx);
    };
    let callback = args.get(1).copied().unwrap_or_else(value::encode_undefined);
    let source_handle = value::decode_handle(source);
    let Some((source_state, source_async_id)) = state
        .promises
        .get(&source_handle)
        .map(|promise| (promise.state, promise.async_id))
    else {
        return fail_dispatch(ctx);
    };
    let Some(result_promise) = new_promise_with_trigger(ctx, state, Some(source_async_id)) else {
        return fail_dispatch(ctx);
    };
    let target_promise = value::decode_handle(result_promise);
    super::node_async_hooks::inherit_promise_stores(state, target_promise, source_handle);
    let reaction = if value::is_callable(callback) {
        NativePromiseReaction::Finally {
            callback,
            target_promise,
        }
    } else {
        NativePromiseReaction::Handler {
            on_fulfilled: value::encode_undefined(),
            on_rejected: value::encode_undefined(),
            target_promise,
        }
    };
    let context = super::node_async_hooks::promise_context(state, target_promise)
        .unwrap_or_else(|| super::node_async_hooks::capture_context(state));
    schedule_reaction(state, source_handle, source_state, reaction, context);
    result_promise
}

fn static_resolve(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let input = args
        .get(1)
        .or_else(|| args.first())
        .copied()
        .unwrap_or_else(value::encode_undefined);
    promise_resolve_value(ctx, state, input)
}

/// PromiseResolve(%Promise%, value)（§27.2.4.7.1）：宿主 promise 原样返回，
/// 其余值（含 thenable）装入新 promise 解析。Await（§6.2.9.3）步骤 2 复用。
pub(crate) fn promise_resolve_value(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    input: i64,
) -> i64 {
    if value::is_object(input) && state.promises.contains_key(&value::decode_handle(input)) {
        return input;
    }
    let Some(promise) = new_promise(ctx, state) else {
        return fail_dispatch(ctx);
    };
    resolve_into(ctx, state, value::decode_handle(promise), input);
    promise
}

fn static_reject(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(promise) = new_promise(ctx, state) else {
        return fail_dispatch(ctx);
    };
    settle_promise(
        state,
        value::decode_handle(promise),
        args.get(1)
            .or_else(|| args.first())
            .copied()
            .unwrap_or_else(value::encode_undefined),
        true,
    );
    promise
}

fn queue_microtask(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(callback) = args
        .first()
        .copied()
        .filter(|value| value::is_callable(*value))
    else {
        return fail_dispatch(ctx);
    };
    enqueue_callback(state, callback, Vec::new());
    value::encode_undefined()
}

pub(crate) fn enqueue_callback(state: &mut NativeAgentState, callback: i64, arguments: Vec<i64>) {
    enqueue_microtask(
        state,
        NativeMicrotask::Callback {
            callback,
            arguments,
            resource: None,
            repeat: false,
        },
    );
}

pub(crate) fn enqueue_next_tick(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    callback: i64,
    arguments: Vec<i64>,
) -> i64 {
    let (resource, context) =
        match super::node_async_hooks::create_scheduled_resource(ctx, state, "TickObject") {
            Ok(scheduled) => scheduled,
            Err(exception) => return exception,
        };
    state.next_ticks.push_back(NativeScheduledMicrotask {
        task: NativeMicrotask::Callback {
            callback,
            arguments,
            resource: Some(resource),
            repeat: false,
        },
        context,
    });
    resource
}

fn create_timer_resource(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    type_name: &str,
) -> Result<(i64, super::node_async_hooks::AsyncContextSnapshot), i64> {
    let (resource, context) =
        super::node_async_hooks::create_scheduled_resource(ctx, state, type_name)?;
    let Some(brand) = state.intern_text(type_name.into(), value::TAG_STRING) else {
        return Err(fail_dispatch(ctx));
    };
    let Some(constructor) = state.native_callable(NativeCallableKind::TimerConstructor(
        type_name == "Immediate",
    )) else {
        return Err(fail_dispatch(ctx));
    };
    if super::modules::set_named_property(state, resource, "__brand__", brand).is_err()
        || super::modules::set_named_property(
            state,
            resource,
            "__timer_id__",
            value::encode_f64(f64::from(value::decode_handle(resource))),
        )
        .is_err()
        || super::modules::set_named_property(state, resource, "constructor", constructor).is_err()
    {
        return Err(fail_dispatch(ctx));
    }
    Ok((resource, context))
}

pub(crate) fn enqueue_timer(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    callback: i64,
    arguments: Vec<i64>,
    type_name: &str,
    delay_ms: u64,
    repeat: bool,
) -> i64 {
    let (resource, context) = match create_timer_resource(ctx, state, type_name) {
        Ok(scheduled) => scheduled,
        Err(exception) => return exception,
    };
    let due_ms = state.timer_now_ms.saturating_add(delay_ms);
    let sequence = state.next_timer_sequence;
    state.next_timer_sequence = state.next_timer_sequence.wrapping_add(1);
    state.timers.push(NativeTimer {
        scheduled: NativeScheduledMicrotask {
            task: NativeMicrotask::Callback {
                callback,
                arguments,
                resource: Some(resource),
                repeat,
            },
            context,
        },
        due_ms,
        interval_ms: repeat.then_some(delay_ms.max(1)),
        sequence,
    });
    resource
}

pub(crate) fn enqueue_immediate(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    callback: i64,
    arguments: Vec<i64>,
) -> i64 {
    let (resource, context) = match create_timer_resource(ctx, state, "Immediate") {
        Ok(scheduled) => scheduled,
        Err(exception) => return exception,
    };
    state.immediates.push_back(NativeScheduledMicrotask {
        task: NativeMicrotask::Callback {
            callback,
            arguments,
            resource: Some(resource),
            repeat: false,
        },
        context,
    });
    resource
}

pub(crate) fn drain_microtasks(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    checkpoint: RejectionCheckpoint,
) -> i64 {
    let mut macrotask_ran = false;
    loop {
        if state.requested_exit_code.is_some() {
            return value::encode_undefined();
        }
        if let Some(exception) = super::node_async_hooks::drain_hook_events(ctx, state) {
            return exception;
        }
        // 微任务队列排空检查点：在返回或执行下一个宏任务（timer/immediate）之前
        // 处理未处理 rejection，对齐 Node 的 processTicksAndRejections 时机——
        // 同一轮微任务级联内挂上的 handler 不误报，之后的宏任务无机会再挂。
        if checkpoint == RejectionCheckpoint::Check
            && state.next_ticks.is_empty()
            && state.microtasks.is_empty()
            && report_unhandled_rejection(state)
        {
            return value::encode_undefined();
        }
        if macrotask_ran && state.next_ticks.is_empty() && state.microtasks.is_empty() {
            return value::encode_undefined();
        }
        let immediate = if state.next_ticks.is_empty() && state.microtasks.is_empty() {
            state.immediates.pop_front()
        } else {
            None
        };
        let timer = if state.next_ticks.is_empty()
            && state.microtasks.is_empty()
            && immediate.is_none()
            && state.timers.peek().is_some_and(|timer| {
                timer.due_ms <= state.timer_now_ms
                    || matches!(
                        &timer.scheduled.task,
                        NativeMicrotask::Callback {
                            resource: Some(resource),
                            ..
                        } if state.cancelled_timers.contains(&value::decode_handle(*resource))
                    )
            }) {
            state.timers.pop()
        } else {
            None
        };
        let (scheduled, timer_schedule, was_immediate) =
            if let Some(scheduled) = state.next_ticks.pop_front() {
                (scheduled, None, false)
            } else if let Some(scheduled) = state.microtasks.pop_front() {
                (scheduled, None, false)
            } else if let Some(scheduled) = immediate {
                (scheduled, None, true)
            } else if let Some(timer) = timer {
                state.timer_now_ms = state.timer_now_ms.max(timer.due_ms);
                (
                    timer.scheduled,
                    Some((timer.due_ms, timer.interval_ms)),
                    false,
                )
            } else {
                return value::encode_undefined();
            };
        let was_timer = was_immediate || timer_schedule.is_some();
        macrotask_ran |= was_timer;
        let scheduled_context = scheduled.context.clone();
        let callback_resource = match &scheduled.task {
            NativeMicrotask::Callback { resource, .. } => *resource,
            _ => None,
        };
        if callback_resource.is_some_and(|resource| {
            state
                .cancelled_timers
                .contains(&value::decode_handle(resource))
        }) {
            if let Some(resource) = callback_resource {
                let _ = super::node_async_hooks::destroy_scheduled_resource(ctx, state, resource);
            }
            continue;
        }
        let previous = super::node_async_hooks::enter_context(state, scheduled.context);
        if let Some(exception) = super::node_async_hooks::emit_current_phase(ctx, state, true) {
            super::node_async_hooks::restore_context(state, previous);
            return exception;
        }
        let (result, repeat) = match scheduled.task {
            NativeMicrotask::Callback {
                callback,
                arguments,
                repeat,
                ..
            } => (
                state
                    .invoke_callable(ctx, callback, value::encode_undefined(), &arguments)
                    .unwrap_or_else(|| fail_dispatch(ctx)),
                repeat.then_some((callback, arguments)),
            ),
            NativeMicrotask::PromiseReaction {
                reaction,
                value,
                rejected,
            } => (run_reaction(ctx, state, reaction, value, rejected), None),
            NativeMicrotask::AsyncResume {
                continuation,
                state: resume_state,
                value,
                rejected,
            } => (
                run_async_resume(ctx, state, continuation, resume_state, value, rejected),
                None,
            ),
            NativeMicrotask::DynamicImport {
                specifier,
                referrer,
                promise,
            } => (
                super::modules::run_dynamic_import(ctx, state, specifier, referrer, promise),
                None,
            ),
            NativeMicrotask::Stream(task) => (super::streams::run_task(ctx, state, task), None),
            NativeMicrotask::ResolveThenable {
                promise,
                thenable,
                then,
            } => (run_thenable_job(ctx, state, promise, thenable, then), None),
        };
        if state.requested_exit_code.is_some() {
            super::node_async_hooks::restore_context(state, previous);
            return value::encode_undefined();
        }
        let hook_exception = super::node_async_hooks::drain_hook_events(ctx, state)
            .or_else(|| super::node_async_hooks::emit_current_phase(ctx, state, false));
        let cancelled = callback_resource.is_some_and(|resource| {
            state
                .cancelled_timers
                .contains(&value::decode_handle(resource))
        });
        if let Some(resource) = callback_resource {
            if let Some((callback, arguments)) = repeat.filter(|_| !cancelled) {
                let Some((due_ms, Some(interval_ms))) = timer_schedule else {
                    return fail_dispatch(ctx);
                };
                let sequence = state.next_timer_sequence;
                state.next_timer_sequence = state.next_timer_sequence.wrapping_add(1);
                state.timers.push(NativeTimer {
                    scheduled: NativeScheduledMicrotask {
                        task: NativeMicrotask::Callback {
                            callback,
                            arguments,
                            resource: Some(resource),
                            repeat: true,
                        },
                        context: scheduled_context,
                    },
                    due_ms: due_ms.saturating_add(interval_ms),
                    interval_ms: Some(interval_ms),
                    sequence,
                });
            } else if let Some(exception) =
                super::node_async_hooks::destroy_scheduled_resource(ctx, state, resource)
            {
                super::node_async_hooks::restore_context(state, previous);
                return exception;
            }
        }
        super::node_async_hooks::restore_context(state, previous);
        if let Some(exception) = hook_exception {
            return exception;
        }
        if value::is_exception(result) {
            if was_timer {
                let text = super::modules::exception_text(state, result);
                // 经 emit_output 写 stderr：CLI（OutputMode::Inherit）下才对用户可见
                state.emit_output(format!("Uncaught exception: {text}\n").as_bytes(), true);
                continue;
            }
            return result;
        }
    }
}

/// Node 默认 `--unhandled-rejections=throw` 对齐：微任务队列排空后仍无 handler
/// 的 rejection 视为致命错误——报告第一个 reason 并以运行时错误退出码终止事件循环。
/// 已被 handle 的 promise 在 mark_promise_handled 时已从列表移除，
/// 因此列表中的条目即「排空后仍未处理」；进程在第一个报告处终止（与 Node 一致）。
fn report_unhandled_rejection(state: &mut NativeAgentState) -> bool {
    let Some((_, reason_text)) = state.pending_unhandled_rejections.first() else {
        return false;
    };
    let message = format!("UnhandledPromiseRejection: {reason_text}\n");
    state.emit_output(message.as_bytes(), true);
    state.pending_unhandled_rejections.clear();
    state.requested_exit_code = Some(UNHANDLED_REJECTION_EXIT_CODE);
    true
}

pub(crate) fn drain_event_loop(ctx: &mut NativeVmContext, state: &mut NativeAgentState) -> i64 {
    loop {
        refresh_timer_clock(state);
        let result = drain_microtasks(ctx, state, RejectionCheckpoint::Check);
        if state.requested_exit_code.is_some() {
            return value::encode_undefined();
        }
        if value::is_exception(result) {
            return result;
        }
        if !state.has_pending_external_events() {
            if !state.next_ticks.is_empty()
                || !state.microtasks.is_empty()
                || !state.immediates.is_empty()
                || !state.timers.is_empty()
            {
                sleep_until_next_timer(state);
                continue;
            }
            return result;
        }
        let result = state.poll_external_events(ctx);
        if value::is_exception(result) {
            return result;
        }
    }
}

fn refresh_timer_clock(state: &mut NativeAgentState) {
    let elapsed = state.process_started_at.elapsed().as_millis();
    state.timer_now_ms = state
        .timer_now_ms
        .max(elapsed.min(u128::from(u64::MAX)) as u64);
}

fn sleep_until_next_timer(state: &NativeAgentState) {
    if !state.next_ticks.is_empty() || !state.microtasks.is_empty() || !state.immediates.is_empty()
    {
        return;
    }
    let Some(timer) = state.timers.peek() else {
        return;
    };
    let wait_ms = timer.due_ms.saturating_sub(state.timer_now_ms);
    if wait_ms != 0 {
        std::thread::sleep(Duration::from_millis(wait_ms));
    }
}

fn run_reaction(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    reaction: NativePromiseReaction,
    value: i64,
    rejected: bool,
) -> i64 {
    match reaction {
        NativePromiseReaction::Handler {
            on_fulfilled,
            on_rejected,
            target_promise,
        } => {
            let handler = if rejected { on_rejected } else { on_fulfilled };
            let result = invoke_handler(ctx, state, handler, value);
            let result_rejected = if value::is_callable(handler) {
                value::is_exception(result)
            } else {
                rejected
            };
            if result_rejected {
                let reason = state.exception_value(result).unwrap_or(result);
                settle_promise(state, target_promise, reason, true);
            } else {
                resolve_into(ctx, state, target_promise, result);
            }
            value::encode_undefined()
        }
        NativePromiseReaction::Finally {
            callback,
            target_promise,
        } => run_finally(ctx, state, callback, target_promise, value, rejected),
        NativePromiseReaction::FinallyResult {
            target_promise,
            original,
            original_rejected,
        } => {
            if rejected {
                settle_promise(state, target_promise, value, true);
            } else if original_rejected {
                settle_promise(state, target_promise, original, true);
            } else {
                resolve_into(ctx, state, target_promise, original);
            }
            value::encode_undefined()
        }
        NativePromiseReaction::AsyncResume {
            continuation,
            state: resume_state,
        } => run_async_resume(ctx, state, continuation, resume_state, value, rejected),
        NativePromiseReaction::CombinatorElement { combinator, index } => {
            combinator::settle_element(ctx, state, combinator, index, value, rejected)
        }
        NativePromiseReaction::Stream(reaction) => {
            super::streams::run_reaction(ctx, state, reaction, value, rejected)
        }
        NativePromiseReaction::ArrayFromAsync { operation, phase } => {
            super::array_from_async::run_reaction(ctx, state, operation, phase, value, rejected)
        }
    }
}

fn run_finally(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    callback: i64,
    target_promise: u32,
    original: i64,
    original_rejected: bool,
) -> i64 {
    let result = state
        .invoke_callable(ctx, callback, value::encode_undefined(), &[])
        .unwrap_or_else(|| fail_dispatch(ctx));
    if value::is_exception(result) {
        let reason = state.exception_value(result).unwrap_or(result);
        settle_promise(state, target_promise, reason, true);
        return value::encode_undefined();
    }
    let wait_promise = if state.promises.contains_key(&value::decode_handle(result)) {
        value::decode_handle(result)
    } else {
        let Some(wait_promise) = new_promise(ctx, state) else {
            return fail_dispatch(ctx);
        };
        let wait_promise = value::decode_handle(wait_promise);
        resolve_into(ctx, state, wait_promise, result);
        wait_promise
    };
    let Some(wait_state) = state
        .promises
        .get(&wait_promise)
        .map(|promise| promise.state)
    else {
        return fail_dispatch(ctx);
    };
    let context = super::node_async_hooks::promise_context(state, target_promise)
        .unwrap_or_else(|| super::node_async_hooks::capture_context(state));
    schedule_reaction(
        state,
        wait_promise,
        wait_state,
        NativePromiseReaction::FinallyResult {
            target_promise,
            original,
            original_rejected,
        },
        context,
    );
    value::encode_undefined()
}

pub(crate) fn resolve_into(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    target_promise: u32,
    result: i64,
) {
    if !state
        .promises
        .get(&target_promise)
        .is_some_and(|promise| matches!(promise.state, PromiseState::Pending))
    {
        return;
    }
    if value::is_exception(result) {
        let reason = state.exception_value(result).unwrap_or(result);
        settle_promise(state, target_promise, reason, true);
        return;
    }
    if !value::is_js_object(result) {
        settle_promise(state, target_promise, result, false);
        return;
    }

    let source_promise = value::decode_handle(result);
    if source_promise == target_promise {
        let reason = super::modules::named_error_object(
            state,
            "TypeError",
            "Chaining cycle detected for promise".into(),
        )
        .unwrap_or_else(|| fail_dispatch(ctx));
        settle_promise(state, target_promise, reason, true);
        return;
    }
    if let Some(source_state) = state
        .promises
        .get(&source_promise)
        .map(|promise| promise.state)
    {
        let context = super::node_async_hooks::promise_context(state, target_promise)
            .unwrap_or_else(|| super::node_async_hooks::capture_context(state));
        mark_promise_handled(state, source_promise);
        match source_state {
            PromiseState::Pending => state
                .promise_reactions
                .entry(source_promise)
                .or_default()
                .push(NativeScheduledReaction {
                    reaction: NativePromiseReaction::Handler {
                        on_fulfilled: value::encode_undefined(),
                        on_rejected: value::encode_undefined(),
                        target_promise,
                    },
                    context,
                }),
            PromiseState::Fulfilled(value) => settle_promise(state, target_promise, value, false),
            PromiseState::Rejected(reason) => settle_promise(state, target_promise, reason, true),
        }
        return;
    }

    let Some(then_key) = state.intern_text("then".into(), value::TAG_STRING) else {
        settle_promise(state, target_promise, fail_dispatch(ctx), true);
        return;
    };
    let then = super::runtime::get_property(ctx, state, result, then_key)
        .unwrap_or_else(|()| fail_dispatch(ctx));
    if value::is_exception(then) {
        let reason = state.exception_value(then).unwrap_or(then);
        settle_promise(state, target_promise, reason, true);
    } else if value::is_callable(then) {
        enqueue_microtask(
            state,
            NativeMicrotask::ResolveThenable {
                promise: target_promise,
                thenable: result,
                then,
            },
        );
    } else {
        settle_promise(state, target_promise, result, false);
    }
}

fn run_thenable_job(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    promise: u32,
    thenable: i64,
    then: i64,
) -> i64 {
    if !state
        .promises
        .get(&promise)
        .is_some_and(|entry| matches!(entry.state, PromiseState::Pending))
    {
        return value::encode_undefined();
    }
    let Some(resolve) = state.native_callable(NativeCallableKind::PromiseResolve(promise)) else {
        return fail_dispatch(ctx);
    };
    let Some(reject) = state.native_callable(NativeCallableKind::PromiseReject(promise)) else {
        return fail_dispatch(ctx);
    };
    let result = state
        .invoke_callable(ctx, then, thenable, &[resolve, reject])
        .unwrap_or_else(|| fail_dispatch(ctx));
    if value::is_exception(result)
        && state
            .promises
            .get(&promise)
            .is_some_and(|entry| matches!(entry.state, PromiseState::Pending))
    {
        let reason = state.exception_value(result).unwrap_or(result);
        settle_promise(state, promise, reason, true);
    }
    value::encode_undefined()
}

fn run_async_resume(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    continuation: i64,
    resume_state: i64,
    resume_value: i64,
    rejected: bool,
) -> i64 {
    let continuation_handle = value::decode_handle(continuation);
    let Some(entry) = state.continuations.get(&continuation_handle).cloned() else {
        return fail_dispatch(ctx);
    };
    if !store_async_resume_state(state, continuation_handle, resume_state, rejected) {
        return fail_dispatch(ctx);
    }
    let result = state
        .invoke_callable_with_environment(ctx, entry.function, continuation, resume_value, &[])
        .unwrap_or_else(|| fail_dispatch(ctx));
    if value::is_exception(result) {
        // async generator 续延没有单一 outer promise：机器返回的异常按
        // AsyncGeneratorBody 抛出语义拒绝活动请求并完成 generator，异常已
        // 被消费，不得再漏回微任务循环成为顶层错误。
        if super::async_generator::reject_active_for_continuation(ctx, state, continuation, result)
        {
            return value::encode_undefined();
        }
        settle_promise(
            state,
            value::decode_handle(entry.outer_promise),
            result,
            true,
        );
    }
    result
}

fn store_async_resume_state(
    state: &mut NativeAgentState,
    continuation_handle: u32,
    resume_state: i64,
    rejected: bool,
) -> bool {
    let completion = state
        .async_generator_resume_completions
        .remove(&continuation_handle)
        .unwrap_or(if rejected { 1.0 } else { 0.0 });
    let Some(continuation) = state.continuations.get_mut(&continuation_handle) else {
        return false;
    };
    let Some([state_slot, rejection_slot]) = continuation.vars.get_mut(..2) else {
        return false;
    };
    *state_slot = resume_state;
    *rejection_slot = value::encode_f64(completion);
    true
}

fn continuation_create(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let Some(&[function, outer_promise, count]) = args.get(..3) else {
        return fail_dispatch(ctx);
    };
    let Some(count) = value::decode_f64(count).to_usize() else {
        return fail_dispatch(ctx);
    };
    let Ok(object) = state.allocate_object_with_gc_retry(ctx, 2, false) else {
        return fail_dispatch(ctx);
    };
    state.continuations.insert(
        value::decode_handle(object),
        NativeContinuation {
            function,
            outer_promise,
            vars: vec![value::encode_undefined(); count],
        },
    );
    object
}

fn continuation_save_var(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let Some(&[continuation, slot, stored]) = args.get(..3) else {
        return fail_dispatch(ctx);
    };
    let continuation_handle = value::decode_handle(continuation);
    let Some(continuation) = state.continuations.get_mut(&continuation_handle) else {
        return fail_dispatch(ctx);
    };
    let Some(slot) = value::decode_f64(slot).to_usize() else {
        return fail_dispatch(ctx);
    };
    let Some(destination) = continuation.vars.get_mut(slot) else {
        return fail_dispatch(ctx);
    };
    *destination = stored;
    value::encode_undefined()
}

fn continuation_load_var(ctx: &mut NativeVmContext, state: &NativeAgentState, args: &[i64]) -> i64 {
    let Some(&[continuation, slot]) = args.get(..2) else {
        return fail_dispatch(ctx);
    };
    state
        .continuations
        .get(&value::decode_handle(continuation))
        .and_then(|continuation| {
            value::decode_f64(slot)
                .to_usize()
                .and_then(|slot| continuation.vars.get(slot).copied())
        })
        .unwrap_or_else(value::encode_undefined)
}

fn async_function_resume(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let Some(
        &[
            provided_function,
            continuation,
            state_value,
            resume_value,
            rejected,
        ],
    ) = args.get(..5)
    else {
        return fail_dispatch(ctx);
    };
    let continuation_handle = value::decode_handle(continuation);
    let Some(entry) = state.continuations.get(&continuation_handle).cloned() else {
        return fail_dispatch(ctx);
    };
    let rejected = value::is_bool(rejected) && value::decode_bool(rejected);
    if value::is_undefined(provided_function) {
        enqueue_microtask(
            state,
            NativeMicrotask::AsyncResume {
                continuation,
                state: state_value,
                value: resume_value,
                rejected,
            },
        );
        return value::encode_undefined();
    }
    if !store_async_resume_state(state, continuation_handle, state_value, rejected) {
        return fail_dispatch(ctx);
    }
    let result = state
        .invoke_callable_with_environment(ctx, provided_function, continuation, resume_value, &[])
        .unwrap_or_else(|| fail_dispatch(ctx));
    if value::is_exception(result) {
        settle_promise(
            state,
            value::decode_handle(entry.outer_promise),
            result,
            true,
        );
    }
    result
}

fn async_function_suspend(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let Some(&[promise, resume_state]) = args.get(..2) else {
        return fail_dispatch(ctx);
    };
    let continuation = state
        .call_environment()
        .unwrap_or_else(value::encode_undefined);
    let continuation_handle = value::decode_handle(continuation);
    if !state.continuations.contains_key(&continuation_handle) {
        return fail_dispatch(ctx);
    }
    let Some(promise_state) = state
        .promises
        .get(&value::decode_handle(promise))
        .map(|promise| promise.state)
    else {
        return fail_dispatch(ctx);
    };
    let reaction = NativeScheduledReaction {
        reaction: NativePromiseReaction::AsyncResume {
            continuation,
            state: resume_state,
        },
        context: super::node_async_hooks::capture_context(state),
    };
    mark_promise_handled(state, value::decode_handle(promise));
    match promise_state {
        PromiseState::Pending => state
            .promise_reactions
            .entry(value::decode_handle(promise))
            .or_default()
            .push(reaction),
        PromiseState::Fulfilled(value) => enqueue_microtask_with_context(
            state,
            NativeMicrotask::AsyncResume {
                continuation,
                state: resume_state,
                value,
                rejected: false,
            },
            reaction.context,
        ),
        PromiseState::Rejected(value) => enqueue_microtask_with_context(
            state,
            NativeMicrotask::AsyncResume {
                continuation,
                state: resume_state,
                value,
                rejected: true,
            },
            reaction.context,
        ),
    }
    value::encode_undefined()
}

fn with_resolvers(ctx: &mut NativeVmContext, state: &mut NativeAgentState) -> i64 {
    let Some(promise) = new_promise(ctx, state) else {
        return fail_dispatch(ctx);
    };
    let Ok(result) = state.allocate_object_with_gc_retry(ctx, 4, false) else {
        return fail_dispatch(ctx);
    };
    let handle = value::decode_handle(promise);
    let Some(resolve) = state.native_callable(NativeCallableKind::PromiseResolve(handle)) else {
        return fail_dispatch(ctx);
    };
    let Some(reject) = state.native_callable(NativeCallableKind::PromiseReject(handle)) else {
        return fail_dispatch(ctx);
    };
    for (name, property) in [
        ("promise", promise),
        ("resolve", resolve),
        ("reject", reject),
    ] {
        let Some(key) = state.intern_property_string(name.into()) else {
            return fail_dispatch(ctx);
        };
        if state
            .gc
            .heap()
            .set_property(value::decode_handle(result), key, property as u64)
            .is_err()
        {
            return fail_dispatch(ctx);
        }
    }
    result
}
