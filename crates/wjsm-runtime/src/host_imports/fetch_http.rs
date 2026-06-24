use base64::Engine;
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

pub(crate) fn perform_data_url_fetch(
    caller: &mut Caller<'_, RuntimeState>,
    url: &str,
) -> std::result::Result<i64, String> {
    let (mediatype, is_base64, data_part) = parse_data_url(url)?;
    let bytes = if is_base64 {
        base64::engine::general_purpose::STANDARD
            .decode(data_part.as_bytes())
            .map_err(|e| format!("invalid base64 in data URL: {}", e))?
    } else {
        percent_decode_to_bytes(&data_part)
    };
    let resp_headers = create_empty_headers(caller);
    {
        let mut htable = caller.data().headers_table.lock().expect("headers mutex");
        if let Some(entry) = htable.get_mut(resp_headers as usize) {
            entry.pairs.push(("content-type".to_string(), mediatype));
        }
    }
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
    if let Some(handle) = signal_handle
        && is_signal_aborted(caller.data(), handle)
    {
        return Err("The operation was aborted".to_string());
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
    if let Some(handle) = signal_handle
        && is_signal_aborted(caller.data(), handle)
    {
        return Err("The operation was aborted".to_string());
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

const DATA_URL_DEFAULT_MEDIATYPE: &str = "text/plain;charset=US-ASCII";

/// RFC 2397: `data:[<mediatype>][;base64],<data>`
fn parse_data_url(url: &str) -> std::result::Result<(String, bool, String), String> {
    let rest = url
        .strip_prefix("data:")
        .ok_or_else(|| "invalid data URL".to_string())?;
    let comma = rest
        .find(',')
        .ok_or_else(|| "invalid data URL: missing ','".to_string())?;
    let meta = &rest[..comma];
    let data_part = rest[comma + 1..].to_string();
    let meta_lower = meta.to_ascii_lowercase();
    let is_base64 = meta_lower.contains(";base64");
    let mediatype_raw = if is_base64 {
        meta_lower
            .split_once(";base64")
            .map(|(before, _)| before)
            .unwrap_or("")
    } else {
        meta
    };
    let mediatype = if mediatype_raw.is_empty() {
        DATA_URL_DEFAULT_MEDIATYPE.to_string()
    } else {
        mediatype_raw.to_string()
    };
    Ok((mediatype, is_base64, data_part))
}

fn percent_decode_to_bytes(input: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(input.len());
    let mut chars = input.chars();
    while let Some(ch) = chars.next() {
        if ch == '%' {
            let high = chars.next().and_then(|c| c.to_digit(16));
            let low = chars.next().and_then(|c| c.to_digit(16));
            if let (Some(h), Some(l)) = (high, low) {
                bytes.push((h * 16 + l) as u8);
            } else {
                bytes.push(b'%');
            }
        } else if ch == '+' {
            bytes.push(b' ');
        } else {
            let mut buf = [0u8; 4];
            let encoded = ch.encode_utf8(&mut buf);
            bytes.extend_from_slice(encoded.as_bytes());
        }
    }
    bytes
}
