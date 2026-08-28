use wjsm_ir::value;
use wjsm_native_abi::NativeVmContext;

use super::{FetchCallable, FetchObjectKind, FetchProperty, HeadersMethod};
use crate::NativeAgentState;

pub(super) struct HeadersState {
    pub(super) entries: Vec<(String, Vec<String>)>,
}

pub(super) fn construct(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let init = args
        .first()
        .copied()
        .filter(|value| !value::is_undefined(*value));
    match from_value(ctx, state, init) {
        Ok(headers) => headers,
        Err(exception) => exception,
    }
}

pub(super) fn from_value(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    init: Option<i64>,
) -> Result<i64, i64> {
    let entries = match init {
        None => Vec::new(),
        Some(init) => collect_entries(ctx, state, init)?,
    };
    create(state, entries).ok_or_else(|| super::super::fail_dispatch(ctx))
}

pub(super) fn clone_headers(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    headers: i64,
) -> Result<i64, i64> {
    let Some(entries) = state
        .fetch
        .objects
        .get(&value::decode_handle(headers))
        .and_then(|kind| match kind {
            FetchObjectKind::Headers(handle) => state.fetch.headers.get(*handle as usize),
            _ => None,
        })
        .map(|headers| headers.entries.clone())
    else {
        return Err(super::type_error(ctx, state, "Headers source is invalid"));
    };
    create(state, entries).ok_or_else(|| super::super::fail_dispatch(ctx))
}

fn create(state: &mut NativeAgentState, entries: Vec<(String, Vec<String>)>) -> Option<i64> {
    let object = state.allocate_object(0, false).ok()?;
    state
        .set_web_instance_prototype(object, wjsm_ir::Builtin::HeadersConstructor)
        .ok()?;
    let handle = u32::try_from(state.fetch.headers.len()).ok()?;
    state.fetch.headers.push(HeadersState { entries });
    super::register_object(state, object, FetchObjectKind::Headers(handle));
    Some(object)
}

pub(super) fn from_pairs(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    pairs: Vec<(String, String)>,
) -> Result<i64, i64> {
    let mut entries = Vec::with_capacity(pairs.len());
    for (name, value) in pairs {
        let Some(name) = normalize_name(&name) else {
            return Err(super::type_error(
                ctx,
                state,
                "invalid response header name",
            ));
        };
        let Some(value) = normalize_value(&value) else {
            return Err(super::type_error(
                ctx,
                state,
                "invalid response header value",
            ));
        };
        append_entry(&mut entries, name, value);
    }
    create(state, entries).ok_or_else(|| super::super::fail_dispatch(ctx))
}

fn collect_entries(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    init: i64,
) -> Result<Vec<(String, Vec<String>)>, i64> {
    if let Some(entries) = state
        .fetch
        .objects
        .get(&value::decode_handle(init))
        .and_then(|kind| match kind {
            FetchObjectKind::Headers(handle) => state.fetch.headers.get(*handle as usize),
            _ => None,
        })
        .map(|headers| headers.entries.clone())
    {
        return Ok(entries);
    }
    if value::is_array(init) {
        return collect_sequence(ctx, state, init);
    }
    if !value::is_js_object(init) {
        return Err(super::type_error(
            ctx,
            state,
            "Headers init must be an object",
        ));
    }
    let Some(properties) = super::super::object::own_keys(state, init, true) else {
        return Err(super::super::fail_dispatch(ctx));
    };
    let mut entries = Vec::with_capacity(properties.len());
    for (name, stored) in properties {
        if !value::is_string(name) {
            continue;
        }
        append_entry(
            &mut entries,
            normalize_name(&super::to_string(state, name))
                .ok_or_else(|| super::type_error(ctx, state, "invalid header name"))?,
            normalize_value(&super::to_string(state, stored))
                .ok_or_else(|| super::type_error(ctx, state, "invalid header value"))?,
        );
    }
    Ok(entries)
}

