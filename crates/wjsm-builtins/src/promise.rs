//! Promise.prototype.then/catch/finally + Promise.resolve/reject/withResolvers
//! 后端无关算法实现。
//!
//! 算法逻辑通过 `<E: ExecContext>` 泛型单态化，零 vtable 开销。
//! host-wasm 注册层仅做 `WasmExecContext::new(caller)` + 委托调用。

use wjsm_host::{ExecContext, PromiseEntry, PromiseReaction, PromiseState, ReactionType, Value};
use wjsm_ir::value;

/// §27.2.5.4 `Promise.prototype.then(onFulfilled, onRejected)`。
pub fn promise_then_impl<E: ExecContext>(
    ctx: &mut E,
    promise: Value,
    on_fulfilled: Value,
    on_rejected: Value,
) -> Value {
    let temp_root_len = ctx.push_host_temp_roots(&[promise, on_fulfilled, on_rejected]);
    let species_constructor = ctx.promise_result_species_constructor_handle(promise);
    let result_promise = ctx.alloc_object(0);
    ctx.push_host_temp_roots(&[result_promise]);
    ctx.set_promise_proto_from_constructor(
        result_promise,
        species_constructor.unwrap_or_else(value::encode_undefined),
    );
    let result_handle = ctx.raw_promise_handle(result_promise);

    // 查询源 promise 的 capture_scope（parent scope 继承）
    let parent_scope = ctx.promise_capture_scope(promise);
    let result_scope = ctx.capture_child_promise_scope(result_promise, parent_scope);

    // 创建结果 promise entry 并插入
    let mut result_entry = PromiseEntry::pending();
    result_entry.constructor_handle = species_constructor;
    result_entry.capture_scope = result_scope;
    ctx.insert_promise_entry(result_handle, result_entry);

    // 标记源 promise 为已处理，查询状态决定 reaction 入队还是立即 microtask
    ctx.mark_promise_handled(promise);
    let state = ctx.promise_state(promise);
    match state {
        PromiseState::Pending => {
            ctx.push_promise_reaction(
                promise,
                PromiseReaction::new(on_fulfilled, result_handle as Value, ReactionType::Fulfill),
                true,
            );
            ctx.push_promise_reaction(
                promise,
                PromiseReaction::new(on_rejected, result_handle as Value, ReactionType::Reject),
                false,
            );
        }
        PromiseState::Fulfilled(val) => {
            ctx.queue_promise_reaction_microtask(
                result_handle as Value,
                ReactionType::Fulfill,
                on_fulfilled,
                val,
                result_scope,
            );
        }
        PromiseState::Rejected(reason) => {
            ctx.queue_promise_reaction_microtask(
                result_handle as Value,
                ReactionType::Reject,
                on_rejected,
                reason,
                result_scope,
            );
        }
    }
    ctx.truncate_host_temp_roots(temp_root_len);
    result_promise
}

/// §27.2.5.5 `Promise.prototype.catch(onRejected)`。
pub fn promise_catch_impl<E: ExecContext>(
    ctx: &mut E,
    promise: Value,
    on_rejected: Value,
) -> Value {
    // catch = then(undefined, onRejected)
    promise_then_impl(ctx, promise, value::encode_undefined(), on_rejected)
}

/// §27.2.5.6 `Promise.prototype.finally(onFinally)`。
pub fn promise_finally_impl<E: ExecContext>(
    ctx: &mut E,
    promise: Value,
    on_finally: Value,
) -> Value {
    let temp_root_len = ctx.push_host_temp_roots(&[promise, on_finally]);
    let species_constructor = ctx.promise_result_species_constructor_handle(promise);
    let result_promise = ctx.alloc_object(0);
    ctx.push_host_temp_roots(&[result_promise]);
    ctx.set_promise_proto_from_constructor(
        result_promise,
        species_constructor.unwrap_or_else(value::encode_undefined),
    );
    let result_handle = ctx.raw_promise_handle(result_promise);

    let parent_scope = ctx.promise_capture_scope(promise);
    let result_scope = ctx.capture_child_promise_scope(result_promise, parent_scope);

    let mut result_entry = PromiseEntry::pending();
    result_entry.constructor_handle = species_constructor;
    result_entry.capture_scope = result_scope;
    ctx.insert_promise_entry(result_handle, result_entry);

    ctx.mark_promise_handled(promise);
    let state = ctx.promise_state(promise);
    match state {
        PromiseState::Pending => {
            ctx.push_promise_reaction(
                promise,
                PromiseReaction::new(
                    on_finally,
                    result_handle as Value,
                    ReactionType::FinallyFulfill,
                ),
                true,
            );
            ctx.push_promise_reaction(
                promise,
                PromiseReaction::new(
                    on_finally,
                    result_handle as Value,
                    ReactionType::FinallyReject,
                ),
                false,
            );
        }
        PromiseState::Fulfilled(val) => {
            ctx.queue_promise_reaction_microtask(
                result_handle as Value,
                ReactionType::FinallyFulfill,
                on_finally,
                val,
                result_scope,
            );
        }
        PromiseState::Rejected(reason) => {
            ctx.queue_promise_reaction_microtask(
                result_handle as Value,
                ReactionType::FinallyReject,
                on_finally,
                reason,
                result_scope,
            );
        }
    }
    ctx.truncate_host_temp_roots(temp_root_len);
    result_promise
}

/// §27.2.4.6 `Promise.resolve(C, x)` — species-aware。
pub fn promise_resolve_static_impl<E: ExecContext>(
    ctx: &mut E,
    constructor: Value,
    val: Value,
) -> Value {
    // 若 x 是 promise，检查 SameValue(x.constructor, C)
    if ctx.is_promise_value(val) {
        let ctor_handle = ctx.promise_constructor_handle(val);
        let matches = match (ctor_handle, value::is_undefined(constructor)) {
            (None, true) => true,                       // 都是内建 Promise
            (Some(_), true) => false,                   // 子类 vs 内建
            (None, false) => false,                     // 内建 vs 子类
            (Some(ctor), false) => ctor == constructor, // 同一子类
        };
        if matches {
            return val;
        }
    }
    // NewPromiseCapability(C) + resolve(x)
    let mut entry = PromiseEntry::pending();
    if !value::is_undefined(constructor) && !value::is_null(constructor) {
        entry.constructor_handle = Some(constructor);
    }
    let promise = ctx.alloc_promise_with_entry(entry);
    ctx.resolve_promise(promise, val);
    promise
}

/// §27.2.4.5 `Promise.reject(C, r)` — species-aware。
pub fn promise_reject_static_impl<E: ExecContext>(
    ctx: &mut E,
    constructor: Value,
    reason: Value,
) -> Value {
    let mut entry = PromiseEntry::rejected(reason);
    if !value::is_undefined(constructor) && !value::is_null(constructor) {
        entry.constructor_handle = Some(constructor);
    }
    let promise = ctx.alloc_promise_with_entry(entry);
    let handle = ctx.raw_promise_handle(promise);
    ctx.push_pending_unhandled_rejection(handle);
    promise
}

/// §27.2.3.9 `Promise.withResolvers(C)` — ES2024。
pub fn promise_with_resolvers_impl<E: ExecContext>(ctx: &mut E, constructor: Value) -> Value {
    let (promise, resolve, reject) = ctx.new_promise_capability(constructor);
    let obj = ctx.alloc_object(3);
    ctx.define_data_property(obj, "promise", promise);
    ctx.define_data_property(obj, "resolve", resolve);
    ctx.define_data_property(obj, "reject", reject);
    obj
}
