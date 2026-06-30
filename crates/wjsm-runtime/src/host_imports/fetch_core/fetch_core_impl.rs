use super::*;
pub(crate) fn push_native_callable(
    caller: &mut Caller<'_, RuntimeState>,
    callable: NativeCallable,
) -> u32 {
    let mut table = caller
        .data()
        .native_callables
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    // Prefer free slot if available
    if let Some(free) = caller
        .data()
        .native_callable_free_slots
        .lock()
        .unwrap_or_else(|e| e.into_inner())
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
                let joined = values
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
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
        HeadersMethodKind::Keys => {
            let mut iters = caller.data().iterators.lock().ok()?;
            let iter_handle = iters.len() as u32;
            iters.push(IteratorState::HeadersKeyIter {
                headers_handle: handle,
                index: 0,
            });
            Some(value::encode_handle(value::TAG_ITERATOR, iter_handle))
        }
        HeadersMethodKind::Values => {
            let mut iters = caller.data().iterators.lock().ok()?;
            let iter_handle = iters.len() as u32;
            iters.push(IteratorState::HeadersValueIter {
                headers_handle: handle,
                index: 0,
            });
            Some(value::encode_handle(value::TAG_ITERATOR, iter_handle))
        }
        HeadersMethodKind::Entries => {
            let mut iters = caller.data().iterators.lock().ok()?;
            let iter_handle = iters.len() as u32;
            iters.push(IteratorState::HeadersEntryIter {
                headers_handle: handle,
                index: 0,
            });
            Some(value::encode_handle(value::TAG_ITERATOR, iter_handle))
        }
        HeadersMethodKind::ForEach => {
            let Some(cb) = args.first().copied() else {
                return Some(value::encode_undefined());
            };
            if !value::is_callable(cb) {
                return Some(value::encode_undefined());
            }
            let this_arg = args.get(1).copied().unwrap_or_else(value::encode_undefined);
            let pairs: Vec<(String, String)> = {
                let table = caller.data().headers_table.lock().ok()?;
                let Some(entry) = table.get(handle as usize) else {
                    return Some(value::encode_undefined());
                };
                entry.pairs.clone()
            };
            let env = WasmEnv::from_caller(caller).expect("WasmEnv");
            let rt = tokio::runtime::Handle::current();
            for (name, val) in pairs {
                let name_js = store_runtime_string(caller, name);
                let val_js = store_runtime_string(caller, val);
                if rt
                    .block_on(invoke_resolved_callback_async_option(
                        caller,
                        &env,
                        cb,
                        this_arg,
                        &[val_js, name_js, this_val],
                    ))
                    .is_none()
                {
                    return Some(value::encode_undefined());
                }
            }
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
    let (
        status,
        status_text,
        headers_handle,
        url,
        body,
        response_type,
        redirected,
        was_body_used,
        is_consuming,
        http_response_handle,
        stream_handle,
    ) = {
        let mut table = caller
            .data()
            .fetch_response_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let entry = table.get_mut(handle as usize)?;
        let is_consuming = matches!(
            kind,
            ResponseMethodKind::Text | ResponseMethodKind::Json | ResponseMethodKind::ArrayBuffer
        );
        let was_body_used = entry.body_used;
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
            entry.http_response_handle,
            entry.stream_handle,
        )
    };
    if is_consuming && was_body_used {
        let p = alloc_promise_from_caller(caller, PromiseEntry::pending());
        let err = alloc_type_error_from_caller(caller, "body stream already read");
        settle_promise(caller.data_mut(), p, PromiseSettlement::Reject(err));
        return Some(p);
    }
    // WHATWG Fetch §1.3：body 的 ReadableStream 已 locked（getReader 已调用）时，
    // text()/json()/arrayBuffer() 必须 reject with TypeError。
    if is_consuming && let Some(sh) = stream_handle {
        let locked = caller
            .data()
            .readable_stream_table
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(sh as usize)
            .is_some_and(|e| e.locked);
        if locked {
            let p = alloc_promise_from_caller(caller, PromiseEntry::pending());
            let err = alloc_type_error_from_caller(caller, "body stream is locked");
            settle_promise(caller.data_mut(), p, PromiseSettlement::Reject(err));
            return Some(p);
        }
    }
    // HTTP Response — 异步 body 消费
    if is_consuming && let Some(http_handle) = http_response_handle {
        let promise = alloc_promise_from_caller(caller, PromiseEntry::pending());
        if super::super::streams_fetch_body::consume_fetch_body_to_bytes(
            caller,
            http_handle,
            promise,
            kind,
        ) {
            let _ = set_host_data_property_from_caller(
                caller,
                this_val,
                "bodyUsed",
                value::encode_bool(true),
            );
            return Some(promise);
        }
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
            let text = store_runtime_string(caller, body_str);
            let parsed = json_parse_to_wasm(caller, text, value::encode_undefined());
            let p = alloc_promise_from_caller(caller, PromiseEntry::pending());
            if value::is_exception(parsed) {
                let reason = exception_reason(caller, parsed);
                settle_promise(caller.data_mut(), p, PromiseSettlement::Reject(reason));
            } else {
                settle_promise(caller.data_mut(), p, PromiseSettlement::Fulfill(parsed));
            }
            Some(p)
        }
        ResponseMethodKind::ArrayBuffer => {
            let ab = create_arraybuffer_with_bytes(caller, &body);
            let p = alloc_promise_from_caller(caller, PromiseEntry::pending());
            settle_promise(caller.data_mut(), p, PromiseSettlement::Fulfill(ab));
            Some(p)
        }
        ResponseMethodKind::Clone => {
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
                None,
            );
            Some(new_resp)
        }
    };
    if is_consuming {
        let _ = set_host_data_property_from_caller(
            caller,
            this_val,
            "bodyUsed",
            value::encode_bool(true),
        );
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
            let req =
                create_request_object(caller, method, url, new_headers, body, redirect, None, None);
            if let Some(url_val) = object_property(caller, this_val, "url") {
                let _ = set_host_data_property_from_caller(caller, req, "url", url_val);
            }
            Some(req)
        }
    }
}

