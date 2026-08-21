use std::cell::RefCell;
use std::rc::Rc;

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use num_traits::ToPrimitive;
use wjsm_ir::value;
use wjsm_native_abi::NativeVmContext;

use super::buffers::NativeArrayBuffer;
use super::modules;
use super::runtime::{fail_dispatch, render_value, to_number, to_string_coerced};
use super::typedarray::{NativeTypedArray, TypedArrayKind};
use crate::{NativeAgentState, NativeCallableKind};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum BufferStaticKind {
    Alloc,
    AllocUnsafe,
    ByteLength,
    Concat,
    From,
    IsBuffer,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum BufferMethodKind {
    Compare,
    Copy,
    Equals,
    Fill,
    Includes,
    IndexOf,
    ReadDoubleBe,
    ReadDoubleLe,
    ReadFloatBe,
    ReadFloatLe,
    ReadInt8,
    ReadInt16Be,
    ReadInt16Le,
    ReadInt32Be,
    ReadInt32Le,
    ReadUInt8,
    ReadUInt16Be,
    ReadUInt16Le,
    ReadUInt32Be,
    ReadUInt32Le,
    Slice,
    Subarray,
    ToJson,
    ToString,
    Write,
    WriteDoubleBe,
    WriteDoubleLe,
    WriteFloatBe,
    WriteFloatLe,
    WriteInt8,
    WriteInt16Be,
    WriteInt16Le,
    WriteInt32Be,
    WriteInt32Le,
    WriteUInt8,
    WriteUInt16Be,
    WriteUInt16Le,
    WriteUInt32Be,
    WriteUInt32Le,
}

#[derive(Clone)]
pub(crate) struct NativeBuffer {
    bytes: Rc<RefCell<Vec<u8>>>,
    pub(crate) array_buffer: i64,
    offset: usize,
    length: usize,
}

#[derive(Clone, Copy)]
enum Encoding {
    Ascii,
    Base64,
    Base64Url,
    Hex,
    Latin1,
    Utf8,
    Utf16Le,
}

#[derive(Clone, Copy)]
enum NumberAccess {
    F32Be,
    F32Le,
    F64Be,
    F64Le,
    I8,
    I16Be,
    I16Le,
    I32Be,
    I32Le,
    U8,
    U16Be,
    U16Le,
    U32Be,
    U32Le,
}

impl NumberAccess {
    fn size(self) -> usize {
        match self {
            Self::I8 | Self::U8 => 1,
            Self::I16Be | Self::I16Le | Self::U16Be | Self::U16Le => 2,
            Self::F32Be | Self::F32Le | Self::I32Be | Self::I32Le | Self::U32Be | Self::U32Le => 4,
            Self::F64Be | Self::F64Le => 8,
        }
    }
}

pub(crate) fn constructor_property(key: &str) -> Option<NativeCallableKind> {
    let kind = match key {
        "alloc" => BufferStaticKind::Alloc,
        "allocUnsafe" => BufferStaticKind::AllocUnsafe,
        "byteLength" => BufferStaticKind::ByteLength,
        "concat" => BufferStaticKind::Concat,
        "from" => BufferStaticKind::From,
        "isBuffer" => BufferStaticKind::IsBuffer,
        _ => return None,
    };
    Some(NativeCallableKind::BufferStatic(kind))
}

pub(crate) fn property(
    state: &NativeAgentState,
    receiver: i64,
    key: &str,
) -> Option<BufferProperty> {
    let buffer = state.buffers.get(&value::decode_handle(receiver))?;
    let property = match key {
        "buffer" => BufferProperty::Value(buffer.array_buffer),
        "byteLength" | "length" => BufferProperty::Value(value::encode_f64(buffer.length as f64)),
        "byteOffset" => BufferProperty::Value(value::encode_f64(buffer.offset as f64)),
        "compare" => BufferProperty::Method(BufferMethodKind::Compare),
        "copy" => BufferProperty::Method(BufferMethodKind::Copy),
        "equals" => BufferProperty::Method(BufferMethodKind::Equals),
        "fill" => BufferProperty::Method(BufferMethodKind::Fill),
        "includes" => BufferProperty::Method(BufferMethodKind::Includes),
        "indexOf" => BufferProperty::Method(BufferMethodKind::IndexOf),
        "readDoubleBE" => BufferProperty::Method(BufferMethodKind::ReadDoubleBe),
        "readDoubleLE" => BufferProperty::Method(BufferMethodKind::ReadDoubleLe),
        "readFloatBE" => BufferProperty::Method(BufferMethodKind::ReadFloatBe),
        "readFloatLE" => BufferProperty::Method(BufferMethodKind::ReadFloatLe),
        "readInt8" => BufferProperty::Method(BufferMethodKind::ReadInt8),
        "readInt16BE" => BufferProperty::Method(BufferMethodKind::ReadInt16Be),
        "readInt16LE" => BufferProperty::Method(BufferMethodKind::ReadInt16Le),
        "readInt32BE" => BufferProperty::Method(BufferMethodKind::ReadInt32Be),
        "readInt32LE" => BufferProperty::Method(BufferMethodKind::ReadInt32Le),
        "readUInt8" => BufferProperty::Method(BufferMethodKind::ReadUInt8),
        "readUInt16BE" => BufferProperty::Method(BufferMethodKind::ReadUInt16Be),
        "readUInt16LE" => BufferProperty::Method(BufferMethodKind::ReadUInt16Le),
        "readUInt32BE" => BufferProperty::Method(BufferMethodKind::ReadUInt32Be),
        "readUInt32LE" => BufferProperty::Method(BufferMethodKind::ReadUInt32Le),
        "slice" => BufferProperty::Method(BufferMethodKind::Slice),
        "subarray" => BufferProperty::Method(BufferMethodKind::Subarray),
        "toJSON" => BufferProperty::Method(BufferMethodKind::ToJson),
        "toString" => BufferProperty::Method(BufferMethodKind::ToString),
        "write" => BufferProperty::Method(BufferMethodKind::Write),
        "writeDoubleBE" => BufferProperty::Method(BufferMethodKind::WriteDoubleBe),
        "writeDoubleLE" => BufferProperty::Method(BufferMethodKind::WriteDoubleLe),
        "writeFloatBE" => BufferProperty::Method(BufferMethodKind::WriteFloatBe),
        "writeFloatLE" => BufferProperty::Method(BufferMethodKind::WriteFloatLe),
        "writeInt8" => BufferProperty::Method(BufferMethodKind::WriteInt8),
        "writeInt16BE" => BufferProperty::Method(BufferMethodKind::WriteInt16Be),
        "writeInt16LE" => BufferProperty::Method(BufferMethodKind::WriteInt16Le),
        "writeInt32BE" => BufferProperty::Method(BufferMethodKind::WriteInt32Be),
        "writeInt32LE" => BufferProperty::Method(BufferMethodKind::WriteInt32Le),
        "writeUInt8" => BufferProperty::Method(BufferMethodKind::WriteUInt8),
        "writeUInt16BE" => BufferProperty::Method(BufferMethodKind::WriteUInt16Be),
        "writeUInt16LE" => BufferProperty::Method(BufferMethodKind::WriteUInt16Le),
        "writeUInt32BE" => BufferProperty::Method(BufferMethodKind::WriteUInt32Be),
        "writeUInt32LE" => BufferProperty::Method(BufferMethodKind::WriteUInt32Le),
        _ => return None,
    };
    Some(property)
}

pub(crate) enum BufferProperty {
    Method(BufferMethodKind),
    Value(i64),
}

pub(crate) fn call_constructor(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    if args.first().is_some_and(|value| value::is_f64(*value)) {
        alloc(ctx, state, args)
    } else {
        from(ctx, state, args)
    }
}

pub(crate) fn call_static(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    kind: BufferStaticKind,
    args: &[i64],
) -> i64 {
    match kind {
        BufferStaticKind::Alloc | BufferStaticKind::AllocUnsafe => alloc(ctx, state, args),
        BufferStaticKind::ByteLength => byte_length(ctx, state, args),
        BufferStaticKind::Concat => concat(ctx, state, args),
        BufferStaticKind::From => from(ctx, state, args),
        BufferStaticKind::IsBuffer => value::encode_bool(
            args.first()
                .is_some_and(|input| state.buffers.contains_key(&value::decode_handle(*input))),
        ),
    }
}

/// 安装 `node:buffer` 使用的 host 桥（目前仅 `transcode`）。
pub(crate) fn ensure_bridge(state: &mut NativeAgentState) -> Option<i64> {
    if let Some(bridge) = state.node_buffer_bridge {
        return Some(bridge);
    }
    let bridge = state.allocate_object(1, false).ok()?;
    let callable = state.native_callable(NativeCallableKind::BufferTranscode)?;
    modules::set_named_property(state, bridge, "transcode", callable).ok()?;
    state.node_buffer_bridge = Some(bridge);
    Some(bridge)
}

/// `import { transcode } from 'node:buffer'`：受限编码集合上的再编码。
pub(crate) fn transcode(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let Some(source) = args.first().copied() else {
        return type_error(
            ctx,
            state,
            "The \"source\" argument must be an instance of Buffer or Uint8Array. Received undefined",
        );
    };
    let Some(bytes) = source_bytes(state, source) else {
        return type_error(
            ctx,
            state,
            "The \"source\" argument must be an instance of Buffer or Uint8Array. Received type that is not BufferSource",
        );
    };
    let from_enc = match args.get(1).copied() {
        Some(encoded) => match to_string_coerced(ctx, state, encoded) {
            Ok(label) => label,
            Err(exception) => return exception,
        },
        None => {
            return transcode_error(ctx, state);
        }
    };
    let to_enc = match args.get(2).copied() {
        Some(encoded) => match to_string_coerced(ctx, state, encoded) {
            Ok(label) => label,
            Err(exception) => return exception,
        },
        None => {
            return transcode_error(ctx, state);
        }
    };
    let Some(from) = transcode_encoding(&from_enc) else {
        return transcode_error(ctx, state);
    };
    let Some(to) = transcode_encoding(&to_enc) else {
        return transcode_error(ctx, state);
    };
    let text = decode_for_transcode(&bytes, from);
    let out = encode_for_transcode(&text, to);
    create(state, out).unwrap_or_else(|| fail_dispatch(ctx))
}

fn source_bytes(state: &NativeAgentState, encoded: i64) -> Option<Vec<u8>> {
    // 字符串/数字等标量的 handle payload 可能与对象槽位碰撞，必须先确认是对象。
    if !value::is_js_object(encoded) {
        return None;
    }
    if let Some(bytes) = bytes(state, encoded) {
        return Some(bytes);
    }
    let array = state.typed_arrays.get(&value::decode_handle(encoded))?;
    if array.kind != TypedArrayKind::Uint8 {
        return None;
    }
    typed_array_bytes(state, encoded)
}

#[derive(Clone, Copy)]
enum TranscodeEncoding {
    Ascii,
    Latin1,
    Utf8,
    Utf16Le,
}

fn transcode_encoding(label: &str) -> Option<TranscodeEncoding> {
    match label.to_ascii_lowercase().as_str() {
        "ascii" => Some(TranscodeEncoding::Ascii),
        "latin1" | "binary" => Some(TranscodeEncoding::Latin1),
        "utf8" | "utf-8" => Some(TranscodeEncoding::Utf8),
        "ucs2" | "ucs-2" | "utf16le" | "utf-16le" => Some(TranscodeEncoding::Utf16Le),
        _ => None,
    }
}

fn decode_for_transcode(bytes: &[u8], encoding: TranscodeEncoding) -> String {
    match encoding {
        TranscodeEncoding::Ascii => bytes
            .iter()
            .map(|byte| {
                if *byte < 0x80 {
                    char::from(*byte)
                } else {
                    '\u{FFFD}'
                }
            })
            .collect(),
        TranscodeEncoding::Latin1 => bytes.iter().map(|byte| char::from(*byte)).collect(),
        TranscodeEncoding::Utf8 => String::from_utf8_lossy(bytes).into_owned(),
        TranscodeEncoding::Utf16Le => {
            if bytes.len() % 2 == 1 {
                // 奇数长度：末字节无法组成完整 code unit，按 Node/ICU 替换处理。
                let mut units: Vec<u16> = bytes
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|pair| u16::from_le_bytes(*pair))
                    .collect();
                units.push(0xfffd);
                String::from_utf16_lossy(&units)
            } else {
                String::from_utf16_lossy(
                    &bytes
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|pair| u16::from_le_bytes(*pair))
                        .collect::<Vec<_>>(),
                )
            }
        }
    }
}

