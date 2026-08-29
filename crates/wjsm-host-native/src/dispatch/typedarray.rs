use std::cmp::Ordering;

use std::cell::RefCell;
use std::rc::Rc;

use super::buffers::NativeArrayBuffer;
use num_bigint::{BigInt, Sign};
use num_traits::ToPrimitive;
use wjsm_ir::{Builtin, value};
use wjsm_native_abi::NativeVmContext;

use super::runtime::{fail_dispatch, is_truthy, render_value, strict_equal, to_number, type_error};
use crate::NativeAgentState;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TypedArrayKind {
    Int8,
    Uint8,
    Uint8Clamped,
    Int16,
    Uint16,
    Int32,
    Uint32,
    Float32,
    Float64,
    BigInt64,
    BigUint64,
}

impl TypedArrayKind {
    pub(crate) fn element_size(self) -> usize {
        match self {
            Self::Int8 | Self::Uint8 | Self::Uint8Clamped => 1,
            Self::Int16 | Self::Uint16 => 2,
            Self::Int32 | Self::Uint32 | Self::Float32 => 4,
            Self::Float64 | Self::BigInt64 | Self::BigUint64 => 8,
        }
    }

    pub(crate) fn is_bigint(self) -> bool {
        matches!(self, Self::BigInt64 | Self::BigUint64)
    }
}

#[derive(Clone)]
pub(crate) struct NativeTypedArray {
    pub(crate) kind: TypedArrayKind,
    pub(crate) storage: Option<Rc<RefCell<Vec<i64>>>>,
    pub(crate) buffer: Option<Rc<RefCell<Vec<u8>>>>,
    pub(crate) buffer_object: Option<i64>,
    /// SAB 视图：agent 持有的 cluster backing 引用（`Arc<Mutex<Vec<u8>>>`）。
    pub(crate) shared_buffer: Option<std::sync::Arc<std::sync::Mutex<Vec<u8>>>>,
    /// SAB 视图对应的 cluster backing_id（wait/notify 队列定位用）。
    pub(crate) shared_backing_id: Option<u32>,
    pub(crate) is_shared: bool,
    pub(crate) offset: usize,
    pub(crate) length: usize,
    /// [[ArrayLength]] 为 auto（§23.2.5.1 步骤 7.b）：length 实参缺省且
    /// buffer 可变长（resizable AB / growable SAB）时随底层长度重算；
    /// `length` 字段仅记录构造时的快照。
    pub(crate) length_tracking: bool,
}

/// TypedArrayLength + IsTypedArrayOutOfBounds
/// （§10.4.5.12 / §10.4.5.13，经 MakeTypedArrayWithBufferWitnessRecord）：
/// 返回视图当前有效元素个数。length-tracking 视图随底层 buffer 当前长度
/// 重算；底层 detach 或视图越界（resizable buffer shrink 后）返回 None——
/// getter 呈现 0、元素读写按越界处理、方法按 ValidateTypedArray 抛
/// TypeError。内部 storage 视图（流 / 编码器产物）长度固定恒有效。
pub(crate) fn view_length(state: &NativeAgentState, array: &NativeTypedArray) -> Option<usize> {
    let element_size = array.kind.element_size();
    let buffer_length = if let Some(buffer) = &array.buffer {
        // detach 标志在 buffer 对象侧表上（bytes 清空后 tracking offset 0
        // 视图的纯长度计算无法区分 detach 与 resize 到 0）。
        if let Some(buffer_object) = array.buffer_object
            && state
                .array_buffers
                .get(&value::decode_handle(buffer_object))
                .is_some_and(|entry| entry.detached)
        {
            return None;
        }
        buffer.borrow().len()
    } else if let Some(shared) = &array.shared_buffer {
        shared
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len()
    } else {
        return Some(array.length);
    };
    let byte_offset = array.offset.checked_mul(element_size)?;
    if array.length_tracking {
        return buffer_length
            .checked_sub(byte_offset)
            .map(|remaining| remaining / element_size);
    }
    let byte_end = byte_offset.checked_add(array.length.checked_mul(element_size)?)?;
    (byte_end <= buffer_length).then_some(array.length)
}

pub(crate) fn create_uint8_array(state: &mut NativeAgentState, bytes: &[u8]) -> Option<i64> {
    let object = state.allocate_object(2, false).ok()?;
    attach_instance_prototype(state, object, TypedArrayKind::Uint8)?;
    let storage = bytes
        .iter()
        .map(|byte| value::encode_f64(f64::from(*byte)))
        .collect();
    state.typed_arrays.insert(
        value::decode_handle(object),
        NativeTypedArray {
            kind: TypedArrayKind::Uint8,
            storage: Some(Rc::new(RefCell::new(storage))),
            buffer: None,
            buffer_object: None,
            shared_buffer: None,
            shared_backing_id: None,
            is_shared: false,
            offset: 0,
            length: bytes.len(),
            length_tracking: false,
        },
    );
    Some(object)
}

pub(crate) fn prefix_view(state: &mut NativeAgentState, view: i64, length: usize) -> Option<i64> {
    let handle = value::decode_handle(view);
    let entry = state.typed_arrays.get(&handle)?;
    let count = view_length(state, entry).unwrap_or(0).min(length);
    let mut bytes = Vec::with_capacity(count);
    for index in 0..count {
        let stored = get_element(state, view, index)?;
        bytes.push(value::decode_f64(stored) as u8);
    }
    state.typed_arrays.get_mut(&handle)?.length = 0;
    create_uint8_array(state, &bytes)
}

pub(crate) fn byte_length(state: &NativeAgentState, view: i64) -> Option<usize> {
    let array = state.typed_arrays.get(&value::decode_handle(view))?;
    view_length(state, array)
        .unwrap_or(0)
        .checked_mul(array.kind.element_size())
}
pub(crate) enum CloneView {
    Values(Vec<i64>),
    ArrayBuffer {
        buffer: i64,
        offset: usize,
        length: usize,
        length_tracking: bool,
    },
    SharedArrayBuffer {
        object: i64,
        offset: usize,
        length: usize,
        length_tracking: bool,
    },
}

pub(crate) fn clone_view(
    state: &NativeAgentState,
    encoded: i64,
) -> Option<(TypedArrayKind, CloneView)> {
    let array = state.typed_arrays.get(&value::decode_handle(encoded))?;
    if let Some(storage) = &array.storage {
        return Some((array.kind, CloneView::Values(storage.borrow().clone())));
    }
    let current_length = view_length(state, array).unwrap_or(0);
    if array.buffer.is_some() {
        if let Some(buffer_object) = array.buffer_object {
            return Some((
                array.kind,
                CloneView::ArrayBuffer {
                    buffer: buffer_object,
                    offset: array.offset,
                    length: current_length,
                    length_tracking: array.length_tracking,
                },
            ));
        }
        return Some((
            array.kind,
            CloneView::Values(
                (0..current_length)
                    .map(|index| get_element(state, encoded, index))
                    .collect::<Option<Vec<_>>>()?,
            ),
        ));
    }
    let backing_id = array.shared_backing_id?;
    let object = state
        .shared_array_buffers
        .iter()
        .find(|(_, shared)| shared.backing_id == backing_id)
        .map(|(handle, _)| value::encode_object_handle(*handle))?;
    Some((
        array.kind,
        CloneView::SharedArrayBuffer {
            object,
            offset: array.offset,
            length: current_length,
            length_tracking: array.length_tracking,
        },
    ))
}

pub(crate) fn from_values(
    state: &mut NativeAgentState,
    kind: TypedArrayKind,
    values: Vec<i64>,
) -> Option<i64> {
    let length = values.len();
    let object = state.allocate_object(2, false).ok()?;
    attach_instance_prototype(state, object, kind)?;
    state.typed_arrays.insert(
        value::decode_handle(object),
        NativeTypedArray {
            kind,
            storage: Some(Rc::new(RefCell::new(values))),
            buffer: None,
            buffer_object: None,
            shared_buffer: None,
            shared_backing_id: None,
            is_shared: false,
            offset: 0,
            length,
            length_tracking: false,
        },
    );
    Some(object)
}

