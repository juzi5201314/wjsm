use std::cell::RefCell;
use std::rc::Rc;

use num_traits::ToPrimitive;
use wjsm_ir::{Builtin, value};
use wjsm_native_abi::NativeVmContext;

use super::runtime::{fail_dispatch, to_number};
use crate::NativeAgentState;

#[derive(Clone)]
pub(crate) struct NativeArrayBuffer {
    pub(crate) bytes: Rc<RefCell<Vec<u8>>>,
}

#[derive(Clone)]
pub(crate) struct NativeDataView {
    pub(crate) buffer: u32,
    pub(crate) shared: Option<std::sync::Arc<std::sync::Mutex<Vec<u8>>>>,
    pub(crate) offset: usize,
    pub(crate) length: usize,
}

pub(super) fn dispatch_buffer(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    builtin: Builtin,
    args: &[i64],
) -> Option<i64> {
    Some(match builtin {
        Builtin::ArrayBufferConstructor => array_buffer_constructor(ctx, state, args),
        Builtin::ArrayBufferProtoByteLength => array_buffer_byte_length(ctx, state, args),
        Builtin::ArrayBufferProtoSlice => array_buffer_slice(ctx, state, args),
        Builtin::DataViewConstructor => data_view_constructor(ctx, state, args),
        Builtin::DataViewProtoGetFloat64 => data_view_get(ctx, state, args, ViewType::Float64),
        Builtin::DataViewProtoGetFloat32 => data_view_get(ctx, state, args, ViewType::Float32),
        Builtin::DataViewProtoGetInt32 => data_view_get(ctx, state, args, ViewType::Int32),
        Builtin::DataViewProtoGetUint32 => data_view_get(ctx, state, args, ViewType::Uint32),
        Builtin::DataViewProtoGetInt16 => data_view_get(ctx, state, args, ViewType::Int16),
        Builtin::DataViewProtoGetUint16 => data_view_get(ctx, state, args, ViewType::Uint16),
        Builtin::DataViewProtoGetInt8 => data_view_get(ctx, state, args, ViewType::Int8),
        Builtin::DataViewProtoGetUint8 => data_view_get(ctx, state, args, ViewType::Uint8),
        Builtin::DataViewProtoSetFloat64 => data_view_set(ctx, state, args, ViewType::Float64),
        Builtin::DataViewProtoSetFloat32 => data_view_set(ctx, state, args, ViewType::Float32),
        Builtin::DataViewProtoSetInt32 => data_view_set(ctx, state, args, ViewType::Int32),
        Builtin::DataViewProtoSetUint32 => data_view_set(ctx, state, args, ViewType::Uint32),
        Builtin::DataViewProtoSetInt16 => data_view_set(ctx, state, args, ViewType::Int16),
        Builtin::DataViewProtoSetUint16 => data_view_set(ctx, state, args, ViewType::Uint16),
        Builtin::DataViewProtoSetInt8 => data_view_set(ctx, state, args, ViewType::Int8),
        Builtin::DataViewProtoSetUint8 => data_view_set(ctx, state, args, ViewType::Uint8),
        _ => return None,
    })
}

pub(crate) fn buffer_builtin(
    state: &NativeAgentState,
    receiver: i64,
    key: &str,
) -> Option<Builtin> {
    let handle = value::decode_handle(receiver);
    if state.array_buffers.contains_key(&handle) {
        return Some(match key {
            "byteLength" => Builtin::ArrayBufferProtoByteLength,
            "slice" => Builtin::ArrayBufferProtoSlice,
            _ => return None,
        });
    }
    if state.data_views.contains_key(&handle) {
        return Some(match key {
            "getFloat32" => Builtin::DataViewProtoGetFloat32,
            "getFloat64" => Builtin::DataViewProtoGetFloat64,
            "getInt8" => Builtin::DataViewProtoGetInt8,
            "getInt16" => Builtin::DataViewProtoGetInt16,
            "getInt32" => Builtin::DataViewProtoGetInt32,
            "getUint8" => Builtin::DataViewProtoGetUint8,
            "getUint16" => Builtin::DataViewProtoGetUint16,
            "getUint32" => Builtin::DataViewProtoGetUint32,
            "setFloat32" => Builtin::DataViewProtoSetFloat32,
            "setFloat64" => Builtin::DataViewProtoSetFloat64,
            "setInt8" => Builtin::DataViewProtoSetInt8,
            "setInt16" => Builtin::DataViewProtoSetInt16,
            "setInt32" => Builtin::DataViewProtoSetInt32,
            "setUint8" => Builtin::DataViewProtoSetUint8,
            "setUint16" => Builtin::DataViewProtoSetUint16,
            "setUint32" => Builtin::DataViewProtoSetUint32,
            _ => return None,
        });
    }
    None
}