// ── Constructor implementations ─────────────────────────────────────────────

pub(crate) fn construct_headers(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    args: &[i64],
) -> Option<i64> {
    let handle = create_empty_headers(caller);
    if let Some(init) = args.first().copied()
        && let Err(exception) = fill_headers_from_init(caller, handle, init)
    {
        return Some(exception);
    }
    let obj = if value::is_object(this_val) {
        this_val
    } else {
        let env = WasmEnv::from_caller(caller).expect("WasmEnv");
        alloc_host_object(caller, &env, 8)
    };
    init_headers_object(caller, obj, handle);
    Some(obj)
}

/// `fetch(input, init)` 与 `new Request(input, init)` 共用的请求参数解析。
pub(crate) fn resolve_fetch_request_params(
    caller: &mut Caller<'_, RuntimeState>,
    input: i64,
    init: i64,
) -> std::result::Result<
    (
        String,
        String,
        u32,
        Option<Vec<u8>>,
        RedirectMode,
        Option<u32>,
    ),
    i64,
> {
    let (mut method, url, mut headers, mut body, mut redirect, mut signal_handle) =
        if let Some(request_handle) = get_request_handle_from_object(caller, input) {
            let copied = {
                let table = caller
                    .data()
                    .fetch_request_table
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                table.get(request_handle as usize).map(|entry| {
                    (
                        entry.method.clone(),
                        entry.url.clone(),
                        entry.headers_handle,
                        entry.body.clone(),
                        entry.redirect,
                        entry.signal_handle,
                    )
                })
            };
            let Some((method, url, headers_handle, body, redirect, signal_handle)) = copied else {
                return Err(type_error_exception(caller, "invalid Request object"));
            };
            (
                method,
                url,
                clone_headers_handle(caller, headers_handle),
                body,
                redirect,
                signal_handle,
            )
        } else if value::is_string(input) {
            let url = extract_string_from_value(caller, input);
            (
                "GET".to_string(),
                url,
                create_empty_headers(caller),
                None,
                RedirectMode::Follow,
                None,
            )
        } else if value::is_object(input) {
            let url = extract_string_property(caller, input, "url").unwrap_or_default();
            (
                "GET".to_string(),
                url,
                create_empty_headers(caller),
                None,
                RedirectMode::Follow,
                None,
            )
        } else {
            return Err(type_error_exception(
                caller,
                "Failed to parse URL from request.",
            ));
        };

    if url_has_credentials(&url) {
        return Err(type_error_exception(
            caller,
            "Request URL contains credentials",
        ));
    }

    if value::is_object(init) {
        match string_property(caller, init, "method") {
            Ok(Some(init_method)) => {
                let upper = init_method.to_ascii_uppercase();
                if !valid_method(&upper) || forbidden_method(&upper) {
                    return Err(type_error_exception(caller, "invalid Request method"));
                }
                method = upper;
            }
            Ok(None) => {}
            Err(exception) => return Err(exception),
        }
        if let Some(init_headers) = object_property(caller, init, "headers")
            && !value::is_undefined(init_headers)
        {
            match create_headers_from_init(caller, init_headers) {
                Ok(handle) => headers = handle,
                Err(exception) => return Err(exception),
            }
        }
        if let Some(init_body) = object_property(caller, init, "body") {
            match body_bytes_from_value(caller, init_body) {
                Ok(parsed_body) => body = parsed_body,
                Err(exception) => return Err(exception),
            }
        }
        match string_property(caller, init, "redirect") {
            Ok(Some(init_redirect)) => match parse_redirect_mode(caller, &init_redirect) {
                Ok(mode) => redirect = mode,
                Err(exception) => return Err(exception),
            },
            Ok(None) => {}
            Err(exception) => return Err(exception),
        }
        if let Some(init_signal) = object_property(caller, init, "signal")
            && !value::is_undefined(init_signal)
            && let Some(handle) = number_property(caller, init_signal, "__abort_signal_handle__")
        {
            signal_handle = Some(handle as u32);
        }
    }

    if body.is_some() && matches!(method.as_str(), "GET" | "HEAD") {
        return Err(type_error_exception(
            caller,
            "Request with GET/HEAD method cannot have body",
        ));
    }

    Ok((method, url, headers, body, redirect, signal_handle))
}

