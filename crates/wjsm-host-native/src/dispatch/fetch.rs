use std::collections::{HashMap, VecDeque};
use std::sync::mpsc::{self, Receiver, TryRecvError};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use wjsm_ir::{Builtin, value};
use wjsm_native_abi::NativeVmContext;

use super::{fail_dispatch, modules, promise, runtime};
use crate::NativeAgentState;
use crate::slot_table::SlotTable;

mod abort;
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
    AbortControllerAbort(u32),
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
    AbortController(u32),
    AbortSignal(u32),
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
    headers: SlotTable<headers::HeadersState>,
    requests: SlotTable<request::RequestState>,
    responses: SlotTable<response::ResponseState>,
    abort_signals: SlotTable<abort::AbortSignalState>,
    pending: Vec<PendingFetch>,
}

#[cfg(test)]
impl NativeFetchState {
    /// 活包装对象数（`objects` 登记表）。
    pub(crate) fn live_object_count(&self) -> usize {
        self.objects.len()
    }

    /// 各内部侧表的活槽总数。
    pub(crate) fn live_slot_count(&self) -> usize {
        self.headers.len() + self.requests.len() + self.responses.len() + self.abort_signals.len()
    }
}

/// 把 fetch 侧仍在飞的宿主 JS 值并入 GC 根队列：挂起网络请求的 promise
/// 只由 `pending` 表按句柄持有，回收前必须钉扎，否则句柄复用后请求完成
/// 会错误 settle 新 promise。包装对象本身不做永久根，生命周期交给
/// [`extend_gc_edges`] 的宿主边图与 [`sweep_retired`] 的死 owner 清扫。
pub(crate) fn extend_gc_roots(fetch: &NativeFetchState, roots: &mut VecDeque<i64>) {
    for pending in &fetch.pending {
        roots.push_back(value::encode_object_handle(pending.promise));
    }
}

/// 把 fetch 侧表持有的 JS 值按「owner 存活 ⇒ 内部引用存活」并入 GC 边图：
/// Request/Response 持 headers 包装对象与 body 流，AbortSignal 持 reason，
/// AbortController 持对应 signal 对象。owner 死则边不被追踪，内部值可回收。
pub(crate) fn extend_gc_edges(fetch: &NativeFetchState, mut add: impl FnMut(i64, i64)) {
    for (_, request) in fetch.requests.iter() {
        add(request.object, request.headers);
    }
    for (_, response) in fetch.responses.iter() {
        add(response.object, response.headers);
        if !value::is_null(response.body_object) {
            add(response.object, response.body_object);
        }
    }
    for (_, signal) in fetch.abort_signals.iter() {
        if !value::is_undefined(signal.reason) {
            add(signal.object, signal.reason);
        }
    }
    for (handle, kind) in &fetch.objects {
        if let FetchObjectKind::AbortController(signal) = kind
            && let Some(signal) = fetch.abort_signals.get(*signal)
        {
            add(value::encode_object_handle(*handle), signal.object);
        }
    }
}

/// 提取为独立值的 fetch 方法（如 `headers.append`）以槽位下标编码；只要
/// 方法值存活，就必须钉住对应包装对象，否则槽位被清扫复用后旧方法会
/// 操作新 owner。返回该可调用值应指向的包装对象。
pub(crate) fn callable_gc_target(fetch: &NativeFetchState, callable: FetchCallable) -> Option<i64> {
    match callable {
        FetchCallable::AbortControllerAbort(handle) => {
            fetch.abort_signals.get(handle).map(|signal| signal.object)
        }
        FetchCallable::Headers(handle, _) => {
            fetch.headers.get(handle).map(|headers| headers.object)
        }
        FetchCallable::Request(handle, _) => {
            fetch.requests.get(handle).map(|request| request.object)
        }
        FetchCallable::Response(handle, _) => {
            fetch.responses.get(handle).map(|response| response.object)
        }
    }
}