pub(crate) fn from_buffer(
    state: &mut NativeAgentState,
    kind: TypedArrayKind,
    buffer: i64,
    offset: usize,
    length: usize,
    length_tracking: bool,
) -> Option<i64> {
    let buffer_object = buffer;
    let buffer = state
        .array_buffers
        .get(&value::decode_handle(buffer_object))
        .cloned()?;
    if offset.checked_add(length)? > buffer.bytes.borrow().len() / kind.element_size() {
        return None;
    }
    let object = state.allocate_object(2, false).ok()?;
    attach_instance_prototype(state, object, kind)?;
    state.typed_arrays.insert(
        value::decode_handle(object),
        NativeTypedArray {
            kind,
            storage: None,
            buffer: Some(buffer.bytes),
            shared_buffer: None,
            buffer_object: Some(buffer_object),
            shared_backing_id: None,
            is_shared: false,
            offset,
            length,
            length_tracking,
        },
    );
    Some(object)
}

pub(crate) fn from_shared_buffer(
    state: &mut NativeAgentState,
    kind: TypedArrayKind,
    buffer: i64,
    offset: usize,
    length: usize,
    length_tracking: bool,
) -> Option<i64> {
    let buffer_handle = value::decode_handle(buffer);
    let shared = state.shared_array_buffers.get(&buffer_handle).cloned()?;
    if offset.checked_add(length)? > shared.byte_length() / kind.element_size() {
        return None;
    }
    let object = state.allocate_object(2, false).ok()?;
    attach_instance_prototype(state, object, kind)?;
    state.typed_arrays.insert(
        value::decode_handle(object),
        NativeTypedArray {
            kind,
            storage: None,
            buffer: None,
            shared_buffer: Some(shared.backing.bytes),
            buffer_object: Some(buffer),
            shared_backing_id: Some(shared.backing_id),
            is_shared: true,
            offset,
            length,
            length_tracking,
        },
    );
    Some(object)
}

pub(super) fn dispatch_typed_array(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    builtin: Builtin,
    args: &[i64],
) -> Option<i64> {
    // CallBuiltin 是编译期绑定的直接构造（`new Uint8Array(..)` 快路径）：
    // newTarget 即构造器本体，实例挂内在原型。此路径不压新激活帧，绝不能
    // 读取外层函数激活帧的 new.target；子类 super() / Reflect.construct 走
    // `construct_with_new_target` 的可调用对象路径。
    let default_target = value::encode_undefined();
    Some(match builtin {
        Builtin::Int8ArrayConstructor => {
            construct(ctx, state, args, TypedArrayKind::Int8, default_target)
        }
        Builtin::Uint8ArrayConstructor => {
            construct(ctx, state, args, TypedArrayKind::Uint8, default_target)
        }
        Builtin::Uint8ClampedArrayConstructor => construct(
            ctx,
            state,
            args,
            TypedArrayKind::Uint8Clamped,
            default_target,
        ),
        Builtin::Int16ArrayConstructor => {
            construct(ctx, state, args, TypedArrayKind::Int16, default_target)
        }
        Builtin::Uint16ArrayConstructor => {
            construct(ctx, state, args, TypedArrayKind::Uint16, default_target)
        }
        Builtin::Int32ArrayConstructor => {
            construct(ctx, state, args, TypedArrayKind::Int32, default_target)
        }
        Builtin::Uint32ArrayConstructor => {
            construct(ctx, state, args, TypedArrayKind::Uint32, default_target)
        }
        Builtin::Float32ArrayConstructor => {
            construct(ctx, state, args, TypedArrayKind::Float32, default_target)
        }
        Builtin::Float64ArrayConstructor => {
            construct(ctx, state, args, TypedArrayKind::Float64, default_target)
        }
        Builtin::BigInt64ArrayConstructor => {
            construct(ctx, state, args, TypedArrayKind::BigInt64, default_target)
        }
        Builtin::BigUint64ArrayConstructor => {
            construct(ctx, state, args, TypedArrayKind::BigUint64, default_target)
        }
        Builtin::TypedArrayProtoLength => property_length(ctx, state, args),
        Builtin::TypedArrayProtoByteLength => property_byte_length(ctx, state, args),
        Builtin::TypedArrayProtoByteOffset => property_byte_offset(ctx, state, args),
        Builtin::TypedArrayProtoSet => set(ctx, state, args),
        Builtin::TypedArrayProtoSlice => slice(ctx, state, args),
        Builtin::TypedArrayProtoSubarray => subarray(ctx, state, args),
        Builtin::TypedArrayProtoFill => fill(ctx, state, args),
        Builtin::TypedArrayProtoReverse => reverse(ctx, state, args),
        Builtin::TypedArrayProtoIndexOf => index_of(ctx, state, args, false),
        Builtin::TypedArrayProtoLastIndexOf => index_of(ctx, state, args, true),
        Builtin::TypedArrayProtoIncludes => includes(ctx, state, args),
        Builtin::TypedArrayProtoJoin => join(ctx, state, args),
        Builtin::TypedArrayProtoToString => join(ctx, state, args),
        Builtin::TypedArrayProtoCopyWithin => copy_within(ctx, state, args),
        Builtin::TypedArrayProtoAt => at(ctx, state, args),
        Builtin::TypedArrayProtoForEach => {
            callback_iterate(ctx, state, args, CallbackKind::ForEach)
        }
        Builtin::TypedArrayProtoMap => callback_iterate(ctx, state, args, CallbackKind::Map),
        Builtin::TypedArrayProtoFilter => callback_iterate(ctx, state, args, CallbackKind::Filter),
        Builtin::TypedArrayProtoReduce => reduce(ctx, state, args, false),
        Builtin::TypedArrayProtoReduceRight => reduce(ctx, state, args, true),
        Builtin::TypedArrayProtoFind => callback_iterate(ctx, state, args, CallbackKind::Find),
        Builtin::TypedArrayProtoFindIndex => {
            callback_iterate(ctx, state, args, CallbackKind::FindIndex)
        }
        Builtin::TypedArrayProtoSome => callback_iterate(ctx, state, args, CallbackKind::Some),
        Builtin::TypedArrayProtoEvery => callback_iterate(ctx, state, args, CallbackKind::Every),
        Builtin::TypedArrayProtoSort => sort(ctx, state, args),
        Builtin::TypedArrayProtoEntries => {
            iterator(ctx, state, args, crate::NativeIteratorKind::Entries)
        }
        Builtin::TypedArrayProtoKeys => iterator(ctx, state, args, crate::NativeIteratorKind::Keys),
        Builtin::TypedArrayProtoValues => {
            iterator(ctx, state, args, crate::NativeIteratorKind::Values)
        }
        _ => return None,
    })
}

/// %TypedArray%.prototype 的方法名 → Builtin 映射（不含 length / byteLength /
/// byteOffset / buffer 访问器），实例取值（`typed_array_builtin`）与共享原型
/// 对象安装（`install_typed_array_prototype_methods`）共用。
pub(crate) const TYPED_ARRAY_PROTO_METHODS: &[(&str, Builtin)] = &[
    ("at", Builtin::TypedArrayProtoAt),
    ("copyWithin", Builtin::TypedArrayProtoCopyWithin),
    ("entries", Builtin::TypedArrayProtoEntries),
    ("every", Builtin::TypedArrayProtoEvery),
    ("fill", Builtin::TypedArrayProtoFill),
    ("filter", Builtin::TypedArrayProtoFilter),
    ("find", Builtin::TypedArrayProtoFind),
    ("findIndex", Builtin::TypedArrayProtoFindIndex),
    ("forEach", Builtin::TypedArrayProtoForEach),
    ("includes", Builtin::TypedArrayProtoIncludes),
    ("indexOf", Builtin::TypedArrayProtoIndexOf),
    ("join", Builtin::TypedArrayProtoJoin),
    ("keys", Builtin::TypedArrayProtoKeys),
    ("lastIndexOf", Builtin::TypedArrayProtoLastIndexOf),
    ("map", Builtin::TypedArrayProtoMap),
    ("reduce", Builtin::TypedArrayProtoReduce),
    ("reduceRight", Builtin::TypedArrayProtoReduceRight),
    ("reverse", Builtin::TypedArrayProtoReverse),
    ("set", Builtin::TypedArrayProtoSet),
    ("slice", Builtin::TypedArrayProtoSlice),
    ("some", Builtin::TypedArrayProtoSome),
    ("sort", Builtin::TypedArrayProtoSort),
    ("subarray", Builtin::TypedArrayProtoSubarray),
    ("toString", Builtin::TypedArrayProtoToString),
    ("values", Builtin::TypedArrayProtoValues),
];

/// 判定 builtin 是否为 TypedArray 构造器（11 种元素类型之一）。
pub(crate) fn is_typed_array_constructor(builtin: Builtin) -> bool {
    constructor_kind(builtin).is_some()
}