pub(crate) fn construct_request(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    args: &[i64],
) -> Option<i64> {
    let input = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    if value::is_undefined(input) {
        return Some(type_error_exception(caller, "Request input is required"));
    }

    let init = args.get(1).copied().unwrap_or_else(value::encode_undefined);
    let (method, url, headers, body, redirect, signal_handle) =
        match resolve_fetch_request_params(caller, input, init) {
            Ok(v) => v,
            Err(exception) => return Some(exception),
        };

    let mut cache = "default".to_string();
    let mut credentials = "same-origin".to_string();
    let mut integrity = String::new();
    let mut keepalive = false;
    let copied_request = get_request_handle_from_object(caller, input).is_some();
    if copied_request {
        if let Ok(Some(copy_cache)) = string_property(caller, input, "cache") {
            cache = copy_cache;
        }
        if let Ok(Some(copy_credentials)) = string_property(caller, input, "credentials") {
            credentials = copy_credentials;
        }
        if let Ok(Some(copy_integrity)) = string_property(caller, input, "integrity") {
            integrity = copy_integrity;
        }
        if let Some(copy_keepalive) = bool_property(caller, input, "keepalive") {
            keepalive = copy_keepalive;
        }
    }

    if value::is_object(init) {
        match string_property(caller, init, "cache") {
            Ok(Some(init_cache)) => {
                if !valid_request_cache(&init_cache) {
                    return Some(type_error_exception(caller, "invalid Request cache mode"));
                }
                cache = init_cache;
            }
            Ok(None) => {}
            Err(exception) => return Some(exception),
        }
        match string_property(caller, init, "credentials") {
            Ok(Some(init_credentials)) => {
                if !valid_request_credentials(&init_credentials) {
                    return Some(type_error_exception(
                        caller,
                        "invalid Request credentials mode",
                    ));
                }
                credentials = init_credentials;
            }
            Ok(None) => {}
            Err(exception) => return Some(exception),
        }
        match string_property(caller, init, "integrity") {
            Ok(Some(init_integrity)) => integrity = init_integrity,
            Ok(None) => {}
            Err(exception) => return Some(exception),
        }
        if let Some(init_keepalive) = bool_property(caller, init, "keepalive") {
            keepalive = init_keepalive;
        }
    }

    let req = create_request_object(
        caller,
        method,
        url,
        headers,
        body,
        redirect,
        Some(this_val),
        signal_handle,
    );
    define_request_init_properties(caller, req, &cache, &credentials, &integrity, keepalive);
    if copied_request && let Some(url_val) = object_property(caller, input, "url") {
        let _ = set_host_data_property_from_caller(caller, req, "url", url_val);
    }
    Some(req)
}