fn collect_sequence(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    init: i64,
) -> Result<Vec<(String, Vec<String>)>, i64> {
    let Ok(length) = state.gc.heap().array_length(value::decode_handle(init)) else {
        return Err(super::super::fail_dispatch(ctx));
    };
    let mut entries = Vec::with_capacity(length as usize);
    for index in 0..length {
        let entry = state
            .gc
            .heap()
            .get_element(value::decode_handle(init), index)
            .ok()
            .flatten()
            .map(|stored| stored as i64)
            .filter(|stored| value::is_array(*stored))
            .ok_or_else(|| super::type_error(ctx, state, "header sequence entry is invalid"))?;
        let Ok(entry_length) = state.gc.heap().array_length(value::decode_handle(entry)) else {
            return Err(super::super::fail_dispatch(ctx));
        };
        if entry_length != 2 {
            return Err(super::type_error(
                ctx,
                state,
                "header sequence entry must contain exactly two values",
            ));
        }
        let mut values = [value::encode_undefined(); 2];
        for (slot, value) in values.iter_mut().enumerate() {
            *value = state
                .gc
                .heap()
                .get_element(value::decode_handle(entry), slot as u32)
                .ok()
                .flatten()
                .map_or_else(value::encode_undefined, |stored| stored as i64);
        }
        let name = normalize_name(&super::to_string(state, values[0]))
            .ok_or_else(|| super::type_error(ctx, state, "invalid header name"))?;
        let value = normalize_value(&super::to_string(state, values[1]))
            .ok_or_else(|| super::type_error(ctx, state, "invalid header value"))?;
        append_entry(&mut entries, name, value);
    }
    Ok(entries)
}

pub(super) fn property(state: &NativeAgentState, handle: u32, key: &str) -> Option<FetchProperty> {
    state.fetch.headers.get(handle as usize)?;
    let method = match key {
        "append" => HeadersMethod::Append,
        "delete" => HeadersMethod::Delete,
        "get" => HeadersMethod::Get,
        "has" => HeadersMethod::Has,
        "set" => HeadersMethod::Set,
        _ => return None,
    };
    Some(FetchProperty::Callable(FetchCallable::Headers(
        handle, method,
    )))
}

pub(super) fn call(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    handle: u32,
    method: HeadersMethod,
    args: &[i64],
) -> i64 {
    let name = args
        .first()
        .copied()
        .map(|value| super::to_string(state, value))
        .unwrap_or_else(|| "undefined".into());
    let Some(name) = normalize_name(&name) else {
        return super::type_error(ctx, state, "invalid header name");
    };
    match method {
        HeadersMethod::Delete => {
            let Some(headers) = state.fetch.headers.get_mut(handle as usize) else {
                return super::super::fail_dispatch(ctx);
            };
            headers.entries.retain(|(stored, _)| stored != &name);
            value::encode_undefined()
        }
        HeadersMethod::Get => {
            let Some(headers) = state.fetch.headers.get(handle as usize) else {
                return super::super::fail_dispatch(ctx);
            };
            let Some((_, values)) = headers.entries.iter().find(|(stored, _)| stored == &name)
            else {
                return value::encode_null();
            };
            let Some(stored) = state.intern_text(values.join(", "), value::TAG_STRING) else {
                return super::super::fail_dispatch(ctx);
            };
            stored
        }
        HeadersMethod::Has => value::encode_bool(
            state
                .fetch
                .headers
                .get(handle as usize)
                .is_some_and(|headers| headers.entries.iter().any(|(stored, _)| stored == &name)),
        ),
        HeadersMethod::Append | HeadersMethod::Set => {
            let raw = args
                .get(1)
                .copied()
                .map(|value| super::to_string(state, value))
                .unwrap_or_else(|| "undefined".into());
            let Some(stored) = normalize_value(&raw) else {
                return super::type_error(ctx, state, "invalid header value");
            };
            let Some(headers) = state.fetch.headers.get_mut(handle as usize) else {
                return super::super::fail_dispatch(ctx);
            };
            if method == HeadersMethod::Set {
                headers.entries.retain(|(entry, _)| entry != &name);
            }
            append_entry(&mut headers.entries, name, stored);
            value::encode_undefined()
        }
    }
}

fn append_entry(entries: &mut Vec<(String, Vec<String>)>, name: String, value: String) {
    if let Some((_, values)) = entries.iter_mut().find(|(stored, _)| stored == &name) {
        values.push(value);
    } else {
        entries.push((name, vec![value]));
    }
}

fn normalize_name(name: &str) -> Option<String> {
    (!name.is_empty()
        && name.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        }))
    .then(|| name.to_ascii_lowercase())
}

fn normalize_value(value: &str) -> Option<String> {
    (!value.bytes().any(|byte| matches!(byte, 0 | b'\r' | b'\n')))
        .then(|| value.trim_matches([' ', '\t']).to_owned())
}
