use wjsm_host::{
    ExecContext, RedirectMode, RequestMethodKind, ResponseType, Value,
};
use wjsm_ir::value;

use super::headers::{
    clone_handle, create_from_init, object_headers_handle, string_from_value,
};
use super::objects::{
    create_empty_headers, create_request, create_response, define_request_init_properties,
    hidden_handle, RequestSpec, ResponseSpec,
};

pub struct ResolvedRequest {
    pub method: String,
    pub url: String,
    pub headers_handle: u32,
    pub body: Option<Vec<u8>>,
    pub redirect: RedirectMode,
    pub signal_handle: Option<u32>,
}

pub fn construct_request<E: ExecContext>(
    ctx: &mut E,
    this_value: Value,
    args: &[Value],
) -> Value {
    let input = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    if value::is_undefined(input) {
        return ctx.make_type_error("Request input is required");
    }
    let init = args.get(1).copied().unwrap_or_else(value::encode_undefined);
    let resolved = match resolve_request(ctx, input, init) {
        Ok(resolved) => resolved,
        Err(exception) => return exception,
    };
    let mut cache = "default".to_string();
    let mut credentials = "same-origin".to_string();
    let mut integrity = String::new();
    let mut keepalive = false;
    let copied_request = hidden_handle(ctx, input, "__request_handle__").is_some();
    if copied_request {
        cache = string_property(ctx, input, "cache").ok().flatten().unwrap_or(cache);
        credentials = string_property(ctx, input, "credentials")
            .ok()
            .flatten()
            .unwrap_or(credentials);
        integrity = string_property(ctx, input, "integrity")
            .ok()
            .flatten()
            .unwrap_or(integrity);
        keepalive = bool_property(ctx, input, "keepalive").unwrap_or(false);
    }
    if value::is_js_object(init) {
        match string_property(ctx, init, "cache") {
            Ok(Some(raw)) if valid_cache(&raw) => cache = raw,
            Ok(Some(_)) => return ctx.make_type_error("invalid Request cache mode"),
            Ok(None) => {}
            Err(exception) => return exception,
        }
        match string_property(ctx, init, "credentials") {
            Ok(Some(raw)) if matches!(raw.as_str(), "omit" | "same-origin" | "include") => {
                credentials = raw;
            }
            Ok(Some(_)) => return ctx.make_type_error("invalid Request credentials mode"),
            Ok(None) => {}
            Err(exception) => return exception,
        }
        match string_property(ctx, init, "integrity") {
            Ok(Some(raw)) => integrity = raw,
            Ok(None) => {}
            Err(exception) => return exception,
        }
        if let Some(raw) = bool_property(ctx, init, "keepalive") {
            keepalive = raw;
        }
    }
    let request = create_request(
        ctx,
        RequestSpec {
            method: resolved.method,
            url: resolved.url,
            headers_handle: resolved.headers_handle,
            body: resolved.body,
            redirect: resolved.redirect,
            target: Some(this_value),
            signal_handle: resolved.signal_handle,
        },
    );
    define_request_init_properties(
        ctx,
        request,
        &cache,
        &credentials,
        &integrity,
        keepalive,
    );
    if copied_request {
        let url = ctx.read_property_by_string_key(input, "url");
        if let Some(handle) = ctx.handle_index_of(request) {
            ctx.set_property(handle, "url", url);
        }
    }
    request
}

pub fn construct_response<E: ExecContext>(
    ctx: &mut E,
    this_value: Value,
    args: &[Value],
) -> Value {
    let body_raw = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let body = match body_bytes(ctx, body_raw) {
        Ok(body) => body,
        Err(exception) => return exception,
    };
    let mut status = 200u16;
    let mut status_text = String::new();
    let mut headers = create_empty_headers(ctx);
    if let Some(init) = args.get(1).copied().filter(|raw| value::is_js_object(*raw)) {
        if let Some(raw_status) = number_property(ctx, init, "status") {
            if !raw_status.is_finite()
                || raw_status.fract() != 0.0
                || !(200.0..=599.0).contains(&raw_status)
            {
                return ctx.make_type_error("Response status must be 200-599");
            }
            status = raw_status as u16;
        }
        match string_property(ctx, init, "statusText") {
            Ok(Some(raw)) if valid_status_text(&raw) => status_text = raw,
            Ok(Some(_)) => return ctx.make_type_error("invalid Response statusText"),
            Ok(None) => {}
            Err(exception) => return exception,
        }
        let init_headers = ctx.read_property_by_string_key(init, "headers");
        if !value::is_undefined(init_headers) {
            match create_from_init(ctx, init_headers) {
                Ok(handle) => headers = handle,
                Err(exception) => return exception,
            }
        }
    }
    if body.is_some() && matches!(status, 101..=103 | 204 | 205 | 304) {
        return ctx.make_type_error("Response with null-body status cannot have body");
    }
    create_response(
        ctx,
        ResponseSpec {
            status,
            status_text,
            headers_handle: headers,
            url: String::new(),
            body: body.unwrap_or_default(),
            response_type: ResponseType::Basic,
            redirected: false,
            target: Some(this_value),
            http_handle: None,
        },
    )
}