fn encode_for_transcode(text: &str, encoding: TranscodeEncoding) -> Vec<u8> {
    match encoding {
        TranscodeEncoding::Ascii => text
            .chars()
            .map(|character| {
                let code = character as u32;
                if code <= 0x7f { code as u8 } else { b'?' }
            })
            .collect(),
        TranscodeEncoding::Latin1 => text
            .chars()
            .map(|character| {
                let code = character as u32;
                if code <= 0xff { code as u8 } else { b'?' }
            })
            .collect(),
        TranscodeEncoding::Utf8 => text.as_bytes().to_vec(),
        TranscodeEncoding::Utf16Le => text.encode_utf16().flat_map(u16::to_le_bytes).collect(),
    }
}

fn transcode_error(ctx: &mut NativeVmContext, state: &mut NativeAgentState) -> i64 {
    modules::named_error_object(
        state,
        "Error",
        "Unable to transcode Buffer [U_ILLEGAL_ARGUMENT_ERROR]".to_owned(),
    )
    .and_then(|error| state.create_exception(error))
    .unwrap_or_else(|| fail_dispatch(ctx))
}

pub(crate) fn call_method(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    receiver: i64,
    kind: BufferMethodKind,
    args: &[i64],
) -> i64 {
    match kind {
        BufferMethodKind::Compare => compare(ctx, state, receiver, args),
        BufferMethodKind::Copy => copy(ctx, state, receiver, args),
        BufferMethodKind::Equals => equals(ctx, state, receiver, args),
        BufferMethodKind::Fill => fill(ctx, state, receiver, args),
        BufferMethodKind::Includes => index_of(ctx, state, receiver, args, true),
        BufferMethodKind::IndexOf => index_of(ctx, state, receiver, args, false),
        BufferMethodKind::ReadDoubleBe => {
            read_number(ctx, state, receiver, args, NumberAccess::F64Be)
        }
        BufferMethodKind::ReadDoubleLe => {
            read_number(ctx, state, receiver, args, NumberAccess::F64Le)
        }
        BufferMethodKind::ReadFloatBe => {
            read_number(ctx, state, receiver, args, NumberAccess::F32Be)
        }
        BufferMethodKind::ReadFloatLe => {
            read_number(ctx, state, receiver, args, NumberAccess::F32Le)
        }
        BufferMethodKind::ReadInt8 => read_number(ctx, state, receiver, args, NumberAccess::I8),
        BufferMethodKind::ReadInt16Be => {
            read_number(ctx, state, receiver, args, NumberAccess::I16Be)
        }
        BufferMethodKind::ReadInt16Le => {
            read_number(ctx, state, receiver, args, NumberAccess::I16Le)
        }
        BufferMethodKind::ReadInt32Be => {
            read_number(ctx, state, receiver, args, NumberAccess::I32Be)
        }
        BufferMethodKind::ReadInt32Le => {
            read_number(ctx, state, receiver, args, NumberAccess::I32Le)
        }
        BufferMethodKind::ReadUInt8 => read_number(ctx, state, receiver, args, NumberAccess::U8),
        BufferMethodKind::ReadUInt16Be => {
            read_number(ctx, state, receiver, args, NumberAccess::U16Be)
        }
        BufferMethodKind::ReadUInt16Le => {
            read_number(ctx, state, receiver, args, NumberAccess::U16Le)
        }
        BufferMethodKind::ReadUInt32Be => {
            read_number(ctx, state, receiver, args, NumberAccess::U32Be)
        }
        BufferMethodKind::ReadUInt32Le => {
            read_number(ctx, state, receiver, args, NumberAccess::U32Le)
        }
        BufferMethodKind::Slice | BufferMethodKind::Subarray => slice(ctx, state, receiver, args),
        BufferMethodKind::ToJson => to_json(ctx, state, receiver),
        BufferMethodKind::ToString => to_string(ctx, state, receiver, args),
        BufferMethodKind::Write => write_string(ctx, state, receiver, args),
        BufferMethodKind::WriteDoubleBe => {
            write_number(ctx, state, receiver, args, NumberAccess::F64Be)
        }
        BufferMethodKind::WriteDoubleLe => {
            write_number(ctx, state, receiver, args, NumberAccess::F64Le)
        }
        BufferMethodKind::WriteFloatBe => {
            write_number(ctx, state, receiver, args, NumberAccess::F32Be)
        }
        BufferMethodKind::WriteFloatLe => {
            write_number(ctx, state, receiver, args, NumberAccess::F32Le)
        }
        BufferMethodKind::WriteInt8 => write_number(ctx, state, receiver, args, NumberAccess::I8),
        BufferMethodKind::WriteInt16Be => {
            write_number(ctx, state, receiver, args, NumberAccess::I16Be)
        }
        BufferMethodKind::WriteInt16Le => {
            write_number(ctx, state, receiver, args, NumberAccess::I16Le)
        }
        BufferMethodKind::WriteInt32Be => {
            write_number(ctx, state, receiver, args, NumberAccess::I32Be)
        }
        BufferMethodKind::WriteInt32Le => {
            write_number(ctx, state, receiver, args, NumberAccess::I32Le)
        }
        BufferMethodKind::WriteUInt8 => write_number(ctx, state, receiver, args, NumberAccess::U8),
        BufferMethodKind::WriteUInt16Be => {
            write_number(ctx, state, receiver, args, NumberAccess::U16Be)
        }
        BufferMethodKind::WriteUInt16Le => {
            write_number(ctx, state, receiver, args, NumberAccess::U16Le)
        }
        BufferMethodKind::WriteUInt32Be => {
            write_number(ctx, state, receiver, args, NumberAccess::U32Be)
        }
        BufferMethodKind::WriteUInt32Le => {
            write_number(ctx, state, receiver, args, NumberAccess::U32Le)
        }
    }
}

