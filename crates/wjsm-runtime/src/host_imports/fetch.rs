use anyhow::Result;
use wasmtime::{Caller, Func, Linker, Store};
use crate::*;
// ── Public registration ─────────────────────────────────────────────────────

pub(crate) fn define_fetch(
    linker: &mut Linker<RuntimeState>,
    mut store: &mut Store<RuntimeState>,
) -> Result<()> {
    // Import 31 (updated ABI): fetch(i64, i64) → i64
    let f = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, input: i64, init: i64| -> i64 {
            // Create pending Promise immediately (return its handle)
            let promise = alloc_promise_from_caller(&mut caller, PromiseEntry::pending());
            let _promise_handle = value::decode_object_handle(promise) as usize;
            // Parse input (string URL or Request object)
            let (method, url, headers_handle, body_opt, _redirect) =
                parse_fetch_input(&mut caller, input, init);

            // Perform the actual work synchronously (design decision)
            let settle_result = perform_fetch_and_build_response(
                &mut caller,
                method,
                url,
                headers_handle,
                body_opt,
            );

            match settle_result {
                Ok(response_val) => {
                    // Fulfill the promise with the Response object
                    settle_promise(
                        caller.data_mut(),
                        promise,
                        PromiseSettlement::Fulfill(response_val),
                    );
                }
                Err(type_error_msg) => {
                    // Reject with a TypeError (network failure or bad input)
                    let err = alloc_type_error_from_caller(&mut caller, &type_error_msg);
                    settle_promise(
                        caller.data_mut(),
                        promise,
                        PromiseSettlement::Reject(err),
                    );
                }
            }

            promise
        },
    );
    linker.define(&mut store, "env", "fetch", f)?;
    Ok(())
}

// ── Input parsing (URL string or Request + optional init) ───────────────────

fn parse_fetch_input(
    caller: &mut Caller<'_, RuntimeState>,
    input: i64,
    init: i64,
) -> (String, String, u32, Option<Vec<u8>>, RedirectMode) {
    // Minimal MVP: support string URL (data: or http/https).
    // Request object and full init parsing (method, headers, body, redirect) are stubbed
    // to "GET" + default headers for the first cut; the side table entries exist for future.

    let url = if value::is_string(input) {
        extract_string_from_value(caller, input)
    } else if value::is_object(input) {
        // Treat as Request-like: read .url
        // For MVP we only support the simple string case in fixtures.
        extract_string_property(caller, input, "url").unwrap_or_default()
    } else {
        String::new()
    };

    // Default GET, no body, follow redirects, empty headers (created on demand)
    let method = "GET".to_string();
    let headers_handle = create_empty_headers(caller);
    let body = None;
    let redirect = RedirectMode::Follow;

    // If init is an object, we could parse method/headers/body here (future).
    let _ = init;

    (method, url, headers_handle, body, redirect)
}

fn extract_string_from_value(caller: &mut Caller<'_, RuntimeState>, val: i64) -> String {
    if value::is_runtime_string_handle(val) {
        let handle = value::decode_runtime_string_handle(val) as usize;
        caller
            .data()
            .runtime_strings
            .lock()
            .expect("runtime strings mutex")
            .get(handle)
            .cloned()
            .unwrap_or_default()
    } else if value::is_string(val) {
        read_string(caller, value::decode_string_ptr(val)).unwrap_or_default()
    } else {
        String::new()
    }
}

fn extract_string_property(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
    name: &str,
) -> Option<String> {
    let ptr = match resolve_handle(caller, obj) {
        Some(p) => p,
        None => return None,
    };
    let prop = match read_object_property_by_name(caller, ptr, name) {
        Some(v) => v,
        None => return None,
    };
    if value::is_string(prop) {
        Some(extract_string_from_value(caller, prop))
    } else {
        None
    }
}

// ── Core fetch execution + Response construction ────────────────────────────

