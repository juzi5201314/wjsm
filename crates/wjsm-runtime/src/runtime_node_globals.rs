use base64::Engine;
use base64::engine::general_purpose;
use encoding_rs::Encoding;

use crate::runtime_buffer::{create_buffer_from_bytes, visible_bytes, write_entry_bytes};
use crate::runtime_encoding::{decode_base64_string, js_string_lossy, js_string_value};
use crate::runtime_string::RuntimeString;
use crate::*;

pub(crate) fn install_node_web_globals_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    global_obj: i64,
) -> anyhow::Result<()> {
    define_global(caller, global_obj, "global", global_obj);
    install_native(
        caller,
        global_obj,
        "Buffer",
        NativeCallable::BufferConstructor,
    );
    install_native(
        caller,
        global_obj,
        "TextEncoder",
        NativeCallable::TextEncoderConstructor,
    );
    install_native(
        caller,
        global_obj,
        "TextDecoder",
        NativeCallable::TextDecoderConstructor,
    );
    install_native(
        caller,
        global_obj,
        "structuredClone",
        NativeCallable::StructuredClone,
    );
    install_native(
        caller,
        global_obj,
        "queueMicrotask",
        NativeCallable::QueueMicrotask,
    );
    install_native(caller, global_obj, "atob", NativeCallable::Atob);
    install_native(caller, global_obj, "btoa", NativeCallable::Btoa);

    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let performance = alloc_host_object(caller, &env, 1);
    let now = alloc_native_callable(caller, NativeCallable::PerformanceNow);
    define_global(caller, performance, "now", now);
    define_global(caller, global_obj, "performance", performance);
    Ok(())
}

pub(crate) fn call_text_encoder_constructor(caller: &mut Caller<'_, RuntimeState>) -> i64 {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let obj = alloc_host_object(caller, &env, 1);
    if let Some(proto) = crate::runtime_heap::native_callable_text_encoder_prototype(caller) {
        crate::runtime_heap::set_object_proto_header(caller, &env, obj, proto);
    }
    let encoding = store_runtime_string(caller, "utf-8".to_string());
    define_global(caller, obj, "encoding", encoding);
    obj
}

pub(crate) fn call_text_encoder_method(
    caller: &mut Caller<'_, RuntimeState>,
    kind: TextEncoderMethodKind,
    args: &[i64],
) -> i64 {
    match kind {
        TextEncoderMethodKind::Encode => {
            let input = args
                .first()
                .copied()
                .unwrap_or_else(value::encode_undefined);
            let bytes = js_string_lossy(caller, input).into_bytes();
            create_buffer_from_bytes(caller, bytes)
        }
        TextEncoderMethodKind::EncodeInto => text_encoder_encode_into(caller, args),
    }
}

pub(crate) fn call_text_decoder_constructor(
    caller: &mut Caller<'_, RuntimeState>,
    args: &[i64],
) -> i64 {
    let label = args
        .first()
        .copied()
        .filter(|v| !value::is_undefined(*v))
        .map(|v| js_string_lossy(caller, v))
        .unwrap_or_else(|| "utf-8".to_string());
    let Some(encoding) = Encoding::for_label(label.as_bytes()) else {
        return make_range_error_exception(
            caller,
            &format!("The encoding label provided ('{label}') is invalid"),
        );
    };
    let options = args.get(1).copied().unwrap_or_else(value::encode_undefined);
    let fatal = option_bool(caller, options, "fatal");
    let ignore_bom = option_bool(caller, options, "ignoreBOM");

    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let obj = alloc_host_object(caller, &env, 3);
    if let Some(proto) = crate::runtime_heap::native_callable_text_decoder_prototype(caller) {
        crate::runtime_heap::set_object_proto_header(caller, &env, obj, proto);
    }
    let name = store_runtime_string(caller, encoding.name().to_ascii_lowercase());
    define_global(caller, obj, "encoding", name);
    define_global(caller, obj, "fatal", value::encode_bool(fatal));
    define_global(caller, obj, "ignoreBOM", value::encode_bool(ignore_bom));
    obj
}

pub(crate) fn call_text_decoder_method(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    kind: TextDecoderMethodKind,
    args: &[i64],
) -> i64 {
    match kind {
        TextDecoderMethodKind::Decode => text_decoder_decode(caller, this_val, args),
    }
}

pub(crate) fn call_atob(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let input = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    match decode_base64_string(&js_string_lossy(caller, input), false) {
        Ok(bytes) => {
            let units = bytes
                .into_iter()
                .map(|byte| byte as u16)
                .collect::<Vec<_>>();
            store_runtime_string(caller, RuntimeString::from_utf16_units(units))
        }
        Err(_) => make_type_error_exception(
            caller,
            "InvalidCharacterError: atob input is not valid base64",
        ),
    }
}

pub(crate) fn call_btoa(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let input = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let string = js_string_value(caller, input);
    let mut bytes = Vec::with_capacity(string.utf16_len());
    for unit in string.as_utf16_units() {
        if *unit > 0x00ff {
            return make_type_error_exception(
                caller,
                "InvalidCharacterError: btoa input contains characters outside Latin1",
            );
        }
        bytes.push(*unit as u8);
    }
    store_runtime_string(caller, general_purpose::STANDARD.encode(bytes))
}