fn alloc(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(length) = args
        .first()
        .and_then(|input| to_number(state, *input))
        .filter(|length| length.is_finite() && *length >= 0.0)
        .and_then(|length| length.trunc().to_usize())
    else {
        return range_error(ctx, state, "Invalid Buffer size");
    };
    let mut bytes = vec![0; length];
    if let Some(fill) = args
        .get(1)
        .copied()
        .filter(|fill| !value::is_undefined(*fill))
    {
        let encoding = encoding(state, args.get(2).copied()).unwrap_or(Encoding::Utf8);
        let pattern = value_bytes(state, fill, encoding).unwrap_or_else(|| vec![0]);
        repeat_fill(&mut bytes, &pattern);
    }
    create(state, bytes).unwrap_or_else(|| fail_dispatch(ctx))
}

fn from(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let input = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let encoding = match encoding(state, args.get(1).copied()) {
        Some(encoding) => encoding,
        None => return type_error(ctx, state, "Unknown encoding"),
    };
    let bytes = if value::is_string(input) {
        state
            .string_owned(input)
            .and_then(|text| text.to_utf8())
            .map(|text| encode_text(&text, encoding))
    } else if let Some(buffer) = state.buffers.get(&value::decode_handle(input)) {
        Some(visible(buffer))
    } else if let Some(buffer) = state.array_buffers.get(&value::decode_handle(input)) {
        Some(buffer.bytes.borrow().clone())
    } else if value::is_array(input) {
        array_bytes(state, input)
    } else if state
        .typed_arrays
        .contains_key(&value::decode_handle(input))
    {
        typed_array_bytes(state, input)
    } else {
        None
    };
    bytes
        .and_then(|bytes| create(state, bytes))
        .unwrap_or_else(|| type_error(ctx, state, "Invalid Buffer source"))
}

