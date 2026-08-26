use std::collections::HashSet;

use wjsm_ir::value;
use wjsm_native_abi::NativeVmContext;

use crate::NativeAgentState;

use super::super::{modules, object, runtime};

pub(super) fn stringify(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let input = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let replacer = args.get(1).copied().unwrap_or_else(value::encode_undefined);
    let space = args.get(2).copied().unwrap_or_else(value::encode_undefined);
    let property_list = match replacer_property_list(state, replacer) {
        Ok(property_list) => property_list,
        Err(()) => return runtime::fail_dispatch(ctx),
    };
    let replacer = value::is_callable(replacer).then_some(replacer);
    let holder = if replacer.is_some() {
        match root_holder(state, input) {
            Some(holder) => holder,
            None => return runtime::fail_dispatch(ctx),
        }
    } else {
        value::encode_undefined()
    };
    let mut stack = HashSet::new();
    let gap = indentation(state, space);
    let mut serialize = JsonSerialize {
        replacer,
        property_list: property_list.as_deref(),
        stack: &mut stack,
        gap: &gap,
    };
    match serialize_property(ctx, state, "", input, holder, &mut serialize, "") {
        Ok(JsonOutput::Omitted) => value::encode_undefined(),
        Ok(JsonOutput::Text(text)) => state
            .intern_text(text, value::TAG_STRING)
            .unwrap_or_else(|| runtime::fail_dispatch(ctx)),
        Err(exception) => exception,
    }
}

enum JsonOutput {
    Omitted,
    Text(String),
}

struct JsonSerialize<'a> {
    replacer: Option<i64>,
    property_list: Option<&'a [String]>,
    stack: &'a mut HashSet<i64>,
    gap: &'a str,
}

fn serialize_property(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    key: &str,
    encoded: i64,
    holder: i64,
    serialize: &mut JsonSerialize<'_>,
    current_indent: &str,
) -> Result<JsonOutput, i64> {
    let encoded = apply_to_json(ctx, state, key, encoded)?;
    let encoded = if let Some(replacer) = serialize.replacer {
        let key = state
            .intern_text(key.into(), value::TAG_STRING)
            .ok_or_else(|| runtime::fail_dispatch(ctx))?;
        super::invoke(ctx, state, replacer, holder, &[key, encoded])?
    } else {
        encoded
    };
    serialize_value(ctx, state, encoded, serialize, current_indent)
}

fn serialize_value(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    encoded: i64,
    serialize: &mut JsonSerialize<'_>,
    current_indent: &str,
) -> Result<JsonOutput, i64> {
    if value::is_f64(encoded) {
        let number = value::decode_f64(encoded);
        if !number.is_finite() {
            return Ok(JsonOutput::Text("null".into()));
        }
        let number = if number == 0.0 { 0.0 } else { number };
        return Ok(JsonOutput::Text(wjsm_builtins::format_number_js(number)));
    }
    if value::is_undefined(encoded) || value::is_callable(encoded) || value::is_symbol(encoded) {
        return Ok(JsonOutput::Omitted);
    }
    if value::is_bigint(encoded) {
        return Err(runtime::type_error(
            ctx,
            state,
            "Do not know how to serialize a BigInt",
        ));
    }
    if value::is_string(encoded) {
        return state
            .string_owned(encoded)
            .map(|text| JsonOutput::Text(text.to_json_quoted()))
            .ok_or_else(|| runtime::fail_dispatch(ctx));
    }
    if value::is_bool(encoded) {
        return Ok(JsonOutput::Text(value::decode_bool(encoded).to_string()));
    }
    if value::is_null(encoded) {
        return Ok(JsonOutput::Text("null".into()));
    }
    if value::is_array(encoded) {
        return serialize_array(ctx, state, encoded, serialize, current_indent);
    }
    if value::is_js_object(encoded) || value::is_regexp(encoded) {
        return serialize_object(ctx, state, encoded, serialize, current_indent);
    }
    Ok(JsonOutput::Omitted)
}

fn serialize_array(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    array: i64,
    serialize: &mut JsonSerialize<'_>,
    current_indent: &str,
) -> Result<JsonOutput, i64> {
    if !serialize.stack.insert(array) {
        return Err(runtime::type_error(
            ctx,
            state,
            "Converting circular structure to JSON",
        ));
    }
    let result = (|| {
        let length = state
            .gc
            .heap()
            .array_length(value::decode_handle(array))
            .map_err(|_| runtime::fail_dispatch(ctx))?;
        let next_indent = next_indent(serialize.gap, current_indent);
        let mut elements = Vec::with_capacity(length as usize);
        for index in 0..length {
            let encoded = state
                .gc
                .heap()
                .get_element(value::decode_handle(array), index)
                .map_err(|_| runtime::fail_dispatch(ctx))?
                .map(|encoded| encoded as i64)
                .filter(|encoded| !value::is_array_hole(*encoded))
                .unwrap_or_else(value::encode_undefined);
            let serialized = serialize_property(
                ctx,
                state,
                &index.to_string(),
                encoded,
                array,
                serialize,
                &next_indent,
            )?;
            elements.push(match serialized {
                JsonOutput::Omitted => "null".into(),
                JsonOutput::Text(text) => text,
            });
        }
        Ok(render_array(
            elements,
            serialize.gap,
            current_indent,
            &next_indent,
        ))
    })();
    serialize.stack.remove(&array);
    result.map(JsonOutput::Text)
}

