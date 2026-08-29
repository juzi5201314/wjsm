use std::cell::RefCell;
use std::rc::Rc;

use num_bigint::{BigInt, Sign};
use num_traits::ToPrimitive;
use wjsm_ir::{Builtin, value};
use wjsm_native_abi::NativeVmContext;

use super::runtime::{fail_dispatch, range_error, to_number, type_error};
use crate::{BUILTIN_PROTOTYPE_PROPERTY_FLAGS, NativeAgentState, NativeCallableKind};

#[derive(Clone)]
pub(crate) struct NativeArrayBuffer {
    pub(crate) bytes: Rc<RefCell<Vec<u8>>>,
    /// resizable buffer 的 [[ArrayBufferMaxByteLength]]（§25.1.5.1 options.maxByteLength）；
    /// None 表示固定长度 buffer。
    pub(crate) max_byte_length: Option<usize>,
    /// IsDetachedBuffer（§25.1.3.4）：transfer / structuredClone 转移后置位，
    /// bytes 同时清空（既有视图经共享 Rc 观察到零长度）。
    pub(crate) detached: bool,
}

#[derive(Clone)]
pub(crate) struct NativeDataView {
    pub(crate) buffer: u32,
    pub(crate) shared: Option<std::sync::Arc<std::sync::Mutex<Vec<u8>>>>,
    pub(crate) offset: usize,
    pub(crate) length: usize,
    /// [[ByteLength]] 为 auto（§25.3.2.1 步骤 8.b）：byteLength 实参缺省且
    /// buffer 可变长（resizable AB / growable SAB）时随底层长度重算。
    pub(crate) length_tracking: bool,
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
        Builtin::ArrayBufferProtoResize
        | Builtin::ArrayBufferProtoTransfer
        | Builtin::ArrayBufferProtoTransferToFixedLength
        | Builtin::ArrayBufferProtoResizable
        | Builtin::ArrayBufferProtoMaxByteLength
        | Builtin::ArrayBufferProtoDetached => {
            super::buffer_resize::dispatch(ctx, state, builtin, args)
        }
        Builtin::DataViewConstructor => data_view_constructor(ctx, state, args),
        Builtin::DataViewProtoGetFloat64 => {
            data_view_get(ctx, state, builtin, args, ViewType::Float64)
        }
        Builtin::DataViewProtoGetFloat32 => {
            data_view_get(ctx, state, builtin, args, ViewType::Float32)
        }
        Builtin::DataViewProtoGetInt32 => data_view_get(ctx, state, builtin, args, ViewType::Int32),
        Builtin::DataViewProtoGetUint32 => {
            data_view_get(ctx, state, builtin, args, ViewType::Uint32)
        }
        Builtin::DataViewProtoGetInt16 => data_view_get(ctx, state, builtin, args, ViewType::Int16),
        Builtin::DataViewProtoGetUint16 => {
            data_view_get(ctx, state, builtin, args, ViewType::Uint16)
        }
        Builtin::DataViewProtoGetInt8 => data_view_get(ctx, state, builtin, args, ViewType::Int8),
        Builtin::DataViewProtoGetUint8 => data_view_get(ctx, state, builtin, args, ViewType::Uint8),
        Builtin::DataViewProtoSetFloat64 => {
            data_view_set(ctx, state, builtin, args, ViewType::Float64)
        }
        Builtin::DataViewProtoSetFloat32 => {
            data_view_set(ctx, state, builtin, args, ViewType::Float32)
        }
        Builtin::DataViewProtoSetInt32 => data_view_set(ctx, state, builtin, args, ViewType::Int32),
        Builtin::DataViewProtoSetUint32 => {
            data_view_set(ctx, state, builtin, args, ViewType::Uint32)
        }
        Builtin::DataViewProtoSetInt16 => data_view_set(ctx, state, builtin, args, ViewType::Int16),
        Builtin::DataViewProtoSetUint16 => {
            data_view_set(ctx, state, builtin, args, ViewType::Uint16)
        }
        Builtin::DataViewProtoSetInt8 => data_view_set(ctx, state, builtin, args, ViewType::Int8),
        Builtin::DataViewProtoSetUint8 => data_view_set(ctx, state, builtin, args, ViewType::Uint8),
        Builtin::DataViewProtoGetBigInt64 => data_view_get_bigint(ctx, state, builtin, args, true),
        Builtin::DataViewProtoGetBigUint64 => {
            data_view_get_bigint(ctx, state, builtin, args, false)
        }
        Builtin::DataViewProtoSetBigInt64 | Builtin::DataViewProtoSetBigUint64 => {
            data_view_set_bigint(ctx, state, builtin, args)
        }
        Builtin::DataViewProtoBuffer
        | Builtin::DataViewProtoByteLength
        | Builtin::DataViewProtoByteOffset => data_view_accessor(ctx, state, builtin, args),
        _ => return None,
    })
}