fn concat(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let list = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let Some(values) = array_values(state, list) else {
        return type_error(ctx, state, "Buffer.concat list must be an array");
    };
    let mut output = Vec::new();
    for item in values {
        let Some(buffer) = state.buffers.get(&value::decode_handle(item)) else {
            return type_error(ctx, state, "Buffer.concat list contains non-buffer value");
        };
        output.extend_from_slice(&visible(buffer));
    }
    if let Some(length) = args
        .get(1)
        .and_then(|input| to_number(state, *input))
        .and_then(|length| length.max(0.0).trunc().to_usize())
    {
        output.resize(length, 0);
        output.truncate(length);
    }
    create(state, output).unwrap_or_else(|| fail_dispatch(ctx))
}

fn byte_length(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let input = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let encoding = match encoding(state, args.get(1).copied()) {
        Some(encoding) => encoding,
        None => return type_error(ctx, state, "Unknown encoding"),
    };
    let length = if let Some(buffer) = state.buffers.get(&value::decode_handle(input)) {
        buffer.length
    } else if let Some(text) = state.string_owned(input).and_then(|text| text.to_utf8()) {
        encode_text(&text, encoding).len()
    } else {
        return type_error(ctx, state, "Invalid Buffer.byteLength input");
    };
    value::encode_f64(length as f64)
}

