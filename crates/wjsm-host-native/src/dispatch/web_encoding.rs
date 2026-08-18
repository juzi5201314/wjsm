//! WHATWG Encoding：`TextEncoder` / `TextDecoder` / `atob` / `btoa`。
//!
//! `TextDecoder` 经 `wjsm_intl_data::encoding_for_label` 消费 `encoding_rs`，
//! 支持全部 WHATWG 标签、BOM、`fatal` 与 `{ stream }` 跨调用状态。

use base64::Engine;
use wjsm_host::RuntimeString;
use wjsm_intl_data::encoding_rs::{Decoder, DecoderResult, Encoding};
use wjsm_intl_data::encoding_for_label;
use wjsm_ir::{constants, value};
use wjsm_native_abi::NativeVmContext;

use super::modules;
use super::runtime::{
    fail_dispatch, get_property, to_string_coerced, type_error,
};
use crate::{BUILTIN_PROTOTYPE_PROPERTY_FLAGS, NativeAgentState, NativeCallableKind};

const CONFIGURABLE: u32 = constants::FLAG_CONFIGURABLE as u32;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum WebEncodingCallable {
    Atob,
    Btoa,
    TextDecoderConstructor,
    TextDecoderDecode,
    TextDecoderEncodingGet,
    TextDecoderFatalGet,
    TextDecoderIgnoreBomGet,
    TextEncoderConstructor,
    TextEncoderEncode,
    TextEncoderEncodeInto,
}

/// TextDecoder 实例槽：编码、选项与 incremental decoder。
pub(crate) struct TextDecoderSlot {
    encoding: &'static Encoding,
    encoding_label: String,
    fatal: bool,
    ignore_bom: bool,
    decoder: Decoder,
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
        WebEncodingCallable::TextDecoderEncodingGet => decoder_encoding_get(ctx, state, receiver),
        WebEncodingCallable::TextDecoderFatalGet => decoder_bool_get(ctx, state, receiver, true),
        WebEncodingCallable::TextDecoderIgnoreBomGet => {
            decoder_bool_get(ctx, state, receiver, false)
        }
        WebEncodingCallable::TextEncoderConstructor => construct_encoder(ctx, state),
        WebEncodingCallable::TextEncoderEncode => encode(ctx, state, args),
        WebEncodingCallable::TextEncoderEncodeInto => encode_into(ctx, state, args),
    }
}

/// 确保 `TextDecoder.prototype` 已安装 getter 与 `decode`。
pub(crate) fn ensure_text_decoder_prototype(state: &mut NativeAgentState) -> Option<i64> {
    if let Some(prototype) = state.text_decoder_prototype {
        return Some(prototype);
    }
    let constructor = state.native_callable(NativeCallableKind::WebEncoding(
        WebEncodingCallable::TextDecoderConstructor,
    ))?;
    let prototype = state.allocate_object(4, false).ok()?;
    install_data_property(
        state,
        prototype,
        "constructor",
        constructor,
        BUILTIN_PROTOTYPE_PROPERTY_FLAGS,
    )?;
    install_method(
        state,
        prototype,
        "decode",
        WebEncodingCallable::TextDecoderDecode,
    )?;
    install_accessor(
        state,
        prototype,
        "encoding",
        WebEncodingCallable::TextDecoderEncodingGet,
    )?;
    install_accessor(
        state,
        prototype,
        "fatal",
        WebEncodingCallable::TextDecoderFatalGet,
    )?;
    install_accessor(
        state,
        prototype,
        "ignoreBOM",
        WebEncodingCallable::TextDecoderIgnoreBomGet,
    )?;
    state.text_decoder_prototype = Some(prototype);
    Some(prototype)
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
        Some(label) if value::is_undefined(label) => "utf-8".to_owned(),
        Some(label) => match to_string_coerced(ctx, state, label) {
            Ok(label) => label,
            Err(exception) => return exception,
        },
    };
    let Some(encoding) = encoding_for_label(&label) else {
        return range_error(ctx, state, "The encoding label provided is invalid");
    };

    let options = args.get(1).copied().unwrap_or_else(value::encode_undefined);
    let (fatal, ignore_bom) = match read_decoder_options(ctx, state, options) {
        Ok(pair) => pair,
        Err(exception) => return exception,
    };

    let Some(prototype) = ensure_text_decoder_prototype(state) else {
        return fail_dispatch(ctx);
    };
    let Ok(object) =
        state.allocate_object_with_prototype(0, false, value::decode_handle(prototype))
    else {
        return fail_dispatch(ctx);
    };

    let encoding_label = encoding.name().to_ascii_lowercase();
    let decoder = make_decoder(encoding, ignore_bom);
    state.text_decoders.insert(
        value::decode_handle(object),
        TextDecoderSlot {
            encoding,
            encoding_label,
            fatal,
            ignore_bom,
            decoder,
        },
    );
    object
}

