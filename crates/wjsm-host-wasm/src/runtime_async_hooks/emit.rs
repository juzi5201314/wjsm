use super::HookRecord;
use crate::runtime_microtask::call_host_function_with_args_async;
use crate::runtime_process::request_process_exit;
use crate::runtime_promises::set_runtime_error;
use crate::runtime_render::render_unhandled_rejection_reason_with_env;
use crate::{RuntimeState, RuntimeStateAccess, WasmEnv, value};
use wasmtime::{AsContextMut, Caller};

#[derive(Clone, Copy)]
enum HookEvent {
    Init,
    Before,
    After,
    Destroy,
    PromiseResolve,
}

fn callback_for(record: &HookRecord, event: HookEvent, promise: bool) -> Option<i64> {
    if !record.enabled || (promise && !record.track_promises) {
        return None;
    }
    let callback = match event {
        HookEvent::Init => record.init,
        HookEvent::Before => record.before,
        HookEvent::After => record.after,
        HookEvent::Destroy => record.destroy,
        HookEvent::PromiseResolve => record.promise_resolve,
    };
    (callback != 0).then_some(callback)
}

fn begin_emit(state: &RuntimeState, event: HookEvent, promise: bool) -> Vec<i64> {
    let mut hooks = state.async_hooks.lock().unwrap_or_else(|e| e.into_inner());
    hooks.begin_emit();
    hooks
        .active_hooks()
        .iter()
        .filter_map(|record| callback_for(record, event, promise))
        .collect()
}

fn end_emit(state: &RuntimeState) {
    state
        .async_hooks
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .end_emit();
}

fn fatal_hook_result<C>(ctx: &mut C, env: &WasmEnv, result: Option<i64>) -> bool
where
    C: AsContextMut<Data = RuntimeState> + RuntimeStateAccess,
{
    let message = match result {
        Some(raw) if value::is_exception(raw) => {
            let reason =
                crate::runtime_host_helpers::exception_reason_from_state(ctx.state_mut(), raw);
            render_unhandled_rejection_reason_with_env(ctx, env, reason)
        }
        Some(_) => return false,
        None => ctx
            .state_mut()
            .runtime_error
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .unwrap_or_else(|| "async hook callback failed".to_string()),
    };
    {
        let mut hooks = ctx
            .state_mut()
            .async_hooks
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if hooks.fatal_in_progress() {
            return true;
        }
        hooks.set_fatal_in_progress(true);
    }
    crate::runtime_host_helpers::append_runtime_diagnostic(ctx, &format!("{message}\n"));
    set_runtime_error(ctx.state_mut(), message);
    request_process_exit(ctx.state_mut(), 1);
    true
}

async fn emit_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    event: HookEvent,
    promise: bool,
    args: &[i64],
) -> bool {
    let callbacks = begin_emit(caller.data(), event, promise);
    let callback_roots = caller
        .data()
        .push_host_temp_roots(callbacks.iter().copied());
    let env = match WasmEnv::from_caller(caller) {
        Some(env) => env,
        None => {
            caller.data().truncate_host_temp_roots(callback_roots);
            end_emit(caller.data());
            return false;
        }
    };
    let mut fatal = false;
    for callback in callbacks {
        let result = call_host_function_with_args_async(
            caller,
            &env,
            callback,
            value::encode_undefined(),
            args,
        )
        .await;
        if fatal_hook_result(caller, &env, result) {
            fatal = true;
            break;
        }
    }
    caller.data().truncate_host_temp_roots(callback_roots);
    end_emit(caller.data());
    fatal
}

