use wjsm_host::{ExecContext, PromiseSettlement, ResponseMethodKind, Value};
use wjsm_ir::value;

use super::headers::clone_handle;
use super::objects::{ResponseSpec, create_response};
use super::resource_timing;

pub fn call_method<E: ExecContext>(
    ctx: &mut E,
    this_value: Value,
    handle: u32,
    kind: ResponseMethodKind,
) -> Option<Value> {
    let consuming = matches!(
        kind,
        ResponseMethodKind::Text | ResponseMethodKind::Json | ResponseMethodKind::ArrayBuffer
    );
    let (entry, was_body_used) = ctx.with_fetch_response(handle, |entry| {
        let was_body_used = entry.body_used;
        if consuming {
            entry.body_used = true;
        }
        (entry.clone(), was_body_used)
    })?;
    if consuming && was_body_used {
        return Some(rejected_type_error(ctx, "body stream already read"));
    }
    if consuming
        && let Some(stream_handle) = entry.stream_handle
        && ctx
            .with_readable_stream(stream_handle, |stream| stream.locked)
            .unwrap_or(false)
    {
        return Some(rejected_type_error(ctx, "body stream is locked"));
    }
    if consuming && let Some(http_handle) = entry.http_response_handle {
        let promise = ctx.alloc_promise();
        if ctx.consume_fetch_body_to_bytes(http_handle, promise, kind) {
            set_body_used(ctx, this_value);
            return Some(promise);
        }
    }
    let body_length = entry.body.len();
    let result = match kind {
        ResponseMethodKind::Text => {
            let text = String::from_utf8_lossy(&entry.body).into_owned();
            let text = ctx.store_string_owned(text);
            fulfilled(ctx, text)
        }
        ResponseMethodKind::Json => {
            let text = String::from_utf8_lossy(&entry.body).into_owned();
            let text = ctx.store_string_owned(text);
            let parsed = crate::json::json_parse_sync_impl(ctx, text, value::encode_undefined());
            let promise = ctx.alloc_promise();
            if value::is_exception(parsed) {
                let reason = ctx.exception_reason(parsed);
                ctx.settle_promise(promise, PromiseSettlement::Reject(reason));
            } else {
                ctx.settle_promise(promise, PromiseSettlement::Fulfill(parsed));
            }
            promise
        }
        ResponseMethodKind::ArrayBuffer => {
            let buffer = ctx.create_arraybuffer_from_bytes(&entry.body);
            fulfilled(ctx, buffer)
        }
        ResponseMethodKind::Clone => {
            let headers = clone_handle(ctx, entry.headers_handle);
            let timing = entry.resource_timing.clone();
            let response = create_response(
                ctx,
                ResponseSpec {
                    status: entry.status,
                    status_text: entry.status_text,
                    headers_handle: headers,
                    url: entry.url,
                    body: entry.body,
                    response_type: entry.response_type,
                    redirected: entry.redirected,
                    target: None,
                    http_handle: None,
                },
            );
            super::objects::set_response_resource_timing(ctx, response, timing);
            return Some(response);
        }
    };
    if consuming {
        if entry.http_response_handle.is_none() {
            resource_timing::record_body_bytes(&entry.resource_timing, body_length, body_length);
            resource_timing::complete(ctx, &entry.resource_timing);
        }
        set_body_used(ctx, this_value);
    }
    Some(result)
}

fn fulfilled<E: ExecContext>(ctx: &mut E, raw: Value) -> Value {
    let promise = ctx.alloc_promise();
    ctx.settle_promise(promise, PromiseSettlement::Fulfill(raw));
    promise
}

fn rejected_type_error<E: ExecContext>(ctx: &mut E, message: &str) -> Value {
    let exception = ctx.make_type_error(message);
    let error = ctx.exception_reason(exception);
    let promise = ctx.alloc_promise();
    ctx.settle_promise(promise, PromiseSettlement::Reject(error));
    promise
}

fn set_body_used<E: ExecContext>(ctx: &mut E, response: Value) {
    if let Some(handle) = ctx.handle_index_of(response) {
        ctx.set_property(handle, "bodyUsed", value::encode_bool(true));
    }
}