fn read_decoder_options(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    options: i64,
) -> Result<(bool, bool), i64> {
    if value::is_undefined(options) || value::is_null(options) {
        return Ok((false, false));
    }
    if !is_type_object(options) {
        return Err(type_error(
            ctx,
            state,
            "TextDecoder options must be an object",
        ));
    }
    let fatal = read_bool_option(ctx, state, options, "fatal")?;
    let ignore_bom = read_bool_option(ctx, state, options, "ignoreBOM")?;
    Ok((fatal, ignore_bom))
}

fn read_bool_option(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    options: i64,
    name: &str,
) -> Result<bool, i64> {
    let key = string(state, name);
    match get_property(ctx, state, options, key) {
        Ok(value) if value::is_exception(value) => Err(value),
        Ok(value) if value::is_undefined(value) => Ok(false),
        Ok(value) => Ok(to_boolean(state, value)),
        Err(()) => Err(fail_dispatch(ctx)),
    }
}

fn make_decoder(encoding: &'static Encoding, ignore_bom: bool) -> Decoder {
    if ignore_bom {
        encoding.new_decoder_without_bom_handling()
    } else {
        encoding.new_decoder_with_bom_removal()
    }
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
    receiver: i64,
    args: &[i64],
) -> i64 {
    let handle = value::decode_handle(receiver);
    if !state.text_decoders.contains_key(&handle) {
        return type_error(ctx, state, "TextDecoder.prototype.decode called on incompatible receiver");
    }

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

    let stream = match read_stream_option(ctx, state, args.get(1).copied()) {
        Ok(stream) => stream,
        Err(exception) => return exception,
    };

    let fatal = state.text_decoders[&handle].fatal;
    let encoding = state.text_decoders[&handle].encoding;
    let ignore_bom = state.text_decoders[&handle].ignore_bom;

    let result = {
        let slot = state.text_decoders.get_mut(&handle).expect("checked");
        decode_chunk(&mut slot.decoder, &bytes, !stream, fatal)
    };

    match result {
        Ok(text) => {
            if !stream {
                // 非 stream：flush 后重置 decoder，供后续 decode 复用同一实例。
                if let Some(slot) = state.text_decoders.get_mut(&handle) {
                    slot.decoder = make_decoder(encoding, ignore_bom);
                }
            }
            state
                .intern_text(text, value::TAG_STRING)
                .unwrap_or_else(|| fail_dispatch(ctx))
        }
        Err(()) => type_error(
            ctx,
            state,
            &format!(
                "The encoded data was not valid for encoding {}",
                state
                    .text_decoders
                    .get(&handle)
                    .map(|slot| slot.encoding_label.as_str())
                    .unwrap_or("unknown")
            ),
        ),
    }
}

fn read_stream_option(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    options: Option<i64>,
) -> Result<bool, i64> {
    let Some(options) = options.filter(|value| !value::is_undefined(*value)) else {
        return Ok(false);
    };
    if value::is_null(options) {
        return Ok(false);
    }
    if !is_type_object(options) {
        return Err(type_error(
            ctx,
            state,
            "TextDecoder.decode options must be an object",
        ));
    }
    read_bool_option(ctx, state, options, "stream")
}

fn decode_chunk(
    decoder: &mut Decoder,
    bytes: &[u8],
    last: bool,
    fatal: bool,
) -> Result<String, ()> {
    if fatal {
        decode_without_replacement(decoder, bytes, last)
    } else {
        Ok(decode_with_replacement(decoder, bytes, last))
    }
}

fn decode_with_replacement(decoder: &mut Decoder, bytes: &[u8], last: bool) -> String {
    use wjsm_intl_data::encoding_rs::CoderResult;
    let mut output = String::with_capacity(decoder.max_utf8_buffer_length(bytes.len()).unwrap_or(0));
    let mut input = bytes;
    loop {
        let (result, read, _) = decoder.decode_to_string(input, &mut output, last);
        input = &input[read..];
        match result {
            CoderResult::InputEmpty => break,
            CoderResult::OutputFull => {
                output.reserve(output.capacity().max(16));
            }
        }
    }
    output
}