pub(crate) fn call_queue_microtask(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let callback = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    if !is_callable_in_runtime(caller, callback) {
        return make_type_error_exception(caller, "queueMicrotask callback must be a function");
    }
    let mut queue = caller
        .data()
        .microtask_queue
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    queue.push_back(Microtask::MicrotaskCallback { callback });
    value::encode_undefined()
}

pub(crate) fn call_performance_now(caller: &mut Caller<'_, RuntimeState>) -> i64 {
    let elapsed = caller.data().performance_origin.elapsed().as_secs_f64() * 1000.0;
    value::encode_f64(elapsed)
}

fn text_encoder_encode_into(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let src = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let dest = args.get(1).copied().unwrap_or_else(value::encode_undefined);
    let Some(entry) = typedarray_entry_from_value(caller, dest) else {
        return make_type_error_exception(
            caller,
            "TextEncoder.encodeInto destination must be a Uint8Array",
        );
    };
    let source = js_string_value(caller, src);
    let bytes = source.to_utf8_lossy().into_bytes();
    let count = bytes.len().min(entry.length as usize);
    write_entry_bytes(caller, &entry, 0, &bytes[..count]);
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let result = alloc_host_object(caller, &env, 2);
    define_global(
        caller,
        result,
        "read",
        value::encode_f64(source.utf16_len() as f64),
    );
    define_global(caller, result, "written", value::encode_f64(count as f64));
    result
}

fn text_decoder_decode(caller: &mut Caller<'_, RuntimeState>, this_val: i64, args: &[i64]) -> i64 {
    let Some(ptr) = value::is_object(this_val)
        .then(|| resolve_handle(caller, this_val))
        .flatten()
    else {
        return make_type_error_exception(
            caller,
            "TextDecoder.decode called on incompatible receiver",
        );
    };
    let encoding_name = read_object_property_by_name(caller, ptr, "encoding")
        .map(|v| js_string_lossy(caller, v))
        .unwrap_or_else(|| "utf-8".to_string());
    let fatal = read_object_property_by_name(caller, ptr, "fatal")
        .is_some_and(|v| value::is_bool(v) && value::decode_bool(v));
    let Some(encoding) = Encoding::for_label(encoding_name.as_bytes()) else {
        return make_type_error_exception(caller, "TextDecoder has invalid encoding state");
    };
    let input = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let bytes = if value::is_undefined(input) {
        Vec::new()
    } else if let Some(bytes) = visible_bytes(caller, input) {
        bytes
    } else if let Some((handle, len)) = arraybuffer_handle(caller, input) {
        let table = caller
            .data()
            .arraybuffer_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        table
            .get(handle as usize)
            .and_then(|entry| entry.data.get(..len as usize).map(|slice| slice.to_vec()))
            .unwrap_or_default()
    } else {
        return make_type_error_exception(
            caller,
            "TextDecoder.decode input must be ArrayBuffer or ArrayBufferView",
        );
    };
    if let Ok(valid_utf8) = std::str::from_utf8(&bytes) {
        return store_runtime_string(caller, valid_utf8.to_string());
    }
    let (decoded, _, had_errors) = encoding.decode(&bytes);
    if fatal && had_errors {
        return make_type_error_exception(
            caller,
            &format!("The encoded data was not valid for encoding {encoding_name}"),
        );
    }
    store_runtime_string(caller, decoded.into_owned())
}

fn arraybuffer_handle(caller: &mut Caller<'_, RuntimeState>, value_raw: i64) -> Option<(u32, u32)> {
    if !value::is_object(value_raw) {
        return None;
    }
    let ptr = resolve_handle(caller, value_raw)?;
    let handle = read_object_property_by_name(caller, ptr, "__arraybuffer_handle__")?;
    let byte_length = read_object_property_by_name(caller, ptr, "byteLength")?;
    Some((
        value::decode_f64(handle) as u32,
        value::decode_f64(byte_length) as u32,
    ))
}

fn option_bool(caller: &mut Caller<'_, RuntimeState>, options: i64, name: &str) -> bool {
    if !value::is_object(options) {
        return false;
    }
    let Some(ptr) = resolve_handle(caller, options) else {
        return false;
    };
    read_object_property_by_name(caller, ptr, name).is_some_and(|v| to_boolean(caller, v))
}

fn install_native(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
    name: &str,
    callable: NativeCallable,
) {
    let value_raw = alloc_native_callable(caller, callable);
    define_global(caller, obj, name, value_raw);
}

fn alloc_native_callable(caller: &mut Caller<'_, RuntimeState>, callable: NativeCallable) -> i64 {
    let mut table = caller
        .data()
        .native_callables
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let idx = table.len() as u32;
    table.push(callable);
    value::encode_native_callable_idx(idx)
}

fn define_global(caller: &mut Caller<'_, RuntimeState>, obj: i64, name: &str, value_raw: i64) {
    let _ = define_host_data_property_from_caller(caller, obj, name, value_raw);
}
