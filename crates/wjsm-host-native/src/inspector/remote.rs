use serde_json::{Value, json};
use wjsm_ir::value;

use crate::NativeAgentState;

pub(super) fn remote_object(state: &NativeAgentState, encoded: i64) -> Value {
    if value::is_undefined(encoded) {
        return json!({"type": "undefined"});
    }
    if value::is_null(encoded) {
        return json!({"type": "object", "subtype": "null", "value": null});
    }
    if value::is_bool(encoded) {
        return json!({"type": "boolean", "value": value::decode_bool(encoded)});
    }
    if value::is_f64(encoded) {
        return number_object(value::decode_f64(encoded));
    }
    if value::is_string(encoded) {
        let text = state
            .string(encoded)
            .and_then(wjsm_host::RuntimeString::to_utf8)
            .unwrap_or_default();
        return json!({"type": "string", "value": text, "description": text});
    }
    if value::is_bigint(encoded) {
        let description = crate::dispatch::render_value(state, encoded);
        return json!({
            "type": "bigint",
            "unserializableValue": description,
            "description": description,
        });
    }
    if value::is_symbol(encoded) {
        return json!({
            "type": "symbol",
            "description": crate::dispatch::render_value(state, encoded),
        });
    }
    if value::is_callable(encoded) {
        return json!({
            "type": "function",
            "className": "Function",
            "description": crate::dispatch::render_value(state, encoded),
            "objectId": encode_object_id(encoded),
        });
    }
    if value::is_array(encoded) {
        let length = state
            .heap
            .array_length(value::decode_handle(encoded))
            .unwrap_or(0);
        return json!({
            "type": "object",
            "subtype": "array",
            "className": "Array",
            "description": format!("Array({length})"),
            "objectId": encode_object_id(encoded),
        });
    }
    if value::is_js_object(encoded) || value::is_proxy(encoded) || value::is_regexp(encoded) {
        return json!({
            "type": "object",
            "className": "Object",
            "description": crate::dispatch::render_value(state, encoded),
            "objectId": encode_object_id(encoded),
        });
    }
    json!({
        "type": "undefined",
        "description": crate::dispatch::render_value(state, encoded),
    })
}

pub(super) fn properties(state: &NativeAgentState, encoded: i64) -> Vec<Value> {
    if value::is_array(encoded) {
        return array_properties(state, encoded);
    }
    if value::is_object(encoded) {
        return object_properties(state, encoded);
    }
    Vec::new()
}

pub(super) fn decode_object_id(object_id: &str) -> Option<i64> {
    object_id.strip_prefix("wjsm:").and_then(|bits| {
        u64::from_str_radix(bits, 16)
            .ok()
            .map(|bits| i64::from_ne_bytes(bits.to_ne_bytes()))
    })
}

fn encode_object_id(encoded: i64) -> String {
    format!("wjsm:{:016x}", encoded as u64)
}

fn number_object(number: f64) -> Value {
    if number.is_nan() {
        return json!({
            "type": "number",
            "unserializableValue": "NaN",
            "description": "NaN",
        });
    }
    if number == f64::INFINITY {
        return json!({
            "type": "number",
            "unserializableValue": "Infinity",
            "description": "Infinity",
        });
    }
    if number == f64::NEG_INFINITY {
        return json!({
            "type": "number",
            "unserializableValue": "-Infinity",
            "description": "-Infinity",
        });
    }
    if number == 0.0 && number.is_sign_negative() {
        return json!({
            "type": "number",
            "unserializableValue": "-0",
            "description": "-0",
        });
    }
    json!({"type": "number", "value": number, "description": number.to_string()})
}

fn array_properties(state: &NativeAgentState, encoded: i64) -> Vec<Value> {
    let handle = value::decode_handle(encoded);
    let length = state.heap.array_length(handle).unwrap_or(0);
    let mut properties = Vec::with_capacity(length as usize + 1);
    for index in 0..length {
        if let Ok(Some(element)) = state.heap.get_element(handle, index) {
            properties.push(data_property(
                index.to_string(),
                remote_object(state, element as i64),
                true,
            ));
        }
    }
    properties.push(data_property(
        "length".into(),
        remote_object(state, value::encode_f64(f64::from(length))),
        false,
    ));
    properties
}

fn object_properties(state: &NativeAgentState, encoded: i64) -> Vec<Value> {
    let handle = value::decode_handle(encoded);
    let Ok(slots) = state.heap.own_property_slots(handle) else {
        return Vec::new();
    };
    slots
        .into_iter()
        .filter_map(|(key, flags)| {
            let name = state
                .string(value::encode_handle(value::TAG_STRING, key))?
                .to_utf8()?;
            let property = state.heap.get_property_slot(handle, key).ok()??;
            let stored = if property.flags & wjsm_ir::constants::FLAG_IS_ACCESSOR as u32 != 0 {
                value::encode_undefined()
            } else {
                property.value as i64
            };
            Some(data_property(
                name,
                remote_object(state, stored),
                flags & wjsm_ir::constants::FLAG_ENUMERABLE as u32 != 0,
            ))
        })
        .collect()
}

fn data_property(name: String, value: Value, enumerable: bool) -> Value {
    json!({
        "name": name,
        "value": value,
        "writable": true,
        "configurable": true,
        "enumerable": enumerable,
        "isOwn": true,
    })
}
