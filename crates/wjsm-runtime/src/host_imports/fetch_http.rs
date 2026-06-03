use crate::host_imports::fetch_core::*;
use crate::*;
use wasmtime::Caller;

// ── Input parsing (URL string or Request + optional init) ───────────────────

pub(crate) fn parse_fetch_input(
    caller: &mut Caller<'_, RuntimeState>,
    input: i64,
    init: i64,
) -> (String, String, u32, Option<Vec<u8>>, RedirectMode) {
    // fetch() 当前后端 ABI 只传入 input；Request 对象在此处按其公开 url 退化处理。
    // 构造器负责解析 RequestInit / ResponseInit / HeadersInit 的完整字段。

    let url = if value::is_string(input) {
        extract_string_from_value(caller, input)
    } else if value::is_object(input) {
        // 兼容 fetch(new Request(...))：从 Request-like 对象读取 .url。
        extract_string_property(caller, input, "url").unwrap_or_default()
    } else {
        String::new()
    };

    // Default GET, no body, follow redirects, empty headers (created on demand)
    let method = "GET".to_string();
    let headers_handle = create_empty_headers(caller);
    let body = None;
    let redirect = RedirectMode::Follow;

    let _ = init;

    (method, url, headers_handle, body, redirect)
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

    // HTTP/HTTPS and other schemes are not supported in this build.
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
        caller, 200, "OK".to_string(), resp_headers,
        url.to_string(), bytes, ResponseType::Basic, false, None,
    ))
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