pub fn resolve_request<E: ExecContext>(
    ctx: &mut E,
    input: Value,
    init: Value,
) -> Result<ResolvedRequest, Value> {
    let (mut method, url, mut headers_handle, mut body, mut redirect, mut signal_handle) =
        if let Some(request_handle) = hidden_handle(ctx, input, "__request_handle__") {
            let Some(entry) = ctx.with_fetch_request(request_handle, |entry| entry.clone()) else {
                return Err(ctx.make_type_error("invalid Request object"));
            };
            (
                entry.method,
                entry.url,
                clone_handle(ctx, entry.headers_handle),
                entry.body,
                entry.redirect,
                entry.signal_handle,
            )
        } else if value::is_string(input) {
            (
                "GET".to_string(),
                ctx.read_string_utf8_lossy(input),
                create_empty_headers(ctx),
                None,
                RedirectMode::Follow,
                None,
            )
        } else if value::is_js_object(input) {
            let url = string_property(ctx, input, "url")?.unwrap_or_default();
            (
                "GET".to_string(),
                url,
                create_empty_headers(ctx),
                None,
                RedirectMode::Follow,
                None,
            )
        } else {
            return Err(ctx.make_type_error("Failed to parse URL from request."));
        };
    if url_has_credentials(&url) {
        return Err(ctx.make_type_error("Request URL contains credentials"));
    }
    if value::is_js_object(init) {
        if let Some(init_method) = string_property(ctx, init, "method")? {
            let upper = init_method.to_ascii_uppercase();
            if !valid_method(&upper) || matches!(upper.as_str(), "CONNECT" | "TRACE" | "TRACK") {
                return Err(ctx.make_type_error("invalid Request method"));
            }
            method = upper;
        }
        let init_headers = ctx.read_property_by_string_key(init, "headers");
        if !value::is_undefined(init_headers) {
            headers_handle = create_from_init(ctx, init_headers)?;
        }
        let init_body = ctx.read_property_by_string_key(init, "body");
        if !value::is_undefined(init_body) {
            body = body_bytes(ctx, init_body)?;
        }
        if let Some(raw_redirect) = string_property(ctx, init, "redirect")? {
            redirect = match raw_redirect.as_str() {
                "follow" => RedirectMode::Follow,
                "error" => RedirectMode::Error,
                "manual" => RedirectMode::Manual,
                _ => return Err(ctx.make_type_error("invalid Request redirect mode")),
            };
        }
        let signal = ctx.read_property_by_string_key(init, "signal");
        if !value::is_undefined(signal) {
            signal_handle = hidden_handle(ctx, signal, "__abort_signal_handle__");
        }
    }
    if body.is_some() && matches!(method.as_str(), "GET" | "HEAD") {
        return Err(ctx.make_type_error("Request with GET/HEAD method cannot have body"));
    }
    Ok(ResolvedRequest {
        method,
        url,
        headers_handle,
        body,
        redirect,
        signal_handle,
    })
}

pub fn call_request_method<E: ExecContext>(
    ctx: &mut E,
    this_value: Value,
    handle: u32,
    kind: RequestMethodKind,
) -> Option<Value> {
    match kind {
        RequestMethodKind::Clone => {
            let entry = ctx.with_fetch_request(handle, |entry| entry.clone())?;
            let headers = clone_handle(ctx, entry.headers_handle);
            let request = create_request(
                ctx,
                RequestSpec {
                    method: entry.method,
                    url: entry.url,
                    headers_handle: headers,
                    body: entry.body,
                    redirect: entry.redirect,
                    target: None,
                    signal_handle: entry.signal_handle,
                },
            );
            let url = ctx.read_property_by_string_key(this_value, "url");
            if let Some(request_handle) = ctx.handle_index_of(request) {
                ctx.set_property(request_handle, "url", url);
            }
            Some(request)
        }
    }
}

pub fn body_bytes<E: ExecContext>(ctx: &mut E, raw: Value) -> Result<Option<Vec<u8>>, Value> {
    if value::is_undefined(raw) || value::is_null(raw) {
        return Ok(None);
    }
    Ok(Some(string_from_value(ctx, raw)?.into_bytes()))
}

pub fn string_property<E: ExecContext>(
    ctx: &mut E,
    object: Value,
    name: &str,
) -> Result<Option<String>, Value> {
    let raw = ctx.read_property_by_string_key(object, name);
    if value::is_undefined(raw) {
        Ok(None)
    } else {
        string_from_value(ctx, raw).map(Some)
    }
}

fn number_property<E: ExecContext>(ctx: &mut E, object: Value, name: &str) -> Option<f64> {
    let raw = ctx.read_property_by_string_key(object, name);
    value::is_f64(raw).then(|| value::decode_f64(raw))
}

fn bool_property<E: ExecContext>(ctx: &mut E, object: Value, name: &str) -> Option<bool> {
    let raw = ctx.read_property_by_string_key(object, name);
    (!value::is_undefined(raw)).then(|| !value::is_falsy(raw))
}

fn valid_method(method: &str) -> bool {
    !method.is_empty()
        && method.as_bytes().iter().all(|byte| {
            matches!(
                *byte,
                b'0'..=b'9'
                    | b'A'..=b'Z'
                    | b'a'..=b'z'
                    | b'!'
                    | b'#'
                    | b'$'
                    | b'%'
                    | b'&'
                    | b'\''
                    | b'*'
                    | b'+'
                    | b'-'
                    | b'.'
                    | b'^'
                    | b'_'
                    | b'`'
                    | b'|'
                    | b'~'
            )
        })
}

fn url_has_credentials(url: &str) -> bool {
    let Some(scheme_end) = url.find("://") else {
        return false;
    };
    let rest = &url[scheme_end + 3..];
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    rest[..authority_end].contains('@')
}

fn valid_cache(raw: &str) -> bool {
    matches!(
        raw,
        "default" | "no-store" | "reload" | "no-cache" | "force-cache" | "only-if-cached"
    )
}

fn valid_status_text(raw: &str) -> bool {
    raw.bytes()
        .all(|byte| matches!(byte, b'\t' | 0x20..=0x7e | 0x80..=0xff))
}

pub fn object_is_headers<E: ExecContext>(ctx: &mut E, object: Value) -> bool {
    object_headers_handle(ctx, object).is_some()
}
