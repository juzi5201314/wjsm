use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, TryRecvError};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use wjsm_ir::{Builtin, value};
use wjsm_native_abi::NativeVmContext;

use super::{fail_dispatch, modules, promise, runtime};
use crate::NativeAgentState;

mod headers;
mod request;
mod response;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum HeadersMethod {
    Append,
    Delete,
    Get,
    Has,
    Set,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum RequestMethod {
    Clone,
    Text,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum ResponseMethod {
    ArrayBuffer,
    Clone,
    Text,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum FetchCallable {
    Headers(u32, HeadersMethod),
    Request(u32, RequestMethod),
    Response(u32, ResponseMethod),
}

#[derive(Clone, Copy)]
pub(crate) enum FetchProperty {
    Callable(FetchCallable),
    Value(i64),
}

#[derive(Clone, Copy)]
pub(super) enum FetchObjectKind {
    Headers(u32),
    Request(u32),
    Response(u32),
}

struct PendingFetch {
    promise: u32,
    receiver: Receiver<Result<NetworkResponse, String>>,
    url: String,
    suppress_resource_timing: bool,
}

struct NetworkRequest {
    url: String,
    method: String,
    headers: Vec<(String, String)>,
    body: Option<String>,
    redirect: String,
}

pub(super) struct NetworkResponse {
    status: u16,
    status_text: String,
    headers: Vec<(String, String)>,
    body: Result<String, String>,
}

#[derive(Default)]
pub(crate) struct NativeFetchState {
    objects: HashMap<u32, FetchObjectKind>,
    headers: Vec<headers::HeadersState>,
    requests: Vec<request::RequestState>,
    responses: Vec<response::ResponseState>,
    pending: Vec<PendingFetch>,
}

pub(super) fn dispatch_fetch(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    builtin: Builtin,
    args: &[i64],
) -> Option<i64> {
    Some(match builtin {
        Builtin::Fetch => fetch(ctx, state, args),
        Builtin::HeadersConstructor => headers::construct(ctx, state, args),
        Builtin::RequestConstructor => request::construct(ctx, state, args),
        Builtin::ResponseConstructor => response::construct(ctx, state, args),
        _ => return None,
    })
}

pub(crate) fn property(
    state: &mut NativeAgentState,
    receiver: i64,
    key: &str,
) -> Option<FetchProperty> {
    match *state.fetch.objects.get(&value::decode_handle(receiver))? {
        FetchObjectKind::Headers(handle) => headers::property(state, handle, key),
        FetchObjectKind::Request(handle) => request::property(state, handle, key),
        FetchObjectKind::Response(handle) => response::property(state, handle, key),
    }
}

pub(crate) fn call(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    callable: FetchCallable,
    args: &[i64],
) -> i64 {
    match callable {
        FetchCallable::Headers(handle, method) => headers::call(ctx, state, handle, method, args),
        FetchCallable::Request(handle, method) => request::call(ctx, state, handle, method),
        FetchCallable::Response(handle, method) => response::call(ctx, state, handle, method),
    }
}

pub(crate) fn mark_response_used(state: &mut NativeAgentState, handle: u32) {
    if let Some(response) = state.fetch.responses.get_mut(handle as usize) {
        response.used = true;
    }
}

pub(crate) fn complete_response_body(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    handle: u32,
) {
    response::complete_timing(ctx, state, handle);
}

pub(crate) fn has_pending(state: &NativeAgentState) -> bool {
    !state.fetch.pending.is_empty()
}

pub(crate) fn poll(ctx: &mut NativeVmContext, state: &mut NativeAgentState) -> i64 {
    let completed =
        state
            .fetch
            .pending
            .iter()
            .enumerate()
            .find_map(|(index, pending)| match pending.receiver.try_recv() {
                Ok(result) => Some((index, result)),
                Err(TryRecvError::Disconnected) => {
                    Some((index, Err("fetch worker disconnected".into())))
                }
                Err(TryRecvError::Empty) => None,
            });
    let Some((index, result)) = completed else {
        std::thread::yield_now();
        return value::encode_undefined();
    };
    let pending = state.fetch.pending.swap_remove(index);
    match result {
        Ok(network) => {
            let response = response::create_network_response(
                ctx,
                state,
                pending.url,
                pending.suppress_resource_timing,
                network,
            );
            if value::is_exception(response) {
                let reason = state.exception_value(response).unwrap_or(response);
                promise::settle_promise(state, pending.promise, reason, true);
            } else {
                promise::settle_promise(state, pending.promise, response, false);
            }
        }
        Err(message) => {
            let reason = modules::named_error_object(state, "TypeError", message)
                .unwrap_or_else(|| fail_dispatch(ctx));
            promise::settle_promise(state, pending.promise, reason, true);
        }
    }
    value::encode_undefined()
}

fn fetch(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let request_object = request::construct(ctx, state, args);
    if value::is_exception(request_object) {
        let reason = state
            .exception_value(request_object)
            .unwrap_or(request_object);
        return promise::rejected_promise(ctx, state, reason);
    }
    let Some(request) = state
        .fetch
        .objects
        .get(&value::decode_handle(request_object))
        .and_then(|kind| match kind {
            FetchObjectKind::Request(handle) => state.fetch.requests.get(*handle as usize),
            _ => None,
        })
        .cloned()
    else {
        return fail_dispatch(ctx);
    };
    if let Some(body) = decode_data_url(&request.url) {
        let response = response::create_fetch_response(
            ctx,
            state,
            request.url,
            body,
            request.suppress_resource_timing,
        );
        if value::is_exception(response) {
            return response;
        }
        return promise::resolved_promise(ctx, state, response);
    }
    if !request.url.starts_with("http://") && !request.url.starts_with("https://") {
        let reason = modules::named_error_object(state, "TypeError", "fetch failed".into())
            .unwrap_or_else(|| fail_dispatch(ctx));
        return promise::rejected_promise(ctx, state, reason);
    }
    let Some(headers) = state
        .fetch
        .objects
        .get(&value::decode_handle(request.headers))
        .and_then(|kind| match kind {
            FetchObjectKind::Headers(handle) => state.fetch.headers.get(*handle as usize),
            _ => None,
        })
        .map(|headers| {
            headers
                .entries
                .iter()
                .map(|(name, values)| (name.clone(), values.join(", ")))
                .collect()
        })
    else {
        return fail_dispatch(ctx);
    };
    let Some(promise) = promise::new_promise(ctx, state) else {
        return fail_dispatch(ctx);
    };
    let promise_handle = value::decode_handle(promise);
    let network_request = NetworkRequest {
        url: request.url.clone(),
        method: request.method,
        headers,
        body: request.body,
        redirect: request.redirect,
    };
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(send_request(network_request));
    });
    state.fetch.pending.push(PendingFetch {
        promise: promise_handle,
        receiver,
        url: request.url,
        suppress_resource_timing: request.suppress_resource_timing,
    });
    promise
}

fn send_request(request: NetworkRequest) -> Result<NetworkResponse, String> {
    let redirect = match request.redirect.as_str() {
        "follow" => reqwest::redirect::Policy::limited(20),
        _ => reqwest::redirect::Policy::none(),
    };
    let client = reqwest::blocking::Client::builder()
        .redirect(redirect)
        .build()
        .map_err(|error| format!("fetch failed: {error}"))?;
    let method = reqwest::Method::from_bytes(request.method.as_bytes())
        .map_err(|error| format!("fetch failed: {error}"))?;
    let mut builder = client.request(method, &request.url);
    for (name, value) in request.headers {
        builder = builder.header(name, value);
    }
    if let Some(body) = request.body {
        builder = builder.body(body);
    }
    let response = builder
        .send()
        .map_err(|error| format!("fetch failed: {error}"))?;
    if request.redirect == "error" && response.status().is_redirection() {
        return Err("fetch failed: redirect encountered".into());
    }
    let status = response.status();
    let status_text = status.canonical_reason().unwrap_or_default().to_owned();
    let headers = response
        .headers()
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_owned(),
                String::from_utf8_lossy(value.as_bytes()).into_owned(),
            )
        })
        .collect();
    let body = if matches!(status.as_u16(), 101 | 204 | 205 | 304) {
        Ok(String::new())
    } else {
        response
            .bytes()
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .map_err(|error| format!("fetch body failed: {error}"))
    };
    Ok(NetworkResponse {
        status: status.as_u16(),
        status_text,
        headers,
        body,
    })
}

