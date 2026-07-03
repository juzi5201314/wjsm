use super::streams_readable::create_closed_readable_stream_from_bytes;
use crate::*;
use wasmtime::Caller;

// ── Object construction helpers (Headers / Response / Request) ──────────────

pub(crate) fn create_empty_headers(caller: &mut Caller<'_, RuntimeState>) -> u32 {
    let mut table = caller
        .data()
        .headers_table
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let h = table.len() as u32;
    table.push(HeadersEntry {
        pairs: Vec::new(),
        guard: HeadersGuard::None,
    });
    h
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn create_response_object(
    caller: &mut Caller<'_, RuntimeState>,
    status: u16,
    status_text: String,
    headers_handle: u32,
    url: String,
    body: Vec<u8>,
    response_type: ResponseType,
    redirected: bool,
    target_obj: Option<i64>,
) -> i64 {
    let mut table = caller
        .data()
        .fetch_response_table
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let handle = table.len() as u32;
    table.push(FetchResponseEntry {
        status,
        status_text: status_text.clone(),
        headers_handle,
        headers_object: None,
        url: url.clone(),
        body,
        response_type,
        redirected,
        body_used: false,
        http_response_handle: None,
        stream_handle: None,
    });
    drop(table);

    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let obj = target_obj
        .filter(|obj| value::is_object(*obj))
        .unwrap_or_else(|| alloc_host_object(caller, &env, 12));

    // Hidden handle for native methods
    let handle_val = value::encode_f64(handle as f64);
    let _ = define_host_data_property_from_caller(caller, obj, "__response_handle__", handle_val);

    // Public properties (data descriptors)
    let ok_val = value::encode_bool((200..300).contains(&status));
    let _ = define_host_data_property_from_caller(caller, obj, "ok", ok_val);

    let status_val = value::encode_f64(status as f64);
    let _ = define_host_data_property_from_caller(caller, obj, "status", status_val);

    let status_text_val = store_runtime_string(caller, status_text);
    let _ = define_host_data_property_from_caller(caller, obj, "statusText", status_text_val);

    let url_val = store_runtime_string(caller, url);
    let _ = define_host_data_property_from_caller(caller, obj, "url", url_val);

    let type_val = store_runtime_string(caller, "basic".to_string());
    let _ = define_host_data_property_from_caller(caller, obj, "type", type_val);

    let redirected_val = value::encode_bool(redirected);
    let _ = define_host_data_property_from_caller(caller, obj, "redirected", redirected_val);

    // body / bodyUsed — 非空 body 创建已关闭的 ReadableStream，空 body 保持 null
    let body_bytes_opt = {
        let table = caller
            .data()
            .fetch_response_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        table
            .get(handle as usize)
            .filter(|entry| !entry.body.is_empty())
            .map(|entry| entry.body.clone())
    };
    let (body_val, stream_handle) = if let Some(bytes) = &body_bytes_opt {
        let (obj, sh) =
            create_closed_readable_stream_from_bytes(caller, bytes, Some(handle), Some(obj));
        (obj, Some(sh))
    } else {
        (value::encode_null(), None)
    };
    if let Some(sh) = stream_handle {
        let mut table = caller
            .data()
            .fetch_response_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = table.get_mut(handle as usize) {
            entry.stream_handle = Some(sh);
        }
    }
    let _ = define_host_data_property_from_caller(caller, obj, "body", body_val);
    let _ =
        define_host_data_property_from_caller(caller, obj, "bodyUsed", value::encode_bool(false));

    // headers object
    let headers_obj = create_headers_object_from_handle(caller, headers_handle);
    if let Some(entry) = caller
        .data()
        .fetch_response_table
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_mut(handle as usize)
    {
        entry.headers_object = Some(headers_obj);
    }
    let _ = define_host_data_property_from_caller(caller, obj, "headers", headers_obj);

    // Attach method callables (text, json, arrayBuffer, clone)
    attach_response_methods(caller, obj, handle);

    obj
}

pub(crate) fn init_headers_object(caller: &mut Caller<'_, RuntimeState>, obj: i64, handle: u32) {
    let handle_val = value::encode_f64(handle as f64);
    let _ = define_host_data_property_from_caller(caller, obj, "__headers_handle__", handle_val);
    attach_headers_methods(caller, obj, handle);
}

pub(crate) fn create_headers_object_from_handle(
    caller: &mut Caller<'_, RuntimeState>,
    headers_handle: u32,
) -> i64 {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let obj = alloc_host_object(caller, &env, 8);
    init_headers_object(caller, obj, headers_handle);
    obj
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn create_response_object_with_http_handle(
    caller: &mut Caller<'_, RuntimeState>,
    status: u16,
    status_text: String,
    headers_handle: u32,
    url: String,
    response_type: ResponseType,
    redirected: bool,
    http_handle: u32,
) -> i64 {
    let mut table = caller
        .data()
        .fetch_response_table
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let handle = table.len() as u32;
    table.push(FetchResponseEntry {
        status,
        status_text: status_text.clone(),
        headers_handle,
        headers_object: None,
        url: url.clone(),
        body: Vec::new(),
        response_type,
        redirected,
        body_used: false,
        http_response_handle: Some(http_handle),
        stream_handle: None,
    });
    drop(table);

    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let obj = alloc_host_object(caller, &env, 12);

    let handle_val = value::encode_f64(handle as f64);
    let _ = define_host_data_property_from_caller(caller, obj, "__response_handle__", handle_val);

    let ok_val = value::encode_bool((200..300).contains(&status));
    let _ = define_host_data_property_from_caller(caller, obj, "ok", ok_val);

    let status_val = value::encode_f64(status as f64);
    let _ = define_host_data_property_from_caller(caller, obj, "status", status_val);

    let status_text_val = store_runtime_string(caller, status_text);
    let _ = define_host_data_property_from_caller(caller, obj, "statusText", status_text_val);

    let url_val = store_runtime_string(caller, url);
    let _ = define_host_data_property_from_caller(caller, obj, "url", url_val);

    let type_val = store_runtime_string(caller, "basic".to_string());
    let _ = define_host_data_property_from_caller(caller, obj, "type", type_val);

    let redirected_val = value::encode_bool(redirected);
    let _ = define_host_data_property_from_caller(caller, obj, "redirected", redirected_val);

    // body / bodyUsed
    let stream_handle = caller
        .data()
        .readable_stream_table
        .alloc(ReadableStreamEntry {
            state: StreamState::Readable,
            error: None,
            disturbed: false,
            locked: false,
            http_response_handle: Some(http_handle),
            response_body_handle: Some(handle),
            response_body_object: Some(obj),
            controller_handle: None,
            is_byte_stream: true,
            pipe_to: None,
        });
    {
        let mut table = caller
            .data()
            .fetch_response_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = table.get_mut(handle as usize) {
            entry.stream_handle = Some(stream_handle);
        }
    }
    let stream_obj =
        super::streams_readable::create_readable_stream_js_object(caller, stream_handle);
    let stream_obj_handle = weak_target_handle_index_of(caller, stream_obj).unwrap_or(0);
    caller
        .data()
        .readable_stream_table
        .bind_obj_handle(stream_obj_handle, stream_handle);
    let _ = define_host_data_property_from_caller(caller, obj, "body", stream_obj);
    let _ =
        define_host_data_property_from_caller(caller, obj, "bodyUsed", value::encode_bool(false));

    let headers_obj = create_headers_object_from_handle(caller, headers_handle);
    if let Some(entry) = caller
        .data()
        .fetch_response_table
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_mut(handle as usize)
    {
        entry.headers_object = Some(headers_obj);
    }
    let _ = define_host_data_property_from_caller(caller, obj, "headers", headers_obj);

    attach_response_methods(caller, obj, handle);

    obj
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn create_request_object(
    caller: &mut Caller<'_, RuntimeState>,
    method: String,
    url: String,
    headers_handle: u32,
    body: Option<Vec<u8>>,
    redirect: RedirectMode,
    target_obj: Option<i64>,
    signal_handle: Option<u32>,
) -> i64 {
    let mut table = caller
        .data()
        .fetch_request_table
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let handle = table.len() as u32;
    table.push(FetchRequestEntry {
        method: method.clone(),
        url: url.clone(),
        headers_handle,
        headers_object: None,
        body,
        redirect,
        body_used: false,
        signal_handle,
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
    drop(table);
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let obj = target_obj
        .filter(|obj| value::is_object(*obj))
        .unwrap_or_else(|| alloc_host_object(caller, &env, 8));
    let handle_val = value::encode_f64(handle as f64);
    let _ = define_host_data_property_from_caller(caller, obj, "__request_handle__", handle_val);

    let method_val = store_runtime_string(caller, method);
    let _ = define_host_data_property_from_caller(caller, obj, "method", method_val);

    let url_val = store_runtime_string(caller, url);
    let _ = define_host_data_property_from_caller(caller, obj, "url", url_val);

    let redirect_str = match redirect {
        RedirectMode::Follow => "follow",
        RedirectMode::Error => "error",
        RedirectMode::Manual => "manual",
    };
    let redirect_val = store_runtime_string(caller, redirect_str.to_string());
    let _ = define_host_data_property_from_caller(caller, obj, "redirect", redirect_val);

    let _ = define_host_data_property_from_caller(caller, obj, "body", value::encode_null());
    let _ =
        define_host_data_property_from_caller(caller, obj, "bodyUsed", value::encode_bool(false));
    define_request_string_property(caller, obj, "cache", "default");
    define_request_string_property(caller, obj, "credentials", "same-origin");
    define_request_string_property(caller, obj, "integrity", "");
    let _ =
        define_host_data_property_from_caller(caller, obj, "keepalive", value::encode_bool(false));
    let headers_obj = create_headers_object_from_handle(caller, headers_handle);
    if let Some(entry) = caller
        .data()
        .fetch_request_table
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_mut(handle as usize)
    {
        entry.headers_object = Some(headers_obj);
    }
    let _ = define_host_data_property_from_caller(caller, obj, "headers", headers_obj);

    attach_request_methods(caller, obj, handle);
    obj
}

// ── Method attachment helpers ───────────────────────────────────────────────

fn attach_headers_methods(caller: &mut Caller<'_, RuntimeState>, obj: i64, handle: u32) {
    let methods: &[(&str, HeadersMethodKind)] = &[
        ("get", HeadersMethodKind::Get),
        ("set", HeadersMethodKind::Set),
        ("has", HeadersMethodKind::Has),
        ("delete", HeadersMethodKind::Delete),
        ("append", HeadersMethodKind::Append),
        ("entries", HeadersMethodKind::Entries),
        ("forEach", HeadersMethodKind::ForEach),
        ("keys", HeadersMethodKind::Keys),
        ("values", HeadersMethodKind::Values),
    ];

    for (name, kind) in methods {
        let callable = NativeCallable::HeadersMethod {
            handle,
            kind: *kind,
        };
        let idx = push_native_callable(caller, callable);
        let val = value::encode_native_callable_idx(idx);
        let _ = define_host_data_property_from_caller(caller, obj, name, val);
    }
}

fn attach_response_methods(caller: &mut Caller<'_, RuntimeState>, obj: i64, handle: u32) {
    let methods: &[(&str, ResponseMethodKind)] = &[
        ("text", ResponseMethodKind::Text),
        ("json", ResponseMethodKind::Json),
        ("arrayBuffer", ResponseMethodKind::ArrayBuffer),
        ("clone", ResponseMethodKind::Clone),
    ];

    for (name, kind) in methods {
        let callable = NativeCallable::ResponseMethod {
            handle,
            kind: *kind,
        };
        let idx = push_native_callable(caller, callable);
        let val = value::encode_native_callable_idx(idx);
        let _ = define_host_data_property_from_caller(caller, obj, name, val);
    }
}

fn attach_request_methods(caller: &mut Caller<'_, RuntimeState>, obj: i64, handle: u32) {
    let callable = NativeCallable::RequestMethod {
        handle,
        kind: RequestMethodKind::Clone,
    };
    let idx = push_native_callable(caller, callable);
    let val = value::encode_native_callable_idx(idx);
    let _ = define_host_data_property_from_caller(caller, obj, "clone", val);
}

mod fetch_core_impl;
pub(crate) use fetch_core_impl::*;