/// RequireInternalSlot 失败的 V8 口径 TypeError：
/// `Method {method} called on incompatible receiver {receiver}`。
pub(super) fn incompatible_receiver(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    method: &str,
    args: &[i64],
) -> i64 {
    let receiver = args.first().copied().unwrap_or_else(value::encode_undefined);
    let message = format!(
        "Method {method} called on incompatible receiver {}",
        super::iterator_prototypes::render_incompatible_receiver(state, receiver)
    );
    type_error(ctx, state, &message)
}

/// ToIndex（§7.1.22）近似：缺省 / undefined / NaN / 不可转换 → 0，数值
/// 截断取整；负值或超出 usize 可表示范围时 Err 携带截断值供 V8 文案渲染。
pub(super) fn to_index(state: &NativeAgentState, encoded: Option<i64>) -> Result<usize, f64> {
    let Some(encoded) = encoded else {
        return Ok(0);
    };
    if value::is_undefined(encoded) {
        return Ok(0);
    }
    let number = to_number(state, encoded).unwrap_or(0.0);
    if number.is_nan() {
        return Ok(0);
    }
    let truncated = number.trunc();
    truncated.to_usize().ok_or(truncated)
}

/// GetViewByteLength（§25.3.1.1）：length-tracking 视图随底层 buffer 当前
/// 长度重算；底层 detach 或视图越界（resizable buffer shrink 后，
/// IsViewOutOfBounds §25.3.1.2）返回 None。
pub(crate) fn data_view_current_length(
    state: &NativeAgentState,
    view: &NativeDataView,
) -> Option<usize> {
    let buffer_length = if let Some(shared) = &view.shared {
        shared
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len()
    } else {
        let buffer = state.array_buffers.get(&view.buffer)?;
        if buffer.detached {
            return None;
        }
        buffer.bytes.borrow().len()
    };
    if view.length_tracking {
        return buffer_length.checked_sub(view.offset);
    }
    (view.offset.checked_add(view.length)? <= buffer_length).then_some(view.length)
}

/// ValidateViewLength：detach / 越界按 V8 文案抛 TypeError（IsViewOutOfBounds
/// 与 detach 共用 "detached ArrayBuffer" 措辞），成功返回当前 byteLength。
fn require_data_view_length(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    view: &NativeDataView,
    method: &str,
) -> Result<usize, i64> {
    data_view_current_length(state, view).ok_or_else(|| {
        let message = format!("Cannot perform {method} on a detached ArrayBuffer");
        type_error(ctx, state, &message)
    })
}