fn to_string(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    receiver: i64,
    args: &[i64],
) -> i64 {
    let Some(buffer) = state.buffers.get(&value::decode_handle(receiver)) else {
        return type_error(ctx, state, "Incompatible Buffer receiver");
    };
    let encoding = match encoding(state, args.first().copied()) {
        Some(encoding) => encoding,
        None => return type_error(ctx, state, "Unknown encoding"),
    };
    let bytes = visible(buffer);
    let start = offset(state, args.get(1).copied(), bytes.len(), 0);
    let end = offset(state, args.get(2).copied(), bytes.len(), bytes.len()).max(start);
    state
        .intern_text(decode_text(&bytes[start..end], encoding), value::TAG_STRING)
        .unwrap_or_else(|| fail_dispatch(ctx))
}

fn slice(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    receiver: i64,
    args: &[i64],
) -> i64 {
    let Some(source) = state.buffers.get(&value::decode_handle(receiver)).cloned() else {
        return type_error(ctx, state, "Incompatible Buffer receiver");
    };
    let start = relative_offset(state, args.first().copied(), source.length, 0);
    let end = relative_offset(state, args.get(1).copied(), source.length, source.length).max(start);
    create_view(
        state,
        source.bytes,
        source.array_buffer,
        source.offset + start,
        end - start,
    )
    .unwrap_or_else(|| fail_dispatch(ctx))
}

fn copy(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    receiver: i64,
    args: &[i64],
) -> i64 {
    let Some(source) = state
        .buffers
        .get(&value::decode_handle(receiver))
        .map(visible)
    else {
        return type_error(ctx, state, "Incompatible Buffer receiver");
    };
    let target_value = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let Some(target) = state
        .buffers
        .get(&value::decode_handle(target_value))
        .cloned()
    else {
        return type_error(ctx, state, "Buffer.copy target must be a Buffer");
    };
    let target_start = offset(state, args.get(1).copied(), target.length, 0);
    let source_start = offset(state, args.get(2).copied(), source.len(), 0);
    let source_end =
        offset(state, args.get(3).copied(), source.len(), source.len()).max(source_start);
    let count = (source_end - source_start).min(target.length - target_start);
    target.bytes.borrow_mut()[target.offset + target_start..target.offset + target_start + count]
        .copy_from_slice(&source[source_start..source_start + count]);
    value::encode_f64(count as f64)
}

fn compare(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    receiver: i64,
    args: &[i64],
) -> i64 {
    let Some(left) = state
        .buffers
        .get(&value::decode_handle(receiver))
        .map(visible)
    else {
        return type_error(ctx, state, "Incompatible Buffer receiver");
    };
    let other = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let Some(right) = state.buffers.get(&value::decode_handle(other)).map(visible) else {
        return type_error(ctx, state, "Buffer.compare target must be a Buffer");
    };
    value::encode_f64(match left.cmp(&right) {
        std::cmp::Ordering::Less => -1.0,
        std::cmp::Ordering::Equal => 0.0,
        std::cmp::Ordering::Greater => 1.0,
    })
}

fn equals(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    receiver: i64,
    args: &[i64],
) -> i64 {
    let Some(left) = state
        .buffers
        .get(&value::decode_handle(receiver))
        .map(visible)
    else {
        return type_error(ctx, state, "Incompatible Buffer receiver");
    };
    let other = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let Some(right) = state.buffers.get(&value::decode_handle(other)).map(visible) else {
        return type_error(ctx, state, "Buffer.equals target must be a Buffer");
    };
    value::encode_bool(left == right)
}

fn fill(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    receiver: i64,
    args: &[i64],
) -> i64 {
    let Some(buffer) = state.buffers.get(&value::decode_handle(receiver)).cloned() else {
        return type_error(ctx, state, "Incompatible Buffer receiver");
    };
    let fill = args
        .first()
        .copied()
        .unwrap_or_else(|| value::encode_f64(0.0));
    let start = offset(state, args.get(1).copied(), buffer.length, 0);
    let end = offset(state, args.get(2).copied(), buffer.length, buffer.length).max(start);
    let encoding_arg = args.get(3).copied().or_else(|| {
        args.get(1)
            .copied()
            .filter(|argument| value::is_string(*argument))
    });
    let encoding = encoding(state, encoding_arg).unwrap_or(Encoding::Utf8);
    let pattern = value_bytes(state, fill, encoding).unwrap_or_else(|| vec![0]);
    let mut bytes = buffer.bytes.borrow_mut();
    for index in start..end {
        bytes[buffer.offset + index] = pattern[(index - start) % pattern.len()];
    }
    receiver
}