pub(crate) fn allocate_array_buffer(state: &mut NativeAgentState, length: usize) -> Option<i64> {
    let object = state.allocate_object(1, false).ok()?;
    state.array_buffers.insert(
        value::decode_handle(object),
        NativeArrayBuffer {
            bytes: Rc::new(RefCell::new(vec![0; length])),
        },
    );
    Some(object)
}

fn array_buffer_constructor(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let Some(length) = args
        .first()
        .and_then(|encoded| to_number(state, *encoded))
        .and_then(|number| number.to_usize())
    else {
        return fail_dispatch(ctx);
    };
    allocate_array_buffer(state, length).unwrap_or_else(|| fail_dispatch(ctx))
}

fn array_buffer_byte_length(
    ctx: &mut NativeVmContext,
    state: &NativeAgentState,
    args: &[i64],
) -> i64 {
    args.first()
        .and_then(|object| state.array_buffers.get(&value::decode_handle(*object)))
        .and_then(|buffer| u32::try_from(buffer.bytes.borrow().len()).ok())
        .map(|length| value::encode_f64(f64::from(length)))
        .unwrap_or_else(|| fail_dispatch(ctx))
}

fn array_buffer_slice(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let Some(receiver) = args.first().copied() else {
        return fail_dispatch(ctx);
    };
    let Some(buffer) = state
        .array_buffers
        .get(&value::decode_handle(receiver))
        .cloned()
    else {
        return fail_dispatch(ctx);
    };
    let length = buffer.bytes.borrow().len();
    let start = relative_index(state, args.get(1).copied(), length);
    let end = args.get(2).map_or(length, |encoded| {
        relative_index(state, Some(*encoded), length)
    });
    let bytes = buffer.bytes.borrow()[start.min(end)..end.min(length)].to_vec();
    let Ok(object) = state.allocate_object_with_gc_retry(ctx, 1, false) else {
        return fail_dispatch(ctx);
    };
    state.array_buffers.insert(
        value::decode_handle(object),
        NativeArrayBuffer {
            bytes: Rc::new(RefCell::new(bytes)),
        },
    );
    object
}

fn data_view_constructor(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let Some(buffer) = args.first().copied() else {
        return fail_dispatch(ctx);
    };
    let buffer_handle = value::decode_handle(buffer);
    let shared = state
        .shared_array_buffers
        .get(&buffer_handle)
        .map(|sab| sab.backing.bytes.clone());
    let total_length = if let Some(shared) = &shared {
        shared.lock().map(|bytes| bytes.len()).unwrap_or(0)
    } else {
        let Some(array_buffer) = state.array_buffers.get(&buffer_handle) else {
            return fail_dispatch(ctx);
        };
        array_buffer.bytes.borrow().len()
    };
    let offset = relative_index(state, args.get(1).copied(), total_length);
    let length = args
        .get(2)
        .and_then(|encoded| to_number(state, *encoded))
        .and_then(|number| number.to_usize())
        .unwrap_or_else(|| total_length.saturating_sub(offset));
    if offset.saturating_add(length) > total_length {
        return fail_dispatch(ctx);
    }
    let Ok(object) = state.allocate_object_with_gc_retry(ctx, 1, false) else {
        return fail_dispatch(ctx);
    };
    state.data_views.insert(
        value::decode_handle(object),
        NativeDataView {
            buffer: buffer_handle,
            shared,
            offset,
            length,
        },
    );
    object
}

#[derive(Clone, Copy)]
enum ViewType {
    Int8,
    Uint8,
    Int16,
    Uint16,
    Int32,
    Uint32,
    Float32,
    Float64,
}

