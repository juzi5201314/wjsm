use url::Url;
use wjsm_ir::value;
use wjsm_native_abi::NativeVmContext;

use super::{FetchCallable, FetchObjectKind, FetchProperty, RequestMethod, headers};
use crate::NativeAgentState;

#[derive(Clone)]
pub(super) struct RequestState {
    /// 包装对象；供 GC 边图钉住 headers 与提取的实例方法。由 `create` 写入。
    pub(super) object: i64,
    pub(super) url: String,
    pub(super) method: String,
    pub(super) headers: i64,
    pub(super) body: Option<String>,
    used: bool,
    pub(super) redirect: String,
    cache: String,
    credentials: String,
    integrity: String,
    keepalive: bool,
    pub(super) suppress_resource_timing: bool,
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
        Ok(request) => request,
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
    let Some(input) = args.first().copied() else {
        return Err(super::type_error(ctx, state, "Request input is required"));
    };
    let source = state
        .fetch
        .objects
        .get(&value::decode_handle(input))
        .and_then(|kind| match kind {
            FetchObjectKind::Request(handle) => state.fetch.requests.get(*handle),
            _ => None,
        })
        .cloned();
    if source.as_ref().is_some_and(|request| request.used) {
        return Err(super::type_error(
            ctx,
            state,
            "Request body is already used",
        ));
    }
    let mut url = source
        .as_ref()
        .map(|request| request.url.clone())
        .unwrap_or_else(|| super::to_string(state, input));
    let mut method = source
        .as_ref()
        .map(|request| request.method.clone())
        .unwrap_or_else(|| "GET".into());
    let mut body = source.as_ref().and_then(|request| request.body.clone());
    let mut redirect = source
        .as_ref()
        .map(|request| request.redirect.clone())
        .unwrap_or_else(|| "follow".into());
    let mut cache = source
        .as_ref()
        .map(|request| request.cache.clone())
        .unwrap_or_else(|| "default".into());
    let mut credentials = source
        .as_ref()
        .map(|request| request.credentials.clone())
        .unwrap_or_else(|| "same-origin".into());
    let mut integrity = source
        .as_ref()
        .map(|request| request.integrity.clone())
        .unwrap_or_default();
    let mut keepalive = source.as_ref().is_some_and(|request| request.keepalive);
    let mut suppress_resource_timing = source
        .as_ref()
        .is_some_and(|request| request.suppress_resource_timing);
    let mut headers = if let Some(source) = &source {
        headers::clone_headers(ctx, state, source.headers)?
    } else {
        headers::from_value(ctx, state, None)?
    };
    state.temporary_roots.push(headers);

    if let Some(init) = args
        .get(1)
        .copied()
        .filter(|init| !value::is_undefined(*init) && !value::is_null(*init))
    {
        if !value::is_js_object(init) {
            return Err(super::type_error(
                ctx,
                state,
                "Request init must be an object",
            ));
        }
        if let Some(stored) = super::read_optional_property(ctx, state, init, "method")? {
            method = super::to_string(state, stored).to_ascii_uppercase();
        }
        if let Some(stored) = super::read_optional_property(ctx, state, init, "headers")? {
            headers = headers::from_value(ctx, state, Some(stored))?;
            state.temporary_roots.push(headers);
        }
        if let Some(stored) = super::read_optional_property(ctx, state, init, "body")? {
            body = (!value::is_null(stored)).then(|| super::to_string(state, stored));
        }
        if let Some(stored) = super::read_optional_property(ctx, state, init, "redirect")? {
            redirect = enum_value(
                ctx,
                state,
                stored,
                "redirect",
                &["follow", "error", "manual"],
            )?;
        }
        if let Some(stored) = super::read_optional_property(ctx, state, init, "cache")? {
            cache = enum_value(
                ctx,
                state,
                stored,
                "cache",
                &[
                    "default",
                    "no-store",
                    "reload",
                    "no-cache",
                    "force-cache",
                    "only-if-cached",
                ],
            )?;
        }
        if let Some(stored) = super::read_optional_property(ctx, state, init, "credentials")? {
            credentials = enum_value(
                ctx,
                state,
                stored,
                "credentials",
                &["omit", "same-origin", "include"],
            )?;
        }
        if let Some(stored) = super::read_optional_property(ctx, state, init, "integrity")? {
            integrity = super::to_string(state, stored);
        }
        if let Some(stored) = super::read_optional_property(ctx, state, init, "keepalive")? {
            keepalive = super::super::runtime::is_truthy(state, stored);
        }
        if let Some(stored) =
            super::read_optional_property(ctx, state, init, "__wjsm_internal_no_resource_timing")?
        {
            suppress_resource_timing = super::super::runtime::is_truthy(state, stored);
        }
    }

