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
}

pub(crate) fn create_uint8_array(state: &mut NativeAgentState, bytes: &[u8]) -> Option<i64> {
    let object = state.allocate_object(2, false).ok()?;
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
        },
    );
    Some(object)
}

pub(crate) fn prefix_view(state: &mut NativeAgentState, view: i64, length: usize) -> Option<i64> {
    let handle = value::decode_handle(view);
    let count = state.typed_arrays.get(&handle)?.length.min(length);
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
    array.length.checked_mul(array.kind.element_size())
}
pub(crate) enum CloneView {
    Values(Vec<i64>),
    ArrayBuffer {
        buffer: i64,
        offset: usize,
        length: usize,
    },
    SharedArrayBuffer {
        object: i64,
        offset: usize,
        length: usize,
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
    if array.buffer.is_some() {
        if let Some(buffer_object) = array.buffer_object {
            return Some((
                array.kind,
                CloneView::ArrayBuffer {
                    buffer: buffer_object,
                    offset: array.offset,
                    length: array.length,
                },
            ));
        }
        return Some((
            array.kind,
            CloneView::Values(
                (0..array.length)
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
            length: array.length,
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
) -> Option<i64> {
    let buffer_handle = value::decode_handle(buffer);
    let shared = state.shared_array_buffers.get(&buffer_handle).cloned()?;
    if offset.checked_add(length)? > shared.byte_length() / kind.element_size() {
        return None;
    }
    let object = state.allocate_object(2, false).ok()?;
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
    Some(match builtin {
        Builtin::Int8ArrayConstructor => construct(ctx, state, args, TypedArrayKind::Int8),
        Builtin::Uint8ArrayConstructor => construct(ctx, state, args, TypedArrayKind::Uint8),
        Builtin::Uint8ClampedArrayConstructor => {
            construct(ctx, state, args, TypedArrayKind::Uint8Clamped)
        }
        Builtin::Int16ArrayConstructor => construct(ctx, state, args, TypedArrayKind::Int16),
        Builtin::Uint16ArrayConstructor => construct(ctx, state, args, TypedArrayKind::Uint16),
        Builtin::Int32ArrayConstructor => construct(ctx, state, args, TypedArrayKind::Int32),
        Builtin::Uint32ArrayConstructor => construct(ctx, state, args, TypedArrayKind::Uint32),
        Builtin::Float32ArrayConstructor => construct(ctx, state, args, TypedArrayKind::Float32),
        Builtin::Float64ArrayConstructor => construct(ctx, state, args, TypedArrayKind::Float64),
        Builtin::BigInt64ArrayConstructor => construct(ctx, state, args, TypedArrayKind::BigInt64),
        Builtin::BigUint64ArrayConstructor => {
            construct(ctx, state, args, TypedArrayKind::BigUint64)
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
        Builtin::TypedArrayProtoEntries => iterator(
            ctx,
            state,
            args,
            super::collections::CollectionIteratorKind::Entries,
        ),
        Builtin::TypedArrayProtoKeys => iterator(
            ctx,
            state,
            args,
            super::collections::CollectionIteratorKind::Keys,
        ),
        Builtin::TypedArrayProtoValues => iterator(
            ctx,
            state,
            args,
            super::collections::CollectionIteratorKind::Values,
        ),
        _ => return None,
    })
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
    Some(match key {
        "at" => Builtin::TypedArrayProtoAt,
        "byteLength" => Builtin::TypedArrayProtoByteLength,
        "byteOffset" => Builtin::TypedArrayProtoByteOffset,
        "copyWithin" => Builtin::TypedArrayProtoCopyWithin,
        "entries" => Builtin::TypedArrayProtoEntries,
        "every" => Builtin::TypedArrayProtoEvery,
        "fill" => Builtin::TypedArrayProtoFill,
        "filter" => Builtin::TypedArrayProtoFilter,
        "find" => Builtin::TypedArrayProtoFind,
        "findIndex" => Builtin::TypedArrayProtoFindIndex,
        "forEach" => Builtin::TypedArrayProtoForEach,
        "includes" => Builtin::TypedArrayProtoIncludes,
        "indexOf" => Builtin::TypedArrayProtoIndexOf,
        "join" => Builtin::TypedArrayProtoJoin,
        "keys" => Builtin::TypedArrayProtoKeys,
        "lastIndexOf" => Builtin::TypedArrayProtoLastIndexOf,
        "length" => Builtin::TypedArrayProtoLength,
        "map" => Builtin::TypedArrayProtoMap,
        "reduce" => Builtin::TypedArrayProtoReduce,
        "reduceRight" => Builtin::TypedArrayProtoReduceRight,
        "reverse" => Builtin::TypedArrayProtoReverse,
        "set" => Builtin::TypedArrayProtoSet,
        "slice" => Builtin::TypedArrayProtoSlice,
        "some" => Builtin::TypedArrayProtoSome,
        "sort" => Builtin::TypedArrayProtoSort,
        "subarray" => Builtin::TypedArrayProtoSubarray,
        "values" => Builtin::TypedArrayProtoValues,
        "toString" => Builtin::TypedArrayProtoToString,
        _ => return None,
    })
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
    if index >= array.length {
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
    if index >= array.length {
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
    let byte_offset = array.offset.checked_mul(array.kind.element_size())?;
    let byte_length = array.length.checked_mul(array.kind.element_size())?;
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
    for (index, encoded) in storage[array.offset..array.offset + array.length]
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
    if index >= array.length {
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

fn construct(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
    kind: TypedArrayKind,
) -> i64 {
    if let Some(sab) = args
        .first()
        .and_then(|encoded| {
            state
                .shared_array_buffers
                .get(&value::decode_handle(*encoded))
        })
        .cloned()
    {
        return construct_shared_buffer_view(ctx, state, args, kind, sab);
    }
    if let Some(buffer) = args
        .first()
        .and_then(|encoded| state.array_buffers.get(&value::decode_handle(*encoded)))
        .cloned()
    {
        return construct_buffer_view(ctx, state, args, kind, buffer);
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
    let Ok(object) = state.allocate_object(2, false) else {
        return fail_dispatch(ctx);
    };
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
) -> i64 {
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
    let Ok(object) = state.allocate_object(2, false) else {
        return fail_dispatch(ctx);
    };
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
    let Ok(object) = state.allocate_object(2, false) else {
        return fail_dispatch(ctx);
    };
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
        },
    );
    object
}

fn property_length(ctx: &mut NativeVmContext, state: &NativeAgentState, args: &[i64]) -> i64 {
    args.first()
        .and_then(|object| state.typed_arrays.get(&value::decode_handle(*object)))
        .and_then(|array| u32::try_from(array.length).ok())
        .map(|length| value::encode_f64(f64::from(length)))
        .unwrap_or_else(|| fail_dispatch(ctx))
}

fn property_byte_length(ctx: &mut NativeVmContext, state: &NativeAgentState, args: &[i64]) -> i64 {
    args.first()
        .and_then(|object| state.typed_arrays.get(&value::decode_handle(*object)))
        .and_then(|array| array.length.checked_mul(array.kind.element_size()))
        .and_then(|length| u32::try_from(length).ok())
        .map(|length| value::encode_f64(f64::from(length)))
        .unwrap_or_else(|| fail_dispatch(ctx))
}

fn property_byte_offset(ctx: &mut NativeVmContext, state: &NativeAgentState, args: &[i64]) -> i64 {
    args.first()
        .and_then(|object| state.typed_arrays.get(&value::decode_handle(*object)))
        .and_then(|array| array.offset.checked_mul(array.kind.element_size()))
        .and_then(|offset| u32::try_from(offset).ok())
        .map(|offset| value::encode_f64(f64::from(offset)))
        .unwrap_or_else(|| fail_dispatch(ctx))
}

fn set(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
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
    let Some(array) = state
        .typed_arrays
        .get(&value::decode_handle(receiver))
        .cloned()
    else {
        return fail_dispatch(ctx);
    };
    if offset.saturating_add(values.len()) > array.length {
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
    let Some(array) = state
        .typed_arrays
        .get(&value::decode_handle(receiver))
        .cloned()
    else {
        return fail_dispatch(ctx);
    };
    let start = relative_index(state, args.get(1).copied(), array.length);
    let end = args.get(2).map_or(array.length, |encoded| {
        relative_index(state, Some(*encoded), array.length)
    });
    let values = (start.min(end)..end.min(array.length))
        .filter_map(|index| get_element(state, receiver, index))
        .collect::<Vec<_>>();
    construct_values(ctx, state, array.kind, &values)
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
    let start = relative_index(state, args.get(1).copied(), array.length);
    let end = args.get(2).map_or(array.length, |encoded| {
        relative_index(state, Some(*encoded), array.length)
    });
    let Ok(object) = state.allocate_object(2, false) else {
        return fail_dispatch(ctx);
    };
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
            offset: array.offset + start.min(end),
            length: end
                .saturating_sub(start)
                .min(array.length.saturating_sub(start)),
        },
    );
    object
}

fn fill(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
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
    let start = relative_index(state, args.get(2).copied(), array.length);
    let end = args.get(3).map_or(array.length, |encoded| {
        relative_index(state, Some(*encoded), array.length)
    });
    for index in start.min(end)..end.min(array.length) {
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
    let Some(length) = state
        .typed_arrays
        .get(&value::decode_handle(receiver))
        .map(|array| array.length)
    else {
        return fail_dispatch(ctx);
    };
    for index in 0..length / 2 {
        let right = length - index - 1;
        let (Some(left), Some(right_value)) = (
            get_element(state, receiver, index),
            get_element(state, receiver, right),
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
    state: &NativeAgentState,
    args: &[i64],
    reverse: bool,
) -> i64 {
    let Some(receiver) = args.first().copied() else {
        return fail_dispatch(ctx);
    };
    let Some(length) = state
        .typed_arrays
        .get(&value::decode_handle(receiver))
        .map(|array| array.length)
    else {
        return fail_dispatch(ctx);
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
                && get_element(state, receiver, *index)
                    .is_some_and(|stored| strict_equal(state, stored, needle))
        })
    } else {
        (start..length).find(|index| {
            get_element(state, receiver, *index)
                .is_some_and(|stored| strict_equal(state, stored, needle))
        })
    };
    found.map_or_else(
        || value::encode_f64(-1.0),
        |index| value::encode_f64(index as f64),
    )
}

fn includes(ctx: &mut NativeVmContext, state: &NativeAgentState, args: &[i64]) -> i64 {
    let Some(receiver) = args.first().copied() else {
        return fail_dispatch(ctx);
    };
    let Some(array) = state.typed_arrays.get(&value::decode_handle(receiver)) else {
        return fail_dispatch(ctx);
    };
    let needle = args.get(1).copied().unwrap_or_else(value::encode_undefined);
    value::encode_bool((0..array.length).any(|index| {
        get_element(state, receiver, index).is_some_and(|stored| {
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
    let Some(array) = state.typed_arrays.get(&value::decode_handle(receiver)) else {
        return fail_dispatch(ctx);
    };
    let separator = args
        .get(1)
        .map(|encoded| render_value(state, *encoded))
        .unwrap_or_else(|| ",".into());
    let output = (0..array.length)
        .filter_map(|index| get_element(state, receiver, index))
        .map(|stored| render_value(state, stored))
        .collect::<Vec<_>>()
        .join(&separator);
    state
        .intern_text(output, value::TAG_STRING)
        .unwrap_or_else(|| fail_dispatch(ctx))
}

fn copy_within(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(receiver) = args.first().copied() else {
        return fail_dispatch(ctx);
    };
    let Some(length) = state
        .typed_arrays
        .get(&value::decode_handle(receiver))
        .map(|array| array.length)
    else {
        return fail_dispatch(ctx);
    };
    let target = relative_index(state, args.get(1).copied(), length);
    let start = relative_index(state, args.get(2).copied(), length);
    let end = args.get(3).map_or(length, |encoded| {
        relative_index(state, Some(*encoded), length)
    });
    let count = end.saturating_sub(start).min(length.saturating_sub(target));
    let values = (0..count)
        .filter_map(|index| get_element(state, receiver, start + index))
        .collect::<Vec<_>>();
    for (index, stored) in values.into_iter().enumerate() {
        if set_element(state, receiver, target + index, stored).is_none() {
            return fail_dispatch(ctx);
        }
    }
    receiver
}

fn at(ctx: &mut NativeVmContext, state: &NativeAgentState, args: &[i64]) -> i64 {
    let Some(receiver) = args.first().copied() else {
        return fail_dispatch(ctx);
    };
    let Some(array) = state.typed_arrays.get(&value::decode_handle(receiver)) else {
        return fail_dispatch(ctx);
    };
    let Some(index) = args
        .get(1)
        .and_then(|encoded| to_number(state, *encoded))
        .and_then(|number| number.to_isize())
    else {
        return value::encode_undefined();
    };
    let index = if index < 0 {
        array.length as isize + index
    } else {
        index
    };
    usize::try_from(index)
        .ok()
        .and_then(|index| get_element(state, receiver, index))
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
    let Some(callback) = args
        .get(1)
        .copied()
        .filter(|value| value::is_callable(*value))
    else {
        return fail_dispatch(ctx);
    };
    let Some(length) = state
        .typed_arrays
        .get(&value::decode_handle(receiver))
        .map(|array| array.length)
    else {
        return fail_dispatch(ctx);
    };
    let mut output_values = Vec::new();
    for index in 0..length {
        let Some(stored) = get_element(state, receiver, index) else {
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
            .allocate_array_values(&output_values)
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
    let Some(length) = state
        .typed_arrays
        .get(&value::decode_handle(receiver))
        .map(|array| array.length)
    else {
        return fail_dispatch(ctx);
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
        get_element(state, receiver, index).unwrap_or_else(value::encode_undefined)
    };
    for index in iter {
        let Some(stored) = get_element(state, receiver, index) else {
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
    let Some(length) = state
        .typed_arrays
        .get(&value::decode_handle(receiver))
        .map(|array| array.length)
    else {
        return fail_dispatch(ctx);
    };
    let comparator = args
        .get(1)
        .copied()
        .filter(|value| !value::is_undefined(*value));
    if comparator.is_some_and(|comparator| !value::is_callable(comparator)) {
        return super::runtime::type_error(ctx, state, "compare function must be callable");
    }
    let mut values = (0..length)
        .filter_map(|index| get_element(state, receiver, index))
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

fn iterator(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
    kind: super::collections::CollectionIteratorKind,
) -> i64 {
    let Some(receiver) = args.first().copied() else {
        return fail_dispatch(ctx);
    };
    let handle = value::decode_handle(receiver);
    if !state.typed_arrays.contains_key(&handle) {
        return fail_dispatch(ctx);
    }
    let Ok(iterator_object) = state.allocate_object(1, false) else {
        return fail_dispatch(ctx);
    };
    let Ok(iterator_id) = u32::try_from(state.collection_iterators.len()) else {
        return fail_dispatch(ctx);
    };
    state
        .collection_iterators
        .push(super::collections::CollectionIterator {
            source: super::collections::CollectionIteratorSource::TypedArray(handle),
            kind,
            index: 0,
        });
    let Some(next) = state.native_callable(crate::NativeCallableKind::CollectionNext(iterator_id))
    else {
        return fail_dispatch(ctx);
    };
    state.iterator_next.insert(
        value::decode_handle(iterator_object),
        value::decode_native_callable_idx(next),
    );
    iterator_object
}

fn construct_values(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    kind: TypedArrayKind,
    values: &[i64],
) -> i64 {
    let object = construct(ctx, state, &[value::encode_f64(values.len() as f64)], kind);
    if !state
        .typed_arrays
        .contains_key(&value::decode_handle(object))
    {
        return fail_dispatch(ctx);
    }
    for (index, stored) in values.iter().copied().enumerate() {
        if set_element(state, object, index, stored).is_none() {
            return fail_dispatch(ctx);
        }
    }
    object
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

fn array_values(state: &NativeAgentState, encoded: i64) -> Option<Vec<i64>> {
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
                    .map(|stored| stored as i64)
            })
            .collect();
    }
    if value::is_js_object(encoded) {
        let typed = state.typed_arrays.get(&value::decode_handle(encoded))?;
        return (0..typed.length)
            .map(|index| get_element(state, encoded, index))
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
