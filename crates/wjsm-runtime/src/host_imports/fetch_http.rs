use crate::host_imports::fetch_core::*;
use crate::*;
use base64::Engine;
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
    match resolve_fetch_request_params(caller, input, init) {
        Ok(v) => v,
        Err(_) => (
            "GET".to_string(),
            String::new(),
            create_empty_headers(caller),
            None,
            RedirectMode::Follow,
            None,
        ),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn perform_http_fetch(
    caller: &mut Caller<'_, RuntimeState>,
    method: String,
    url: String,
    headers_handle: u32,
    body: Option<Vec<u8>>,
    redirect: RedirectMode,
    signal_handle: Option<u32>,
    resource_timing: Option<SharedFetchResourceTiming>,
) -> std::result::Result<i64, String> {
    if let Some(handle) = signal_handle
        && is_signal_aborted(caller, handle)
    {
        return Err("The operation was aborted".to_string());
    }
    let client = caller
        .data()
        .http_client_for_redirect(redirect)
        .map_err(|e| e.to_string())?;
    let mut req_builder = client.request(
        reqwest::Method::from_bytes(method.as_bytes()).map_err(|e| e.to_string())?,
        &url,
    );

    let header_pairs = {
        let table = caller
            .data()
            .headers_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        table
            .get(headers_handle as usize)
            .map(|h| h.pairs.clone())
            .unwrap_or_default()
    };
    for (name, value) in header_pairs {
        req_builder = req_builder.header(&name, &value);
    }
    if let Some(body_bytes) = body {
        req_builder = req_builder.body(body_bytes);
    }
    mark_fetch_request_start(caller.data(), &resource_timing);

    let response = req_builder
        .send()
        .await
        .map_err(|error| format!("fetch failed: {error}"))?;
    if let Some(handle) = signal_handle
        && is_signal_aborted(caller, handle)
    {
        return Err("The operation was aborted".to_string());
    }
    let status = response.status().as_u16();
    mark_fetch_response_start(caller.data(), &resource_timing, status);
    let status_text = response
        .status()
        .canonical_reason()
        .unwrap_or("")
        .to_string();
    let final_url = response.url().to_string();
    let redirected = final_url != url;

    let response_headers = create_empty_headers(caller);
    {
        let mut htable = caller
            .data()
            .headers_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = htable.get_mut(response_headers as usize) {
            for (name, value) in response.headers().iter() {
                if let Ok(v) = value.to_str() {
                    entry
                        .pairs
                        .push((name.as_str().to_ascii_lowercase(), v.to_string()));
                }
            }
        }
    }
    if method.eq_ignore_ascii_case("HEAD") || matches!(status, 204 | 205 | 304) {
        let response = create_response_object(
            caller,
            status,
            status_text,
            response_headers,
            final_url,
            Vec::new(),
            ResponseType::Basic,
            redirected,
            None,
        );
        set_response_resource_timing(caller, response, resource_timing.clone());
        complete_fetch_resource_timing(caller.data(), &resource_timing);
        return Ok(response);
    }

    let http_handle = {
        let mut table = caller
            .data()
            .http_response_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let handle = table.len() as u32;
        table.push(HttpResponseEntry {
            response: Some(response),
            pending_read_promise: None,
            pending_bytes: std::collections::VecDeque::new(),
            eof: false,
            error: None,
            resource_timing: resource_timing.clone(),
        });
        handle
    };
    let response = create_response_object_with_http_handle(
        caller,
        status,
        status_text,
        response_headers,
        final_url,
        ResponseType::Basic,
        redirected,
        http_handle,
    );
    set_response_resource_timing(caller, response, resource_timing);
    Ok(response)
}

fn is_signal_aborted(caller: &Caller<'_, RuntimeState>, handle: u32) -> bool {
    caller
        .data()
        .abort_signal_table
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(handle as usize)
        .map(|s| s.aborted)
        .unwrap_or(false)
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
        let mut htable = caller
            .data()
            .headers_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
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