    method = normalize_method(ctx, state, &method)?;
    if matches!(method.as_str(), "GET" | "HEAD") && body.is_some() {
        return Err(super::type_error(
            ctx,
            state,
            "GET or HEAD request cannot have a body",
        ));
    }
    let parsed =
        Url::parse(&url).map_err(|_| super::type_error(ctx, state, "Request URL is invalid"))?;
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(super::type_error(
            ctx,
            state,
            "Request URL cannot contain credentials",
        ));
    }
    url = parsed.into();
    create(
        state,
        RequestState {
            object: value::encode_undefined(),
            url,
            method,
            headers,
            body,
            used: false,
            redirect,
            cache,
            credentials,
            integrity,
            keepalive,
            suppress_resource_timing,
        },
    )
    .ok_or_else(|| super::super::fail_dispatch(ctx))
}

fn enum_value(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    stored: i64,
    name: &str,
    allowed: &[&str],
) -> Result<String, i64> {
    let text = super::to_string(state, stored);
    if allowed.contains(&text.as_str()) {
        Ok(text)
    } else {
        Err(super::type_error(
            ctx,
            state,
            &format!("invalid Request {name}"),
        ))
    }
}

fn normalize_method(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    method: &str,
) -> Result<String, i64> {
    let method = method.to_ascii_uppercase();
    if method.is_empty()
        || !method
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        || matches!(method.as_str(), "CONNECT" | "TRACE" | "TRACK")
    {
        Err(super::type_error(ctx, state, "invalid Request method"))
    } else {
        Ok(method)
    }
}

fn create(state: &mut NativeAgentState, mut request: RequestState) -> Option<i64> {
    let object = state.allocate_object(0, false).ok()?;
    state
        .set_web_instance_prototype(object, wjsm_ir::Builtin::RequestConstructor)
        .ok()?;
    request.object = object;
    let handle = state.fetch.requests.insert(request)?;
    super::register_object(state, object, FetchObjectKind::Request(handle));
    Some(object)
}

pub(super) fn property(
    state: &mut NativeAgentState,
    handle: u32,
    key: &str,
) -> Option<FetchProperty> {
    let request = state.fetch.requests.get(handle)?;
    let text = match key {
        "url" => Some(request.url.clone()),
        "method" => Some(request.method.clone()),
        "redirect" => Some(request.redirect.clone()),
        "cache" => Some(request.cache.clone()),
        "credentials" => Some(request.credentials.clone()),
        "integrity" => Some(request.integrity.clone()),
        _ => None,
    };
    if let Some(text) = text {
        return state
            .intern_text(text, value::TAG_STRING)
            .map(FetchProperty::Value);
    }
    match key {
        "body" => Some(FetchProperty::Value(value::encode_null())),
        "bodyUsed" => Some(FetchProperty::Value(value::encode_bool(request.used))),
        "headers" => Some(FetchProperty::Value(request.headers)),
        "keepalive" => Some(FetchProperty::Value(value::encode_bool(request.keepalive))),
        "clone" => Some(FetchProperty::Callable(FetchCallable::Request(
            handle,
            RequestMethod::Clone,
        ))),
        "text" => Some(FetchProperty::Callable(FetchCallable::Request(
            handle,
            RequestMethod::Text,
        ))),
        _ => None,
    }
}

pub(super) fn call(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    handle: u32,
    method: RequestMethod,
) -> i64 {
    let Some(request) = state.fetch.requests.get(handle).cloned() else {
        return super::super::fail_dispatch(ctx);
    };
    if request.used {
        return match method {
            RequestMethod::Clone => super::type_error(ctx, state, "Request body is already used"),
            RequestMethod::Text => body_used_rejection(ctx, state),
        };
    }
    match method {
        RequestMethod::Clone => {
            let Ok(headers) = headers::clone_headers(ctx, state, request.headers) else {
                return super::super::fail_dispatch(ctx);
            };
            create(state, RequestState { headers, ..request })
                .unwrap_or_else(|| super::super::fail_dispatch(ctx))
        }
        RequestMethod::Text => {
            if let Some(request) = state.fetch.requests.get_mut(handle) {
                request.used = true;
            }
            let Some(text) = state.intern_text(request.body.unwrap_or_default(), value::TAG_STRING)
            else {
                return super::super::fail_dispatch(ctx);
            };
            super::super::promise::resolved_promise(ctx, state, text)
        }
    }
}

fn body_used_rejection(ctx: &mut NativeVmContext, state: &mut NativeAgentState) -> i64 {
    let reason = super::modules::named_error_object(
        state,
        "TypeError",
        "Request body is already used".into(),
    )
    .unwrap_or_else(|| super::super::fail_dispatch(ctx));
    super::super::promise::rejected_promise(ctx, state, reason)
}