/// TypedArray 构造器 builtin → 元素类型；非 TypedArray 构造器为 None。
pub(crate) fn constructor_kind(builtin: Builtin) -> Option<TypedArrayKind> {
    Some(match builtin {
        Builtin::Int8ArrayConstructor => TypedArrayKind::Int8,
        Builtin::Uint8ArrayConstructor => TypedArrayKind::Uint8,
        Builtin::Uint8ClampedArrayConstructor => TypedArrayKind::Uint8Clamped,
        Builtin::Int16ArrayConstructor => TypedArrayKind::Int16,
        Builtin::Uint16ArrayConstructor => TypedArrayKind::Uint16,
        Builtin::Int32ArrayConstructor => TypedArrayKind::Int32,
        Builtin::Uint32ArrayConstructor => TypedArrayKind::Uint32,
        Builtin::Float32ArrayConstructor => TypedArrayKind::Float32,
        Builtin::Float64ArrayConstructor => TypedArrayKind::Float64,
        Builtin::BigInt64ArrayConstructor => TypedArrayKind::BigInt64,
        Builtin::BigUint64ArrayConstructor => TypedArrayKind::BigUint64,
        _ => return None,
    })
}

/// 元素类型 → 对应 TypedArray 构造器 builtin。
pub(super) fn constructor_builtin(kind: TypedArrayKind) -> Builtin {
    match kind {
        TypedArrayKind::Int8 => Builtin::Int8ArrayConstructor,
        TypedArrayKind::Uint8 => Builtin::Uint8ArrayConstructor,
        TypedArrayKind::Uint8Clamped => Builtin::Uint8ClampedArrayConstructor,
        TypedArrayKind::Int16 => Builtin::Int16ArrayConstructor,
        TypedArrayKind::Uint16 => Builtin::Uint16ArrayConstructor,
        TypedArrayKind::Int32 => Builtin::Int32ArrayConstructor,
        TypedArrayKind::Uint32 => Builtin::Uint32ArrayConstructor,
        TypedArrayKind::Float32 => Builtin::Float32ArrayConstructor,
        TypedArrayKind::Float64 => Builtin::Float64ArrayConstructor,
        TypedArrayKind::BigInt64 => Builtin::BigInt64ArrayConstructor,
        TypedArrayKind::BigUint64 => Builtin::BigUint64ArrayConstructor,
    }
}

/// 新建实例的 [[Prototype]] 挂到对应构造器的 `prototype` 对象（§23.2.5.1），
/// 使 Object.getPrototypeOf / instanceof 沿三层链成立。
fn attach_instance_prototype(
    state: &mut NativeAgentState,
    object: i64,
    kind: TypedArrayKind,
) -> Option<()> {
    state.set_typed_array_instance_prototype(object, constructor_builtin(kind))
}

/// 实例 [[Prototype]]：无覆盖槽挂对应构造器缺省 prototype（§23.2.5.1）；
/// newTarget 覆盖槽存在时挂 newTarget.prototype
/// （OrdinaryCreateFromConstructor，§10.1.13），子类实例的 instanceof
/// 沿子类原型链成立。
fn attach_prototype(
    state: &mut NativeAgentState,
    object: i64,
    kind: TypedArrayKind,
    proto_override: Option<u32>,
) -> Option<()> {
    match proto_override {
        None => attach_instance_prototype(state, object, kind),
        Some(slot) => state
            .gc
            .heap()
            .set_prototype(value::decode_handle(object), slot)
            .ok(),
    }
}

/// 把 %TypedArray%.prototype 方法作为不可枚举数据属性安装到共享原型对象上，
/// 各构造器 `prototype` 沿链继承，`Uint8Array.prototype.slice` 等可取值并经
/// `call`/`apply` 调用。
pub(crate) fn install_typed_array_prototype_methods(
    state: &mut NativeAgentState,
    prototype: i64,
) -> Result<(), ()> {
    let prototype = value::decode_handle(prototype);
    for &(name, builtin) in TYPED_ARRAY_PROTO_METHODS {
        let key = state.intern_property_string(name.into()).ok_or(())?;
        let callable = state
            .native_callable(crate::NativeCallableKind::Builtin(builtin, true))
            .ok_or(())?;
        state
            .gc
            .heap()
            .set_property(prototype, key, callable as u64)
            .map_err(|_| ())?;
        state
            .gc
            .heap()
            .update_property_flags(prototype, key, crate::BUILTIN_PROTOTYPE_PROPERTY_FLAGS)
            .map_err(|_| ())?;
    }
    Ok(())
}

pub(crate) fn typed_array_builtin(
    state: &NativeAgentState,
    receiver: i64,
    key: &str,
) -> Option<Builtin> {
    let handle = value::decode_handle(receiver);
    if !state.typed_arrays.contains_key(&handle) {
        return None;
    }
    match key {
        "length" => Some(Builtin::TypedArrayProtoLength),
        "byteLength" => Some(Builtin::TypedArrayProtoByteLength),
        "byteOffset" => Some(Builtin::TypedArrayProtoByteOffset),
        _ => TYPED_ARRAY_PROTO_METHODS
            .iter()
            .find(|(name, _)| *name == key)
            .map(|(_, builtin)| *builtin),
    }
}

pub(crate) fn get_element(state: &NativeAgentState, object: i64, index: usize) -> Option<i64> {
    get_element_from(state, object, index)
}

/// 读取元素；BigInt 视图需要 intern，因此走可变状态。
pub(crate) fn get_element_intern(
    state: &mut NativeAgentState,
    object: i64,
    index: usize,
) -> Option<i64> {
    let array = state
        .typed_arrays
        .get(&value::decode_handle(object))?
        .clone();
    if index >= view_length(state, &array).unwrap_or(0) {
        return None;
    }
    if let Some(storage) = &array.storage {
        let encoded = storage.borrow()[array.offset + index];
        if array.kind.is_bigint() && !value::is_bigint(encoded) {
            return super::bigint::store(state, BigInt::from(0));
        }
        return Some(encoded);
    }
    if !array.kind.is_bigint() {
        return get_element(state, object, index);
    }
    let size = array.kind.element_size();
    let start = (array.offset + index).checked_mul(size)?;
    let raw = if let Some(shared) = &array.shared_buffer {
        let bytes = shared.lock().ok()?;
        bytes.get(start..start + size)?.to_vec()
    } else {
        let buffer = array.buffer.as_ref()?;
        buffer.borrow().get(start..start + size)?.to_vec()
    };
    decode_bigint_element(state, &raw, array.kind)
}

fn get_element_from(state: &NativeAgentState, object: i64, index: usize) -> Option<i64> {
    let array = state.typed_arrays.get(&value::decode_handle(object))?;
    if index >= view_length(state, array).unwrap_or(0) {
        return None;
    }
    if let Some(storage) = &array.storage {
        return Some(storage.borrow()[array.offset + index]);
    }
    if let Some(shared) = &array.shared_buffer {
        let start = (array.offset + index).checked_mul(array.kind.element_size())?;
        let bytes = shared.lock().ok()?;
        let raw = bytes.get(start..start + array.kind.element_size())?;
        return decode_buffer_element(raw, array.kind);
    }
    let buffer = array.buffer.as_ref()?;
    let start = (array.offset + index).checked_mul(array.kind.element_size())?;
    let bytes = buffer.borrow();
    let raw = bytes.get(start..start + array.kind.element_size())?;
    decode_buffer_element(raw, array.kind)
}

pub(crate) fn visible_bytes(state: &NativeAgentState, object: i64) -> Option<Vec<u8>> {
    let array = state.typed_arrays.get(&value::decode_handle(object))?;
    let current_length = view_length(state, array).unwrap_or(0);
    let byte_offset = array.offset.checked_mul(array.kind.element_size())?;
    let byte_length = current_length.checked_mul(array.kind.element_size())?;
    if let Some(buffer) = &array.buffer {
        return buffer
            .borrow()
            .get(byte_offset..byte_offset.checked_add(byte_length)?)
            .map(<[u8]>::to_vec);
    }
    if let Some(shared) = &array.shared_buffer {
        return shared
            .lock()
            .ok()?
            .get(byte_offset..byte_offset.checked_add(byte_length)?)
            .map(<[u8]>::to_vec);
    }
    let storage = array.storage.as_ref()?.borrow();
    let mut bytes = vec![0; byte_length];
    for (index, encoded) in storage[array.offset..array.offset + current_length]
        .iter()
        .copied()
        .enumerate()
    {
        let start = index * array.kind.element_size();
        encode_buffer_element(
            state,
            &mut bytes[start..start + array.kind.element_size()],
            array.kind,
            encoded,
        )?;
    }
    Some(bytes)
}

