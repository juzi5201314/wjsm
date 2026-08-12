use base64::Engine;
use wjsm_host::RuntimeString;
use wjsm_ir::value;
use wjsm_native_abi::NativeVmContext;

use super::modules;
use super::runtime::{fail_dispatch, to_string_coerced, type_error};
use crate::{NativeAgentState, NativeCallableKind};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum WebEncodingCallable {
    Atob,
    Btoa,
    TextDecoderConstructor,
    TextDecoderDecode,
    TextEncoderConstructor,
    TextEncoderEncode,
    TextEncoderEncodeInto,
}

pub(crate) fn call(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    callable: WebEncodingCallable,
    receiver: i64,
    args: &[i64],
) -> i64 {
    match callable {
        WebEncodingCallable::Atob => atob(ctx, state, args),
        WebEncodingCallable::Btoa => btoa(ctx, state, args),
        WebEncodingCallable::TextDecoderConstructor => construct_decoder(ctx, state, args),
        WebEncodingCallable::TextDecoderDecode => decode(ctx, state, receiver, args),
        WebEncodingCallable::TextEncoderConstructor => construct_encoder(ctx, state),
        WebEncodingCallable::TextEncoderEncode => encode(ctx, state, args),
        WebEncodingCallable::TextEncoderEncodeInto => encode_into(ctx, state, args),
    }
}

fn construct_encoder(ctx: &mut NativeVmContext, state: &mut NativeAgentState) -> i64 {
    let encoding = string(state, "utf-8");
    let Some(object) = object_with_callables(
        state,
        &[
            ("encode", WebEncodingCallable::TextEncoderEncode),
            ("encodeInto", WebEncodingCallable::TextEncoderEncodeInto),
        ],
        &[("encoding", encoding)],
    ) else {
        return fail_dispatch(ctx);
    };
    object
}

fn construct_decoder(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let label = match args.first().copied() {
        None => "utf-8".to_owned(),
        Some(label) => match to_string_coerced(ctx, state, label) {
            Ok(label) => label.trim().to_ascii_lowercase(),
            Err(exception) => return exception,
        },
    };
    if !matches!(label.as_str(), "utf-8" | "utf8" | "unicode-1-1-utf-8") {
        return range_error(ctx, state, "The encoding label provided is invalid");
    }

    let encoding = string(state, "utf-8");
    let Some(object) = object_with_callables(
        state,
        &[("decode", WebEncodingCallable::TextDecoderDecode)],
        &[
            ("encoding", encoding),
            ("fatal", value::encode_bool(false)),
            ("ignoreBOM", value::encode_bool(false)),
        ],
    ) else {
        return fail_dispatch(ctx);
    };
    object
}

fn encode(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let input = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let text = match to_string_coerced(ctx, state, input) {
        Ok(text) => text,
        Err(exception) => return exception,
    };
    super::typedarray::create_uint8_array(state, text.as_bytes())
        .unwrap_or_else(|| fail_dispatch(ctx))
}

fn encode_into(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let source = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let source = match to_string_coerced(ctx, state, source) {
        Ok(source) => source,
        Err(exception) => return exception,
    };
    let Some(destination) = args.get(1).copied().filter(|destination| {
        state
            .typed_arrays
            .get(&value::decode_handle(*destination))
            .is_some_and(|array| array.kind == super::typedarray::TypedArrayKind::Uint8)
    }) else {
        return type_error(
            ctx,
            state,
            "TextEncoder.encodeInto destination must be a Uint8Array",
        );
    };

    let capacity = state
        .typed_arrays
        .get(&value::decode_handle(destination))
        .map_or(0, |array| array.length);
    let mut read = 0usize;
    let mut written = 0usize;
    for character in source.chars() {
        let mut encoded = [0; 4];
        let bytes = character.encode_utf8(&mut encoded).as_bytes();
        if written + bytes.len() > capacity {
            break;
        }
        for byte in bytes {
            let stored = value::encode_f64(f64::from(*byte));
            if super::typedarray::set_element(state, destination, written, stored).is_none() {
                return fail_dispatch(ctx);
            }
            written += 1;
        }
        read += character.len_utf16();
    }
    result_pair(ctx, state, read, written)
}

fn decode(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    _receiver: i64,
    args: &[i64],
) -> i64 {
    let bytes = match args.first().copied() {
        None => Vec::new(),
        Some(input) if value::is_undefined(input) => Vec::new(),
        Some(input) => match buffer_source_bytes(state, input) {
            Some(bytes) => bytes,
            None => {
                return type_error(
                    ctx,
                    state,
                    "TextDecoder.decode input must be a BufferSource",
                );
            }
        },
    };
    state
        .intern_text(
            String::from_utf8_lossy(&bytes).into_owned(),
            value::TAG_STRING,
        )
        .unwrap_or_else(|| fail_dispatch(ctx))
}

