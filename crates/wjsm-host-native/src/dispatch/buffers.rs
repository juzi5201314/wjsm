use std::cell::RefCell;
use std::rc::Rc;

use num_bigint::{BigInt, Sign};
use num_traits::ToPrimitive;
use wjsm_ir::{Builtin, value};
use wjsm_native_abi::NativeVmContext;

use super::runtime::{fail_dispatch, to_number, type_error};
use crate::{BUILTIN_PROTOTYPE_PROPERTY_FLAGS, NativeAgentState, NativeCallableKind};

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
        Builtin::DataViewProtoGetBigInt64 => data_view_get_bigint(ctx, state, args, true),
        Builtin::DataViewProtoGetBigUint64 => data_view_get_bigint(ctx, state, args, false),
        Builtin::DataViewProtoSetBigInt64 | Builtin::DataViewProtoSetBigUint64 => {
            data_view_set_bigint(ctx, state, args)
        }
        Builtin::DataViewProtoBuffer
        | Builtin::DataViewProtoByteLength
        | Builtin::DataViewProtoByteOffset => data_view_accessor(ctx, state, builtin, args),
        _ => return None,
    })
}

/// `get DataView.prototype.buffer` / `byteLength` / `byteOffset`
/// （§25.3.4.1–3）：receiver 必须携带 [[DataView]] 品牌（side table 有条目）。
fn data_view_accessor(
    ctx: &mut NativeVmContext,
    state: &NativeAgentState,
    builtin: Builtin,
    args: &[i64],
) -> i64 {
    let Some(view) = args
        .first()
        .and_then(|object| state.data_views.get(&value::decode_handle(*object)))
    else {
        return fail_dispatch(ctx);
    };
    match builtin {
        Builtin::DataViewProtoBuffer => value::encode_object_handle(view.buffer),
        Builtin::DataViewProtoByteLength => value::encode_f64(view.length as f64),
        Builtin::DataViewProtoByteOffset => value::encode_f64(view.offset as f64),
        _ => fail_dispatch(ctx),
    }
}

/// DataView.prototype 的方法名 → Builtin 映射，
/// `install_data_view_prototype_methods` 按此安装原型方法。
pub(crate) const DATA_VIEW_PROTO_METHODS: &[(&str, Builtin)] = &[
    ("getBigInt64", Builtin::DataViewProtoGetBigInt64),
    ("getBigUint64", Builtin::DataViewProtoGetBigUint64),
    ("getFloat32", Builtin::DataViewProtoGetFloat32),
    ("getFloat64", Builtin::DataViewProtoGetFloat64),
    ("getInt8", Builtin::DataViewProtoGetInt8),
    ("getInt16", Builtin::DataViewProtoGetInt16),
    ("getInt32", Builtin::DataViewProtoGetInt32),
    ("getUint8", Builtin::DataViewProtoGetUint8),
    ("getUint16", Builtin::DataViewProtoGetUint16),
    ("getUint32", Builtin::DataViewProtoGetUint32),
    ("setBigInt64", Builtin::DataViewProtoSetBigInt64),
    ("setBigUint64", Builtin::DataViewProtoSetBigUint64),
    ("setFloat32", Builtin::DataViewProtoSetFloat32),
    ("setFloat64", Builtin::DataViewProtoSetFloat64),
    ("setInt8", Builtin::DataViewProtoSetInt8),
    ("setInt16", Builtin::DataViewProtoSetInt16),
    ("setInt32", Builtin::DataViewProtoSetInt32),
    ("setUint8", Builtin::DataViewProtoSetUint8),
    ("setUint16", Builtin::DataViewProtoSetUint16),
    ("setUint32", Builtin::DataViewProtoSetUint32),
];