fn serialize_object(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    object: i64,
    serialize: &mut JsonSerialize<'_>,
    current_indent: &str,
) -> Result<JsonOutput, i64> {
    if !serialize.stack.insert(object) {
        return Err(runtime::type_error(
            ctx,
            state,
            "Converting circular structure to JSON",
        ));
    }
    let result = (|| {
        let names = match serialize.property_list {
            Some(names) => names.to_vec(),
            None => enumerable_property_names(ctx, state, object)?,
        };
        let next_indent = next_indent(serialize.gap, current_indent);
        let mut properties = Vec::with_capacity(names.len());
        for name in names {
            let encoded = super::get_property(ctx, state, object, &name)?;
            let serialized =
                serialize_property(ctx, state, &name, encoded, object, serialize, &next_indent)?;
            if let JsonOutput::Text(serialized) = serialized {
                let separator = if serialize.gap.is_empty() { ":" } else { ": " };
                let name = state
                    .intern_text(name, value::TAG_STRING)
                    .and_then(|name| state.string_owned(name).map(|name| name.to_json_quoted()))
                    .ok_or_else(|| runtime::fail_dispatch(ctx))?;
                properties.push(format!("{name}{separator}{serialized}"));
            }
        }
        Ok(render_object(
            properties,
            serialize.gap,
            current_indent,
            &next_indent,
        ))
    })();
    serialize.stack.remove(&object);
    result.map(JsonOutput::Text)
}

fn apply_to_json(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    key: &str,
    encoded: i64,
) -> Result<i64, i64> {
    if !value::is_js_object(encoded) {
        return Ok(encoded);
    }
    let method = super::get_property(ctx, state, encoded, "toJSON")?;
    if !value::is_callable(method) {
        return Ok(encoded);
    }
    let key = state
        .intern_text(key.into(), value::TAG_STRING)
        .ok_or_else(|| runtime::fail_dispatch(ctx))?;
    super::invoke(ctx, state, method, encoded, &[key])
}

fn root_holder(state: &mut NativeAgentState, input: i64) -> Option<i64> {
    let holder = state.allocate_object(1, false).ok()?;
    modules::set_named_property(state, holder, "", input).ok()?;
    Some(holder)
}

fn indentation(state: &NativeAgentState, encoded: i64) -> String {
    if value::is_f64(encoded) {
        let width = value::decode_f64(encoded).trunc().clamp(0.0, 10.0) as usize;
        " ".repeat(width)
    } else if value::is_string(encoded) {
        state
            .string_owned(encoded)
            .map(|text| {
                text.slice_units(0..text.utf16_len().min(10))
                    .to_utf8_lossy()
            })
            .unwrap_or_default()
    } else {
        String::new()
    }
}

fn replacer_property_list(
    state: &NativeAgentState,
    replacer: i64,
) -> Result<Option<Vec<String>>, ()> {
    if !value::is_array(replacer) {
        return Ok(None);
    }
    let length = state
        .gc
        .heap()
        .array_length(value::decode_handle(replacer))
        .map_err(|_| ())?;
    let mut properties = Vec::new();
    for index in 0..length {
        let Some(encoded) = state
            .gc
            .heap()
            .get_element(value::decode_handle(replacer), index)
            .map_err(|_| ())?
            .map(|encoded| encoded as i64)
            .filter(|encoded| !value::is_array_hole(*encoded))
        else {
            continue;
        };
        let name = if value::is_string(encoded) {
            state.string_owned(encoded).map(|text| text.to_utf8_lossy())
        } else if value::is_f64(encoded) {
            let number = value::decode_f64(encoded);
            number
                .is_finite()
                .then(|| wjsm_builtins::format_number_js(if number == 0.0 { 0.0 } else { number }))
        } else {
            None
        };
        if let Some(name) = name
            && !properties.contains(&name)
        {
            properties.push(name);
        }
    }
    Ok(Some(properties))
}

fn enumerable_property_names(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    object: i64,
) -> Result<Vec<String>, i64> {
    if value::is_regexp(object) {
        return Ok(Vec::new());
    }
    if value::is_callable(object) {
        let mut names = Vec::new();
        for (&(callable, key), flags) in &state.callable_property_flags {
            if callable == object && flags & wjsm_ir::constants::FLAG_ENUMERABLE as u32 != 0 {
                let encoded = key.to_value();
                if let Some(name) = state.string_owned(encoded).map(|name| name.to_utf8_lossy()) {
                    names.push(name);
                }
            }
        }
        return Ok(names);
    }
    let keys = object::own_keys(state, object, true).ok_or_else(|| runtime::fail_dispatch(ctx))?;
    Ok(keys
        .into_iter()
        .filter_map(|(key, _)| state.string_owned(key).map(|name| name.to_utf8_lossy()))
        .collect())
}

fn next_indent(gap: &str, current_indent: &str) -> String {
    if gap.is_empty() {
        String::new()
    } else {
        format!("{current_indent}{gap}")
    }
}

fn render_array(
    elements: Vec<String>,
    gap: &str,
    current_indent: &str,
    next_indent: &str,
) -> String {
    if elements.is_empty() {
        "[]".into()
    } else if gap.is_empty() {
        format!("[{}]", elements.join(","))
    } else {
        format!(
            "[\n{}{}\n{}]",
            next_indent,
            elements.join(&format!(",\n{next_indent}")),
            current_indent
        )
    }
}

fn render_object(
    properties: Vec<String>,
    gap: &str,
    current_indent: &str,
    next_indent: &str,
) -> String {
    if properties.is_empty() {
        "{}".into()
    } else if gap.is_empty() {
        format!("{{{}}}", properties.join(","))
    } else {
        format!(
            "{{\n{}{}\n{}}}",
            next_indent,
            properties.join(&format!(",\n{next_indent}")),
            current_indent
        )
    }
}