pub(crate) fn construct_response(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    args: &[i64],
) -> Option<i64> {
    let body_arg = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let body = match body_bytes_from_value(caller, body_arg) {
        Ok(body) => body,
        Err(exception) => return Some(exception),
    };
    let mut status = 200u16;
    let mut status_text = String::new();
    let mut headers = create_empty_headers(caller);

    if let Some(init) = args.get(1).copied()
        && value::is_object(init)
    {
        if let Some(init_status) = number_property(caller, init, "status") {
            if !init_status.is_finite()
                || init_status.fract() != 0.0
                || !(200.0..=599.0).contains(&init_status)
            {
                return Some(type_error_exception(
                    caller,
                    "Response status must be 200-599",
                ));
            }
            status = init_status as u16;
        }
        match string_property(caller, init, "statusText") {
            Ok(Some(init_status_text)) => {
                if !valid_status_text(&init_status_text) {
                    return Some(type_error_exception(caller, "invalid Response statusText"));
                }
                status_text = init_status_text;
            }
            Ok(None) => {}
            Err(exception) => return Some(exception),
        }
        if let Some(init_headers) = object_property(caller, init, "headers")
            && !value::is_undefined(init_headers)
        {
            match create_headers_from_init(caller, init_headers) {
                Ok(handle) => headers = handle,
                Err(exception) => return Some(exception),
            }
        }
    }

    if body.is_some() && null_body_status(status) {
        return Some(type_error_exception(
            caller,
            "Response with null-body status cannot have body",
        ));
    }

    let resp = create_response_object(
        caller,
        status,
        status_text,
        headers,
        String::new(),
        body.unwrap_or_default(),
        ResponseType::Basic,
        false,
        Some(this_val),
    );
    Some(resp)
}

// ── Small helpers ───────────────────────────────────────────────────────────

