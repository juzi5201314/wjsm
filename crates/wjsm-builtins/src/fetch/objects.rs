use wjsm_host::{
    AbortSignalEntry, ExecContext, FetchRequestEntry, FetchResponseEntry, HeadersEntry,
    HeadersGuard, HeadersMethodKind, NativeCallableRef, ReadableStreamEntry, RedirectMode,
    RequestCache, RequestCredentials, RequestMethodKind, RequestMode, ResponseMethodKind,
    ResponseType, SharedFetchResourceTiming, StreamState, Value,
};
use wjsm_ir::{constants, value};

use crate::streams::define_data_property_with_flags;

pub struct ResponseSpec {
    pub status: u16,
    pub status_text: String,
    pub headers_handle: u32,
    pub url: String,
    pub body: Vec<u8>,
    pub response_type: ResponseType,
    pub redirected: bool,
    pub target: Option<Value>,
    pub http_handle: Option<u32>,
}

pub struct RequestSpec {
    pub method: String,
    pub url: String,
    pub headers_handle: u32,
    pub body: Option<Vec<u8>>,
    pub redirect: RedirectMode,
    pub target: Option<Value>,
    pub signal_handle: Option<u32>,
}

fn define_data<E: ExecContext>(ctx: &mut E, object: Value, name: &str, raw: Value) {
    define_data_property_with_flags(
        ctx,
        object,
        name,
        raw,
        constants::FLAG_CONFIGURABLE | constants::FLAG_WRITABLE | constants::FLAG_ENUMERABLE,
    );
}

fn define_private<E: ExecContext>(ctx: &mut E, object: Value, name: &str, raw: Value) {
    define_data_property_with_flags(ctx, object, name, raw, constants::FLAG_PRIVATE);
}

pub fn create_empty_headers<E: ExecContext>(ctx: &mut E) -> u32 {
    ctx.alloc_headers(HeadersEntry {
        pairs: Vec::new(),
        guard: HeadersGuard::None,
    })
}

pub fn create_headers_object<E: ExecContext>(ctx: &mut E, handle: u32) -> Value {
    let object = ctx.alloc_object(16);
    init_headers_object(ctx, object, handle);
    object
}

pub fn init_headers_object<E: ExecContext>(ctx: &mut E, object: Value, handle: u32) {
    define_private(
        ctx,
        object,
        "__headers_handle__",
        value::encode_f64(handle as f64),
    );
    for (name, kind) in [
        ("get", HeadersMethodKind::Get),
        ("set", HeadersMethodKind::Set),
        ("has", HeadersMethodKind::Has),
        ("delete", HeadersMethodKind::Delete),
        ("append", HeadersMethodKind::Append),
        ("entries", HeadersMethodKind::Entries),
        ("forEach", HeadersMethodKind::ForEach),
        ("keys", HeadersMethodKind::Keys),
        ("values", HeadersMethodKind::Values),
    ] {
        let callable =
            ctx.create_native_callable(NativeCallableRef::HeadersMethod { handle, kind });
        define_data(ctx, object, name, callable);
    }
}

pub fn create_response<E: ExecContext>(ctx: &mut E, spec: ResponseSpec) -> Value {
    let handle = ctx.alloc_fetch_response(FetchResponseEntry {
        status: spec.status,
        status_text: spec.status_text.clone(),
        headers_handle: spec.headers_handle,
        headers_object: None,
        url: spec.url.clone(),
        body: spec.body.clone(),
        response_type: spec.response_type,
        redirected: spec.redirected,
        body_used: false,
        http_response_handle: spec.http_handle,
        stream_handle: None,
        resource_timing: None,
    });
    let object = spec
        .target
        .filter(|target| value::is_js_object(*target))
        .unwrap_or_else(|| ctx.alloc_object(24));
    define_private(
        ctx,
        object,
        "__response_handle__",
        value::encode_f64(handle as f64),
    );
    define_data(
        ctx,
        object,
        "ok",
        value::encode_bool((200..300).contains(&spec.status)),
    );
    define_data(
        ctx,
        object,
        "status",
        value::encode_f64(f64::from(spec.status)),
    );
    let status_text = ctx.store_string_owned(spec.status_text);
    define_data(ctx, object, "statusText", status_text);
    let url = ctx.store_string_owned(spec.url);
    define_data(ctx, object, "url", url);
    let response_type = match spec.response_type {
        ResponseType::Basic => "basic",
        ResponseType::Cors => "cors",
        ResponseType::Error => "error",
        ResponseType::Opaque => "opaque",
        ResponseType::OpaqueRedirect => "opaqueredirect",
    };
    let response_type = ctx.store_string(response_type);
    define_data(ctx, object, "type", response_type);
    define_data(
        ctx,
        object,
        "redirected",
        value::encode_bool(spec.redirected),
    );

    let (body_object, stream_handle) = if let Some(http_handle) = spec.http_handle {
        let stream_handle = ctx.alloc_readable_stream(ReadableStreamEntry {
            state: StreamState::Readable,
            error: None,
            disturbed: false,
            locked: false,
            http_response_handle: Some(http_handle),
            response_body_handle: Some(handle),
            response_body_object: Some(object),
            controller_handle: None,
            is_byte_stream: true,
            pipe_to: None,
        });
        (
            crate::streams::create_readable_stream_object(ctx, stream_handle),
            Some(stream_handle),
        )
    } else if spec.body.is_empty() {
        (value::encode_null(), None)
    } else {
        let (stream, stream_handle) = crate::streams::readable::create_closed_from_bytes(
            ctx,
            &spec.body,
            Some(handle),
            Some(object),
        );
        (stream, Some(stream_handle))
    };
    let _ = ctx.with_fetch_response(handle, |entry| entry.stream_handle = stream_handle);
    define_data(ctx, object, "body", body_object);
    define_data(ctx, object, "bodyUsed", value::encode_bool(false));
    let headers = create_headers_object(ctx, spec.headers_handle);
    let _ = ctx.with_fetch_response(handle, |entry| entry.headers_object = Some(headers));
    define_data(ctx, object, "headers", headers);
    for (name, kind) in [
        ("text", ResponseMethodKind::Text),
        ("json", ResponseMethodKind::Json),
        ("arrayBuffer", ResponseMethodKind::ArrayBuffer),
        ("clone", ResponseMethodKind::Clone),
    ] {
        let callable =
            ctx.create_native_callable(NativeCallableRef::ResponseMethod { handle, kind });
        define_data(ctx, object, name, callable);
    }
    object
}

