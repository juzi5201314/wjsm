//! op_in / iterator async builtins。
//!
//! Proxy `has` trap 与 ObjectIter `.next`/`.return` 经 `call_js_async` 再入；
//! 非 Proxy `in`、迭代器表推进走 ExecContext 低层原语。

use wjsm_host::{ExecContext, IteratorNextStep, Value, encode_symbol_name_id};
use wjsm_ir::{value, wk_symbol};

pub async fn op_in<E: ExecContext>(ctx: &mut E, object: Value, prop: Value) -> Value {
    if value::is_proxy(object) {
        let handle = value::decode_proxy_handle(object);
        if ctx.proxy_is_revoked(handle) {
            return ctx.make_type_error(
                "TypeError: Cannot perform 'has' on a proxy that has been revoked",
            );
        }
        let Some(entry) = ctx.proxy_entry(handle) else {
            return value::encode_bool(false);
        };
        let trap = ctx.read_data_property(entry.handler, "has");
        if !value::is_undefined(trap) && !value::is_null(trap) {
            let result = ctx
                .call_js_async(trap, entry.handler, &[entry.target, prop])
                .await
                .unwrap_or_else(|_| value::encode_bool(false));
            return value::encode_bool(ctx.to_boolean(result));
        }
        return Box::pin(op_in(ctx, entry.target, prop)).await;
    }
    crate::core::ordinary_has_property(ctx, object, prop)
}

pub async fn iterator_from<E: ExecContext>(ctx: &mut E, val: Value) -> Value {
    if value::is_iterator(val) {
        return val;
    }
    if value::is_string(val) {
        let s = ctx.get_runtime_string(val);
        return ctx.create_string_iterator(s);
    }
    iterator_from_fallback(ctx, val).await
}

async fn iterator_from_fallback<E: ExecContext>(ctx: &mut E, val: Value) -> Value {
    if !value::is_js_object(val) {
        return ctx.create_error_iterator();
    }
    let name_id = encode_symbol_name_id(wk_symbol::ITERATOR);
    let method = match crate::get_method::get_method_by_name_id(ctx, val, name_id) {
        Ok(Some(method)) => method,
        Ok(None) => return ctx.create_error_iterator(),
        Err(exception) => return exception,
    };
    let iterator = match ctx.call_js_async(method, val, &[]).await {
        Ok(iterator) => iterator,
        Err(_) => return ctx.create_error_iterator(),
    };
    if value::is_exception(iterator) || value::is_iterator(iterator) {
        return iterator;
    }
    if !value::is_js_object(iterator) {
        return ctx.create_error_iterator();
    }
    let wrapped = ctx.create_object_iterator(iterator);
    if value::is_undefined(wrapped) {
        ctx.create_error_iterator()
    } else {
        wrapped
    }
}

/// 将外层 iterator/next 对应到 async-from-sync 条目。
pub fn resolve_async_from_sync_afs_handle<E: ExecContext>(
    ctx: &mut E,
    handle: Value,
    next: Value,
) -> Option<u32> {
    ctx.iterator_lookup_afs(handle)
        .or_else(|| ctx.async_from_sync_native_handle(next))
}

/// 推进 async-from-sync，并把已物化 IteratorResult 写回外层迭代器状态。
pub async fn materialize_async_from_sync_next<E: ExecContext>(ctx: &mut E, afs: u32) -> Value {
    let outer = ctx
        .async_from_sync_outer_iterator(afs)
        .unwrap_or_else(|| value::encode_handle(value::TAG_ITERATOR, afs));
    let promise = ctx.advance_async_from_sync(afs).await;
    if value::is_exception(promise) {
        let reason = ctx.exception_reason(promise);
        return ctx.alloc_rejected_promise(reason);
    }
    if !ctx.is_promise_value(promise) {
        if let Some((current, done)) = ctx.parse_iterator_result(promise) {
            ctx.iterator_store_object_current(outer, current, done, true);
        }
        return promise;
    }
    match ctx.promise_settled(promise) {
        Some(Ok(result)) => {
            if let Some((current, done)) = ctx.parse_iterator_result(result) {
                ctx.iterator_store_object_current(outer, current, done, true);
            }
            result
        }
        Some(Err(_)) | None => promise,
    }
}