fn index_of(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    receiver: i64,
    args: &[i64],
    as_bool: bool,
) -> i64 {
    let Some(bytes) = state
        .buffers
        .get(&value::decode_handle(receiver))
        .map(visible)
    else {
        return type_error(ctx, state, "Incompatible Buffer receiver");
    };
    let needle_value = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let needle = if value::is_f64(needle_value) {
        vec![to_number(state, needle_value).unwrap_or(0.0) as u8]
    } else {
        value_bytes(state, needle_value, Encoding::Utf8).unwrap_or_default()
    };
    let start = offset(state, args.get(1).copied(), bytes.len(), 0);
    let found = if needle.is_empty() {
        Some(start)
    } else {
        bytes[start..]
            .windows(needle.len())
            .position(|window| window == needle)
            .map(|index| index + start)
    };
    if as_bool {
        value::encode_bool(found.is_some())
    } else {
        value::encode_f64(found.map_or(-1.0, |index| index as f64))
    }
}

fn write_string(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    receiver: i64,
    args: &[i64],
) -> i64 {
    let Some(buffer) = state.buffers.get(&value::decode_handle(receiver)).cloned() else {
        return type_error(ctx, state, "Incompatible Buffer receiver");
    };
    let text = args
        .first()
        .and_then(|input| state.string_owned(*input))
        .and_then(|text| text.to_utf8())
        .unwrap_or_else(|| {
            render_value(
                state,
                args.first()
                    .copied()
                    .unwrap_or_else(value::encode_undefined),
            )
        });
    let start = offset(state, args.get(1).copied(), buffer.length, 0);
    let requested = args
        .get(2)
        .filter(|argument| !value::is_string(**argument))
        .and_then(|argument| to_number(state, *argument))
        .and_then(|length| length.max(0.0).trunc().to_usize())
        .unwrap_or_else(|| buffer.length - start)
        .min(buffer.length - start);
    let encoding_arg = args
        .get(3)
        .copied()
        .or_else(|| {
            args.get(2)
                .copied()
                .filter(|argument| value::is_string(*argument))
        })
        .or_else(|| {
            args.get(1)
                .copied()
                .filter(|argument| value::is_string(*argument))
        });
    let Some(encoding) = encoding(state, encoding_arg) else {
        return type_error(ctx, state, "Unknown encoding");
    };
    let encoded = encode_text(&text, encoding);
    let count = requested.min(encoded.len());
    buffer.bytes.borrow_mut()[buffer.offset + start..buffer.offset + start + count]
        .copy_from_slice(&encoded[..count]);
    value::encode_f64(count as f64)
}

fn read_number(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    receiver: i64,
    args: &[i64],
    access: NumberAccess,
) -> i64 {
    let Some(buffer) = state.buffers.get(&value::decode_handle(receiver)) else {
        return type_error(ctx, state, "Incompatible Buffer receiver");
    };
    let offset = offset(state, args.first().copied(), buffer.length, 0);
    let size = access.size();
    if offset
        .checked_add(size)
        .is_none_or(|end| end > buffer.length)
    {
        return range_error(ctx, state, "Index out of range");
    }
    let bytes = buffer.bytes.borrow();
    let bytes = &bytes[buffer.offset + offset..buffer.offset + offset + size];
    let number = match access {
        NumberAccess::U8 => bytes[0] as f64,
        NumberAccess::I8 => bytes[0] as i8 as f64,
        NumberAccess::U16Be => u16::from_be_bytes([bytes[0], bytes[1]]) as f64,
        NumberAccess::U16Le => u16::from_le_bytes([bytes[0], bytes[1]]) as f64,
        NumberAccess::I16Be => i16::from_be_bytes([bytes[0], bytes[1]]) as f64,
        NumberAccess::I16Le => i16::from_le_bytes([bytes[0], bytes[1]]) as f64,
        NumberAccess::U32Be => u32::from_be_bytes(bytes.try_into().expect("four bytes")) as f64,
        NumberAccess::U32Le => u32::from_le_bytes(bytes.try_into().expect("four bytes")) as f64,
        NumberAccess::I32Be => i32::from_be_bytes(bytes.try_into().expect("four bytes")) as f64,
        NumberAccess::I32Le => i32::from_le_bytes(bytes.try_into().expect("four bytes")) as f64,
        NumberAccess::F32Be => f32::from_be_bytes(bytes.try_into().expect("four bytes")) as f64,
        NumberAccess::F32Le => f32::from_le_bytes(bytes.try_into().expect("four bytes")) as f64,
        NumberAccess::F64Be => f64::from_be_bytes(bytes.try_into().expect("eight bytes")),
        NumberAccess::F64Le => f64::from_le_bytes(bytes.try_into().expect("eight bytes")),
    };
    value::encode_f64(number)
}

fn write_number(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    receiver: i64,
    args: &[i64],
    access: NumberAccess,
) -> i64 {
    let Some(buffer) = state.buffers.get(&value::decode_handle(receiver)).cloned() else {
        return type_error(ctx, state, "Incompatible Buffer receiver");
    };
    let number = args
        .first()
        .and_then(|input| to_number(state, *input))
        .unwrap_or(0.0);
    let offset = offset(state, args.get(1).copied(), buffer.length, 0);
    let size = access.size();
    if offset
        .checked_add(size)
        .is_none_or(|end| end > buffer.length)
    {
        return range_error(ctx, state, "Index out of range");
    }
    let encoded = match access {
        NumberAccess::U8 => vec![number as u8],
        NumberAccess::I8 => vec![number as i8 as u8],
        NumberAccess::U16Be => (number as u16).to_be_bytes().to_vec(),
        NumberAccess::U16Le => (number as u16).to_le_bytes().to_vec(),
        NumberAccess::I16Be => (number as i16).to_be_bytes().to_vec(),
        NumberAccess::I16Le => (number as i16).to_le_bytes().to_vec(),
        NumberAccess::U32Be => (number as u32).to_be_bytes().to_vec(),
        NumberAccess::U32Le => (number as u32).to_le_bytes().to_vec(),
        NumberAccess::I32Be => (number as i32).to_be_bytes().to_vec(),
        NumberAccess::I32Le => (number as i32).to_le_bytes().to_vec(),
        NumberAccess::F32Be => (number as f32).to_be_bytes().to_vec(),
        NumberAccess::F32Le => (number as f32).to_le_bytes().to_vec(),
        NumberAccess::F64Be => number.to_be_bytes().to_vec(),
        NumberAccess::F64Le => number.to_le_bytes().to_vec(),
    };
    buffer.bytes.borrow_mut()[buffer.offset + offset..buffer.offset + offset + size]
        .copy_from_slice(&encoded);
    value::encode_f64((offset + size) as f64)
}