fn perform_fetch_and_build_response(
    caller: &mut Caller<'_, RuntimeState>,
    _method: String,
    url: String,
    _headers_handle: u32,
    _body: Option<Vec<u8>>,
) -> std::result::Result<i64, String> {
    if url.is_empty() {
        return Err("Failed to parse URL from request.".to_string());
    }
    if url.starts_with("data:") {
        let body = url.split(',').nth(1).unwrap_or("").to_string();
        let decoded = urlencoding_decode(&body);
        let bytes = decoded.into_bytes();
        let resp_headers = create_empty_headers(caller);
        let resp_handle = create_response_object(
            caller,
            200,
            "OK".to_string(),
            resp_headers,
            url,
            bytes,
            ResponseType::Basic,
            false,
        );
        return Ok(resp_handle);
    }

    // HTTP/HTTPS and other schemes are not supported in this build.
    // Per documented limitation (AGENTS.md): fetch supports data: URLs only (synchronous string).
    // Real async fetch (with scheduler completion materialization for Response objects etc.)
    // is explicitly out of scope for the 2026-05-31 async scheduler implementation plan.
    // The scheduler + AsyncHostCompletion channel created by this plan provides the exact
    // materialization boundary future fetch will use.
    Err(format!("fetch for non-data: URL not implemented in this build: {}", url))
}

// ── Object construction helpers (Headers / Response / Request) ──────────────

fn create_empty_headers(caller: &mut Caller<'_, RuntimeState>) -> u32 {
    let mut table = caller
        .data()
        .headers_table
        .lock()
        .expect("headers_table mutex");
    let h = table.len() as u32;
    table.push(HeadersEntry { pairs: Vec::new(), guard: HeadersGuard::None });
    h
}

fn create_response_object(
    caller: &mut Caller<'_, RuntimeState>,
    status: u16,
    status_text: String,
    headers_handle: u32,
    url: String,
    body: Vec<u8>,
    response_type: ResponseType,
    redirected: bool,
) -> i64 {
    let mut table = caller
        .data()
        .fetch_response_table
        .lock()
        .expect("fetch_response_table mutex");
    let handle = table.len() as u32;
    table.push(FetchResponseEntry {
        status,
        status_text: status_text.clone(),
        headers_handle,
        url: url.clone(),
        body,
        response_type,
        redirected,
        body_used: false,
    });
    drop(table);

    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let obj = alloc_host_object(caller, &env, 12);

    // Hidden handle for native methods
    let handle_val = value::encode_f64(handle as f64);
    let _ = define_host_data_property_from_caller(caller, obj, "__response_handle__", handle_val);

    // Public properties (data descriptors)
    let ok_val = value::encode_bool(status >= 200 && status < 300);
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
    let _ = define_host_data_property_from_caller(caller, obj, "body", value::encode_null());
    let _ = define_host_data_property_from_caller(caller, obj, "bodyUsed", value::encode_bool(false));

    // headers object
    let headers_obj = create_headers_object_from_handle(caller, headers_handle);
    let _ = define_host_data_property_from_caller(caller, obj, "headers", headers_obj);

    // Attach method callables (text, json, arrayBuffer, clone)
    attach_response_methods(caller, obj, handle);

    obj
}

fn init_headers_object(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
    handle: u32,
) {
    let handle_val = value::encode_f64(handle as f64);
    let _ = define_host_data_property_from_caller(caller, obj, "__headers_handle__", handle_val);
    attach_headers_methods(caller, obj, handle);
}
fn create_headers_object_from_handle(
    caller: &mut Caller<'_, RuntimeState>,
    headers_handle: u32,
) -> i64 {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let obj = alloc_host_object(caller, &env, 8);
    init_headers_object(caller, obj, headers_handle);
    obj
}

