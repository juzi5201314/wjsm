use crate::host_imports::fetch_core::*;
use crate::*;
use wasmtime::Caller;

// ── Input parsing (URL string or Request + optional init) ───────────────────

pub(crate) fn parse_fetch_input(
    caller: &mut Caller<'_, RuntimeState>,
    input: i64,
    init: i64,
) -> (
    String,
    String,
    u32,
    Option<Vec<u8>>,
    RedirectMode,
    Option<u32>,
) {
    let url = if value::is_string(input) {
        extract_string_from_value(caller, input)
    } else if value::is_object(input) {
        extract_string_property(caller, input, "url").unwrap_or_default()
    } else {
        String::new()
    };

    let method = "GET".to_string();
    let headers_handle = create_empty_headers(caller);
    let body = None;
    let redirect = RedirectMode::Follow;
    let signal_handle = None;

    let _ = init;

    (method, url, headers_handle, body, redirect, signal_handle)
}

// ── Core fetch execution + Response construction ────────────────────────────

pub(crate) fn perform_fetch_and_build_response(
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
        return perform_data_url_fetch(caller, &url);
    }

    Err(format!(
        "fetch for non-data: URL not implemented in this build: {}",
        url
    ))
}

pub(crate) fn perform_data_url_fetch(
    caller: &mut Caller<'_, RuntimeState>,
    url: &str,
) -> std::result::Result<i64, String> {
    let body = url.split(',').nth(1).unwrap_or("").to_string();
    let decoded = urlencoding_decode(&body);
    let bytes = decoded.into_bytes();
    let resp_headers = create_empty_headers(caller);
    Ok(create_response_object(
        caller,
        200,
        "OK".to_string(),
        resp_headers,
        url.to_string(),
        bytes,
        ResponseType::Basic,
        false,
        None,
    ))
}

pub(crate) async fn perform_http_fetch(
    caller: &mut Caller<'_, RuntimeState>,
    method: String,
    url: String,
    headers_handle: u32,
    body: Option<Vec<u8>>,
    redirect: RedirectMode,
    signal_handle: Option<u32>,
) -> std::result::Result<i64, String> {
    // 1. 检查 abort
    if let Some(handle) = signal_handle {
        if is_signal_aborted(caller.data(), handle) {
            return Err("The operation was aborted".to_string());
        }
    }

    // 2. 构建 reqwest 请求
    let redirect_policy = match redirect {
        RedirectMode::Follow => reqwest::redirect::Policy::limited(20),
        RedirectMode::Error => reqwest::redirect::Policy::none(),
        RedirectMode::Manual => reqwest::redirect::Policy::limited(0),
    };

    let client = reqwest::Client::builder()
        .redirect(redirect_policy)
        .build()
        .map_err(|e| format!("fetch client error: {}", e))?;

    let http_method: reqwest::Method = method
        .parse()
        .map_err(|e| format!("invalid method: {}", e))?;

    let mut builder = client.request(http_method, &url);

    // 3. 添加 headers（限制锁作用域）
    {
        let headers = caller.data().headers_table.lock().expect("headers mutex");
        if let Some(entry) = headers.get(headers_handle as usize) {
            for (name, value) in &entry.pairs {
                builder = builder.header(name.as_str(), value.as_str());
            }
        }
    }

    // 4. 添加 body
    if let Some(body_bytes) = body {
        builder = builder.body(body_bytes);
    }

    // 5. 发送请求（await — wasmtime 自动 yield）
    let response = builder
        .send()
        .await
        .map_err(|e| format!("fetch failed: {}", e))?;

    // 6. 检查 abort（请求完成后）
    if let Some(handle) = signal_handle {
        if is_signal_aborted(caller.data(), handle) {
            return Err("The operation was aborted".to_string());
        }
    }

    // 7. 提取响应信息
    let status = response.status().as_u16();
    let status_text = response
        .status()
        .canonical_reason()
        .unwrap_or("")
        .to_string();
    let resp_url = response.url().to_string();
    let redirected = response.url().as_str() != url;

    // 8. 提取响应 headers
    let resp_headers = create_empty_headers(caller);
    {
        let mut htable = caller.data().headers_table.lock().expect("headers mutex");
        if let Some(entry) = htable.get_mut(resp_headers as usize) {
            for (key, value) in response.headers() {
                entry.pairs.push((
                    key.as_str().to_ascii_lowercase(),
                    value.to_str().unwrap_or("").to_string(),
                ));
            }
        }
    }

    // 9. 存储 reqwest Response（用于后续流式读取）
    let http_handle = {
        let mut table = caller
            .data()
            .http_response_table
            .lock()
            .expect("http_response mutex");
        let handle = table.len() as u32;
        table.push(HttpResponseEntry {
            response: Some(response),
            pending_read_promise: None,
            pending_bytes: std::collections::VecDeque::new(),
            eof: false,
            error: None,
        });
        handle
    };

    // 10. 构造 Response 对象（body 暂为 null，通过 ReadableStream 懒加载）
    let resp_obj = create_response_object_with_http_handle(
        caller,
        status,
        status_text,
        resp_headers,
        resp_url,
        ResponseType::Basic,
        redirected,
        http_handle,
    );

    Ok(resp_obj)
}

pub(crate) fn is_signal_aborted(state: &RuntimeState, handle: u32) -> bool {
    state
        .abort_signal_table
        .lock()
        .expect("abort_signal mutex")
        .get(handle as usize)
        .map(|e| e.aborted)
        .unwrap_or(false)
}

pub(crate) fn urlencoding_decode(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut chars = input.chars();
    while let Some(ch) = chars.next() {
        if ch == '%' {
            let high = chars.next().and_then(|c| c.to_digit(16));
            let low = chars.next().and_then(|c| c.to_digit(16));
            if let (Some(h), Some(l)) = (high, low) {
                result.push(char::from_u32((h * 16 + l) as u32).unwrap_or('?'));
            } else {
                result.push('%');
            }
        } else if ch == '+' {
            result.push(' ');
        } else {
            result.push(ch);
        }
    }
    result
}
