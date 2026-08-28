use std::cell::RefCell;
use std::rc::Rc;

use wjsm_ir::value;
use wjsm_native_abi::NativeVmContext;

use super::{FetchCallable, FetchObjectKind, FetchProperty, ResponseMethod, headers};
use crate::NativeAgentState;

#[derive(Clone)]
pub(super) struct ResponseState {
    /// 包装对象；供 GC 边图钉住 headers/body 流与提取的实例方法。由
    /// `create` 写入。
    pub(super) object: i64,
    body: Option<String>,
    body_error: Option<String>,
    pub(super) body_object: i64,
    pub(super) headers: i64,
    status: u16,
    status_text: String,
    timing: Option<Rc<RefCell<ResourceTiming>>>,
    pub(super) used: bool,
}

struct ResourceTiming {
    url: String,
    status: u16,
    encoded_body_size: usize,
    decoded_body_size: usize,
    suppressed: bool,
    completed: bool,
}

pub(super) fn construct(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    // build 期间 headers 包装对象仅由局部值持有，而 init 属性读取可再入
    // JS（getter）触发 GC，必须经 temporary_roots 钉扎到构造完成。
    let initial_temp_roots = state.temporary_roots.len();
    let result = match build(ctx, state, args) {
        Ok(response) => response,
        Err(exception) => exception,
    };
    state.temporary_roots.truncate(initial_temp_roots);
    result
}