/// `get DataView.prototype.buffer` / `byteLength` / `byteOffset`
/// （§25.3.4.1–3）：receiver 必须携带 [[DataView]] 品牌（side table 有条目）；
/// byteLength / byteOffset 对 detach / 越界视图抛 TypeError（§25.3.4.2–3
/// 步骤 4），buffer 始终可读。
fn data_view_accessor(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    builtin: Builtin,
    args: &[i64],
) -> i64 {
    let Some(view) = args
        .first()
        .and_then(|object| state.data_views.get(&value::decode_handle(*object)))
        .cloned()
    else {
        let method = format!("get {}", builtin.as_str());
        return incompatible_receiver(ctx, state, &method, args);
    };
    if builtin == Builtin::DataViewProtoBuffer {
        return value::encode_object_handle(view.buffer);
    }
    let method = format!("get {}", builtin.as_str());
    let length = match require_data_view_length(ctx, state, &view, &method) {
        Ok(length) => length,
        Err(exception) => return exception,
    };
    match builtin {
        Builtin::DataViewProtoByteLength => value::encode_f64(length as f64),
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
    from_shared_bytes_with_max(state, bytes, None)
}

/// 同 [`from_shared_bytes`]，另携带 [[ArrayBufferMaxByteLength]]（resizable
/// buffer 的构造 / transfer / structuredClone 复原路径）。
pub(crate) fn from_shared_bytes_with_max(
    state: &mut NativeAgentState,
    bytes: Rc<RefCell<Vec<u8>>>,
    max_byte_length: Option<usize>,
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
    state.array_buffers.insert(
        value::decode_handle(object),
        NativeArrayBuffer {
            bytes,
            max_byte_length,
            detached: false,
        },
    );
    Some(object)
}

pub(crate) fn allocate_array_buffer(state: &mut NativeAgentState, length: usize) -> Option<i64> {
    from_shared_bytes(state, Rc::new(RefCell::new(vec![0; length])))
}

fn array_buffer_constructor(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    // §25.1.4.1：ToIndex(length)，无实参 / undefined 取 0，负值 RangeError。
    let Ok(length) = to_index(state, args.first().copied()) else {
        return range_error(ctx, state, "Invalid array buffer length");
    };
    // §25.1.4.1 步骤 3 GetArrayBufferMaxByteLengthOption：options 非对象或
    // maxByteLength 为 undefined 时保持固定长度；否则 ToIndex 且必须 ≥ length
    //（§25.1.3.1 AllocateArrayBuffer 步骤 3，V8 文案 RangeError）。
    let max_option = args.get(1).and_then(|options| {
        if value::is_undefined(*options) {
            None
        } else {
            super::modules::named_property(state, *options, "maxByteLength")
        }
    });
    let max_byte_length = match max_option {
        None => None,
        Some(encoded) if value::is_undefined(encoded) => None,
        Some(encoded) => match to_index(state, Some(encoded)) {
            Ok(max) if length <= max => Some(max),
            _ => return range_error(ctx, state, "Invalid array buffer max length"),
        },
    };
    from_shared_bytes_with_max(state, Rc::new(RefCell::new(vec![0; length])), max_byte_length)
        .unwrap_or_else(|| fail_dispatch(ctx))
}

fn array_buffer_byte_length(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    // §25.1.6.2：detached buffer 返回 +0（detach 时 bytes 已清空，len 即 0）。
    let Some(length) = args
        .first()
        .and_then(|object| state.array_buffers.get(&value::decode_handle(*object)))
        .and_then(|buffer| u32::try_from(buffer.bytes.borrow().len()).ok())
    else {
        return incompatible_receiver(ctx, state, "get ArrayBuffer.prototype.byteLength", args);
    };
    value::encode_f64(f64::from(length))
}

fn array_buffer_slice(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let Some(buffer) = args
        .first()
        .and_then(|receiver| state.array_buffers.get(&value::decode_handle(*receiver)))
        .cloned()
    else {
        return incompatible_receiver(ctx, state, "ArrayBuffer.prototype.slice", args);
    };
    // §25.1.6.16 步骤 4：IsDetachedBuffer 抛 TypeError（V8 文案）。
    if buffer.detached {
        return type_error(
            ctx,
            state,
            "Cannot perform ArrayBuffer.prototype.slice on a detached ArrayBuffer",
        );
    }
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
    // §25.1.6.16：slice 结果是固定长度 buffer（resizable 源亦然）。
    state.array_buffers.insert(
        value::decode_handle(object),
        NativeArrayBuffer {
            bytes: Rc::new(RefCell::new(bytes)),
            max_byte_length: None,
            detached: false,
        },
    );
    object
}

fn data_view_constructor(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let buffer = args.first().copied().unwrap_or_else(value::encode_undefined);
    let buffer_handle = value::decode_handle(buffer);
    let shared = state
        .shared_array_buffers
        .get(&buffer_handle)
        .map(|sab| sab.backing.bytes.clone());
    // buffer 可变长（resizable AB / growable SAB）且 byteLength 缺省时视图为
    // length-tracking（§25.3.2.1 步骤 8.b [[ByteLength]] = auto）。
    let buffer_resizable = if shared.is_some() {
        state
            .shared_array_buffers
            .get(&buffer_handle)
            .is_some_and(super::sab::NativeSharedArrayBuffer::growable)
    } else {
        state
            .array_buffers
            .get(&buffer_handle)
            .is_some_and(|array_buffer| array_buffer.max_byte_length.is_some())
    };
    let total_length = if let Some(shared) = &shared {
        shared.lock().map(|bytes| bytes.len()).unwrap_or(0)
    } else if let Some(array_buffer) = state.array_buffers.get(&buffer_handle) {
        // §25.3.2.1 步骤 6：IsDetachedBuffer 抛 TypeError（V8 文案）。
        if array_buffer.detached {
            return type_error(
                ctx,
                state,
                "Cannot perform DataView constructor on a detached ArrayBuffer",
            );
        }
        array_buffer.bytes.borrow().len()
    } else {
        // §25.3.2.1 步骤 2 RequireInternalSlot(buffer) 失败（V8 文案）。
        return type_error(
            ctx,
            state,
            "First argument to DataView constructor must be an ArrayBuffer",
        );
    };
    // §25.3.2.1 步骤 3–5：ToIndex(byteOffset)，越界 RangeError（V8 文案）。
    let offset = match to_index(state, args.get(1).copied()) {
        Ok(offset) if offset <= total_length => offset,
        Ok(offset) => {
            let message = format!("Start offset {offset} is outside the bounds of the buffer");
            return range_error(ctx, state, &message);
        }
        Err(invalid) => {
            let message = format!(
                "Start offset {} is outside the bounds of the buffer",
                wjsm_builtins::number_format::format_number_js(invalid)
            );
            return range_error(ctx, state, &message);
        }
    };
    // §25.3.2.1 步骤 8–9：byteLength 缺省取剩余长度（可变长 buffer 转为
    // length-tracking），越界 RangeError。
    let length_tracking =
        buffer_resizable && args.get(2).is_none_or(|encoded| value::is_undefined(*encoded));
    let length = match args.get(2) {
        None => total_length - offset,
        Some(encoded) if value::is_undefined(*encoded) => total_length - offset,
        Some(encoded) => match to_index(state, Some(*encoded)) {
            Ok(length) if offset.saturating_add(length) <= total_length => length,
            Ok(length) => {
                let message = format!("Invalid DataView length {length}");
                return range_error(ctx, state, &message);
            }
            Err(invalid) => {
                let message = format!(
                    "Invalid DataView length {}",
                    wjsm_builtins::number_format::format_number_js(invalid)
                );
                return range_error(ctx, state, &message);
            }
        },
    };
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
            length_tracking,
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

/// GetViewValue / SetViewValue 越界（§25.3.3.1 步骤 11，V8 文案）。
fn data_view_out_of_bounds(ctx: &mut NativeVmContext, state: &mut NativeAgentState) -> i64 {
    range_error(ctx, state, "Offset is outside the bounds of the DataView")
}

fn data_view_get(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    builtin: Builtin,
    args: &[i64],
    kind: ViewType,
) -> i64 {
    let Some(view) = args
        .first()
        .and_then(|object| state.data_views.get(&value::decode_handle(*object)))
        .cloned()
    else {
        return incompatible_receiver(ctx, state, builtin.as_str(), args);
    };
    let view_length = match require_data_view_length(ctx, state, &view, builtin.as_str()) {
        Ok(length) => length,
        Err(exception) => return exception,
    };
    let Ok(index) = to_index(state, args.get(1).copied()) else {
        return data_view_out_of_bounds(ctx, state);
    };
    let start = view.offset.saturating_add(index);
    if index.saturating_add(kind.size()) > view_length {
        return data_view_out_of_bounds(ctx, state);
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
    builtin: Builtin,
    args: &[i64],
    kind: ViewType,
) -> i64 {
    let Some(view) = args
        .first()
        .and_then(|receiver| state.data_views.get(&value::decode_handle(*receiver)))
        .cloned()
    else {
        return incompatible_receiver(ctx, state, builtin.as_str(), args);
    };
    let view_length = match require_data_view_length(ctx, state, &view, builtin.as_str()) {
        Ok(length) => length,
        Err(exception) => return exception,
    };
    let Ok(index) = to_index(state, args.get(1).copied()) else {
        return data_view_out_of_bounds(ctx, state);
    };
    if index.saturating_add(kind.size()) > view_length {
        return data_view_out_of_bounds(ctx, state);
    }
    let number = args
        .get(2)
        .and_then(|stored| to_number(state, *stored))
        .unwrap_or(f64::NAN);
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
    builtin: Builtin,
    args: &[i64],
    signed: bool,
) -> i64 {
    let Some(view) = args
        .first()
        .and_then(|object| state.data_views.get(&value::decode_handle(*object)))
        .cloned()
    else {
        return incompatible_receiver(ctx, state, builtin.as_str(), args);
    };
    let view_length = match require_data_view_length(ctx, state, &view, builtin.as_str()) {
        Ok(length) => length,
        Err(exception) => return exception,
    };
    let Ok(index) = to_index(state, args.get(1).copied()) else {
        return data_view_out_of_bounds(ctx, state);
    };
    if index.saturating_add(8) > view_length {
        return data_view_out_of_bounds(ctx, state);
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
    builtin: Builtin,
    args: &[i64],
) -> i64 {
    let Some(view) = args
        .first()
        .and_then(|receiver| state.data_views.get(&value::decode_handle(*receiver)))
        .cloned()
    else {
        return incompatible_receiver(ctx, state, builtin.as_str(), args);
    };
    let view_length = match require_data_view_length(ctx, state, &view, builtin.as_str()) {
        Ok(length) => length,
        Err(exception) => return exception,
    };
    let Ok(index) = to_index(state, args.get(1).copied()) else {
        return data_view_out_of_bounds(ctx, state);
    };
    if index.saturating_add(8) > view_length {
        return data_view_out_of_bounds(ctx, state);
    }
    let stored = args.get(2).copied().unwrap_or_else(value::encode_undefined);
    let Some(integer) = super::bigint::read(state, stored) else {
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

/// structuredClone 序列化元数据：（[[ArrayBufferMaxByteLength]], detached）。
pub(crate) fn array_buffer_clone_meta(
    state: &NativeAgentState,
    encoded: i64,
) -> Option<(Option<usize>, bool)> {
    state
        .array_buffers
        .get(&value::decode_handle(encoded))
        .map(|buffer| (buffer.max_byte_length, buffer.detached))
}

pub(crate) fn from_bytes(state: &mut NativeAgentState, bytes: Vec<u8>) -> Option<i64> {
    from_shared_bytes(state, Rc::new(RefCell::new(bytes)))
}

/// DetachArrayBuffer（§25.1.3.5）：置位 detached 并清空 bytes，既有视图
/// 经共享 Rc 立即观察到零长度（元素读 undefined / 方法抛 TypeError）。
pub(crate) fn detach(state: &mut NativeAgentState, handle: u32) {
    if let Some(buffer) = state.array_buffers.get_mut(&handle) {
        buffer.detached = true;
        buffer.bytes.borrow_mut().clear();
    }
}

pub(crate) fn data_view_parts(
    state: &NativeAgentState,
    encoded: i64,
) -> Option<(ViewBacking, usize, usize, bool)> {
    let handle = value::decode_handle(encoded);
    let view = state.data_views.get(&handle)?;
    let backing = if state.shared_array_buffers.contains_key(&view.buffer) {
        ViewBacking::SharedArrayBuffer(value::encode_object_handle(view.buffer))
    } else {
        ViewBacking::ArrayBuffer(value::encode_object_handle(view.buffer))
    };
    Some((backing, view.offset, view.length, view.length_tracking))
}

pub(crate) fn from_view(
    state: &mut NativeAgentState,
    backing: i64,
    offset: usize,
    length: usize,
    length_tracking: bool,
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
            length_tracking,
        },
    );
    Some(object)
}