pub(super) fn set_element(
    state: &mut NativeAgentState,
    object: i64,
    index: usize,
    stored: i64,
) -> Option<i64> {
    let array = state
        .typed_arrays
        .get(&value::decode_handle(object))?
        .clone();
    if index >= view_length(state, &array).unwrap_or(0) {
        return None;
    }
    let converted = convert_value(state, array.kind, stored)?;
    if let Some(storage) = array.storage {
        storage.borrow_mut()[array.offset + index] = converted;
    } else if let Some(shared) = array.shared_buffer {
        let start = (array.offset + index).checked_mul(array.kind.element_size())?;
        let mut bytes = shared.lock().ok()?;
        let destination = bytes.get_mut(start..start + array.kind.element_size())?;
        encode_buffer_element(state, destination, array.kind, converted)?;
    } else {
        let buffer = array.buffer?;
        let start = (array.offset + index).checked_mul(array.kind.element_size())?;
        let mut bytes = buffer.borrow_mut();
        let destination = bytes.get_mut(start..start + array.kind.element_size())?;
        encode_buffer_element(state, destination, array.kind, converted)?;
    }
    Some(converted)
}

/// 缺省内在构造器直建：与 Construct(%<kind>Array%, args) 等价的合流快路径
/// （newTarget 缺省，实例挂内在原型）。slice 缺省 species 与 from / of 的
/// 内在构造器 receiver 共用。
pub(super) fn construct_default(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
    kind: TypedArrayKind,
) -> i64 {
    construct(ctx, state, args, kind, value::encode_undefined())
}

/// TypedArray 构造器的可调用对象路径入口（类 extends 的 super()、
/// Reflect.construct）：newTarget 已由调用方归一（undefined 表示缺省形态，
/// 实例挂内在原型）。
pub(crate) fn construct_with_new_target(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    builtin: Builtin,
    args: &[i64],
    new_target: i64,
) -> i64 {
    let Some(kind) = constructor_kind(builtin) else {
        return fail_dispatch(ctx);
    };
    construct(ctx, state, args, kind, new_target)
}

fn construct(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
    kind: TypedArrayKind,
    new_target: i64,
) -> i64 {
    // §23.2.5.1 AllocateTypedArray 步骤 1：GetPrototypeFromConstructor 先于
    // 长度求值与缓冲分配，newTarget.prototype 的 getter（含 Proxy trap）
    // 异常先行传播。
    let proto_override =
        match super::typedarray_create::instance_prototype_slot(ctx, state, new_target) {
            Ok(slot) => slot,
            Err(exception) => return exception,
        };
    if let Some(sab) = args
        .first()
        .and_then(|encoded| {
            state
                .shared_array_buffers
                .get(&value::decode_handle(*encoded))
        })
        .cloned()
    {
        return construct_shared_buffer_view(ctx, state, args, kind, sab, proto_override);
    }
    if let Some(buffer) = args
        .first()
        .and_then(|encoded| state.array_buffers.get(&value::decode_handle(*encoded)))
        .cloned()
    {
        return construct_buffer_view(ctx, state, args, kind, buffer, proto_override);
    }
    let values = args.first().and_then(|input| array_values(state, *input));
    let length = if let Some(values) = &values {
        values.len()
    } else if let Some(input) = args.first().copied() {
        let Some(number) = to_number(state, input) else {
            return super::runtime::type_error(ctx, state, "Invalid typed array length");
        };
        if number.is_infinite() || number < 0.0 || number > usize::MAX as f64 {
            return range_error(ctx, state, "Invalid typed array length");
        }
        if number.is_nan() {
            0
        } else {
            number.trunc() as usize
        }
    } else {
        0
    };
    let byte_length = match length.checked_mul(kind.element_size()) {
        Some(byte_length) => byte_length,
        None => return range_error(ctx, state, "Invalid typed array length"),
    };
    let Some(buffer_object) = super::buffers::allocate_array_buffer(state, byte_length) else {
        return fail_dispatch(ctx);
    };
    let Some(buffer) = state
        .array_buffers
        .get(&value::decode_handle(buffer_object))
        .cloned()
    else {
        return fail_dispatch(ctx);
    };
    let Ok(object) = state.allocate_object_with_gc_retry(ctx, 2, false) else {
        return fail_dispatch(ctx);
    };
    if attach_prototype(state, object, kind, proto_override).is_none() {
        return fail_dispatch(ctx);
    }
    state.typed_arrays.insert(
        value::decode_handle(object),
        NativeTypedArray {
            kind,
            storage: None,
            buffer: Some(buffer.bytes),
            buffer_object: Some(buffer_object),
            shared_buffer: None,
            shared_backing_id: None,
            is_shared: false,
            offset: 0,
            length,
            length_tracking: false,
        },
    );
    if let Some(values) = values {
        for (index, stored) in values.into_iter().enumerate() {
            if set_element(state, object, index, stored).is_none() {
                return fail_dispatch(ctx);
            }
        }
    }
    object
}

fn construct_buffer_view(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
    kind: TypedArrayKind,
    buffer: NativeArrayBuffer,
    proto_override: Option<u32>,
) -> i64 {
    // §23.2.5.1（InitializeTypedArrayFromArrayBuffer 步骤 5）：detached
    // buffer 抛 TypeError（V8 文案）。
    if buffer.detached {
        return type_error(
            ctx,
            state,
            "Cannot perform Construct on a detached ArrayBuffer",
        );
    }
    let element_size = kind.element_size();
    let total_bytes = buffer.bytes.borrow().len();
    let Some(byte_offset) = args
        .get(1)
        .and_then(|encoded| to_number(state, *encoded))
        .and_then(|number| number.to_usize())
        .or(Some(0))
    else {
        return fail_dispatch(ctx);
    };
    // length 实参缺省且 buffer 可变长 → length-tracking 视图
    // （§23.2.5.1 步骤 7.b [[ArrayLength]] = auto）。
    let length_arg_absent = args
        .get(2)
        .is_none_or(|encoded| value::is_undefined(*encoded));
    let length_tracking = length_arg_absent && buffer.max_byte_length.is_some();
    let Some(length) = args
        .get(2)
        .and_then(|encoded| to_number(state, *encoded))
        .and_then(|number| number.to_usize())
        .or_else(|| {
            total_bytes
                .checked_sub(byte_offset)
                .map(|bytes| bytes / element_size)
        })
    else {
        return fail_dispatch(ctx);
    };
    if byte_offset % element_size != 0
        || byte_offset.saturating_add(length.saturating_mul(element_size)) > total_bytes
    {
        return fail_dispatch(ctx);
    }
    let Ok(object) = state.allocate_object_with_gc_retry(ctx, 2, false) else {
        return fail_dispatch(ctx);
    };
    if attach_prototype(state, object, kind, proto_override).is_none() {
        return fail_dispatch(ctx);
    }
    state.typed_arrays.insert(
        value::decode_handle(object),
        NativeTypedArray {
            kind,
            storage: None,
            buffer: Some(buffer.bytes),
            buffer_object: args.first().copied(),
            shared_buffer: None,
            shared_backing_id: None,
            is_shared: false,
            offset: byte_offset / element_size,
            length,
            length_tracking,
        },
    );
    object
}

fn construct_shared_buffer_view(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
    kind: TypedArrayKind,
    sab: super::sab::NativeSharedArrayBuffer,
    proto_override: Option<u32>,
) -> i64 {
    let element_size = kind.element_size();
    let total_bytes = sab.byte_length();
    let Some(byte_offset) = args
        .get(1)
        .and_then(|encoded| to_number(state, *encoded))
        .and_then(|number| number.to_usize())
        .or(Some(0))
    else {
        return fail_dispatch(ctx);
    };
    // length 实参缺省且 SAB 可增长 → length-tracking 视图（grow 后重算）。
    let length_arg_absent = args
        .get(2)
        .is_none_or(|encoded| value::is_undefined(*encoded));
    let length_tracking = length_arg_absent && sab.growable();
    let Some(length) = args
        .get(2)
        .and_then(|encoded| to_number(state, *encoded))
        .and_then(|number| number.to_usize())
        .or_else(|| {
            total_bytes
                .checked_sub(byte_offset)
                .map(|bytes| bytes / element_size)
        })
    else {
        return fail_dispatch(ctx);
    };
    if byte_offset % element_size != 0
        || byte_offset.saturating_add(length.saturating_mul(element_size)) > total_bytes
    {
        return fail_dispatch(ctx);
    }
    let Ok(object) = state.allocate_object_with_gc_retry(ctx, 2, false) else {
        return fail_dispatch(ctx);
    };
    if attach_prototype(state, object, kind, proto_override).is_none() {
        return fail_dispatch(ctx);
    }
    state.typed_arrays.insert(
        value::decode_handle(object),
        NativeTypedArray {
            kind,
            storage: None,
            buffer: None,
            buffer_object: args.first().copied(),
            shared_buffer: Some(sab.backing.bytes),
            shared_backing_id: Some(sab.backing_id),
            is_shared: true,
            offset: byte_offset / element_size,
            length,
            length_tracking,
        },
    );
    object
}

