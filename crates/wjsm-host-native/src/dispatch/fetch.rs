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

/// `Request.prototype` 的访问器（已实现子集，Web IDL readonly attribute）。
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum RequestGetter {
    Body,
    BodyUsed,
    Cache,
    Credentials,
    Headers,
    Integrity,
    Keepalive,
    Method,
    Redirect,
    Url,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum ResponseMethod {
    ArrayBuffer,
    Clone,
    Text,
}

/// `Response.prototype` 的访问器（已实现子集，Web IDL readonly attribute）。
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum ResponseGetter {
    Body,
    BodyUsed,
    Headers,
    Ok,
    Status,
    StatusText,
}

/// fetch 家族的方法/访问器可调用值。不携带实例句柄：方法安装在共享
/// prototype 上，调用时按实际 `this` 经 `objects` 登记表解析品牌，
/// 借用（`a.get.call(b)`）操作 `b`，品牌不符抛 TypeError（Web IDL
/// brand check）。
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum FetchCallable {
    AbortControllerAbort,
    AbortControllerSignalGetter,
    Headers(HeadersMethod),
    Request(RequestMethod),
    RequestGetter(RequestGetter),
    Response(ResponseMethod),
    ResponseGetter(ResponseGetter),
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
        // abort 监听器回调与 onabort 处理器由 signal 对象持有。
        super::events::extend_target_edges(&signal.events, signal.object, &mut add);
    }
    for (handle, kind) in &fetch.objects {
        if let FetchObjectKind::AbortController(signal) = kind
            && let Some(signal) = fetch.abort_signals.get(*signal)
        {
            add(value::encode_object_handle(*handle), signal.object);
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

/// 按实际 `this` 解析 AbortSignal 品牌（`dispatch::events` 的 EventTarget
/// 分派与 AbortSignal 访问器共用）。
pub(super) fn abort_signal_of(state: &NativeAgentState, this_value: i64) -> Option<u32> {
    match this_object_kind(state, this_value)? {
        FetchObjectKind::AbortSignal(handle) => Some(handle),
        _ => None,
    }
}

/// AbortSignal 的监听器登记表（内嵌在 fetch 侧表）。
pub(super) fn abort_signal_events_mut(
    state: &mut NativeAgentState,
    handle: u32,
) -> Option<&mut super::events::EventTargetData> {
    state
        .fetch
        .abort_signals
        .get_mut(handle)
        .map(|signal| &mut signal.events)
}

/// AbortSignal 的 JS 包装对象。
pub(super) fn abort_signal_object(state: &NativeAgentState, handle: u32) -> Option<i64> {
    state
        .fetch
        .abort_signals
        .get(handle)
        .map(|signal| signal.object)
}

/// AbortSignal 的 `(aborted, reason)` 快照。
pub(super) fn abort_signal_flags(state: &NativeAgentState, handle: u32) -> Option<(bool, i64)> {
    state
        .fetch
        .abort_signals
        .get(handle)
        .map(|signal| (signal.aborted, signal.reason))
}

/// 按实际 `this` 解析品牌：非对象或未登记为 fetch 包装对象时返回 None。
fn this_object_kind(state: &NativeAgentState, this_value: i64) -> Option<FetchObjectKind> {
    if !value::is_js_object(this_value) {
        return None;
    }
    state
        .fetch
        .objects
        .get(&value::decode_handle(this_value))
        .copied()
}

/// undici 系（Headers/Request/Response）的品牌失败：同步抛
/// `TypeError: Illegal invocation`（与 Node 逐字节一致）。
fn illegal_invocation(ctx: &mut NativeVmContext, state: &mut NativeAgentState) -> i64 {
    type_error(ctx, state, "Illegal invocation")
}

/// body 消费方法（返回 promise）的品牌失败：按 Web IDL 以 rejected
/// promise 交付 TypeError，而非同步抛出（与 Node 一致）。
fn illegal_invocation_rejection(ctx: &mut NativeVmContext, state: &mut NativeAgentState) -> i64 {
    let Some(reason) =
        modules::named_error_object(state, "TypeError", "Illegal invocation".into())
    else {
        return fail_dispatch(ctx);
    };
    promise::rejected_promise(ctx, state, reason)
}

/// AbortController 的品牌失败：`Value of "this" must be of type X` 形态
///（Node 因私有字段实现抛 V8 artifact 消息，此处采用其 ERR_INVALID_THIS
/// 惯用格式，name 同为 TypeError）。
fn invalid_this(ctx: &mut NativeVmContext, state: &mut NativeAgentState, interface: &str) -> i64 {
    type_error(
        ctx,
        state,
        &format!("Value of \"this\" must be of type {interface}"),
    )
}

pub(crate) fn call(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    callable: FetchCallable,
    this_value: i64,
    args: &[i64],
) -> i64 {
    let kind = this_object_kind(state, this_value);
    match callable {
        FetchCallable::AbortControllerAbort => match kind {
            Some(FetchObjectKind::AbortController(handle)) => abort::abort(ctx, state, handle, args),
            _ => invalid_this(ctx, state, "AbortController"),
        },
        FetchCallable::AbortControllerSignalGetter => match kind {
            Some(FetchObjectKind::AbortController(handle)) => state
                .fetch
                .abort_signals
                .get(handle)
                .map(|signal| signal.object)
                .unwrap_or_else(|| fail_dispatch(ctx)),
            _ => invalid_this(ctx, state, "AbortController"),
        },
        FetchCallable::Headers(method) => match kind {
            Some(FetchObjectKind::Headers(handle)) => headers::call(ctx, state, handle, method, args),
            _ => illegal_invocation(ctx, state),
        },
        FetchCallable::Request(method) => match kind {
            Some(FetchObjectKind::Request(handle)) => request::call(ctx, state, handle, method),
            _ => match method {
                RequestMethod::Clone => illegal_invocation(ctx, state),
                RequestMethod::Text => illegal_invocation_rejection(ctx, state),
            },
        },
        FetchCallable::RequestGetter(getter) => match kind {
            Some(FetchObjectKind::Request(handle)) => request::getter(ctx, state, handle, getter),
            _ => illegal_invocation(ctx, state),
        },
        FetchCallable::Response(method) => match kind {
            Some(FetchObjectKind::Response(handle)) => response::call(ctx, state, handle, method),
            _ => match method {
                ResponseMethod::Clone => illegal_invocation(ctx, state),
                ResponseMethod::ArrayBuffer | ResponseMethod::Text => {
                    illegal_invocation_rejection(ctx, state)
                }
            },
        },
        FetchCallable::ResponseGetter(getter) => match kind {
            Some(FetchObjectKind::Response(handle)) => response::getter(ctx, state, handle, getter),
            _ => illegal_invocation(ctx, state),
        },
    }
}

/// 把已实现的方法/访问器安装为对应 `prototype` 对象的自有属性（Web IDL
/// 描述符：方法 {writable, enumerable, configurable}，访问器
/// {enumerable, configurable}），次序与 Node 一致。
pub(crate) fn install_prototype_members(
    state: &mut NativeAgentState,
    prototype: i64,
    builtin: Builtin,
) -> Option<()> {
    match builtin {
        Builtin::HeadersConstructor => {
            for (name, method) in [
                ("append", HeadersMethod::Append),
                ("delete", HeadersMethod::Delete),
                ("get", HeadersMethod::Get),
                ("has", HeadersMethod::Has),
                ("set", HeadersMethod::Set),
            ] {
                state.install_web_prototype_method(
                    prototype,
                    name,
                    crate::NativeCallableKind::Fetch(FetchCallable::Headers(method)),
                )?;
            }
        }
        Builtin::RequestConstructor => {
            for (name, getter) in [
                ("method", RequestGetter::Method),
                ("url", RequestGetter::Url),
                ("headers", RequestGetter::Headers),
                ("credentials", RequestGetter::Credentials),
                ("cache", RequestGetter::Cache),
                ("redirect", RequestGetter::Redirect),
                ("integrity", RequestGetter::Integrity),
                ("keepalive", RequestGetter::Keepalive),
                ("body", RequestGetter::Body),
                ("bodyUsed", RequestGetter::BodyUsed),
            ] {
                state.install_web_prototype_getter(
                    prototype,
                    name,
                    crate::NativeCallableKind::Fetch(FetchCallable::RequestGetter(getter)),
                )?;
            }
            for (name, method) in [
                ("clone", RequestMethod::Clone),
                ("text", RequestMethod::Text),
            ] {
                state.install_web_prototype_method(
                    prototype,
                    name,
                    crate::NativeCallableKind::Fetch(FetchCallable::Request(method)),
                )?;
            }
        }
        Builtin::ResponseConstructor => {
            for (name, getter) in [
                ("status", ResponseGetter::Status),
                ("ok", ResponseGetter::Ok),
                ("statusText", ResponseGetter::StatusText),
                ("headers", ResponseGetter::Headers),
                ("body", ResponseGetter::Body),
                ("bodyUsed", ResponseGetter::BodyUsed),
            ] {
                state.install_web_prototype_getter(
                    prototype,
                    name,
                    crate::NativeCallableKind::Fetch(FetchCallable::ResponseGetter(getter)),
                )?;
            }
            for (name, method) in [
                ("clone", ResponseMethod::Clone),
                ("arrayBuffer", ResponseMethod::ArrayBuffer),
                ("text", ResponseMethod::Text),
            ] {
                state.install_web_prototype_method(
                    prototype,
                    name,
                    crate::NativeCallableKind::Fetch(FetchCallable::Response(method)),
                )?;
            }
        }
        Builtin::AbortControllerConstructor => {
            state.install_web_prototype_getter(
                prototype,
                "signal",
                crate::NativeCallableKind::Fetch(FetchCallable::AbortControllerSignalGetter),
            )?;
            state.install_web_prototype_method(
                prototype,
                "abort",
                crate::NativeCallableKind::Fetch(FetchCallable::AbortControllerAbort),
            )?;
        }
        _ => {}
    }
    Some(())
}

/// fetch 家族可调用值的 JS 可见 `(name, length)`（与 Node 实测一致；
/// 访问器 name 为 `get <attr>` 形态）。
pub(crate) fn metadata(callable: FetchCallable) -> Option<(&'static str, u32)> {
    Some(match callable {
        FetchCallable::AbortControllerAbort => ("abort", 0),
        FetchCallable::AbortControllerSignalGetter => ("get signal", 0),
        FetchCallable::Headers(HeadersMethod::Append) => ("append", 2),
        FetchCallable::Headers(HeadersMethod::Delete) => ("delete", 1),
        FetchCallable::Headers(HeadersMethod::Get) => ("get", 1),
        FetchCallable::Headers(HeadersMethod::Has) => ("has", 1),
        FetchCallable::Headers(HeadersMethod::Set) => ("set", 2),
        FetchCallable::Request(RequestMethod::Clone) => ("clone", 0),
        FetchCallable::Request(RequestMethod::Text) => ("text", 0),
        FetchCallable::RequestGetter(RequestGetter::Body) => ("get body", 0),
        FetchCallable::RequestGetter(RequestGetter::BodyUsed) => ("get bodyUsed", 0),
        FetchCallable::RequestGetter(RequestGetter::Cache) => ("get cache", 0),
        FetchCallable::RequestGetter(RequestGetter::Credentials) => ("get credentials", 0),
        FetchCallable::RequestGetter(RequestGetter::Headers) => ("get headers", 0),
        FetchCallable::RequestGetter(RequestGetter::Integrity) => ("get integrity", 0),
        FetchCallable::RequestGetter(RequestGetter::Keepalive) => ("get keepalive", 0),
        FetchCallable::RequestGetter(RequestGetter::Method) => ("get method", 0),
        FetchCallable::RequestGetter(RequestGetter::Redirect) => ("get redirect", 0),
        FetchCallable::RequestGetter(RequestGetter::Url) => ("get url", 0),
        FetchCallable::Response(ResponseMethod::ArrayBuffer) => ("arrayBuffer", 0),
        FetchCallable::Response(ResponseMethod::Clone) => ("clone", 0),
        FetchCallable::Response(ResponseMethod::Text) => ("text", 0),
        FetchCallable::ResponseGetter(ResponseGetter::Body) => ("get body", 0),
        FetchCallable::ResponseGetter(ResponseGetter::BodyUsed) => ("get bodyUsed", 0),
        FetchCallable::ResponseGetter(ResponseGetter::Headers) => ("get headers", 0),
        FetchCallable::ResponseGetter(ResponseGetter::Ok) => ("get ok", 0),
        FetchCallable::ResponseGetter(ResponseGetter::Status) => ("get status", 0),
        FetchCallable::ResponseGetter(ResponseGetter::StatusText) => ("get statusText", 0),
    })
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