impl ViewType {
    fn size(self) -> usize {
        match self {
            Self::Int8 | Self::Uint8 => 1,
            Self::Int16 | Self::Uint16 => 2,
            Self::Int32 | Self::Uint32 | Self::Float32 => 4,
            Self::Float64 => 8,
        }
    }
}

fn data_view_get(
    ctx: &mut NativeVmContext,
    state: &NativeAgentState,
    args: &[i64],
    kind: ViewType,
) -> i64 {
    let Some(view) = args
        .first()
        .and_then(|object| state.data_views.get(&value::decode_handle(*object)))
        .cloned()
    else {
        return fail_dispatch(ctx);
    };
    let Some(index) = args
        .get(1)
        .and_then(|encoded| to_number(state, *encoded))
        .and_then(|number| number.to_usize())
    else {
        return fail_dispatch(ctx);
    };
    let start = view.offset.saturating_add(index);
    if index.saturating_add(kind.size()) > view.length {
        return fail_dispatch(ctx);
    }
    let little_endian = args
        .get(2)
        .is_some_and(|encoded| value::is_bool(*encoded) && value::decode_bool(*encoded));
    let value = if let Some(shared) = &view.shared {
        let bytes = shared
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(raw) = bytes.get(start..start + kind.size()) else {
            return fail_dispatch(ctx);
        };
        decode(raw, kind, little_endian)
    } else {
        let Some(buffer) = state.array_buffers.get(&view.buffer) else {
            return fail_dispatch(ctx);
        };
        let bytes = buffer.bytes.borrow();
        let Some(raw) = bytes.get(start..start + kind.size()) else {
            return fail_dispatch(ctx);
        };
        decode(raw, kind, little_endian)
    };
    value::encode_f64(value)
}

fn data_view_set(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
    kind: ViewType,
) -> i64 {
    let [receiver, index, stored, ..] = args else {
        return fail_dispatch(ctx);
    };
    let Some(view) = state
        .data_views
        .get(&value::decode_handle(*receiver))
        .cloned()
    else {
        return fail_dispatch(ctx);
    };
    let Some(index) = to_number(state, *index).and_then(|number| number.to_usize()) else {
        return fail_dispatch(ctx);
    };
    if index.saturating_add(kind.size()) > view.length {
        return fail_dispatch(ctx);
    }
    let Some(number) = to_number(state, *stored) else {
        return fail_dispatch(ctx);
    };
    let little_endian = args
        .get(3)
        .is_some_and(|encoded| value::is_bool(*encoded) && value::decode_bool(*encoded));
    let start = view.offset + index;
    if let Some(shared) = &view.shared {
        let mut bytes = shared
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        encode(
            &mut bytes[start..start + kind.size()],
            kind,
            little_endian,
            number,
        );
    } else {
        let Some(buffer) = state.array_buffers.get(&view.buffer) else {
            return fail_dispatch(ctx);
        };
        let mut bytes = buffer.bytes.borrow_mut();
        encode(
            &mut bytes[start..start + kind.size()],
            kind,
            little_endian,
            number,
        );
    }
    value::encode_undefined()
}

fn decode(bytes: &[u8], kind: ViewType, little_endian: bool) -> f64 {
    match kind {
        ViewType::Int8 => i8::from_ne_bytes([bytes[0]]) as f64,
        ViewType::Uint8 => f64::from(bytes[0]),
        ViewType::Int16 => i16::from_ne_bytes(order::<2>(bytes, little_endian)) as f64,
        ViewType::Uint16 => f64::from(u16::from_ne_bytes(order::<2>(bytes, little_endian))),
        ViewType::Int32 => i32::from_ne_bytes(order::<4>(bytes, little_endian)) as f64,
        ViewType::Uint32 => u32::from_ne_bytes(order::<4>(bytes, little_endian)) as f64,
        ViewType::Float32 => f32::from_ne_bytes(order::<4>(bytes, little_endian)) as f64,
        ViewType::Float64 => f64::from_ne_bytes(order::<8>(bytes, little_endian)),
    }
}