/// ValidateTypedArray（§23.2.4.4）：receiver 必须是登记在侧表中的 TypedArray
/// 实例且视图未 detach / 未越界；违反时按 V8 文案抛 `Cannot perform
/// %TypedArray%.prototype.{method} on a detached ArrayBuffer`（V8 对越界与
/// detach 共用 "detached" 措辞）。成功返回（条目快照, 当前长度）。
fn validated_view(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
    method: &str,
) -> Result<(NativeTypedArray, usize), i64> {
    let Some(array) = args
        .first()
        .and_then(|receiver| state.typed_arrays.get(&value::decode_handle(*receiver)))
        .cloned()
    else {
        return Err(fail_dispatch(ctx));
    };
    match view_length(state, &array) {
        Some(length) => Ok((array, length)),
        None => {
            let message =
                format!("Cannot perform %TypedArray%.prototype.{method} on a detached ArrayBuffer");
            Err(type_error(ctx, state, &message))
        }
    }
}

/// getter 的品牌检查（RequireInternalSlot(O, [[TypedArrayName]])，
/// §23.2.3.21 等）：receiver 必须是登记在侧表中的 TypedArray 实例。
fn receiver_typed_array<'a>(
    state: &'a NativeAgentState,
    args: &[i64],
) -> Option<&'a NativeTypedArray> {
    let object = args.first().copied()?;
    if !value::is_object(object) {
        return None;
    }
    state.typed_arrays.get(&value::decode_handle(object))
}

/// 品牌检查失败：按 V8 口径抛 `Method get TypedArray.prototype.<name>
/// called on incompatible receiver <receiver>` 的 TypeError。
fn incompatible_receiver(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    name: &str,
    receiver: Option<i64>,
) -> i64 {
    let rendered = render_getter_receiver(state, receiver);
    let message = format!(
        "Method get TypedArray.prototype.{name} called on incompatible receiver {rendered}"
    );
    type_error(ctx, state, &message)
}

/// getter 错误消息中的 receiver 渲染，对齐 V8 NoSideEffectsToString 的常见
/// 形态：基元按值渲染；数组 `[object Array]`；callable 用源文本；原型对象
/// （各 Ctor.prototype、%TypedArray%.prototype、%Object.prototype%）
/// `[object Object]`；DataView 实例 `#<DataView>`；其余对象 `#<Object>`。
pub(super) fn render_getter_receiver(state: &NativeAgentState, receiver: Option<i64>) -> String {
    let Some(receiver) = receiver else {
        return "undefined".into();
    };
    if value::is_array(receiver) {
        return "[object Array]".into();
    }
    if value::is_callable(receiver) {
        return state
            .callable_to_string_source(receiver)
            .unwrap_or_else(|| "function () { [native code] }".into());
    }
    if value::is_js_object(receiver) || value::is_proxy(receiver) {
        let handle = value::decode_handle(receiver);
        if state.is_typed_array_prototype(handle)
            || state.object_prototype == Some(receiver)
            || state
                .view_prototypes
                .values()
                .any(|prototype| *prototype == receiver)
        {
            return "[object Object]".into();
        }
        if state.data_views.contains_key(&handle) {
            return "#<DataView>".into();
        }
        return "#<Object>".into();
    }
    render_value(state, receiver)
}

fn property_length(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    // §23.2.3.21.1：越界 / detach 视图返回 +0，tracking 视图随底层重算。
    let Some(length) =
        receiver_typed_array(state, args).map(|array| view_length(state, array).unwrap_or(0))
    else {
        return incompatible_receiver(ctx, state, "length", args.first().copied());
    };
    u32::try_from(length)
        .ok()
        .map(|length| value::encode_f64(f64::from(length)))
        .unwrap_or_else(|| fail_dispatch(ctx))
}

fn property_byte_length(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    // §23.2.3.2：越界 / detach 视图返回 +0（当前长度 × 元素宽）。
    let Some(byte_length) = receiver_typed_array(state, args).map(|array| {
        view_length(state, array)
            .unwrap_or(0)
            .checked_mul(array.kind.element_size())
    }) else {
        return incompatible_receiver(ctx, state, "byteLength", args.first().copied());
    };
    byte_length
        .and_then(|length| u32::try_from(length).ok())
        .map(|length| value::encode_f64(f64::from(length)))
        .unwrap_or_else(|| fail_dispatch(ctx))
}

fn property_byte_offset(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    // §23.2.3.3：越界 / detach 视图返回 +0。
    let Some(byte_offset) = receiver_typed_array(state, args).map(|array| {
        if view_length(state, array).is_none() {
            Some(0)
        } else {
            array.offset.checked_mul(array.kind.element_size())
        }
    }) else {
        return incompatible_receiver(ctx, state, "byteOffset", args.first().copied());
    };
    byte_offset
        .and_then(|offset| u32::try_from(offset).ok())
        .map(|offset| value::encode_f64(f64::from(offset)))
        .unwrap_or_else(|| fail_dispatch(ctx))
}

fn set(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let (_, target_length) = match validated_view(ctx, state, args, "set") {
        Ok(validated) => validated,
        Err(exception) => return exception,
    };
    let [receiver, source, offset] = [
        args.first()
            .copied()
            .unwrap_or_else(value::encode_undefined),
        args.get(1).copied().unwrap_or_else(value::encode_undefined),
        args.get(2)
            .copied()
            .unwrap_or_else(|| value::encode_f64(0.0)),
    ];
    let Some(offset) = to_number(state, offset).and_then(|number| number.to_usize()) else {
        return fail_dispatch(ctx);
    };
    let Some(values) = array_values(state, source) else {
        return fail_dispatch(ctx);
    };
    if offset.saturating_add(values.len()) > target_length {
        return fail_dispatch(ctx);
    }
    for (index, stored) in values.into_iter().enumerate() {
        if set_element(state, receiver, offset + index, stored).is_none() {
            return fail_dispatch(ctx);
        }
    }
    value::encode_undefined()
}

fn slice(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(receiver) = args.first().copied() else {
        return fail_dispatch(ctx);
    };
    let (array, current_length) = match validated_view(ctx, state, args, "slice") {
        Ok(validated) => validated,
        Err(exception) => return exception,
    };
    let start = relative_index(state, args.get(1).copied(), current_length);
    let end = args.get(2).map_or(current_length, |encoded| {
        relative_index(state, Some(*encoded), current_length)
    });
    let first = start.min(end);
    let count = end.min(current_length).saturating_sub(first);
    // §23.2.3.24 步骤 9：A = TypedArraySpeciesCreate(O, «𝔽(count)») 先于
    // 元素读取（species 构造器可再入用户代码改写源）；构造出的长度不足
    // count 抛 TypeError（§23.2.4.1 步骤 3）。
    let decision =
        match super::typedarray_create::species_constructor(ctx, state, receiver, array.kind) {
            Ok(decision) => decision,
            Err(exception) => return exception,
        };
    let target = match decision {
        super::typedarray_create::SpeciesDecision::Default => {
            let object =
                construct_default(ctx, state, &[value::encode_f64(count as f64)], array.kind);
            if value::is_exception(object) {
                return object;
            }
            object
        }
        super::typedarray_create::SpeciesDecision::Construct(constructor) => {
            match super::typedarray_create::species_create(
                ctx,
                state,
                array.kind,
                constructor,
                &[value::encode_f64(count as f64)],
                "slice",
                Some(count),
            ) {
                Ok(result) => result,
                Err(exception) => return exception,
            }
        }
    };
    // 步骤 14：逐元素复制；跨元素类型经 set_element 的 ToNumber/ToBigInt
    // 转换。复制期间 BigInt intern 可分配触发 GC，target 锚根。
    let initial_temp_roots = state.temporary_roots.len();
    state.temporary_roots.push(target);
    for index in 0..count {
        let Some(stored) = get_element_intern(state, receiver, first + index) else {
            break;
        };
        if set_element(state, target, index, stored).is_none() {
            state.temporary_roots.truncate(initial_temp_roots);
            return fail_dispatch(ctx);
        }
    }
    state.temporary_roots.truncate(initial_temp_roots);
    target
}

