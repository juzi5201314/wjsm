use wjsm_host::{ExecContext, TypedArrayView, Value};
use wjsm_ir::value;

const DATAVIEW_HANDLE_PROPERTY: &str = "__dataview_handle__";
const TYPEDARRAY_HANDLE_PROPERTY: &str = "__typedarray_handle__";

#[derive(Clone, Copy, Debug)]
pub enum DataViewValueKind {
    Int8,
    Uint8,
    Int16,
    Uint16,
    Int32,
    Uint32,
    Float32,
    Float64,
}

impl DataViewValueKind {
    fn byte_length(self) -> usize {
        match self {
            Self::Int8 | Self::Uint8 => 1,
            Self::Int16 | Self::Uint16 => 2,
            Self::Int32 | Self::Uint32 | Self::Float32 => 4,
            Self::Float64 => 8,
        }
    }
}

pub fn dataview_constructor<E: ExecContext>(
    ctx: &mut E,
    buffer: Value,
    byte_offset: Value,
    byte_length: Value,
) -> Value {
    let Some((buffer_handle, buffer_length, is_shared)) = ctx.resolve_buffer_backing(buffer) else {
        return value::encode_undefined();
    };
    let offset = decode_optional_index(byte_offset, 0);
    let length = if value::is_undefined(byte_length) {
        buffer_length.saturating_sub(offset)
    } else {
        decode_optional_index(byte_length, 0)
    };
    if offset > buffer_length || length > buffer_length.saturating_sub(offset) {
        return range_error(ctx);
    }
    let Some(handle) = ctx.dataview_create(buffer_handle, Some(buffer), offset, length, is_shared)
    else {
        return value::encode_undefined();
    };
    let object = ctx.alloc_object(8);
    if let Some(object_handle) = ctx.handle_index_of(object) {
        ctx.set_property(
            object_handle,
            DATAVIEW_HANDLE_PROPERTY,
            value::encode_f64(handle as f64),
        );
    }
    object
}

pub fn dataview_get<E: ExecContext>(
    ctx: &mut E,
    this_value: Value,
    byte_offset: Value,
    kind: DataViewValueKind,
) -> Value {
    let Some((buffer, absolute_offset)) = resolve_dataview_access(
        ctx,
        this_value,
        decode_optional_index(byte_offset, 0),
        kind.byte_length(),
    ) else {
        return range_error(ctx);
    };
    let Some(bytes) = ctx.buffer_read_bytes(
        buffer.buffer_handle,
        buffer.is_shared,
        absolute_offset,
        kind.byte_length(),
    ) else {
        return range_error(ctx);
    };
    decode_dataview_value(kind, &bytes)
}

pub fn dataview_set<E: ExecContext>(
    ctx: &mut E,
    this_value: Value,
    byte_offset: Value,
    raw: Value,
    kind: DataViewValueKind,
) -> Value {
    let Some((buffer, absolute_offset)) = resolve_dataview_access(
        ctx,
        this_value,
        decode_optional_index(byte_offset, 0),
        kind.byte_length(),
    ) else {
        return range_error(ctx);
    };
    let bytes = encode_dataview_value(kind, raw);
    if !ctx.buffer_write_bytes(
        buffer.buffer_handle,
        buffer.is_shared,
        absolute_offset,
        &bytes,
    ) {
        return range_error(ctx);
    }
    value::encode_undefined()
}

pub fn typedarray_property<E: ExecContext>(ctx: &mut E, this_value: Value, name: &str) -> Value {
    ctx.read_property_for_render(this_value, name)
        .unwrap_or_else(|| value::encode_f64(0.0))
}