pub fn set_response_resource_timing<E: ExecContext>(
    ctx: &mut E,
    response: Value,
    timing: Option<SharedFetchResourceTiming>,
) {
    let Some(handle) = hidden_handle(ctx, response, "__response_handle__") else {
        return;
    };
    let _ = ctx.with_fetch_response(handle, |entry| entry.resource_timing = timing);
}

pub fn create_request<E: ExecContext>(ctx: &mut E, spec: RequestSpec) -> Value {
    let handle = ctx.alloc_fetch_request(FetchRequestEntry {
        method: spec.method.clone(),
        url: spec.url.clone(),
        headers_handle: spec.headers_handle,
        headers_object: None,
        body: spec.body,
        redirect: spec.redirect,
        body_used: false,
        signal_handle: spec.signal_handle,
        mode: RequestMode::Cors,
        credentials: RequestCredentials::SameOrigin,
        cache: RequestCache::Default,
        referrer: String::new(),
        referrer_policy: String::new(),
        integrity: String::new(),
        keepalive: false,
        destination: String::new(),
        duplex: String::new(),
    });
    let object = spec
        .target
        .filter(|target| value::is_js_object(*target))
        .unwrap_or_else(|| ctx.alloc_object(12));
    define_private(
        ctx,
        object,
        "__request_handle__",
        value::encode_f64(handle as f64),
    );
    let method = ctx.store_string_owned(spec.method);
    define_data(ctx, object, "method", method);
    let url = ctx.store_string_owned(spec.url);
    define_data(ctx, object, "url", url);
    let redirect = match spec.redirect {
        RedirectMode::Follow => "follow",
        RedirectMode::Error => "error",
        RedirectMode::Manual => "manual",
    };
    let redirect = ctx.store_string(redirect);
    define_data(ctx, object, "redirect", redirect);
    define_data(ctx, object, "body", value::encode_null());
    define_data(ctx, object, "bodyUsed", value::encode_bool(false));
    define_request_init_properties(ctx, object, "default", "same-origin", "", false);
    let headers = create_headers_object(ctx, spec.headers_handle);
    let _ = ctx.with_fetch_request(handle, |entry| entry.headers_object = Some(headers));
    define_data(ctx, object, "headers", headers);
    let clone = ctx.create_native_callable(NativeCallableRef::RequestMethod {
        handle,
        kind: RequestMethodKind::Clone,
    });
    define_data(ctx, object, "clone", clone);
    object
}

pub fn define_request_init_properties<E: ExecContext>(
    ctx: &mut E,
    object: Value,
    cache: &str,
    credentials: &str,
    integrity: &str,
    keepalive: bool,
) {
    for (name, content) in [
        ("cache", cache),
        ("credentials", credentials),
        ("integrity", integrity),
    ] {
        let raw = ctx.store_string(content);
        define_data(ctx, object, name, raw);
    }
    define_data(ctx, object, "keepalive", value::encode_bool(keepalive));
}

pub fn construct_abort_controller<E: ExecContext>(ctx: &mut E, this_value: Value) -> Value {
    let signal_handle = ctx.alloc_abort_signal(AbortSignalEntry {
        aborted: false,
        reason: None,
    });
    let signal = ctx.alloc_object(3);
    define_private(
        ctx,
        signal,
        "__abort_signal_handle__",
        value::encode_f64(signal_handle as f64),
    );
    define_data(ctx, signal, "aborted", value::encode_bool(false));
    let object = if value::is_js_object(this_value) {
        this_value
    } else {
        ctx.alloc_object(3)
    };
    define_data(ctx, object, "signal", signal);
    define_private(
        ctx,
        object,
        "__abort_signal_handle__",
        value::encode_f64(signal_handle as f64),
    );
    let abort =
        ctx.create_native_callable(NativeCallableRef::AbortControllerAbort { signal_handle });
    define_data(ctx, object, "abort", abort);
    object
}

pub fn abort_controller_abort<E: ExecContext>(
    ctx: &mut E,
    signal_handle: u32,
    args: &[Value],
) -> Option<Value> {
    let reason = args.first().copied();
    let _ = ctx.with_abort_signal(signal_handle, |entry| {
        entry.aborted = true;
        entry.reason = reason;
    });
    Some(value::encode_undefined())
}

pub fn hidden_handle<E: ExecContext>(ctx: &mut E, object: Value, name: &str) -> Option<u32> {
    let raw = ctx.read_property_by_string_key(object, name);
    value::is_f64(raw).then(|| value::decode_f64(raw) as u32)
}