fn create_request_object(
    caller: &mut Caller<'_, RuntimeState>,
    method: String,
    url: String,
    headers_handle: u32,
    body: Option<Vec<u8>>,
    redirect: RedirectMode,
) -> i64 {
    let mut table = caller
        .data()
        .fetch_request_table
        .lock()
        .expect("fetch_request_table mutex");
    let handle = table.len() as u32;
    table.push(FetchRequestEntry {
        method: method.clone(),
        url: url.clone(),
        headers_handle,
        body,
        redirect,
        body_used: false,
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
    let obj = alloc_host_object(caller, &env, 8);
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

    let headers_obj = create_headers_object_from_handle(caller, headers_handle);
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

fn push_native_callable(caller: &mut Caller<'_, RuntimeState>, callable: NativeCallable) -> u32 {
    let mut table = caller
        .data()
        .native_callables
        .lock()
        .expect("native_callables mutex");
    // Prefer free slot if available
    if let Some(free) = caller
        .data()
        .native_callable_free_slots
        .lock()
        .expect("free slots mutex")
        .pop()
    {
        table[free as usize] = callable;
        return free;
    }
    let idx = table.len() as u32;
    table.push(callable);
    idx
}
pub(crate) fn call_headers_method_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    kind: HeadersMethodKind,
    args: &[i64],
) -> Option<i64> {
    let handle = get_headers_handle_from_object(caller, this_val)?;
    match kind {
        HeadersMethodKind::Get => {
            let name = extract_string_from_value(caller, *args.first()?);
            let lower = name.to_lowercase();
            let table = caller.data().headers_table.lock().ok()?;
            let entry = table.get(handle as usize)?;
            let mut values: Vec<&String> = Vec::new();
            for (k, v) in &entry.pairs {
                if k == &lower {
                    values.push(v);
                }
            }
            if values.is_empty() {
                Some(value::encode_null())
            } else {
                let joined = values.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ");
                Some(store_runtime_string(caller, joined))
            }
        }
        HeadersMethodKind::Set => {
            if args.len() < 2 {
                return Some(value::encode_undefined());
            }
            let name = extract_string_from_value(caller, args[0]).to_lowercase();
            let value = extract_string_from_value(caller, args[1]);
            let mut table = caller.data().headers_table.lock().ok()?;
            if let Some(entry) = table.get_mut(handle as usize) {
                entry.pairs.retain(|(k, _)| k != &name);
                entry.pairs.push((name, value));
            }
            Some(value::encode_undefined())
        }
        HeadersMethodKind::Has => {
            let name = extract_string_from_value(caller, *args.first()?).to_lowercase();
            let table = caller.data().headers_table.lock().ok()?;
            let entry = table.get(handle as usize)?;
            let has = entry.pairs.iter().any(|(k, _)| k == &name);
            Some(value::encode_bool(has))
        }
        HeadersMethodKind::Delete => {
            let name = extract_string_from_value(caller, *args.first()?).to_lowercase();
            let mut table = caller.data().headers_table.lock().ok()?;
            if let Some(entry) = table.get_mut(handle as usize) {
                let before = entry.pairs.len();
                entry.pairs.retain(|(k, _)| k != &name);
                Some(value::encode_bool(entry.pairs.len() < before))
            } else {
                Some(value::encode_bool(false))
            }
        }
        HeadersMethodKind::Append => {
            if args.len() < 2 {
                return Some(value::encode_undefined());
            }
            let name = extract_string_from_value(caller, args[0]).to_lowercase();
            let value = extract_string_from_value(caller, args[1]);
            let mut table = caller.data().headers_table.lock().ok()?;
            if let Some(entry) = table.get_mut(handle as usize) {
                entry.pairs.push((name, value));
            }
            Some(value::encode_undefined())
        }
        HeadersMethodKind::Keys | HeadersMethodKind::Values | HeadersMethodKind::Entries => {
            let env = WasmEnv::from_caller(caller).expect("WasmEnv");
            let it = alloc_host_object(caller, &env, 2);
            Some(it)
        }
        HeadersMethodKind::ForEach => {
            Some(value::encode_undefined())
        }
    }
}
pub(crate) fn call_response_method_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    kind: ResponseMethodKind,
    _args: &[i64],
) -> Option<i64> {
    let handle = get_response_handle_from_object(caller, this_val)?;
    // Extract everything we need while the lock is held, then drop the guard immediately.
    let (status, status_text, headers_handle, url, body, response_type, redirected, was_body_used, is_consuming) = {
        let mut table = caller
            .data()
            .fetch_response_table
            .lock()
            .expect("fetch_response_table mutex");
        let entry = match table.get_mut(handle as usize) {
            Some(e) => e,
            None => return None,
        };
        let is_consuming = matches!(kind, ResponseMethodKind::Text | ResponseMethodKind::Json | ResponseMethodKind::ArrayBuffer);
        let was_body_used = entry.body_used;
        if is_consuming && was_body_used {
            return Some(value::encode_undefined()); // sentinel
        }
        if is_consuming {
            entry.body_used = true;
        }
        (
            entry.status,
            entry.status_text.clone(),
            entry.headers_handle,
            entry.url.clone(),
            entry.body.clone(),
            entry.response_type,
            entry.redirected,
            was_body_used,
            is_consuming,
        )
    };
    // Guard is now dropped. Safe to allocate and settle.
    if is_consuming && was_body_used {
        let p = alloc_promise_from_caller(caller, PromiseEntry::pending());
        let err = alloc_type_error_from_caller(caller, "body stream already read");
        settle_promise(caller.data_mut(), p, PromiseSettlement::Reject(err));
        return Some(p);
    }
    let result = match kind {
        ResponseMethodKind::Text => {
            let body_str = String::from_utf8_lossy(&body).to_string();
            let val = store_runtime_string(caller, body_str);
            let p = alloc_promise_from_caller(caller, PromiseEntry::pending());
            settle_promise(caller.data_mut(), p, PromiseSettlement::Fulfill(val));
            Some(p)
        }
        ResponseMethodKind::Json => {
            let body_str = String::from_utf8_lossy(&body).to_string();
            let val = store_runtime_string(caller, body_str);
            let p = alloc_promise_from_caller(caller, PromiseEntry::pending());
            settle_promise(caller.data_mut(), p, PromiseSettlement::Fulfill(val));
            Some(p)
        }
        ResponseMethodKind::ArrayBuffer => {
            let ab = create_arraybuffer_with_bytes(caller, &body);
            let p = alloc_promise_from_caller(caller, PromiseEntry::pending());
            settle_promise(caller.data_mut(), p, PromiseSettlement::Fulfill(ab));
            Some(p)
        }
        ResponseMethodKind::Clone => {
            // Avoid deadlock: read the header pairs under one lock scope, drop, then push under second
            let pairs = {
                let htable = caller.data().headers_table.lock().ok()?;
                let hentry = htable.get(headers_handle as usize)?;
                hentry.pairs.clone()
            };
            let new_headers = {
                let mut new_htable = caller.data().headers_table.lock().ok()?;
                let nh = new_htable.len() as u32;
                new_htable.push(HeadersEntry {
                    pairs,
                    guard: HeadersGuard::None,
                });
                nh
            };
            let new_resp = create_response_object(
                caller,
                status,
                status_text,
                new_headers,
                url,
                body,
                response_type,
                redirected,
            );
            Some(new_resp)
        }
    };
    if is_consuming {
        // Sync the public property after successful consume (side table already updated under lock)
        let _ = define_host_data_property_from_caller(caller, this_val, "bodyUsed", value::encode_bool(true));
    }
    result
}
pub(crate) fn call_request_method_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    kind: RequestMethodKind,
    _args: &[i64],
) -> Option<i64> {
    let handle = get_request_handle_from_object(caller, this_val)?;
    match kind {
        RequestMethodKind::Clone => {
            let (method, url, headers_handle, body, redirect) = {
                let table = caller.data().fetch_request_table.lock().ok()?;
                let entry = table.get(handle as usize)?;
                (
                    entry.method.clone(),
                    entry.url.clone(),
                    entry.headers_handle,
                    entry.body.clone(),
                    entry.redirect,
                )
            };
            // Fix deadlock for request clone too: read pairs first, then push
            let pairs = {
                let htable = caller.data().headers_table.lock().ok()?;
                let hentry = htable.get(headers_handle as usize)?;
                hentry.pairs.clone()
            };
            let new_headers = {
                let mut new_htable = caller.data().headers_table.lock().ok()?;
                let nh = new_htable.len() as u32;
                new_htable.push(HeadersEntry {
                    pairs,
                    guard: HeadersGuard::None,
                });
                nh
            };
            let req = create_request_object(
                caller,
                method,
                url,
                new_headers,
                body,
                redirect,
            );
            Some(req)
        }
    }
}
// ── Constructor implementations (for NativeCallable *Constructor via ConstructCall / generic new) ──
pub(crate) fn construct_headers(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    _args: &[i64],
) -> Option<i64> {
    let handle = create_empty_headers(caller);
    let obj = if value::is_object(this_val) {
        this_val
    } else {
        let env = WasmEnv::from_caller(caller).expect("WasmEnv");
        alloc_host_object(caller, &env, 8)
    };
    init_headers_object(caller, obj, handle);
    // Basic props if needed
    Some(obj)
}
pub(crate) fn construct_request(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    _args: &[i64],
) -> Option<i64> {
    // Minimal: create empty request with defaults; full init in later iteration
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let obj = if value::is_object(this_val) { this_val } else { alloc_host_object(caller, &env, 8) };
    // For full semantics we would parse args[0] as input, args[1] as init and fill side table + props.
    // For now, to unblock fixtures, create via helper (which pushes entry) and copy its props? Simplified: use create and ignore pre this for MVP ctor cutover.
    let h = create_empty_headers(caller);
    let req = create_request_object(
        caller,
        "GET".to_string(),
        String::new(),
        h,
        None,
        RedirectMode::Follow,
    );
    Some(req)
}
pub(crate) fn construct_response(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    _args: &[i64],
) -> Option<i64> {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let obj = if value::is_object(this_val) { this_val } else { alloc_host_object(caller, &env, 12) };
    // Similar, create via helper for now
    let h = create_empty_headers(caller);
    let resp = create_response_object(
        caller,
        200,
        "OK".to_string(),
        h,
        String::new(),
        Vec::new(),
        ResponseType::Basic,
        false,
    );
    Some(resp)
}
// ── Small helpers ───────────────────────────────────────────────────────────
// ── Small helpers ───────────────────────────────────────────────────────────