/// 把 DataView.prototype 方法作为不可枚举数据属性安装到原型对象上，使
/// `DataView.prototype.getUint8` 等可取值并经 `call`/`apply` 调用。
pub(crate) fn install_data_view_prototype_methods(
    state: &mut NativeAgentState,
    prototype: i64,
) -> Result<(), ()> {
    let prototype = value::decode_handle(prototype);
    for &(name, builtin) in DATA_VIEW_PROTO_METHODS {
        let key = state.intern_property_string(name.into()).ok_or(())?;
        let callable = state
            .native_callable(NativeCallableKind::Builtin(builtin, true))
            .ok_or(())?;
        state
            .gc
            .heap()
            .set_property(prototype, key, callable as u64)
            .map_err(|_| ())?;
        state
            .gc
            .heap()
            .update_property_flags(prototype, key, BUILTIN_PROTOTYPE_PROPERTY_FLAGS)
            .map_err(|_| ())?;
    }
    Ok(())
}

/// 从既有共享字节创建 ArrayBuffer 实例：先物化 %ArrayBuffer.prototype% 再
/// 分配实例（物化期间的分配不会悬空尚未入根的实例对象），创建即接线
/// [[Prototype]]（§25.1.5.1 OrdinaryCreateFromConstructor）。
pub(crate) fn from_shared_bytes(
    state: &mut NativeAgentState,
    bytes: Rc<RefCell<Vec<u8>>>,
) -> Option<i64> {
    let prototype = state.ensure_array_buffer_prototype()?;
    let object = state.allocate_object(1, false).ok()?;
    state
        .gc
        .heap()
        .set_prototype(
            value::decode_handle(object),
            value::decode_handle(prototype),
        )
        .ok()?;
    state
        .array_buffers
        .insert(value::decode_handle(object), NativeArrayBuffer { bytes });
    Some(object)
}

pub(crate) fn allocate_array_buffer(state: &mut NativeAgentState, length: usize) -> Option<i64> {
    from_shared_bytes(state, Rc::new(RefCell::new(vec![0; length])))
}

/// `ToIndex(length)` 的既有近似（完整 ToIndex 语义之外的输入走 fail）：
/// 实参缺失或 undefined 按规范取 0（`new ArrayBuffer()` 合法）。
fn byte_length_argument(state: &NativeAgentState, encoded: Option<i64>) -> Option<usize> {
    let Some(encoded) = encoded else {
        return Some(0);
    };
    if value::is_undefined(encoded) {
        return Some(0);
    }
    to_number(state, encoded).and_then(|number| number.to_usize())
}