fn decode_without_replacement(
    decoder: &mut Decoder,
    bytes: &[u8],
    last: bool,
) -> Result<String, ()> {
    let mut output = String::with_capacity(
        decoder
            .max_utf8_buffer_length_without_replacement(bytes.len())
            .unwrap_or(0),
    );
    let mut input = bytes;
    loop {
        let (result, read) =
            decoder.decode_to_string_without_replacement(input, &mut output, last);
        input = &input[read..];
        match result {
            DecoderResult::InputEmpty => return Ok(output),
            DecoderResult::OutputFull => {
                output.reserve(output.capacity().max(16));
            }
            DecoderResult::Malformed(_, _) => return Err(()),
        }
    }
}

fn decoder_encoding_get(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    receiver: i64,
) -> i64 {
    let Some(slot) = state.text_decoders.get(&value::decode_handle(receiver)) else {
        return type_error(
            ctx,
            state,
            "TextDecoder.prototype.encoding called on incompatible receiver",
        );
    };
    string(state, &slot.encoding_label.clone())
}

fn decoder_bool_get(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    receiver: i64,
    fatal: bool,
) -> i64 {
    let Some(slot) = state.text_decoders.get(&value::decode_handle(receiver)) else {
        return type_error(
            ctx,
            state,
            "TextDecoder getter called on incompatible receiver",
        );
    };
    value::encode_bool(if fatal { slot.fatal } else { slot.ignore_bom })
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
    if let Some(bytes) = super::node_buffer::bytes(state, encoded) {
        return Some(bytes);
    }
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

fn install_method(
    state: &mut NativeAgentState,
    object: i64,
    name: &str,
    kind: WebEncodingCallable,
) -> Option<()> {
    let callable = state.native_callable(NativeCallableKind::WebEncoding(kind))?;
    attach_function_prototype(state, callable);
    install_data_property(
        state,
        object,
        name,
        callable,
        BUILTIN_PROTOTYPE_PROPERTY_FLAGS,
    )
}

fn install_accessor(
    state: &mut NativeAgentState,
    object: i64,
    name: &str,
    getter: WebEncodingCallable,
) -> Option<()> {
    let getter = state.native_callable(NativeCallableKind::WebEncoding(getter))?;
    attach_function_prototype(state, getter);
    let key = state
        .intern_text(name.to_owned(), value::TAG_STRING)
        .map(value::decode_handle)?;
    state
        .gc
        .heap()
        .define_accessor_property_with_flags(
            value::decode_handle(object),
            key,
            getter as u64,
            value::encode_undefined() as u64,
            CONFIGURABLE | constants::FLAG_ENUMERABLE as u32,
        )
        .ok()
}

fn install_data_property(
    state: &mut NativeAgentState,
    object: i64,
    name: &str,
    stored: i64,
    flags: u32,
) -> Option<()> {
    let key = state
        .intern_text(name.to_owned(), value::TAG_STRING)
        .map(value::decode_handle)?;
    state
        .gc
        .heap()
        .define_data_property(value::decode_handle(object), key, stored as u64, flags)
        .ok()
}

fn attach_function_prototype(state: &mut NativeAgentState, callable: i64) {
    if let Some(prototype) = state.native_callable(NativeCallableKind::FunctionPrototype) {
        state
            .callable_prototypes
            .entry(callable)
            .or_insert(prototype);
    }
}

fn set_property(state: &mut NativeAgentState, object: i64, name: &str, stored: i64) -> Option<()> {
    let key = state.intern_text(name.to_owned(), value::TAG_STRING)?;
    state
        .gc
        .heap()
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

fn is_type_object(encoded: i64) -> bool {
    !value::is_undefined(encoded)
        && !value::is_null(encoded)
        && !value::is_f64(encoded)
        && !value::is_bool(encoded)
        && !value::is_string(encoded)
        && !value::is_symbol(encoded)
        && !value::is_bigint(encoded)
}

fn to_boolean(state: &NativeAgentState, encoded: i64) -> bool {
    if value::is_bool(encoded) {
        return value::decode_bool(encoded);
    }
    if value::is_undefined(encoded) || value::is_null(encoded) {
        return false;
    }
    if value::is_f64(encoded) {
        let number = value::decode_f64(encoded);
        return number != 0.0 && !number.is_nan();
    }
    if value::is_string(encoded) {
        return state.string(encoded).is_some_and(|text| !text.is_empty());
    }
    true
}