fn to_json(ctx: &mut NativeVmContext, state: &mut NativeAgentState, receiver: i64) -> i64 {
    let Some(bytes) = state
        .buffers
        .get(&value::decode_handle(receiver))
        .map(visible)
    else {
        return type_error(ctx, state, "Incompatible Buffer receiver");
    };
    let values: Vec<_> = bytes
        .into_iter()
        .map(|byte| value::encode_f64(f64::from(byte)))
        .collect();
    let Ok(data) = state.allocate_array_values(&values) else {
        return fail_dispatch(ctx);
    };
    let Ok(object) = state.allocate_object(2, false) else {
        return fail_dispatch(ctx);
    };
    let Some(kind) = state.intern_text("Buffer".into(), value::TAG_STRING) else {
        return fail_dispatch(ctx);
    };
    if set_property(state, object, "type", kind).is_none()
        || set_property(state, object, "data", data).is_none()
    {
        return fail_dispatch(ctx);
    }
    object
}

fn create(state: &mut NativeAgentState, bytes: Vec<u8>) -> Option<i64> {
    let bytes = Rc::new(RefCell::new(bytes));
    let array_buffer = state.allocate_object(1, false).ok()?;
    state.array_buffers.insert(
        value::decode_handle(array_buffer),
        NativeArrayBuffer {
            bytes: Rc::clone(&bytes),
        },
    );
    let length = bytes.borrow().len();
    create_view(state, bytes, array_buffer, 0, length)
}

fn create_view(
    state: &mut NativeAgentState,
    bytes: Rc<RefCell<Vec<u8>>>,
    array_buffer: i64,
    offset: usize,
    length: usize,
) -> Option<i64> {
    let object = state.allocate_object(4, false).ok()?;
    let handle = value::decode_handle(object);
    state.typed_arrays.insert(
        handle,
        NativeTypedArray {
            kind: TypedArrayKind::Uint8,
            storage: None,
            buffer: Some(Rc::clone(&bytes)),
            buffer_object: Some(array_buffer),
            shared_buffer: None,
            shared_backing_id: None,
            is_shared: false,
            offset,
            length,
        },
    );
    state.buffers.insert(
        handle,
        NativeBuffer {
            bytes,
            array_buffer,
            offset,
            length,
        },
    );
    Some(object)
}

pub(crate) fn bytes(state: &NativeAgentState, encoded: i64) -> Option<Vec<u8>> {
    state
        .buffers
        .get(&value::decode_handle(encoded))
        .map(visible)
}

pub(crate) fn from_bytes(state: &mut NativeAgentState, bytes: Vec<u8>) -> Option<i64> {
    create(state, bytes)
}

pub(crate) fn parts(state: &NativeAgentState, encoded: i64) -> Option<(i64, usize, usize)> {
    state
        .buffers
        .get(&value::decode_handle(encoded))
        .map(|buffer| (buffer.array_buffer, buffer.offset, buffer.length))
}

pub(crate) fn from_array_buffer_view(
    state: &mut NativeAgentState,
    array_buffer: i64,
    offset: usize,
    length: usize,
) -> Option<i64> {
    let buffer = state
        .array_buffers
        .get(&value::decode_handle(array_buffer))
        .cloned()?;
    if offset.checked_add(length)? > buffer.bytes.borrow().len() {
        return None;
    }
    create_view(state, buffer.bytes, array_buffer, offset, length)
}

fn visible(buffer: &NativeBuffer) -> Vec<u8> {
    buffer.bytes.borrow()[buffer.offset..buffer.offset + buffer.length].to_vec()
}

fn typed_array_bytes(state: &NativeAgentState, encoded: i64) -> Option<Vec<u8>> {
    let array = state.typed_arrays.get(&value::decode_handle(encoded))?;
    (0..array.length)
        .map(|index| super::typedarray::get_element(state, encoded, index))
        .map(|element| element.map(|element| to_number(state, element).unwrap_or(0.0) as u8))
        .collect()
}

fn array_values(state: &NativeAgentState, encoded: i64) -> Option<Vec<i64>> {
    let handle = value::decode_handle(encoded);
    let length = state.gc.heap().array_length(handle).ok()?;
    (0..length)
        .map(|index| {
            state
                .gc
                .heap()
                .get_element(handle, index)
                .ok()
                .flatten()
                .map(|element| element as i64)
        })
        .collect()
}