fn btoa(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let input = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let input = match to_string_coerced(ctx, state, input) {
        Ok(input) => input,
        Err(exception) => return exception,
    };
    let runtime = RuntimeString::from(input);
    let Some(bytes) = runtime
        .as_utf16_units()
        .iter()
        .copied()
        .map(u8::try_from)
        .collect::<Result<Vec<_>, _>>()
        .ok()
    else {
        return invalid_character_error(ctx, state);
    };
    state
        .intern_text(
            base64::engine::general_purpose::STANDARD.encode(bytes),
            value::TAG_STRING,
        )
        .unwrap_or_else(|| fail_dispatch(ctx))
}

fn atob(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let input = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let input = match to_string_coerced(ctx, state, input) {
        Ok(input) => input,
        Err(exception) => return exception,
    };
    let filtered: Vec<u8> = input
        .bytes()
        .filter(|byte| !matches!(byte, b'\t' | b'\n' | b'\x0c' | b'\r' | b' '))
        .collect();
    let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(filtered) else {
        return invalid_character_error(ctx, state);
    };
    let output = RuntimeString::from_utf16_units(bytes.into_iter().map(u16::from).collect());
    state
        .intern_runtime_string(output, value::TAG_STRING)
        .unwrap_or_else(|| fail_dispatch(ctx))
}

fn buffer_source_bytes(state: &NativeAgentState, encoded: i64) -> Option<Vec<u8>> {
    let handle = value::decode_handle(encoded);
    if let Some(buffer) = state.array_buffers.get(&handle) {
        return Some(buffer.bytes.borrow().clone());
    }
    if let Some(view) = state.data_views.get(&handle) {
        let bytes = if let Some(shared) = &view.shared {
            shared.lock().ok()?.clone()
        } else {
            state
                .array_buffers
                .get(&view.buffer)?
                .bytes
                .borrow()
                .clone()
        };
        return bytes
            .get(view.offset..view.offset.checked_add(view.length)?)
            .map(<[u8]>::to_vec);
    }
    super::typedarray::visible_bytes(state, encoded)
}

fn object_with_callables(
    state: &mut NativeAgentState,
    callables: &[(&str, WebEncodingCallable)],
    values: &[(&str, i64)],
) -> Option<i64> {
    let capacity = u32::try_from(callables.len().checked_add(values.len())?).ok()?;
    let object = state.allocate_object(capacity, false).ok()?;
    for (name, callable) in callables {
        let callable = state.native_callable(NativeCallableKind::WebEncoding(*callable))?;
        set_property(state, object, name, callable)?;
    }
    for (name, value) in values {
        set_property(state, object, name, *value)?;
    }
    Some(object)
}

fn set_property(state: &mut NativeAgentState, object: i64, name: &str, stored: i64) -> Option<()> {
    let key = state.intern_text(name.to_owned(), value::TAG_STRING)?;
    state
        .heap
        .set_property(
            value::decode_handle(object),
            value::decode_handle(key),
            stored as u64,
        )
        .ok()
}

fn string(state: &mut NativeAgentState, text: &str) -> i64 {
    state
        .intern_text(text.to_owned(), value::TAG_STRING)
        .unwrap_or_else(value::encode_undefined)
}

fn result_pair(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    read: usize,
    written: usize,
) -> i64 {
    let Ok(object) = state.allocate_object(2, false) else {
        return fail_dispatch(ctx);
    };
    let read = value::encode_f64(read as f64);
    let written = value::encode_f64(written as f64);
    if set_property(state, object, "read", read).is_none()
        || set_property(state, object, "written", written).is_none()
    {
        return fail_dispatch(ctx);
    }
    object
}

fn invalid_character_error(ctx: &mut NativeVmContext, state: &mut NativeAgentState) -> i64 {
    exception(
        ctx,
        state,
        "InvalidCharacterError",
        "The string to be decoded is not correctly encoded",
    )
}

fn range_error(ctx: &mut NativeVmContext, state: &mut NativeAgentState, message: &str) -> i64 {
    exception(ctx, state, "RangeError", message)
}

fn exception(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    name: &str,
    message: &str,
) -> i64 {
    modules::named_error_object(state, name, message.to_owned())
        .and_then(|error| state.create_exception(error))
        .unwrap_or_else(|| fail_dispatch(ctx))
}