async fn emit_from_store<C>(
    ctx: &mut C,
    env: &WasmEnv,
    event: HookEvent,
    promise: bool,
    args: &[i64],
) -> bool
where
    C: AsContextMut<Data = RuntimeState> + RuntimeStateAccess,
{
    let callbacks = begin_emit(ctx.state_mut(), event, promise);
    let mut fatal = false;
    let callback_roots = ctx
        .state_mut()
        .push_host_temp_roots(callbacks.iter().copied());
    for callback in callbacks {
        let result =
            call_host_function_with_args_async(ctx, env, callback, value::encode_undefined(), args)
                .await;
        if fatal_hook_result(ctx, env, result) {
            fatal = true;
            break;
        }
    }
    end_emit(ctx.state_mut());
    ctx.state_mut().truncate_host_temp_roots(callback_roots);
    fatal
}

pub(crate) async fn emit_init_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    async_id: u64,
    type_value: i64,
    trigger_async_id: u64,
    resource: i64,
    promise: bool,
) -> bool {
    emit_from_caller(
        caller,
        HookEvent::Init,
        promise,
        &[
            value::encode_f64(async_id as f64),
            type_value,
            value::encode_f64(trigger_async_id as f64),
            resource,
        ],
    )
    .await
}

pub(crate) async fn emit_before<C>(ctx: &mut C, env: &WasmEnv, async_id: u64, promise: bool) -> bool
where
    C: AsContextMut<Data = RuntimeState> + RuntimeStateAccess,
{
    emit_from_store(
        ctx,
        env,
        HookEvent::Before,
        promise,
        &[value::encode_f64(async_id as f64)],
    )
    .await
}

pub(crate) async fn emit_after<C>(ctx: &mut C, env: &WasmEnv, async_id: u64, promise: bool) -> bool
where
    C: AsContextMut<Data = RuntimeState> + RuntimeStateAccess,
{
    emit_from_store(
        ctx,
        env,
        HookEvent::After,
        promise,
        &[value::encode_f64(async_id as f64)],
    )
    .await
}

pub(crate) async fn emit_destroy<C>(
    ctx: &mut C,
    env: &WasmEnv,
    async_id: u64,
    promise: bool,
) -> bool
where
    C: AsContextMut<Data = RuntimeState> + RuntimeStateAccess,
{
    emit_from_store(
        ctx,
        env,
        HookEvent::Destroy,
        promise,
        &[value::encode_f64(async_id as f64)],
    )
    .await
}

pub(crate) async fn emit_promise_resolve<C>(ctx: &mut C, env: &WasmEnv, async_id: u64) -> bool
where
    C: AsContextMut<Data = RuntimeState> + RuntimeStateAccess,
{
    emit_from_store(
        ctx,
        env,
        HookEvent::PromiseResolve,
        true,
        &[value::encode_f64(async_id as f64)],
    )
    .await
}

pub(crate) async fn drain_pending_promise_events<C>(ctx: &mut C, env: &WasmEnv) -> bool
where
    C: AsContextMut<Data = RuntimeState> + RuntimeStateAccess,
{
    let mut events = ctx
        .state_mut()
        .async_hooks
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take_promise_events();
    while let Some(event) = events.pop_front() {
        let fatal = match event {
            crate::runtime_async_hooks::PendingPromiseHookEvent::Init { scope, type_value } => {
                emit_from_store(
                    ctx,
                    env,
                    HookEvent::Init,
                    true,
                    &[
                        value::encode_f64(scope.async_id as f64),
                        type_value,
                        value::encode_f64(scope.trigger_async_id as f64),
                        scope.resource,
                    ],
                )
                .await
            }
            crate::runtime_async_hooks::PendingPromiseHookEvent::Resolve { async_id } => {
                emit_promise_resolve(ctx, env, async_id).await
            }
        };
        if fatal {
            return true;
        }
    }
    false
}

pub(crate) async fn drain_destroy_queue<C>(ctx: &mut C, env: &WasmEnv) -> bool
where
    C: AsContextMut<Data = RuntimeState> + RuntimeStateAccess,
{
    let mut destroy_ids = ctx
        .state_mut()
        .async_hooks
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take_destroy_queue();
    while let Some(async_id) = destroy_ids.pop_front() {
        if emit_destroy(ctx, env, async_id, false).await {
            return true;
        }
    }
    false
}
