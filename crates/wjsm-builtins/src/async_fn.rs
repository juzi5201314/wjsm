//! AsyncFunction / Continuation 宿主 builtin。

use wjsm_host::{ExecContext, Value};
use wjsm_ir::value;

/// `env.async_function_start(fn_table_idx)`。
pub fn async_function_start<E: ExecContext>(ctx: &mut E, fn_table_idx: Value) -> Value {
    let resolved = ctx.resolve_func_table_idx(fn_table_idx);
    let outer_promise = ctx.alloc_promise();
    let cont_handle = ctx.alloc_continuation(resolved, outer_promise, 4);
    ctx.continuation_set_var(cont_handle, 2, outer_promise);
    ctx.enqueue_async_resume(
        resolved,
        value::encode_object_handle(cont_handle),
        0,
        value::encode_undefined(),
        0,
    );
    outer_promise
}

/// `env.async_function_resume`。
pub fn async_function_resume<E: ExecContext>(
    ctx: &mut E,
    fn_table_idx: Value,
    continuation: Value,
    state: Value,
    resume_val: Value,
    completion_raw: Value,
) {
    let resolved = ctx.resolve_func_table_idx(fn_table_idx);
    let state_u = nanbox_to_u32(state);
    let completion = nanbox_to_u32(completion_raw) as u8;
    let cont_handle = value::decode_object_handle(continuation);
    ctx.continuation_set_var(cont_handle, 0, value::encode_f64(state_u as f64));
    ctx.continuation_set_var(cont_handle, 1, value::encode_f64(completion as f64));
    // §27.7.5.2：初始调用(state=0)同步执行直至首个 await
    if state_u == 0 {
        let handled = ctx.async_function_initial_call(resolved, continuation, resume_val);
        if handled {
            return;
        }
    }
    ctx.enqueue_async_resume(resolved, continuation, state_u, resume_val, completion);
}

/// `env.async_function_suspend`。
pub fn async_function_suspend<E: ExecContext>(
    ctx: &mut E,
    continuation: Value,
    awaited_promise: Value,
    state: Value,
) {
    ctx.async_function_suspend(continuation, awaited_promise, state);
}

/// `env.continuation_create`。
pub fn continuation_create<E: ExecContext>(
    ctx: &mut E,
    fn_table_idx: Value,
    outer_promise: Value,
    captured_var_count: Value,
) -> Value {
    let resolved = ctx.resolve_func_table_idx(fn_table_idx);
    let total = nanbox_to_usize(captured_var_count);
    let handle = ctx.alloc_continuation(resolved, outer_promise, total);
    value::encode_object_handle(handle)
}

/// `env.continuation_save_var`。
pub fn continuation_save_var<E: ExecContext>(
    ctx: &mut E,
    continuation: Value,
    slot: Value,
    val: Value,
) {
    let handle = value::decode_object_handle(continuation);
    let actual_slot = nanbox_to_usize(slot);
    ctx.continuation_set_var(handle, actual_slot, val);
}

/// `env.continuation_load_var`。
pub fn continuation_load_var<E: ExecContext>(
    ctx: &mut E,
    continuation: Value,
    slot: Value,
) -> Value {
    let handle = value::decode_object_handle(continuation);
    let actual_slot = nanbox_to_usize(slot);
    ctx.continuation_get_var(handle, actual_slot)
}

fn nanbox_to_u32(val: Value) -> u32 {
    nanbox_to_usize(val) as u32
}

fn nanbox_to_usize(val: Value) -> usize {
    if value::is_bool(val) {
        if value::decode_bool(val) { 1 } else { 0 }
    } else {
        value::decode_f64(val) as usize
    }
}