fn encode(bytes: &mut [u8], kind: ViewType, little_endian: bool, number: f64) {
    match kind {
        ViewType::Int8 => bytes.copy_from_slice(&(number as i8).to_ne_bytes()),
        ViewType::Uint8 => bytes.copy_from_slice(&(number as u8).to_ne_bytes()),
        ViewType::Int16 => write_bytes(bytes, (number as i16).to_ne_bytes(), little_endian),
        ViewType::Uint16 => write_bytes(bytes, (number as u16).to_ne_bytes(), little_endian),
        ViewType::Int32 => write_bytes(bytes, (number as i32).to_ne_bytes(), little_endian),
        ViewType::Uint32 => write_bytes(bytes, (number as u32).to_ne_bytes(), little_endian),
        ViewType::Float32 => write_bytes(bytes, (number as f32).to_ne_bytes(), little_endian),
        ViewType::Float64 => write_bytes(bytes, number.to_ne_bytes(), little_endian),
    }
}

fn order<const N: usize>(bytes: &[u8], little_endian: bool) -> [u8; N] {
    let mut ordered = [0; N];
    ordered.copy_from_slice(bytes);
    if little_endian != cfg!(target_endian = "little") {
        ordered.reverse();
    }
    ordered
}

fn write_bytes<const N: usize>(destination: &mut [u8], mut bytes: [u8; N], little_endian: bool) {
    if little_endian != cfg!(target_endian = "little") {
        bytes.reverse();
    }
    destination.copy_from_slice(&bytes);
}

fn relative_index(state: &NativeAgentState, encoded: Option<i64>, length: usize) -> usize {
    let number = encoded
        .and_then(|encoded| to_number(state, encoded))
        .and_then(|number| number.to_isize())
        .unwrap_or(0);
    if number < 0 {
        length.saturating_sub(number.unsigned_abs())
    } else {
        usize::try_from(number).unwrap_or(usize::MAX).min(length)
    }
}
pub(crate) enum ViewBacking {
    ArrayBuffer(i64),
    SharedArrayBuffer(i64),
}

pub(crate) fn array_buffer_bytes(state: &NativeAgentState, encoded: i64) -> Option<Vec<u8>> {
    state
        .array_buffers
        .get(&value::decode_handle(encoded))
        .map(|buffer| buffer.bytes.borrow().clone())
}

pub(crate) fn from_bytes(state: &mut NativeAgentState, bytes: Vec<u8>) -> Option<i64> {
    let object = state.allocate_object(1, false).ok()?;
    state.array_buffers.insert(
        value::decode_handle(object),
        NativeArrayBuffer {
            bytes: Rc::new(RefCell::new(bytes)),
        },
    );
    Some(object)
}

pub(crate) fn detach(state: &mut NativeAgentState, handle: u32) {
    if let Some(buffer) = state.array_buffers.get(&handle) {
        buffer.bytes.borrow_mut().clear();
    }
}

pub(crate) fn data_view_parts(
    state: &NativeAgentState,
    encoded: i64,
) -> Option<(ViewBacking, usize, usize)> {
    let handle = value::decode_handle(encoded);
    let view = state.data_views.get(&handle)?;
    let backing = if state.shared_array_buffers.contains_key(&view.buffer) {
        ViewBacking::SharedArrayBuffer(value::encode_object_handle(view.buffer))
    } else {
        ViewBacking::ArrayBuffer(value::encode_object_handle(view.buffer))
    };
    Some((backing, view.offset, view.length))
}

pub(crate) fn from_view(
    state: &mut NativeAgentState,
    backing: i64,
    offset: usize,
    length: usize,
) -> Option<i64> {
    let buffer_handle = value::decode_handle(backing);
    if let Some(buffer) = state.array_buffers.get(&buffer_handle).cloned() {
        if offset.checked_add(length)? > buffer.bytes.borrow().len() {
            return None;
        }
        let object = state.allocate_object(1, false).ok()?;
        state.data_views.insert(
            value::decode_handle(object),
            NativeDataView {
                buffer: buffer_handle,
                shared: None,
                offset,
                length,
            },
        );
        return Some(object);
    }
    let shared = state.shared_array_buffers.get(&buffer_handle)?.clone();
    if offset.checked_add(length)? > shared.backing.bytes.lock().ok()?.len() {
        return None;
    }
    let object = state.allocate_object(1, false).ok()?;
    state.data_views.insert(
        value::decode_handle(object),
        NativeDataView {
            buffer: buffer_handle,
            shared: Some(shared.backing.bytes),
            offset,
            length,
        },
    );
    Some(object)
}