pub async fn iterator_next<E: ExecContext>(ctx: &mut E, handle: Value) -> Value {
    if value::is_exception(handle) {
        return ctx.promise_reject_exception(handle);
    }
    match ctx.iterator_next_sync_step(handle) {
        IteratorNextStep::Advanced | IteratorNextStep::Missing => value::encode_undefined(),
        IteratorNextStep::ErrorDone => ctx.alloc_iterator_result(value::encode_undefined(), true),
        IteratorNextStep::NeedAsyncFromSync { afs } => {
            materialize_async_from_sync_next(ctx, afs).await
        }
        IteratorNextStep::NeedObjectNext { iterator, next } => {
            advance_object_iterator_next(ctx, handle, iterator, next).await
        }
    }
}

async fn advance_object_iterator_next<E: ExecContext>(
    ctx: &mut E,
    handle: Value,
    iterator: Value,
    next: Value,
) -> Value {
    let result = match ctx.call_js_async(next, iterator, &[]).await {
        Ok(v) => v,
        Err(_) => value::encode_undefined(),
    };

    if value::is_exception(result) {
        return ctx.promise_reject_exception(result);
    }

    let mut result = result;
    let mut current_value = value::encode_undefined();
    let mut done = false;
    let mut has_current = false;

    if let Some((cv, d)) = ctx.parse_iterator_result(result) {
        current_value = cv;
        done = d;
        has_current = true;
    }

    if ctx.is_promise_value(result) {
        match ctx.promise_settled(result) {
            Some(Err(reason)) => {
                return ctx.alloc_rejected_promise(reason);
            }
            Some(Ok(settled_val)) => {
                if let Some((cv, d)) = ctx.parse_iterator_result(settled_val) {
                    result = settled_val;
                    current_value = cv;
                    done = d;
                    has_current = true;
                }
            }
            None => {
                // pending promise：直接返回
                return result;
            }
        }
    }

    ctx.iterator_store_object_current(handle, current_value, done, has_current);
    if has_current {
        if value::is_object(result) || value::is_function(result) || value::is_array(result) {
            return result;
        }
        return ctx.alloc_iterator_result(current_value, done);
    }
    if ctx.is_promise_value(result) {
        return result;
    }
    result
}

pub async fn iterator_done<E: ExecContext>(ctx: &mut E, handle: Value) -> Value {
    if let Some(done) = ctx.iterator_done_sync(handle) {
        return value::encode_bool(done);
    }
    // ObjectIter 需先推进 next
    let Some((iterator, next)) = ctx.iterator_object_next_pair(handle) else {
        return value::encode_bool(true);
    };
    let result = match ctx.call_js_async(next, iterator, &[]).await {
        Ok(v) => v,
        Err(_) => value::encode_undefined(),
    };
    let (current_value, done, has_current) =
        if let Some((cv, d)) = ctx.parse_iterator_result(result) {
            (cv, d, true)
        } else {
            (value::encode_undefined(), false, false)
        };
    ctx.iterator_store_object_current(handle, current_value, done, has_current);
    value::encode_bool(done)
}

pub async fn iterator_close<E: ExecContext>(
    ctx: &mut E,
    handle: Value,
    completion: Value,
) -> Value {
    let Some((iterator, return_method)) = ctx.iterator_object_return_pair(handle) else {
        return completion;
    };
    let Some(return_method) = return_method else {
        ctx.iterator_mark_done(handle);
        return completion;
    };
    let result = match ctx.call_js_async(return_method, iterator, &[]).await {
        Ok(v) => v,
        Err(_) => value::encode_undefined(),
    };

    if value::is_exception(result) {
        ctx.iterator_mark_done(handle);
        if value::is_exception(completion) {
            return completion;
        }
        return result;
    }

    let is_object_like =
        value::is_object(result) || value::is_function(result) || value::is_array(result);
    ctx.iterator_mark_done(handle);
    if !is_object_like {
        return ctx.make_type_error("TypeError: iterator return must return an object");
    }
    completion
}

pub async fn iterator_step_value<E: ExecContext>(ctx: &mut E, handle: Value) -> Value {
    let done = iterator_done(ctx, handle).await;
    if value::decode_bool(done) {
        return value::encode_undefined();
    }
    let value = ctx.iterator_current_value(handle);
    let _ = iterator_next(ctx, handle).await;
    value
}