pub fn typedarray_set<E: ExecContext>(
    ctx: &mut E,
    target: Value,
    source: Value,
    offset_value: Value,
) -> Value {
    let Some(target_view) = ctx.typedarray_resolve(target) else {
        return value::encode_undefined();
    };
    let offset = decode_optional_index(offset_value, 0);
    let Some(values) = typedarray_source_values(ctx, source) else {
        return value::encode_undefined();
    };
    if offset > target_view.length || values.len() as u32 > target_view.length - offset {
        return range_error(ctx);
    }
    for (index, raw) in values.into_iter().enumerate() {
        ctx.typedarray_write_elem(&target_view, offset + index as u32, raw);
    }
    value::encode_undefined()
}

pub fn typedarray_slice<E: ExecContext>(
    ctx: &mut E,
    this_value: Value,
    begin_value: Value,
    end_value: Value,
) -> Value {
    let Some(view) = ctx.typedarray_resolve(this_value) else {
        return value::encode_undefined();
    };
    let (begin, end) = relative_bounds(begin_value, end_value, view.length);
    let length = end.saturating_sub(begin);
    let byte_length = length.saturating_mul(view.element_size as u32);
    let Some(buffer_handle) = ctx.arraybuffer_create(byte_length) else {
        return value::encode_undefined();
    };
    let new_view = TypedArrayView {
        buffer_handle,
        byte_offset: 0,
        length,
        element_size: view.element_size,
        element_kind: view.element_kind,
        is_shared: false,
    };
    let Some(bytes) = ctx.buffer_read_bytes(
        view.buffer_handle,
        view.is_shared,
        view.byte_offset as usize + begin as usize * view.element_size as usize,
        byte_length as usize,
    ) else {
        return value::encode_undefined();
    };
    if !ctx.arraybuffer_write_bytes(buffer_handle, 0, &bytes) {
        return value::encode_undefined();
    }
    create_typedarray_object(ctx, new_view, None)
}

pub fn typedarray_subarray<E: ExecContext>(
    ctx: &mut E,
    this_value: Value,
    begin_value: Value,
    end_value: Value,
) -> Value {
    let Some(view) = ctx.typedarray_resolve(this_value) else {
        return value::encode_undefined();
    };
    let (begin, end) = relative_bounds(begin_value, end_value, view.length);
    let length = end.saturating_sub(begin);
    let new_view = TypedArrayView {
        buffer_handle: view.buffer_handle,
        byte_offset: view.byte_offset + begin * view.element_size as u32,
        length,
        element_size: view.element_size,
        element_kind: view.element_kind,
        is_shared: view.is_shared,
    };
    create_typedarray_object(ctx, new_view, Some(this_value))
}

#[derive(Clone, Copy)]
struct DataViewBuffer {
    buffer_handle: u32,
    is_shared: bool,
}

fn resolve_dataview_access<E: ExecContext>(
    ctx: &mut E,
    this_value: Value,
    offset: u32,
    length: usize,
) -> Option<(DataViewBuffer, usize)> {
    let handle = ctx
        .read_property_for_render(this_value, DATAVIEW_HANDLE_PROPERTY)
        .map(value::decode_f64)? as u32;
    let (buffer_handle, byte_offset, byte_length, is_shared) = ctx.dataview_resolve(handle)?;
    let length = u32::try_from(length).ok()?;
    (offset <= byte_length && length <= byte_length - offset).then_some((
        DataViewBuffer {
            buffer_handle,
            is_shared,
        },
        byte_offset as usize + offset as usize,
    ))
}

fn typedarray_source_values<E: ExecContext>(ctx: &mut E, source: Value) -> Option<Vec<Value>> {
    if value::is_array(source) {
        let length = ctx.array_read_length(source)?;
        return Some(
            (0..length)
                .map(|index| {
                    ctx.array_read_elem(source, index)
                        .unwrap_or_else(value::encode_undefined)
                })
                .collect(),
        );
    }
    let view = ctx.typedarray_resolve(source)?;
    Some(
        (0..view.length)
            .map(|index| {
                ctx.typedarray_read_elem(&view, index)
                    .unwrap_or_else(value::encode_undefined)
            })
            .collect(),
    )
}