pub(super) fn register_object(state: &mut NativeAgentState, object: i64, kind: FetchObjectKind) {
    state
        .fetch
        .objects
        .insert(value::decode_handle(object), kind);
}

pub(super) fn to_string(state: &NativeAgentState, encoded: i64) -> String {
    state
        .string(encoded)
        .map(wjsm_host::RuntimeString::to_utf8_lossy)
        .unwrap_or_else(|| runtime::render_value(state, encoded))
}

pub(super) fn read_optional_property(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    object: i64,
    name: &str,
) -> Result<Option<i64>, i64> {
    let Some(key) = state.intern_text(name.to_owned(), value::TAG_STRING) else {
        return Err(fail_dispatch(ctx));
    };
    let stored = runtime::get_property(ctx, state, object, key).map_err(|()| fail_dispatch(ctx))?;
    if value::is_exception(stored) {
        Err(stored)
    } else if value::is_undefined(stored) {
        Ok(None)
    } else {
        Ok(Some(stored))
    }
}

pub(super) fn type_error(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    message: &str,
) -> i64 {
    runtime::type_error(ctx, state, message)
}

fn decode_data_url(url: &str) -> Option<String> {
    let data = url.strip_prefix("data:")?;
    let (metadata, payload) = data.split_once(',')?;
    let bytes = if metadata.ends_with(";base64") {
        STANDARD.decode(payload).ok()?
    } else {
        percent_decode(payload)?
    };
    String::from_utf8(bytes).ok()
}

fn percent_decode(input: &str) -> Option<Vec<u8>> {
    let bytes = input.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = hex(*bytes.get(index + 1)?)?;
            let low = hex(*bytes.get(index + 2)?)?;
            output.push((high << 4) | low);
            index += 3;
        } else {
            output.push(bytes[index]);
            index += 1;
        }
    }
    Some(output)
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}