fn build(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> Result<i64, i64> {
    let body = args
        .first()
        .copied()
        .filter(|body| !value::is_undefined(*body) && !value::is_null(*body))
        .map(|body| super::to_string(state, body));
    let mut status = 200_u16;
    let mut status_text = String::new();
    let mut response_headers = headers::from_value(ctx, state, None)?;
    state.temporary_roots.push(response_headers);
    if let Some(init) = args
        .get(1)
        .copied()
        .filter(|init| !value::is_undefined(*init) && !value::is_null(*init))
    {
        if !value::is_js_object(init) {
            return Err(super::type_error(
                ctx,
                state,
                "Response init must be an object",
            ));
        }
        if let Some(stored) = super::read_optional_property(ctx, state, init, "status")? {
            let Some(number) = super::super::runtime::to_number(state, stored) else {
                return Err(super::type_error(ctx, state, "Response status is invalid"));
            };
            if !number.is_finite() || number.fract() != 0.0 || !(200.0..=599.0).contains(&number) {
                return Err(super::type_error(ctx, state, "Response status is invalid"));
            }
            status = number as u16;
        }
        if let Some(stored) = super::read_optional_property(ctx, state, init, "statusText")? {
            status_text = super::to_string(state, stored);
            if !status_text
                .bytes()
                .all(|byte| byte == b'\t' || (0x20..=0x7e).contains(&byte))
            {
                return Err(super::type_error(
                    ctx,
                    state,
                    "Response statusText is invalid",
                ));
            }
        }
        if let Some(stored) = super::read_optional_property(ctx, state, init, "headers")? {
            response_headers = headers::from_value(ctx, state, Some(stored))?;
            state.temporary_roots.push(response_headers);
        }
    }
    if body.is_some() && matches!(status, 101 | 204 | 205 | 304) {
        return Err(super::type_error(
            ctx,
            state,
            "Response with this status cannot have a body",
        ));
    }
    create(
        ctx,
        state,
        ResponseState {
            object: value::encode_undefined(),
            body,
            body_error: None,
            body_object: value::encode_null(),
            headers: response_headers,
            status,
            status_text,
            timing: None,
            used: false,
        },
    )
}

pub(super) fn create_fetch_response(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    url: String,
    body: String,
    suppressed: bool,
) -> i64 {
    let headers = match headers::from_value(ctx, state, None) {
        Ok(headers) => headers,
        Err(exception) => return exception,
    };
    let body_size = body.len();
    let timing = Rc::new(RefCell::new(ResourceTiming {
        url,
        status: 200,
        encoded_body_size: body_size,
        decoded_body_size: body_size,
        suppressed,
        completed: false,
    }));
    create(
        ctx,
        state,
        ResponseState {
            object: value::encode_undefined(),
            body: Some(body),
            body_error: None,
            body_object: value::encode_null(),
            headers,
            status: 200,
            status_text: String::new(),
            timing: Some(timing),
            used: false,
        },
    )
    .unwrap_or_else(|exception| exception)
}

pub(super) fn create_network_response(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    url: String,
    suppressed: bool,
    network: super::NetworkResponse,
) -> i64 {
    let headers = match headers::from_pairs(ctx, state, network.headers) {
        Ok(headers) => headers,
        Err(exception) => return exception,
    };
    let null_body = matches!(network.status, 101 | 204 | 205 | 304);
    let (body, body_error, body_size) = match network.body {
        Ok(body) => {
            let size = body.len();
            ((!null_body).then_some(body), None, size)
        }
        Err(error) => (Some(String::new()), Some(error), 0),
    };
    let timing = Rc::new(RefCell::new(ResourceTiming {
        url,
        status: network.status,
        encoded_body_size: body_size,
        decoded_body_size: body_size,
        suppressed,
        completed: false,
    }));
    let response = create(
        ctx,
        state,
        ResponseState {
            object: value::encode_undefined(),
            body,
            body_error,
            body_object: value::encode_null(),
            headers,
            status: network.status,
            status_text: network.status_text,
            timing: Some(timing),
            used: false,
        },
    )
    .unwrap_or_else(|exception| exception);
    if null_body
        && let Some(FetchObjectKind::Response(handle)) = state
            .fetch
            .objects
            .get(&value::decode_handle(response))
            .copied()
    {
        complete_timing(ctx, state, handle);
    }
    response
}

fn create(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    mut response: ResponseState,
) -> Result<i64, i64> {
    let body_stream = if let Some(body) = &response.body {
        let Some((body_object, stream)) =
            super::super::streams::create_body_stream(ctx, state, body.as_bytes())
        else {
            return Err(super::super::fail_dispatch(ctx));
        };
        response.body_object = body_object;
        Some(stream)
    } else {
        None
    };
    // 包装对象分配可触发 GC，此刻 headers 与 body 流仅由局部 ResponseState
    // 持有，须钉扎到登记完成。
    let initial_temp_roots = state.temporary_roots.len();
    state.temporary_roots.push(response.headers);
    state.temporary_roots.push(response.body_object);
    let object = state
        .allocate_object_with_gc_retry(ctx, 0, false)
        .map_err(|_| {
            state.temporary_roots.truncate(initial_temp_roots);
            super::super::fail_dispatch(ctx)
        })?;
    state.temporary_roots.truncate(initial_temp_roots);
    state
        .set_web_instance_prototype(object, wjsm_ir::Builtin::ResponseConstructor)
        .map_err(|()| super::super::fail_dispatch(ctx))?;
    response.object = object;
    let Some(handle) = state.fetch.responses.insert(response) else {
        return Err(super::super::fail_dispatch(ctx));
    };
    super::register_object(state, object, FetchObjectKind::Response(handle));
    if let Some(stream) = body_stream
        && let Some(stream) = state.streams.readables.get_mut(stream)
    {
        stream.response = Some(handle);
    }
    Ok(object)
}

pub(super) fn property(
    state: &mut NativeAgentState,
    handle: u32,
    key: &str,
) -> Option<FetchProperty> {
    let (body_object, headers, status, status_text, used) =
        state.fetch.responses.get(handle).map(|response| {
            (
                response.body_object,
                response.headers,
                response.status,
                response.status_text.clone(),
                response.used,
            )
        })?;
    match key {
        "body" => Some(FetchProperty::Value(body_object)),
        "bodyUsed" => Some(FetchProperty::Value(value::encode_bool(used))),
        "headers" => Some(FetchProperty::Value(headers)),
        "ok" => Some(FetchProperty::Value(value::encode_bool(
            (200..=299).contains(&status),
        ))),
        "status" => Some(FetchProperty::Value(value::encode_f64(f64::from(status)))),
        "statusText" => state
            .intern_text(status_text, value::TAG_STRING)
            .map(FetchProperty::Value),
        "arrayBuffer" => Some(FetchProperty::Callable(FetchCallable::Response(
            handle,
            ResponseMethod::ArrayBuffer,
        ))),
        "clone" => Some(FetchProperty::Callable(FetchCallable::Response(
            handle,
            ResponseMethod::Clone,
        ))),
        "text" => Some(FetchProperty::Callable(FetchCallable::Response(
            handle,
            ResponseMethod::Text,
        ))),
        _ => None,
    }
}

pub(super) fn call(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    handle: u32,
    method: ResponseMethod,
) -> i64 {
    let Some(response) = state.fetch.responses.get(handle).cloned() else {
        return super::super::fail_dispatch(ctx);
    };
    if response.used {
        return match method {
            ResponseMethod::Clone => super::type_error(ctx, state, "Response body is already used"),
            ResponseMethod::ArrayBuffer | ResponseMethod::Text => body_used_rejection(ctx, state),
        };
    }
    match method {
        ResponseMethod::Clone => {
            let Ok(headers) = headers::clone_headers(ctx, state, response.headers) else {
                return super::super::fail_dispatch(ctx);
            };
            create(
                ctx,
                state,
                ResponseState {
                    object: value::encode_undefined(),
                    headers,
                    body_object: value::encode_null(),
                    ..response
                },
            )
            .unwrap_or_else(|exception| exception)
        }
        ResponseMethod::ArrayBuffer | ResponseMethod::Text => {
            consume_body(ctx, state, handle, response, method)
        }
    }
}

fn consume_body(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    handle: u32,
    response: ResponseState,
    method: ResponseMethod,
) -> i64 {
    if let Some(response) = state.fetch.responses.get_mut(handle) {
        response.used = true;
    }
    complete_timing(ctx, state, handle);
    if let Some(error) = response.body_error {
        let reason = super::modules::named_error_object(state, "TypeError", error)
            .unwrap_or_else(|| super::super::fail_dispatch(ctx));
        return super::super::promise::rejected_promise(ctx, state, reason);
    }
    let body = response.body.unwrap_or_default();
    let encoded = match method {
        ResponseMethod::ArrayBuffer => super::super::buffers::from_bytes(state, body.into_bytes()),
        ResponseMethod::Text => state.intern_text(body, value::TAG_STRING),
        ResponseMethod::Clone => unreachable!("clone does not consume a response body"),
    };
    let Some(encoded) = encoded else {
        return super::super::fail_dispatch(ctx);
    };
    super::super::promise::resolved_promise(ctx, state, encoded)
}

pub(super) fn complete_timing(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    handle: u32,
) {
    let Some(timing) = state
        .fetch
        .responses
        .get(handle)
        .and_then(|response| response.timing.clone())
    else {
        return;
    };
    let timing = {
        let mut timing = timing.borrow_mut();
        if timing.completed {
            return;
        }
        timing.completed = true;
        if timing.suppressed {
            return;
        }
        (
            timing.url.clone(),
            timing.status,
            timing.encoded_body_size,
            timing.decoded_body_size,
        )
    };
    super::super::node_perf_hooks::emit_fetch_resource_entry(
        ctx,
        state,
        &timing.0,
        timing.1,
        super::super::node_perf_hooks::FetchBodySizes {
            encoded: timing.2,
            decoded: timing.3,
        },
    );
}

fn body_used_rejection(ctx: &mut NativeVmContext, state: &mut NativeAgentState) -> i64 {
    let reason = super::modules::named_error_object(
        state,
        "TypeError",
        "Response body is already used".into(),
    )
    .unwrap_or_else(|| super::super::fail_dispatch(ctx));
    super::super::promise::rejected_promise(ctx, state, reason)
}