fn array_buffer_constructor(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let Some(length) = byte_length_argument(state, args.first().copied()) else {
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
    // 先物化原型再分配实例（同 `from_shared_bytes`），此处保留 GC 重试分配。
    let Some(prototype) = state.ensure_array_buffer_prototype() else {
        return fail_dispatch(ctx);
    };
    let Ok(object) = state.allocate_object_with_gc_retry(ctx, 1, false) else {
        return fail_dispatch(ctx);
    };
    if state
        .gc
        .heap()
        .set_prototype(
            value::decode_handle(object),
            value::decode_handle(prototype),
        )
        .is_err()
    {
        return fail_dispatch(ctx);
    }
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
    // 先物化 %DataView.prototype% 再分配实例：创建即接线 [[Prototype]]，
    // instanceof / constructor / @@toStringTag 品牌沿真实原型链成立
    // （§25.3.2.1 OrdinaryCreateFromConstructor）。
    let Some(prototype) = state.ensure_data_view_prototype() else {
        return fail_dispatch(ctx);
    };
    let Ok(object) = state.allocate_object_with_gc_retry(ctx, 1, false) else {
        return fail_dispatch(ctx);
    };
    if state
        .gc
        .heap()
        .set_prototype(
            value::decode_handle(object),
            value::decode_handle(prototype),
        )
        .is_err()
    {
        return fail_dispatch(ctx);
    }
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

/// `getBigInt64` / `getBigUint64`（GetViewValue，ES §25.3.4）：按字节序读取
/// 8 字节整数并 intern 为 BigInt。
fn data_view_get_bigint(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
    signed: bool,
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
    if index.saturating_add(8) > view.length {
        return fail_dispatch(ctx);
    }
    let little_endian = args
        .get(2)
        .is_some_and(|encoded| value::is_bool(*encoded) && value::decode_bool(*encoded));
    let start = view.offset.saturating_add(index);
    let raw: [u8; 8] = if let Some(shared) = &view.shared {
        let bytes = shared
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match bytes.get(start..start + 8) {
            Some(raw) => order::<8>(raw, little_endian),
            None => return fail_dispatch(ctx),
        }
    } else {
        let Some(buffer) = state.array_buffers.get(&view.buffer) else {
            return fail_dispatch(ctx);
        };
        let bytes = buffer.bytes.borrow();
        match bytes.get(start..start + 8) {
            Some(raw) => order::<8>(raw, little_endian),
            None => return fail_dispatch(ctx),
        }
    };
    let bits = u64::from_ne_bytes(raw);
    let integer = if signed {
        BigInt::from(bits as i64)
    } else {
        BigInt::from(bits)
    };
    super::bigint::store(state, integer).unwrap_or_else(|| fail_dispatch(ctx))
}

/// `setBigInt64` / `setBigUint64`（SetViewValue，ES §25.3.4）：非 BigInt 输入按
/// ToBigInt 抛 TypeError；写入前按 2^64 取模，二者字节表示一致。
fn data_view_set_bigint(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
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
    if index.saturating_add(8) > view.length {
        return fail_dispatch(ctx);
    }
    let Some(integer) = super::bigint::read(state, *stored) else {
        return type_error(ctx, state, "Cannot convert value to a BigInt");
    };
    let little_endian = args
        .get(3)
        .is_some_and(|encoded| value::is_bool(*encoded) && value::decode_bool(*encoded));
    let start = view.offset + index;
    let bits = bigint_bits(&integer).to_ne_bytes();
    if let Some(shared) = &view.shared {
        let mut bytes = shared
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        write_bytes(&mut bytes[start..start + 8], bits, little_endian);
    } else {
        let Some(buffer) = state.array_buffers.get(&view.buffer) else {
            return fail_dispatch(ctx);
        };
        let mut bytes = buffer.bytes.borrow_mut();
        write_bytes(&mut bytes[start..start + 8], bits, little_endian);
    }
    value::encode_undefined()
}

/// BigInt → 2^64 取模后的位型（ToBigInt64 / ToBigUint64 写入的字节一致）。
fn bigint_bits(value: &BigInt) -> u64 {
    let modulus = BigInt::from(1u128 << 64);
    let mut normalized = value % &modulus;
    if normalized.sign() == Sign::Minus {
        normalized += &modulus;
    }
    normalized.to_u64().unwrap_or(0)
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
    from_shared_bytes(state, Rc::new(RefCell::new(bytes)))
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
    let shared = if state.array_buffers.get(&buffer_handle).is_some_and(|buffer| {
        offset
            .checked_add(length)
            .is_some_and(|end| end <= buffer.bytes.borrow().len())
    }) {
        None
    } else {
        let shared = state.shared_array_buffers.get(&buffer_handle)?.clone();
        if offset.checked_add(length)? > shared.backing.bytes.lock().ok()?.len() {
            return None;
        }
        Some(shared.backing.bytes)
    };
    // 先物化原型再分配实例（同 `data_view_constructor`）。
    let prototype = state.ensure_data_view_prototype()?;
    let object = state.allocate_object(1, false).ok()?;
    state
        .gc
        .heap()
        .set_prototype(
            value::decode_handle(object),
            value::decode_handle(prototype),
        )
        .ok()?;
    state.data_views.insert(
        value::decode_handle(object),
        NativeDataView {
            buffer: buffer_handle,
            shared,
            offset,
            length,
        },
    );
    Some(object)
}
