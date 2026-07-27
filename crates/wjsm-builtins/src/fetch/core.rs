use base64::Engine;
use wjsm_host::{
    ExecContext, HeadersGuard, HttpRequestSpec, PromiseSettlement, ResponseType, Value,
};
use wjsm_ir::value;

use super::constructors::resolve_request;
use super::objects::{create_response, set_response_resource_timing, ResponseSpec};
use super::resource_timing;

#[derive(Debug, thiserror::Error)]
enum DataUrlError {
    #[error("invalid data URL")]
    Invalid,
    #[error("invalid data URL: missing ','")]
    MissingComma,
    #[error("invalid base64 in data URL: {0}")]
    InvalidBase64(#[from] base64::DecodeError),
}

pub async fn fetch<E: ExecContext>(ctx: &mut E, input: Value, init: Value) -> Value {
    let promise = ctx.alloc_promise();
    let request = match resolve_request(ctx, input, init) {
        Ok(request) => request,
        Err(exception) => {
            let reason = ctx.exception_reason(exception);
            ctx.settle_promise(promise, PromiseSettlement::Reject(reason));
            return promise;
        }
    };
    if request.url.is_empty() {
        reject_type_error(ctx, promise, "Failed to parse URL from request.");
        return promise;
    }
    let suppress_timing = if value::is_js_object(init) {
        let raw = ctx.read_property_by_string_key(init, "__wjsm_internal_no_resource_timing");
        value::is_bool(raw) && value::decode_bool(raw)
    } else {
        false
    };
    let timing = resource_timing::begin(ctx, request.url.clone(), suppress_timing);
    if request.url.starts_with("data:") {
        resource_timing::mark_request_start(ctx, &timing);
        match perform_data_url_fetch(ctx, &request.url) {
            Ok(response) => {
                resource_timing::mark_response_start(ctx, &timing, 200);
                set_response_resource_timing(ctx, response, timing);
                ctx.settle_promise(promise, PromiseSettlement::Fulfill(response));
            }
            Err(error) => reject_type_error(ctx, promise, &error.to_string()),
        }
        return promise;
    }
    let result = ctx
        .http_fetch_begin(HttpRequestSpec {
            method: request.method,
            url: request.url,
            headers_handle: request.headers_handle,
            body: request.body,
            redirect: request.redirect,
            signal_handle: request.signal_handle,
            resource_timing: timing,
        })
        .await;
    match result {
        Ok(response) => ctx.settle_promise(promise, PromiseSettlement::Fulfill(response)),
        Err(error) => reject_type_error(ctx, promise, &error.to_string()),
    }
    promise
}

fn reject_type_error<E: ExecContext>(ctx: &mut E, promise: Value, message: &str) {
    let exception = ctx.make_type_error(message);
    let error = ctx.exception_reason(exception);
    ctx.settle_promise(promise, PromiseSettlement::Reject(error));
}

fn perform_data_url_fetch<E: ExecContext>(
    ctx: &mut E,
    url: &str,
) -> Result<Value, DataUrlError> {
    let (media_type, is_base64, data) = parse_data_url(url)?;
    let bytes = if is_base64 {
        base64::engine::general_purpose::STANDARD.decode(data.as_bytes())?
    } else {
        percent_decode_to_bytes(&data)
    };
    let headers = ctx.alloc_headers(wjsm_host::HeadersEntry {
        pairs: vec![("content-type".to_string(), media_type)],
        guard: HeadersGuard::None,
    });
    Ok(create_response(
        ctx,
        ResponseSpec {
            status: 200,
            status_text: "OK".to_string(),
            headers_handle: headers,
            url: url.to_string(),
            body: bytes,
            response_type: ResponseType::Basic,
            redirected: false,
            target: None,
            http_handle: None,
        },
    ))
}

fn parse_data_url(url: &str) -> Result<(String, bool, String), DataUrlError> {
    let rest = url.strip_prefix("data:").ok_or(DataUrlError::Invalid)?;
    let comma = rest.find(',').ok_or(DataUrlError::MissingComma)?;
    let meta = &rest[..comma];
    let data = rest[comma + 1..].to_string();
    let meta_lower = meta.to_ascii_lowercase();
    let is_base64 = meta_lower.contains(";base64");
    let media_type = if is_base64 {
        meta_lower
            .split_once(";base64")
            .map_or("", |(before, _)| before)
    } else {
        meta
    };
    let media_type = if media_type.is_empty() {
        "text/plain;charset=US-ASCII".to_string()
    } else {
        media_type.to_string()
    };
    Ok((media_type, is_base64, data))
}

fn percent_decode_to_bytes(input: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(input.len());
    let mut characters = input.chars();
    while let Some(character) = characters.next() {
        if character == '%' {
            let high = characters.next().and_then(|digit| digit.to_digit(16));
            let low = characters.next().and_then(|digit| digit.to_digit(16));
            if let (Some(high), Some(low)) = (high, low) {
                bytes.push(u8::try_from(high * 16 + low).expect("two hex digits fit in u8"));
            } else {
                bytes.push(b'%');
            }
        } else if character == '+' {
            bytes.push(b' ');
        } else {
            let mut buffer = [0; 4];
            bytes.extend_from_slice(character.encode_utf8(&mut buffer).as_bytes());
        }
    }
    bytes
}