fn subarray(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(receiver) = args.first().copied() else {
        return fail_dispatch(ctx);
    };
    let Some(array) = state
        .typed_arrays
        .get(&value::decode_handle(receiver))
        .cloned()
    else {
        return fail_dispatch(ctx);
    };
    // §23.2.3.28 无 ValidateTypedArray：srcLength 对越界视图取 0；但底层
    // detach 时 TypedArraySpeciesCreate 的 Construct 会失败（V8 文案）。
    let src_length = match view_length(state, &array) {
        Some(length) => length,
        None if array.buffer_object.is_some_and(|buffer_object| {
            state
                .array_buffers
                .get(&value::decode_handle(buffer_object))
                .is_some_and(|entry| entry.detached)
        }) =>
        {
            return type_error(
                ctx,
                state,
                "Cannot perform Construct on a detached ArrayBuffer",
            );
        }
        None => 0,
    };
    // §23.2.3.28 步骤 13：源为 length-tracking 且 end 缺省时结果亦 tracking。
    let tracking = array.length_tracking
        && args
            .get(2)
            .is_none_or(|encoded| value::is_undefined(*encoded));
    let start = relative_index(state, args.get(1).copied(), src_length);
    let end = args.get(2).map_or(src_length, |encoded| {
        relative_index(state, Some(*encoded), src_length)
    });
    let begin = start.min(end);
    let new_length = end
        .saturating_sub(start)
        .min(src_length.saturating_sub(start));
    // §23.2.3.28 步骤 13–14：TypedArraySpeciesCreate(O, «buffer,
    // 𝔽(beginByteOffset), 𝔽(newLength)»)。缺省构造器合流快路径：直接共享
    // 底层 backing 建视图，与 Construct(default, «buffer, offset, len») 等价。
    let decision =
        match super::typedarray_create::species_constructor(ctx, state, receiver, array.kind) {
            Ok(decision) => decision,
            Err(exception) => return exception,
        };
    if let super::typedarray_create::SpeciesDecision::Construct(constructor) = decision {
        return subarray_species(ctx, state, receiver, &array, constructor, begin, new_length);
    }
    let Ok(object) = state.allocate_object_with_gc_retry(ctx, 2, false) else {
        return fail_dispatch(ctx);
    };
    if attach_instance_prototype(state, object, array.kind).is_none() {
        return fail_dispatch(ctx);
    }
    state.typed_arrays.insert(
        value::decode_handle(object),
        NativeTypedArray {
            kind: array.kind,
            storage: array.storage,
            buffer: array.buffer,
            buffer_object: array.buffer_object,
            shared_buffer: array.shared_buffer,
            shared_backing_id: array.shared_backing_id,
            is_shared: array.is_shared,
            offset: array.offset + begin,
            length: new_length,
            length_tracking: tracking,
        },
    );
    object
}

/// subarray 的自定义 species 路径：以规范实参 «buffer, 𝔽(beginByteOffset),
/// 𝔽(newLength)» 执行 Construct。宿主内部 storage 视图（流 / 编码器产物）
/// 无 [[ViewedArrayBuffer]] 对象，物化当前可见字节为新 ArrayBuffer 传入
/// （内容一致；Node 无此形态，别名关系无从对照）。
fn subarray_species(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    receiver: i64,
    array: &NativeTypedArray,
    constructor: i64,
    begin: usize,
    new_length: usize,
) -> i64 {
    let element_size = array.kind.element_size();
    let (buffer, begin_byte_offset) = match array.buffer_object {
        Some(buffer) => (buffer, (array.offset + begin) * element_size),
        None => {
            let Some(bytes) = visible_bytes(state, receiver) else {
                return fail_dispatch(ctx);
            };
            let Some(buffer) = super::buffers::allocate_array_buffer(state, bytes.len()) else {
                return fail_dispatch(ctx);
            };
            let Some(native) = state.array_buffers.get(&value::decode_handle(buffer)) else {
                return fail_dispatch(ctx);
            };
            native.bytes.borrow_mut().copy_from_slice(&bytes);
            (buffer, begin * element_size)
        }
    };
    // Construct 再入用户代码可触发 GC，buffer 实参锚根。
    let initial_temp_roots = state.temporary_roots.len();
    state.temporary_roots.push(buffer);
    let result = super::typedarray_create::species_create(
        ctx,
        state,
        array.kind,
        constructor,
        &[
            buffer,
            value::encode_f64(begin_byte_offset as f64),
            value::encode_f64(new_length as f64),
        ],
        "subarray",
        None,
    );
    state.temporary_roots.truncate(initial_temp_roots);
    match result {
        Ok(result) => result,
        Err(exception) => exception,
    }
}

fn fill(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(receiver) = args.first().copied() else {
        return fail_dispatch(ctx);
    };
    let (_, current_length) = match validated_view(ctx, state, args, "fill") {
        Ok(validated) => validated,
        Err(exception) => return exception,
    };
    let start = relative_index(state, args.get(2).copied(), current_length);
    let end = args.get(3).map_or(current_length, |encoded| {
        relative_index(state, Some(*encoded), current_length)
    });
    for index in start.min(end)..end.min(current_length) {
        if set_element(
            state,
            receiver,
            index,
            args.get(1).copied().unwrap_or_else(value::encode_undefined),
        )
        .is_none()
        {
            return fail_dispatch(ctx);
        }
    }
    receiver
}

fn reverse(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(receiver) = args.first().copied() else {
        return fail_dispatch(ctx);
    };
    let (_, length) = match validated_view(ctx, state, args, "reverse") {
        Ok(validated) => validated,
        Err(exception) => return exception,
    };
    for index in 0..length / 2 {
        let right = length - index - 1;
        let (Some(left), Some(right_value)) = (
            get_element_intern(state, receiver, index),
            get_element_intern(state, receiver, right),
        ) else {
            return fail_dispatch(ctx);
        };
        if set_element(state, receiver, index, right_value).is_none()
            || set_element(state, receiver, right, left).is_none()
        {
            return fail_dispatch(ctx);
        }
    }
    receiver
}

fn index_of(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
    reverse: bool,
) -> i64 {
    let Some(receiver) = args.first().copied() else {
        return fail_dispatch(ctx);
    };
    let method = if reverse { "lastIndexOf" } else { "indexOf" };
    let (_, length) = match validated_view(ctx, state, args, method) {
        Ok(validated) => validated,
        Err(exception) => return exception,
    };
    let from = args
        .get(2)
        .and_then(|encoded| to_number(state, *encoded))
        .unwrap_or(if reverse {
            length.saturating_sub(1) as f64
        } else {
            0.0
        });
    let from = if from.is_nan() { 0.0 } else { from.trunc() };
    if reverse && from < -(length as f64) {
        return value::encode_f64(-1.0);
    }
    let start = if from < 0.0 {
        (length as f64 + from).max(0.0) as usize
    } else if from.is_infinite() {
        if from.is_sign_positive() { length } else { 0 }
    } else if reverse {
        (from as usize).min(length.saturating_sub(1))
    } else {
        from as usize
    };
    let needle = args.get(1).copied().unwrap_or_else(value::encode_undefined);
    let found = if reverse {
        (0..length).rev().find(|index| {
            *index <= start
                && get_element_intern(state, receiver, *index)
                    .is_some_and(|stored| strict_equal(state, stored, needle))
        })
    } else {
        (start..length).find(|index| {
            get_element_intern(state, receiver, *index)
                .is_some_and(|stored| strict_equal(state, stored, needle))
        })
    };
    found.map_or_else(
        || value::encode_f64(-1.0),
        |index| value::encode_f64(index as f64),
    )
}

fn includes(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(receiver) = args.first().copied() else {
        return fail_dispatch(ctx);
    };
    let (_, length) = match validated_view(ctx, state, args, "includes") {
        Ok(validated) => validated,
        Err(exception) => return exception,
    };
    let needle = args.get(1).copied().unwrap_or_else(value::encode_undefined);
    value::encode_bool((0..length).any(|index| {
        get_element_intern(state, receiver, index).is_some_and(|stored| {
            if value::is_f64(stored) && value::is_f64(needle) {
                let left = value::decode_f64(stored);
                let right = value::decode_f64(needle);
                left == right || left.is_nan() && right.is_nan()
            } else {
                strict_equal(state, stored, needle)
            }
        })
    }))
}