fn create_typedarray_object<E: ExecContext>(
    ctx: &mut E,
    view: TypedArrayView,
    buffer_object: Option<Value>,
) -> Value {
    let table_handle = ctx.typedarray_table_create(view, buffer_object);
    let object = ctx.alloc_object(8);
    let Some(handle) = ctx.handle_index_of(object) else {
        return value::encode_undefined();
    };
    ctx.set_property(
        handle,
        TYPEDARRAY_HANDLE_PROPERTY,
        value::encode_f64(table_handle as f64),
    );
    ctx.set_property(handle, "length", value::encode_f64(view.length as f64));
    ctx.set_property(
        handle,
        "byteLength",
        value::encode_f64((view.length * view.element_size as u32) as f64),
    );
    ctx.set_property(
        handle,
        "byteOffset",
        value::encode_f64(view.byte_offset as f64),
    );
    object
}

fn relative_bounds(begin: Value, end: Value, length: u32) -> (u32, u32) {
    let begin = relative_index(begin, length, 0);
    let end = relative_index(end, length, length);
    (begin, end)
}

fn relative_index(raw: Value, length: u32, default: u32) -> u32 {
    if value::is_undefined(raw) {
        return default;
    }
    let number = value::decode_f64(raw);
    if number < 0.0 {
        (length as i64 + number as i64).clamp(0, length as i64) as u32
    } else {
        (number as u32).min(length)
    }
}

fn decode_optional_index(raw: Value, default: u32) -> u32 {
    if value::is_undefined(raw) {
        default
    } else {
        value::decode_f64(raw) as u32
    }
}

fn range_error<E: ExecContext>(ctx: &mut E) -> Value {
    ctx.set_last_error("RangeError: Offset is outside the bounds of the DataView".to_string());
    value::encode_undefined()
}

fn decode_dataview_value(kind: DataViewValueKind, bytes: &[u8]) -> Value {
    match kind {
        DataViewValueKind::Int8 => value::encode_f64(bytes[0] as i8 as f64),
        DataViewValueKind::Uint8 => value::encode_f64(bytes[0] as f64),
        DataViewValueKind::Int16 => {
            value::encode_f64(i16::from_le_bytes([bytes[0], bytes[1]]) as f64)
        }
        DataViewValueKind::Uint16 => {
            value::encode_f64(u16::from_le_bytes([bytes[0], bytes[1]]) as f64)
        }
        DataViewValueKind::Int32 => value::encode_f64(i32::from_le_bytes(
            bytes.try_into().expect("DataView read length is validated"),
        ) as f64),
        DataViewValueKind::Uint32 => value::encode_f64(u32::from_le_bytes(
            bytes.try_into().expect("DataView read length is validated"),
        ) as f64),
        DataViewValueKind::Float32 => value::encode_f64(f32::from_le_bytes(
            bytes.try_into().expect("DataView read length is validated"),
        ) as f64),
        DataViewValueKind::Float64 => {
            f64::from_le_bytes(bytes.try_into().expect("DataView read length is validated"))
                .to_bits() as i64
        }
    }
}

fn encode_dataview_value(kind: DataViewValueKind, raw: Value) -> Vec<u8> {
    let number = value::decode_f64(raw);
    match kind {
        DataViewValueKind::Int8 => (number as i8).to_le_bytes().to_vec(),
        DataViewValueKind::Uint8 => (number as u8).to_le_bytes().to_vec(),
        DataViewValueKind::Int16 => (number as i16).to_le_bytes().to_vec(),
        DataViewValueKind::Uint16 => (number as u16).to_le_bytes().to_vec(),
        DataViewValueKind::Int32 => (number as i32).to_le_bytes().to_vec(),
        DataViewValueKind::Uint32 => (number as u32).to_le_bytes().to_vec(),
        DataViewValueKind::Float32 => (number as f32).to_le_bytes().to_vec(),
        DataViewValueKind::Float64 => number.to_le_bytes().to_vec(),
    }
}
