//! Fetch HTTP I/O bridge：仅持有 reqwest 请求/响应与 runtime 侧表接合。
//!
//! Request/Response/Headers、data URL 与 Resource Timing 语义位于 `wjsm-builtins::fetch`。

use crate::exec_context_impl::WasmExecContext;
use crate::{
    HttpResponseEntry, RedirectMode, ResponseType, RuntimeState, SharedFetchResourceTiming,
};
use wasmtime::Caller;



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
    {
        let mut context = WasmExecContext::new(caller);
        wjsm_builtins::fetch::resource_timing::mark_request_start(
            &mut context,
            &resource_timing,
        );
    }

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
    {
        let mut context = WasmExecContext::new(caller);
        wjsm_builtins::fetch::resource_timing::mark_response_start(
            &mut context,
            &resource_timing,
            status,
        );
    }
    let status_text = response
        .status()
        .canonical_reason()
        .unwrap_or("")
        .to_string();
    let final_url = response.url().to_string();
    let redirected = final_url != url;

    let response_headers = {
        let mut context = WasmExecContext::new(caller);
        wjsm_builtins::fetch::objects::create_empty_headers(&mut context)
    };
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
        let mut context = WasmExecContext::new(caller);
        let response = wjsm_builtins::fetch::create_response(
            &mut context,
            wjsm_builtins::fetch::ResponseSpec {
                status,
                status_text,
                headers_handle: response_headers,
                url: final_url,
                body: Vec::new(),
                response_type: ResponseType::Basic,
                redirected,
                target: None,
                http_handle: None,
            },
        );
        wjsm_builtins::fetch::set_response_resource_timing(
            &mut context,
            response,
            resource_timing.clone(),
        );
        wjsm_builtins::fetch::resource_timing::complete(&mut context, &resource_timing);
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
    let mut context = WasmExecContext::new(caller);
    let response = wjsm_builtins::fetch::create_response(
        &mut context,
        wjsm_builtins::fetch::ResponseSpec {
            status,
            status_text,
            headers_handle: response_headers,
            url: final_url,
            body: Vec::new(),
            response_type: ResponseType::Basic,
            redirected,
            target: None,
            http_handle: Some(http_handle),
        },
    );
    wjsm_builtins::fetch::set_response_resource_timing(
        &mut context,
        response,
        resource_timing,
    );
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