fn join(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(receiver) = args.first().copied() else {
        return fail_dispatch(ctx);
    };
    // toString（§23.2.3.32）内部调 join，V8 对二者统一报 join 的文案。
    let (_, length) = match validated_view(ctx, state, args, "join") {
        Ok(validated) => validated,
        Err(exception) => return exception,
    };
    let separator = args
        .get(1)
        .map_or_else(|| ",".into(), |encoded| render_value(state, *encoded));

    let mut output = String::new();
    for index in 0..length {
        if index > 0 {
            output.push_str(&separator);
        }
        let Some(stored) = get_element_intern(state, receiver, index) else {
            return fail_dispatch(ctx);
        };
        output.push_str(&render_value(state, stored));
    }

    state
        .intern_text(output, value::TAG_STRING)
        .unwrap_or_else(|| fail_dispatch(ctx))
}

fn copy_within(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(receiver) = args.first().copied() else {
        return fail_dispatch(ctx);
    };
    let (_, length) = match validated_view(ctx, state, args, "copyWithin") {
        Ok(validated) => validated,
        Err(exception) => return exception,
    };
    let target = relative_index(state, args.get(1).copied(), length);
    let start = relative_index(state, args.get(2).copied(), length);
    let end = args.get(3).map_or(length, |encoded| {
        relative_index(state, Some(*encoded), length)
    });
    let count = end.saturating_sub(start).min(length.saturating_sub(target));
    let values = (0..count)
        .filter_map(|index| get_element_intern(state, receiver, start + index))
        .collect::<Vec<_>>();
    for (index, stored) in values.into_iter().enumerate() {
        if set_element(state, receiver, target + index, stored).is_none() {
            return fail_dispatch(ctx);
        }
    }
    receiver
}

fn at(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(receiver) = args.first().copied() else {
        return fail_dispatch(ctx);
    };
    let (_, length) = match validated_view(ctx, state, args, "at") {
        Ok(validated) => validated,
        Err(exception) => return exception,
    };
    let Some(index) = args
        .get(1)
        .and_then(|encoded| to_number(state, *encoded))
        .and_then(|number| number.to_isize())
    else {
        return value::encode_undefined();
    };
    let length = isize::try_from(length).unwrap_or(isize::MAX);
    let index = if index < 0 { length + index } else { index };

    usize::try_from(index)
        .ok()
        .and_then(|index| get_element_intern(state, receiver, index))
        .unwrap_or_else(value::encode_undefined)
}

#[derive(Clone, Copy)]
enum CallbackKind {
    Every,
    Filter,
    Find,
    FindIndex,
    ForEach,
    Map,
    Some,
}

fn callback_iterate(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
    kind: CallbackKind,
) -> i64 {
    let Some(receiver) = args.first().copied() else {
        return fail_dispatch(ctx);
    };
    let method = match kind {
        CallbackKind::Every => "every",
        CallbackKind::Filter => "filter",
        CallbackKind::Find => "find",
        CallbackKind::FindIndex => "findIndex",
        CallbackKind::ForEach => "forEach",
        CallbackKind::Map => "map",
        CallbackKind::Some => "some",
    };
    let (_, length) = match validated_view(ctx, state, args, method) {
        Ok(validated) => validated,
        Err(exception) => return exception,
    };
    let Some(callback) = args
        .get(1)
        .copied()
        .filter(|value| value::is_callable(*value))
    else {
        return fail_dispatch(ctx);
    };
    let mut output_values = Vec::new();
    for index in 0..length {
        let Some(stored) = get_element_intern(state, receiver, index) else {
            return fail_dispatch(ctx);
        };
        let callback_result = state
            .invoke_callable(
                ctx,
                callback,
                args.get(2).copied().unwrap_or_else(value::encode_undefined),
                &[stored, value::encode_f64(index as f64), receiver],
            )
            .unwrap_or_else(|| fail_dispatch(ctx));
        if value::is_exception(callback_result) {
            return callback_result;
        }
        match kind {
            CallbackKind::Every if !is_truthy(state, callback_result) => {
                return value::encode_bool(false);
            }
            CallbackKind::Some if is_truthy(state, callback_result) => {
                return value::encode_bool(true);
            }
            CallbackKind::Find if is_truthy(state, callback_result) => return stored,
            CallbackKind::FindIndex if is_truthy(state, callback_result) => {
                return value::encode_f64(index as f64);
            }
            CallbackKind::Filter if is_truthy(state, callback_result) => {
                output_values.push(stored);
            }
            CallbackKind::Map => output_values.push(callback_result),
            CallbackKind::ForEach | CallbackKind::Every | CallbackKind::Some => {}
            _ => {}
        }
    }
    match kind {
        CallbackKind::Every => value::encode_bool(true),
        CallbackKind::Some => value::encode_bool(false),
        CallbackKind::Find => value::encode_undefined(),
        CallbackKind::FindIndex => value::encode_f64(-1.0),
        CallbackKind::ForEach => value::encode_undefined(),
        CallbackKind::Map | CallbackKind::Filter => state
            .allocate_array_values_with_gc_retry(ctx, &output_values)
            .unwrap_or_else(|_| fail_dispatch(ctx)),
    }
}

fn reduce(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
    reverse: bool,
) -> i64 {
    let Some(receiver) = args.first().copied() else {
        return fail_dispatch(ctx);
    };
    let method = if reverse { "reduceRight" } else { "reduce" };
    let (_, length) = match validated_view(ctx, state, args, method) {
        Ok(validated) => validated,
        Err(exception) => return exception,
    };
    let Some(callback) = args
        .get(1)
        .copied()
        .filter(|value| value::is_callable(*value))
    else {
        return fail_dispatch(ctx);
    };
    let indices = if reverse {
        (0..length).rev().collect::<Vec<_>>()
    } else {
        (0..length).collect::<Vec<_>>()
    };
    let mut iter = indices.into_iter();
    let mut accumulator = if let Some(initial) = args.get(2).copied() {
        initial
    } else {
        let Some(index) = iter.next() else {
            return type_error(ctx, state, "Reduce of empty array with no initial value");
        };
        get_element_intern(state, receiver, index).unwrap_or_else(value::encode_undefined)
    };
    for index in iter {
        let Some(stored) = get_element_intern(state, receiver, index) else {
            return fail_dispatch(ctx);
        };
        accumulator = state
            .invoke_callable(
                ctx,
                callback,
                value::encode_undefined(),
                &[
                    accumulator,
                    stored,
                    value::encode_f64(index as f64),
                    receiver,
                ],
            )
            .unwrap_or_else(|| fail_dispatch(ctx));
        if value::is_exception(accumulator) {
            return accumulator;
        }
    }
    accumulator
}

fn sort(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(receiver) = args.first().copied() else {
        return fail_dispatch(ctx);
    };
    let (_, length) = match validated_view(ctx, state, args, "sort") {
        Ok(validated) => validated,
        Err(exception) => return exception,
    };
    let comparator = args
        .get(1)
        .copied()
        .filter(|value| !value::is_undefined(*value));
    if comparator.is_some_and(|comparator| !value::is_callable(comparator)) {
        return super::runtime::type_error(ctx, state, "compare function must be callable");
    }
    let mut values = (0..length)
        .filter_map(|index| get_element_intern(state, receiver, index))
        .collect::<Vec<_>>();
    let mut exception = None;
    values.sort_by(|left, right| {
        if exception.is_some() {
            return Ordering::Equal;
        }
        if let Some(comparator) = comparator {
            let result = state
                .invoke_callable(ctx, comparator, value::encode_undefined(), &[*left, *right])
                .unwrap_or_else(|| fail_dispatch(ctx));
            if value::is_exception(result) {
                exception = Some(result);
                return Ordering::Equal;
            }
            return to_number(state, result)
                .filter(|number| !number.is_nan())
                .map_or(Ordering::Equal, |number| {
                    if number < 0.0 {
                        Ordering::Less
                    } else if number > 0.0 {
                        Ordering::Greater
                    } else {
                        Ordering::Equal
                    }
                });
        }
        if value::is_bigint(*left) && value::is_bigint(*right) {
            return super::bigint::read(state, *left)
                .zip(super::bigint::read(state, *right))
                .map_or(Ordering::Equal, |(left, right)| left.cmp(&right));
        }
        let left = value::decode_f64(*left);
        let right = value::decode_f64(*right);
        match (left.is_nan(), right.is_nan()) {
            (true, true) => Ordering::Equal,
            (true, false) => Ordering::Greater,
            (false, true) => Ordering::Less,
            (false, false) if left == 0.0 && right == 0.0 => {
                right.is_sign_negative().cmp(&left.is_sign_negative())
            }
            (false, false) => left.partial_cmp(&right).unwrap_or(Ordering::Equal),
        }
    });
    if let Some(exception) = exception {
        return exception;
    }
    for (index, stored) in values.into_iter().enumerate() {
        if set_element(state, receiver, index, stored).is_none() {
            return fail_dispatch(ctx);
        }
    }
    receiver
}