/// GC 完成后按 retired 句柄清扫 fetch 侧表：死 owner 的登记项与槽位一并
/// 释放，防止句柄复用后新对象继承旧品牌。Response 槽位可能仍被存活的
/// body 流（resource timing 完成路径）按下标引用，此时保留槽位、只摘登记。
pub(crate) fn sweep_retired(
    fetch: &mut NativeFetchState,
    streams: &super::streams::NativeStreamsState,
    retired: &[u32],
) {
    let NativeFetchState {
        objects,
        headers,
        requests,
        abort_signals,
        ..
    } = fetch;
    objects.retain(|handle, kind| {
        if retired.binary_search(handle).is_err() {
            return true;
        }
        match kind {
            FetchObjectKind::Headers(slot) => {
                headers.remove(*slot);
            }
            FetchObjectKind::Request(slot) => {
                requests.remove(*slot);
            }
            // AbortController 与 signal 共享槽位，槽位归 signal 所有。
            FetchObjectKind::AbortController(_) => {}
            FetchObjectKind::AbortSignal(slot) => {
                abort_signals.remove(*slot);
            }
            // Response 槽位延后到下面统一判定（body 流可能仍引用）。
            FetchObjectKind::Response(_) => {}
        }
        false
    });
    let dead_responses: Vec<u32> = fetch
        .responses
        .iter()
        .filter(|(slot, response)| {
            // 登记须精确匹配「本槽位的 Response」：包装对象死后堆句柄可能被
            // 新 fetch 对象复用，仅凭句柄存在会把死槽误判为活。
            let registered = fetch
                .objects
                .get(&value::decode_handle(response.object))
                .is_some_and(
                    |kind| matches!(kind, FetchObjectKind::Response(owner) if owner == slot),
                );
            !registered && !streams.body_stream_references_response(*slot)
        })
        .map(|(slot, _)| slot)
        .collect();
    for slot in dead_responses {
        fetch.responses.remove(slot);
    }
}

pub(super) fn dispatch_fetch(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    builtin: Builtin,
    args: &[i64],
) -> Option<i64> {
    Some(match builtin {
        Builtin::Fetch => fetch(ctx, state, args),
        Builtin::AbortControllerConstructor => abort::construct(ctx, state, args),
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
        FetchObjectKind::AbortController(handle) => abort::controller_property(state, handle, key),
        FetchObjectKind::AbortSignal(handle) => abort::signal_property(state, handle, key),
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
        FetchCallable::AbortControllerAbort(handle) => abort::abort(ctx, state, handle, args),
        FetchCallable::Headers(handle, method) => headers::call(ctx, state, handle, method, args),
        FetchCallable::Request(handle, method) => request::call(ctx, state, handle, method),
        FetchCallable::Response(handle, method) => response::call(ctx, state, handle, method),
    }
}

pub(crate) fn mark_response_used(state: &mut NativeAgentState, handle: u32) {
    if let Some(response) = state.fetch.responses.get_mut(handle) {
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
            FetchObjectKind::Request(handle) => state.fetch.requests.get(*handle),
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
            FetchObjectKind::Headers(handle) => state.fetch.headers.get(*handle),
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
    // 测试替身 transport：`WJSM_TEST_FAKE_FETCH` 非空时按 URL path 返回确定性响应，
    // 不进行任何真实网络 I/O。生产环境未设置该变量时不会进入此分支。
    if std::env::var_os("WJSM_TEST_FAKE_FETCH").is_some() {
        return fake_transport_response(&request);
    }
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

/// 确定性的测试替身响应：路径 `empty` → 204 无 body；`error` → body 错误；
/// 其余路径 → 200 + `"hello"`。
fn fake_transport_response(request: &NetworkRequest) -> Result<NetworkResponse, String> {
    let path = request
        .url
        .rsplit('/')
        .next()
        .unwrap_or(&request.url)
        .split('?')
        .next()
        .unwrap_or("");
    match path {
        "empty" => Ok(NetworkResponse {
            status: 204,
            status_text: "No Content".into(),
            headers: Vec::new(),
            body: Ok(String::new()),
        }),
        "error" => Ok(NetworkResponse {
            status: 200,
            status_text: "OK".into(),
            headers: vec![("content-length".into(), "20".into())],
            body: Err("fetch body failed: response body shorter than content-length".into()),
        }),
        _ => Ok(NetworkResponse {
            status: 200,
            status_text: "OK".into(),
            headers: vec![("content-length".into(), "5".into())],
            body: Ok("hello".into()),
        }),
    }
}

pub(super) fn register_object(state: &mut NativeAgentState, object: i64, kind: FetchObjectKind) {
    state
        .fetch
        .objects
        .insert(value::decode_handle(object), kind);
}

pub(super) fn to_string(state: &NativeAgentState, encoded: i64) -> String {
    state
        .string_to_utf8_lossy(encoded)
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