fn array_bytes(state: &NativeAgentState, encoded: i64) -> Option<Vec<u8>> {
    array_values(state, encoded).map(|values| {
        values
            .into_iter()
            .map(|element| to_number(state, element).unwrap_or(0.0) as i64 as u8)
            .collect()
    })
}

fn value_bytes(state: &mut NativeAgentState, encoded: i64, encoding: Encoding) -> Option<Vec<u8>> {
    if let Some(buffer) = state.buffers.get(&value::decode_handle(encoded)) {
        return Some(visible(buffer));
    }
    if value::is_string(encoded) {
        return state
            .string_owned(encoded)
            .and_then(|text| text.to_utf8())
            .map(|text| encode_text(&text, encoding));
    }
    if value::is_f64(encoded) {
        return Some(vec![to_number(state, encoded).unwrap_or(0.0) as u8]);
    }
    None
}

fn encoding(state: &NativeAgentState, encoded: Option<i64>) -> Option<Encoding> {
    let Some(encoded) = encoded.filter(|encoded| !value::is_undefined(*encoded)) else {
        return Some(Encoding::Utf8);
    };
    let label = state.string_owned(encoded)?.to_utf8()?.to_ascii_lowercase();
    match label.as_str() {
        "ascii" => Some(Encoding::Ascii),
        "base64" => Some(Encoding::Base64),
        "base64url" => Some(Encoding::Base64Url),
        "binary" | "latin1" => Some(Encoding::Latin1),
        "hex" => Some(Encoding::Hex),
        "ucs2" | "ucs-2" | "utf16le" | "utf-16le" => Some(Encoding::Utf16Le),
        "utf8" | "utf-8" => Some(Encoding::Utf8),
        _ => None,
    }
}

fn encode_text(text: &str, encoding: Encoding) -> Vec<u8> {
    match encoding {
        Encoding::Ascii | Encoding::Latin1 => text
            .chars()
            .map(|character| character as u32 as u8)
            .collect(),
        Encoding::Base64 => STANDARD.decode(text.as_bytes()).unwrap_or_default(),
        Encoding::Base64Url => URL_SAFE_NO_PAD.decode(text.as_bytes()).unwrap_or_default(),
        Encoding::Hex => text
            .as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .map_while(|pair| {
                let high = (pair[0] as char).to_digit(16)?;
                let low = (pair[1] as char).to_digit(16)?;
                Some(((high << 4) | low) as u8)
            })
            .collect(),
        Encoding::Utf8 => text.as_bytes().to_vec(),
        Encoding::Utf16Le => text.encode_utf16().flat_map(u16::to_le_bytes).collect(),
    }
}

fn decode_text(bytes: &[u8], encoding: Encoding) -> String {
    match encoding {
        Encoding::Ascii => bytes.iter().map(|byte| char::from(byte & 0x7f)).collect(),
        Encoding::Base64 => STANDARD.encode(bytes),
        Encoding::Base64Url => URL_SAFE_NO_PAD.encode(bytes),
        Encoding::Hex => {
            const HEX: &[u8; 16] = b"0123456789abcdef";
            let mut output = String::with_capacity(bytes.len() * 2);
            for byte in bytes {
                output.push(char::from(HEX[usize::from(byte >> 4)]));
                output.push(char::from(HEX[usize::from(byte & 0xf)]));
            }
            output
        }
        Encoding::Latin1 => bytes.iter().map(|byte| char::from(*byte)).collect(),
        Encoding::Utf8 => String::from_utf8_lossy(bytes).into_owned(),
        Encoding::Utf16Le => String::from_utf16_lossy(
            &bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| u16::from_le_bytes(*pair))
                .collect::<Vec<_>>(),
        ),
    }
}

fn repeat_fill(bytes: &mut [u8], pattern: &[u8]) {
    if pattern.is_empty() {
        return;
    }
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = pattern[index % pattern.len()];
    }
}

fn offset(state: &NativeAgentState, encoded: Option<i64>, length: usize, default: usize) -> usize {
    encoded
        .filter(|encoded| !value::is_undefined(*encoded))
        .and_then(|encoded| to_number(state, encoded))
        .map(|offset| offset.max(0.0).trunc() as usize)
        .unwrap_or(default)
        .min(length)
}

fn relative_offset(
    state: &NativeAgentState,
    encoded: Option<i64>,
    length: usize,
    default: usize,
) -> usize {
    let Some(number) = encoded
        .filter(|encoded| !value::is_undefined(*encoded))
        .and_then(|encoded| to_number(state, encoded))
    else {
        return default;
    };
    if number.is_nan() {
        return 0;
    }
    if number < 0.0 {
        (length as f64 + number.trunc()).max(0.0) as usize
    } else {
        number.trunc().min(length as f64) as usize
    }
}

fn set_property(state: &mut NativeAgentState, object: i64, name: &str, stored: i64) -> Option<()> {
    let key = state.intern_property_string(name.into())?;
    state
        .gc
        .heap()
        .set_property(value::decode_handle(object), key, stored as u64)
        .ok()
}

fn type_error(ctx: &mut NativeVmContext, state: &mut NativeAgentState, message: &str) -> i64 {
    exception(ctx, state, "TypeError", message)
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
    super::modules::named_error_object(state, name, message.into())
        .and_then(|error| state.create_exception(error))
        .unwrap_or_else(|| fail_dispatch(ctx))
}