/// %TypedArray%.prototype 的 keys / values / entries（§23.2.3.16 等按
/// CreateArrayIterator）：实例进 `array_iterators` 侧表并接线
/// %ArrayIteratorPrototype% 真实原型。
fn iterator(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
    kind: crate::NativeIteratorKind,
) -> i64 {
    let Some(receiver) = args.first().copied() else {
        return fail_dispatch(ctx);
    };
    let handle = value::decode_handle(receiver);
    let method = match kind {
        crate::NativeIteratorKind::Entries => "entries",
        crate::NativeIteratorKind::Keys => "keys",
        crate::NativeIteratorKind::Values => "values",
    };
    if let Err(exception) = validated_view(ctx, state, args, method) {
        return exception;
    }
    let family = super::iterator_prototypes::NativeIteratorFamily::Array;
    if super::iterator_prototypes::ensure_prototype(state, family).is_none() {
        return fail_dispatch(ctx);
    }
    let Ok(iterator_object) = state.allocate_object_with_gc_retry(ctx, 0, false) else {
        return fail_dispatch(ctx);
    };
    if let Err(exception) = super::iterator_prototypes::attach(ctx, state, iterator_object, family)
    {
        return exception;
    }
    state.array_iterators.insert(
        value::decode_handle(iterator_object),
        crate::NativeArrayIterator {
            source: crate::NativeIteratorSource::TypedArray(handle),
            kind,
            index: 0,
            current: None,
            done: false,
        },
    );
    iterator_object
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

fn array_values(state: &mut NativeAgentState, encoded: i64) -> Option<Vec<i64>> {
    if value::is_array(encoded) {
        let handle = value::decode_handle(encoded);
        let length = state.gc.heap().array_length(handle).ok()?;
        return (0..length)
            .map(|index| {
                state
                    .gc
                    .heap()
                    .get_element(handle, index)
                    .ok()
                    .flatten()
                    .map(|stored| i64::from_ne_bytes(stored.to_ne_bytes()))
            })
            .collect();
    }
    if value::is_js_object(encoded) {
        let entry = state.typed_arrays.get(&value::decode_handle(encoded))?;
        let length = view_length(state, entry).unwrap_or(0);
        return (0..length)
            .map(|index| get_element_intern(state, encoded, index))
            .collect();
    }
    None
}

fn convert_value(state: &mut NativeAgentState, kind: TypedArrayKind, encoded: i64) -> Option<i64> {
    if kind.is_bigint() {
        let input = super::bigint::read(state, encoded)?;
        let modulus = num_bigint::BigInt::from(1_u128 << 64);
        let mut normalized = input % &modulus;
        if normalized.sign() == num_bigint::Sign::Minus {
            normalized += &modulus;
        }
        if matches!(kind, TypedArrayKind::BigInt64)
            && normalized >= num_bigint::BigInt::from(1_u128 << 63)
        {
            normalized -= &modulus;
        }
        return super::bigint::store(state, normalized);
    }
    let number = to_number(state, encoded)?;
    let converted = match kind {
        TypedArrayKind::Int8 => signed_integer(number, 8),
        TypedArrayKind::Uint8 => unsigned_integer(number, 8),
        TypedArrayKind::Uint8Clamped => to_uint8_clamp(number),
        TypedArrayKind::Int16 => signed_integer(number, 16),
        TypedArrayKind::Uint16 => unsigned_integer(number, 16),
        TypedArrayKind::Int32 => signed_integer(number, 32),
        TypedArrayKind::Uint32 => unsigned_integer(number, 32),
        TypedArrayKind::Float32 => (number as f32) as f64,
        TypedArrayKind::Float64 => number,
        TypedArrayKind::BigInt64 | TypedArrayKind::BigUint64 => unreachable!(),
    };
    Some(value::encode_f64(converted))
}

fn to_uint8_clamp(number: f64) -> f64 {
    if number.is_nan() || number <= 0.0 {
        return 0.0;
    }
    if number >= 255.0 {
        return 255.0;
    }
    let floor = number.floor();
    let fraction = number - floor;
    if fraction > 0.5 || fraction == 0.5 && floor % 2.0 != 0.0 {
        floor + 1.0
    } else {
        floor
    }
}

fn unsigned_integer(number: f64, bits: u32) -> f64 {
    if !number.is_finite() || number == 0.0 {
        return 0.0;
    }
    number.trunc().rem_euclid(2_f64.powi(bits as i32))
}

fn signed_integer(number: f64, bits: u32) -> f64 {
    let unsigned = unsigned_integer(number, bits);
    let sign = 2_f64.powi((bits - 1) as i32);
    if unsigned >= sign {
        unsigned - 2.0 * sign
    } else {
        unsigned
    }
}
fn decode_bigint_element(
    state: &mut NativeAgentState,
    bytes: &[u8],
    kind: TypedArrayKind,
) -> Option<i64> {
    let raw: [u8; 8] = bytes.try_into().ok()?;
    let bits = u64::from_ne_bytes(raw);
    let value = match kind {
        TypedArrayKind::BigInt64 => BigInt::from(bits as i64),
        TypedArrayKind::BigUint64 => BigInt::from(bits),
        _ => return None,
    };
    super::bigint::store(state, value)
}

fn decode_buffer_element(bytes: &[u8], kind: TypedArrayKind) -> Option<i64> {
    let value = match kind {
        TypedArrayKind::Int8 => i8::from_ne_bytes([bytes[0]]) as f64,
        TypedArrayKind::Uint8 | TypedArrayKind::Uint8Clamped => f64::from(bytes[0]),
        TypedArrayKind::Int16 => i16::from_ne_bytes(bytes.try_into().ok()?) as f64,
        TypedArrayKind::Uint16 => f64::from(u16::from_ne_bytes(bytes.try_into().ok()?)),
        TypedArrayKind::Int32 => i32::from_ne_bytes(bytes.try_into().ok()?) as f64,
        TypedArrayKind::Uint32 => u32::from_ne_bytes(bytes.try_into().ok()?) as f64,
        TypedArrayKind::Float32 => f32::from_ne_bytes(bytes.try_into().ok()?) as f64,
        TypedArrayKind::Float64 => f64::from_ne_bytes(bytes.try_into().ok()?),
        TypedArrayKind::BigInt64 | TypedArrayKind::BigUint64 => return None,
    };
    Some(value::encode_f64(value))
}

fn encode_buffer_element(
    state: &NativeAgentState,
    destination: &mut [u8],
    kind: TypedArrayKind,
    encoded: i64,
) -> Option<()> {
    if kind.is_bigint() {
        let bigint = super::bigint::read(state, encoded)?;
        destination.copy_from_slice(&bigint_element_bits(&bigint).to_ne_bytes());
        return Some(());
    }
    let number = value::decode_f64(encoded);
    match kind {
        TypedArrayKind::Int8 => destination.copy_from_slice(&(number as i8).to_ne_bytes()),
        TypedArrayKind::Uint8 | TypedArrayKind::Uint8Clamped => {
            destination.copy_from_slice(&(number as u8).to_ne_bytes());
        }
        TypedArrayKind::Int16 => destination.copy_from_slice(&(number as i16).to_ne_bytes()),
        TypedArrayKind::Uint16 => destination.copy_from_slice(&(number as u16).to_ne_bytes()),
        TypedArrayKind::Int32 => destination.copy_from_slice(&(number as i32).to_ne_bytes()),
        TypedArrayKind::Uint32 => destination.copy_from_slice(&(number as u32).to_ne_bytes()),
        TypedArrayKind::Float32 => destination.copy_from_slice(&(number as f32).to_ne_bytes()),
        TypedArrayKind::Float64 => destination.copy_from_slice(&number.to_ne_bytes()),
        TypedArrayKind::BigInt64 | TypedArrayKind::BigUint64 => unreachable!(),
    }
    Some(())
}

fn bigint_element_bits(value: &BigInt) -> u64 {
    let modulus = BigInt::from(1u128 << 64);
    let mut normalized = value % &modulus;
    if normalized.sign() == Sign::Minus {
        normalized += &modulus;
    }
    normalized.to_u64().unwrap_or(0)
}

fn range_error(ctx: &mut NativeVmContext, state: &mut NativeAgentState, message: &str) -> i64 {
    super::runtime::range_error(ctx, state, message)
}