fn get_headers_handle_from_object(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
) -> Option<u32> {
    if !value::is_object(obj) {
        return None;
    }
    let ptr = resolve_handle(caller, obj)?;
    let prop = read_object_property_by_name(caller, ptr, "__headers_handle__")?;
    Some(value::decode_f64(prop) as u32)
}
fn get_response_handle_from_object(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
) -> Option<u32> {
    if !value::is_object(obj) {
        return None;
    }
    let ptr = resolve_handle(caller, obj)?;
    let prop = read_object_property_by_name(caller, ptr, "__response_handle__")?;
    Some(value::decode_f64(prop) as u32)
}
fn get_request_handle_from_object(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
) -> Option<u32> {
    if !value::is_object(obj) {
        return None;
    }
    let ptr = resolve_handle(caller, obj)?;
    let prop = read_object_property_by_name(caller, ptr, "__request_handle__")?;
    Some(value::decode_f64(prop) as u32)
}
fn create_arraybuffer_with_bytes(caller: &mut Caller<'_, RuntimeState>, bytes: &[u8]) -> i64 {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let ab = alloc_host_object(caller, &env, 1);
    let mut ab_table = caller
        .data()
        .arraybuffer_table
        .lock()
        .expect("arraybuffer_table mutex");
    let ab_handle = ab_table.len() as u32;
    ab_table.push(ArrayBufferEntry {
        data: bytes.to_vec(),
    });
    drop(ab_table);

    let handle_val = value::encode_f64(ab_handle as f64);
    let _ = define_host_data_property_from_caller(caller, ab, "__arraybuffer_handle__", handle_val);

    // byteLength
    let len_val = value::encode_f64(bytes.len() as f64);
    let _ = define_host_data_property_from_caller(caller, ab, "byteLength", len_val);

    // Mark heap type if needed (ArrayBuffer has its own detection via the handle)
    ab
}
fn alloc_type_error_from_caller(caller: &mut Caller<'_, RuntimeState>, message: &str) -> i64 {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let obj = alloc_host_object(caller, &env, 2);
    let name_val = store_runtime_string(caller, "TypeError".to_string());
    let msg_val = store_runtime_string(caller, message.to_string());
    let _ = define_host_data_property_from_caller(caller, obj, "name", name_val);
    let _ = define_host_data_property_from_caller(caller, obj, "message", msg_val);
    obj
}