pub(crate) fn get_headers_handle_from_object(
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

pub(crate) fn get_response_handle_from_object(
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

pub(crate) fn get_request_handle_from_object(
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

pub(crate) fn create_arraybuffer_with_bytes(
    caller: &mut Caller<'_, RuntimeState>,
    bytes: &[u8],
) -> i64 {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let ab = alloc_host_object(caller, &env, 1);
    let mut ab_table = caller
        .data()
        .arraybuffer_table
        .lock()
        .unwrap_or_else(|e| e.into_inner());
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

    ab
}

pub(crate) fn alloc_type_error_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    message: &str,
) -> i64 {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let obj = alloc_host_object(caller, &env, 2);
    let name_val = store_runtime_string(caller, "TypeError".to_string());
    let msg_val = store_runtime_string(caller, message.to_string());
    let _ = define_host_data_property_from_caller(caller, obj, "name", name_val);
    let _ = define_host_data_property_from_caller(caller, obj, "message", msg_val);
    obj
}

fn type_error_exception(caller: &mut Caller<'_, RuntimeState>, message: &str) -> i64 {
    let error_obj = alloc_type_error_from_caller(caller, message);
    let mut errors = caller
        .data()
        .error_table
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let idx = errors.len() as u32;
    errors.push(ErrorEntry {
        name: "TypeError".to_string(),
        message: message.to_string(),
        value: error_obj,
    });
    value::encode_exception(idx)
}

fn js_string_from_value(
    caller: &mut Caller<'_, RuntimeState>,
    raw: i64,
) -> std::result::Result<String, i64> {
    if value::is_symbol(raw) {
        return Err(type_error_exception(
            caller,
            "Cannot convert a Symbol value to a string",
        ));
    }
    if value::is_string(raw) {
        return Ok(get_string_value(caller, raw));
    }
    Ok(render_value(caller, raw)
        .unwrap_or_default()
        .trim_matches('"')
        .to_string())
}

pub(crate) fn object_property(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
    name: &str,
) -> Option<i64> {
    let ptr = resolve_handle(caller, obj)?;
    read_object_property_by_name(caller, ptr, name)
}

fn string_property(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
    name: &str,
) -> std::result::Result<Option<String>, i64> {
    let Some(raw) = object_property(caller, obj, name) else {
        return Ok(None);
    };
    if value::is_undefined(raw) {
        return Ok(None);
    }
    Ok(Some(js_string_from_value(caller, raw)?))
}

fn number_property(caller: &mut Caller<'_, RuntimeState>, obj: i64, name: &str) -> Option<f64> {
    let raw = object_property(caller, obj, name)?;
    value::is_f64(raw).then(|| value::decode_f64(raw))
}

fn bool_property(caller: &mut Caller<'_, RuntimeState>, obj: i64, name: &str) -> Option<bool> {
    let raw = object_property(caller, obj, name)?;
    Some(!value::is_falsy(raw))
}

// ── Header validation helpers ───────────────────────────────────────────────

fn valid_header_name(name: &str) -> bool {
    !name.is_empty()
        && name.as_bytes().iter().all(|byte| {
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

fn valid_header_value(value_raw: &str) -> bool {
    !value_raw
        .bytes()
        .any(|byte| matches!(byte, b'\r' | b'\n' | 0))
}

fn append_header_pair(
    caller: &mut Caller<'_, RuntimeState>,
    handle: u32,
    name: String,
    value_raw: String,
) -> std::result::Result<(), i64> {
    if !valid_header_name(&name) {
        return Err(type_error_exception(caller, "invalid header name"));
    }
    if !valid_header_value(&value_raw) {
        return Err(type_error_exception(caller, "invalid header value"));
    }
    let mut table = caller
        .data()
        .headers_table
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(entry) = table.get_mut(handle as usize) {
        entry.pairs.push((name.to_ascii_lowercase(), value_raw));
    }
    Ok(())
}

fn set_header_pair(
    caller: &mut Caller<'_, RuntimeState>,
    handle: u32,
    name: String,
    value_raw: String,
) -> std::result::Result<(), i64> {
    if !valid_header_name(&name) {
        return Err(type_error_exception(caller, "invalid header name"));
    }
    if !valid_header_value(&value_raw) {
        return Err(type_error_exception(caller, "invalid header value"));
    }
    let lower = name.to_ascii_lowercase();
    let mut table = caller
        .data()
        .headers_table
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(entry) = table.get_mut(handle as usize) {
        entry.pairs.retain(|(key, _)| key != &lower);
        entry.pairs.push((lower, value_raw));
    }
    Ok(())
}

fn clone_headers_handle(caller: &mut Caller<'_, RuntimeState>, source: u32) -> u32 {
    let pairs = {
        let table = caller
            .data()
            .headers_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        table
            .get(source as usize)
            .map(|entry| entry.pairs.clone())
            .unwrap_or_default()
    };
    let mut table = caller
        .data()
        .headers_table
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let handle = table.len() as u32;
    table.push(HeadersEntry {
        pairs,
        guard: HeadersGuard::None,
    });
    handle
}

fn copy_headers_into(caller: &mut Caller<'_, RuntimeState>, target: u32, source: u32) {
    let pairs = {
        let table = caller
            .data()
            .headers_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        table
            .get(source as usize)
            .map(|entry| entry.pairs.clone())
            .unwrap_or_default()
    };
    let mut table = caller
        .data()
        .headers_table
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(entry) = table.get_mut(target as usize) {
        entry.pairs.extend(pairs);
    }
}

fn fill_headers_from_init(
    caller: &mut Caller<'_, RuntimeState>,
    handle: u32,
    init: i64,
) -> std::result::Result<(), i64> {
    if value::is_undefined(init) || value::is_null(init) {
        return Ok(());
    }
    if let Some(source) = get_headers_handle_from_object(caller, init) {
        copy_headers_into(caller, handle, source);
        return Ok(());
    }
    if value::is_array(init) {
        let Some(arr_ptr) = resolve_array_ptr(caller, init) else {
            return Err(type_error_exception(caller, "invalid Headers init"));
        };
        let len = read_array_length(caller, arr_ptr).unwrap_or(0);
        for i in 0..len {
            let entry = read_array_elem(caller, arr_ptr, i).unwrap_or_else(value::encode_undefined);
            if !value::is_array(entry) {
                return Err(type_error_exception(
                    caller,
                    "Headers sequence entry must be an array",
                ));
            }
            let Some(entry_ptr) = resolve_array_ptr(caller, entry) else {
                return Err(type_error_exception(
                    caller,
                    "Headers sequence entry must be an array",
                ));
            };
            if read_array_length(caller, entry_ptr).unwrap_or(0) != 2 {
                return Err(type_error_exception(
                    caller,
                    "Headers sequence entry must have length 2",
                ));
            }
            let name_raw =
                read_array_elem(caller, entry_ptr, 0).unwrap_or_else(value::encode_undefined);
            let value_raw =
                read_array_elem(caller, entry_ptr, 1).unwrap_or_else(value::encode_undefined);
            let name = js_string_from_value(caller, name_raw)?;
            let value_str = js_string_from_value(caller, value_raw)?;
            append_header_pair(caller, handle, name, value_str)?;
        }
        return Ok(());
    }
    if value::is_object(init) {
        for key in enumerate_object_keys(caller, init) {
            let raw = object_property(caller, init, &key).unwrap_or_else(value::encode_undefined);
            let value_str = js_string_from_value(caller, raw)?;
            set_header_pair(caller, handle, key, value_str)?;
        }
    }
    Ok(())
}

fn create_headers_from_init(
    caller: &mut Caller<'_, RuntimeState>,
    init: i64,
) -> std::result::Result<u32, i64> {
    let handle = create_empty_headers(caller);
    fill_headers_from_init(caller, handle, init)?;
    Ok(handle)
}

pub(crate) fn body_bytes_from_value(
    caller: &mut Caller<'_, RuntimeState>,
    raw: i64,
) -> std::result::Result<Option<Vec<u8>>, i64> {
    if value::is_undefined(raw) || value::is_null(raw) {
        return Ok(None);
    }
    Ok(Some(js_string_from_value(caller, raw)?.into_bytes()))
}

fn valid_method(method: &str) -> bool {
    valid_header_name(method)
}

fn forbidden_method(method: &str) -> bool {
    matches!(method, "CONNECT" | "TRACE" | "TRACK")
}

fn url_has_credentials(url: &str) -> bool {
    let Some(scheme_end) = url.find("://") else {
        return false;
    };
    let rest = &url[scheme_end + 3..];
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    rest[..authority_end].contains('@')
}

fn parse_redirect_mode(
    caller: &mut Caller<'_, RuntimeState>,
    raw: &str,
) -> std::result::Result<RedirectMode, i64> {
    match raw {
        "follow" => Ok(RedirectMode::Follow),
        "error" => Ok(RedirectMode::Error),
        "manual" => Ok(RedirectMode::Manual),
        _ => Err(type_error_exception(
            caller,
            "invalid Request redirect mode",
        )),
    }
}

fn valid_request_cache(raw: &str) -> bool {
    matches!(
        raw,
        "default" | "no-store" | "reload" | "no-cache" | "force-cache" | "only-if-cached"
    )
}

fn valid_request_credentials(raw: &str) -> bool {
    matches!(raw, "omit" | "same-origin" | "include")
}

pub(super) fn define_request_string_property(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
    name: &str,
    value_raw: &str,
) {
    let val = store_runtime_string(caller, value_raw.to_string());
    let _ = set_host_data_property_from_caller(caller, obj, name, val);
}

fn define_request_init_properties(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
    cache: &str,
    credentials: &str,
    integrity: &str,
    keepalive: bool,
) {
    define_request_string_property(caller, obj, "cache", cache);
    define_request_string_property(caller, obj, "credentials", credentials);
    define_request_string_property(caller, obj, "integrity", integrity);
    let _ =
        set_host_data_property_from_caller(caller, obj, "keepalive", value::encode_bool(keepalive));
}

fn null_body_status(status: u16) -> bool {
    matches!(status, 101..=103 | 204 | 205 | 304)
}

fn valid_status_text(status_text: &str) -> bool {
    !status_text
        .bytes()
        .any(|byte| matches!(byte, b'\r' | b'\n'))
}

pub(crate) fn extract_string_from_value(caller: &mut Caller<'_, RuntimeState>, val: i64) -> String {
    if value::is_runtime_string_handle(val) {
        let handle = value::decode_runtime_string_handle(val) as usize;
        caller
            .data()
            .runtime_strings
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(handle)
            .cloned()
            .unwrap_or_default()
    } else if value::is_string(val) {
        read_string(caller, value::decode_string_ptr(val)).unwrap_or_default()
    } else {
        String::new()
    }
}

pub(crate) fn extract_string_property(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
    name: &str,
) -> Option<String> {
    let ptr = resolve_handle(caller, obj)?;
    let prop = read_object_property_by_name(caller, ptr, name)?;
    if value::is_string(prop) {
        Some(extract_string_from_value(caller, prop))
    } else {
        None
    }
}

pub(crate) fn construct_abort_controller(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    _args: &[i64],
) -> Option<i64> {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let signal_handle = {
        let mut table = caller
            .data()
            .abort_signal_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let handle = table.len() as u32;
        table.push(AbortSignalEntry {
            aborted: false,
            reason: None,
        });
        handle
    };
    let signal_obj = alloc_host_object(caller, &env, 2);
    let handle_val = value::encode_f64(signal_handle as f64);
    let _ = define_host_data_property_from_caller(
        caller,
        signal_obj,
        "__abort_signal_handle__",
        handle_val,
    );
    let _ = define_host_data_property_from_caller(
        caller,
        signal_obj,
        "aborted",
        value::encode_bool(false),
    );
    let _ = define_host_data_property_from_caller(caller, this_val, "signal", signal_obj);
    let _ = define_host_data_property_from_caller(
        caller,
        this_val,
        "__abort_signal_handle__",
        handle_val,
    );
    Some(this_val)
}

pub(crate) fn abort_controller_abort(
    caller: &mut Caller<'_, RuntimeState>,
    signal_handle: u32,
    args: &[i64],
) -> Option<i64> {
    let mut table = caller
        .data()
        .abort_signal_table
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(entry) = table.get_mut(signal_handle as usize) {
        entry.aborted = true;
        entry.reason = args.first().copied();
    }
    Some(value::encode_undefined())
}
