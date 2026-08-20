use wjsm_host::JsonValue;
use wjsm_ir::value;
use wjsm_native_abi::NativeVmContext;

use crate::NativeAgentState;

use super::super::{bigint, modules, object, runtime};

pub(super) fn parse(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let input = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let text = match to_json_string(ctx, state, input) {
        Ok(text) => text,
        Err(exception) => return exception,
    };
    let parsed = match wjsm_builtins::json::parse_json_text(&text) {
        Ok(parsed) => parsed,
        Err(message) => return exception(ctx, state, "SyntaxError", &message),
    };
    let parsed = match materialize(ctx, state, parsed) {
        Ok(parsed) => parsed,
        Err(exception) => return exception,
    };
    let reviver = args
        .get(1)
        .copied()
        .filter(|reviver| value::is_callable(*reviver));
    let Some(reviver) = reviver else {
        return parsed;
    };
    let Ok(root) = state.allocate_object(1, false) else {
        return runtime::fail_dispatch(ctx);
    };
    if modules::set_named_property(state, root, "", parsed).is_err() {
        return runtime::fail_dispatch(ctx);
    }
    internalize(ctx, state, reviver, root, "").unwrap_or_else(|exception| exception)
}

fn to_json_string(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    encoded: i64,
) -> Result<String, i64> {
    if value::is_string(encoded) {
        return state
            .string(encoded)
            .map(|text| text.to_utf8_lossy())
            .ok_or_else(|| runtime::fail_dispatch(ctx));
    }
    if value::is_symbol(encoded) {
        return Err(exception(
            ctx,
            state,
            "TypeError",
            "Cannot convert a Symbol to a string",
        ));
    }
    if value::is_bigint(encoded) {
        return bigint::read(state, encoded)
            .map(|integer| integer.to_string())
            .ok_or_else(|| runtime::fail_dispatch(ctx));
    }
    if !value::is_js_object(encoded) && !value::is_regexp(encoded) {
        return Ok(runtime::render_value(state, encoded));
    }
    for name in ["toString", "valueOf"] {
        let method = super::get_property(ctx, state, encoded, name)?;
        if !value::is_callable(method) {
            continue;
        }
        let primitive = super::invoke(ctx, state, method, encoded, &[])?;
        if !value::is_js_object(primitive) && !value::is_regexp(primitive) {
            return to_json_string(ctx, state, primitive);
        }
    }
    Ok("[object Object]".into())
}

fn materialize(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    input: JsonValue,
) -> Result<i64, i64> {
    match input {
        JsonValue::Null => Ok(value::encode_null()),
        JsonValue::Bool(boolean) => Ok(value::encode_bool(boolean)),
        JsonValue::Number(number) => Ok(value::encode_f64(number)),
        JsonValue::String(text) => state
            .intern_runtime_string(text, value::TAG_STRING)
            .ok_or_else(|| runtime::fail_dispatch(ctx)),
        JsonValue::Array(items) => {
            let mut values = Vec::with_capacity(items.len());
            for item in items {
                values.push(materialize(ctx, state, item)?);
            }
            state
                .allocate_array_values(&values)
                .map_err(|_| runtime::fail_dispatch(ctx))
        }
        JsonValue::Object(properties) => {
            let capacity =
                u32::try_from(properties.len()).map_err(|_| runtime::fail_dispatch(ctx))?;
            let object = state
                .allocate_object(capacity, false)
                .map_err(|_| runtime::fail_dispatch(ctx))?;
            let handle = value::decode_handle(object);
            for (name, property) in properties {
                let name = state
                    .intern_property_string(name)
                    .ok_or_else(|| runtime::fail_dispatch(ctx))?;
                let property = materialize(ctx, state, property)?;
                state
                    .gc
                    .heap()
                    .set_property(handle, name, property as u64)
                    .map_err(|_| runtime::fail_dispatch(ctx))?;
            }
            Ok(object)
        }
    }
}

fn internalize(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    reviver: i64,
    holder: i64,
    key: &str,
) -> Result<i64, i64> {
    let encoded = super::get_property(ctx, state, holder, key)?;
    if value::is_array(encoded) {
        let handle = value::decode_handle(encoded);
        let length = state
            .gc
            .heap()
            .array_length(handle)
            .map_err(|_| runtime::fail_dispatch(ctx))?;
        for index in 0..length {
            let key = index.to_string();
            let replacement = internalize(ctx, state, reviver, encoded, &key)?;
            state
                .gc
                .heap()
                .set_element(
                    handle,
                    index,
                    if value::is_undefined(replacement) {
                        value::encode_array_hole() as u64
                    } else {
                        replacement as u64
                    },
                )
                .map_err(|_| runtime::fail_dispatch(ctx))?;
        }
    } else if value::is_js_object(encoded) {
        for key in own_property_names(ctx, state, encoded)? {
            let replacement = internalize(ctx, state, reviver, encoded, &key)?;
            if value::is_undefined(replacement) {
                delete_property(ctx, state, encoded, &key)?;
            } else {
                set_property(ctx, state, encoded, &key, replacement)?;
            }
        }
    }
    let key = state
        .intern_text(key.into(), value::TAG_STRING)
        .ok_or_else(|| runtime::fail_dispatch(ctx))?;
    super::invoke(ctx, state, reviver, holder, &[key, encoded])
}

fn own_property_names(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    object: i64,
) -> Result<Vec<String>, i64> {
    object::own_keys(state, object, false)
        .ok_or_else(|| runtime::fail_dispatch(ctx))
        .map(|keys| {
            keys.into_iter()
                .filter_map(|(key, _)| state.string(key).map(|name| name.to_utf8_lossy()))
                .collect()
        })
}

fn set_property(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    object: i64,
    name: &str,
    encoded: i64,
) -> Result<(), i64> {
    modules::set_named_property(state, object, name, encoded)
        .map_err(|_| runtime::fail_dispatch(ctx))
}

fn delete_property(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    object: i64,
    name: &str,
) -> Result<(), i64> {
    let key = state
        .intern_text(name.into(), value::TAG_STRING)
        .ok_or_else(|| runtime::fail_dispatch(ctx))?;
    runtime::delete_property(state, object, key)
        .map(|_| ())
        .map_err(|()| runtime::fail_dispatch(ctx))
}

fn exception(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    name: &str,
    message: &str,
) -> i64 {
    modules::named_error_object(state, name, message.into())
        .and_then(|error| state.create_exception(error))
        .unwrap_or_else(|| runtime::fail_dispatch(ctx))
}
