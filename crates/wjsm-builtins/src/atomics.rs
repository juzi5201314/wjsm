//! Atomics 与 SharedArrayBuffer 的后端无关 ECMAScript 语义。

use num_bigint::{BigInt, Sign};
use wjsm_host::{AtomicsRmwOp, ExecContext, TypedArrayView, Value};
use wjsm_ir::value;

struct AtomicAccess {
    view: TypedArrayView,
    byte_offset: u64,
}

fn to_index<E: ExecContext>(ctx: &mut E, input: Value, message: &str) -> Result<u64, Value> {
    let number = ctx.to_number(input);
    if value::is_exception(number) {
        return Err(number);
    }
    let number = value::decode_f64(number);
    if number.is_nan() || number == 0.0 {
        return Ok(0);
    }
    if !number.is_finite() || number < 0.0 || number.trunc() > 9_007_199_254_740_991.0 {
        return Err(ctx.make_range_error(message));
    }
    Ok(number.trunc() as u64)
}

fn prepare_access<E: ExecContext>(
    ctx: &mut E,
    typed_array: Value,
    index: Value,
) -> Result<AtomicAccess, Value> {
    let Some(view) = ctx.typedarray_resolve(typed_array) else {
        return Err(ctx.make_type_error("Typed array is not an integer type for Atomics"));
    };
    if matches!(view.element_kind, 2 | 3) {
        return Err(ctx.make_type_error("Typed array is not an integer type for Atomics"));
    }
    let index = to_index(ctx, index, "Invalid typed array index")?;
    if index >= view.length as u64 {
        return Err(ctx.make_range_error("Invalid typed array index"));
    }
    let byte_offset = view.byte_offset as u64 + index * view.element_size as u64;
    Ok(AtomicAccess { view, byte_offset })
}

fn prepare_waitable_access<E: ExecContext>(
    ctx: &mut E,
    typed_array: Value,
    index: Value,
) -> Result<AtomicAccess, Value> {
    let access = prepare_access(ctx, typed_array, index)?;
    if !access.view.is_shared {
        return Err(ctx.make_type_error("wait/notify/waitAsync called on non-shared TypedArray"));
    }
    if !matches!(
        (access.view.element_size, access.view.element_kind),
        (4, 0) | (8, 4)
    ) {
        return Err(
            ctx.make_type_error("wait/notify/waitAsync requires Int32Array or BigInt64Array")
        );
    }
    Ok(access)
}

fn bigint_low_64(bigint: &BigInt) -> i64 {
    let fill = if bigint.sign() == Sign::Minus {
        0xff
    } else {
        0
    };
    let mut raw = [fill; 8];
    let bytes = bigint.to_signed_bytes_le();
    let len = bytes.len().min(raw.len());
    raw[..len].copy_from_slice(&bytes[..len]);
    i64::from_le_bytes(raw)
}

fn to_uint_n(number: f64, bits: u32) -> u64 {
    if number == 0.0 || !number.is_finite() {
        return 0;
    }
    let modulo = 2.0_f64.powi(bits as i32);
    number.trunc().rem_euclid(modulo) as u64
}

fn to_integer_or_infinity(number: f64) -> f64 {
    if number.is_nan() || number == 0.0 {
        0.0
    } else if number.is_infinite() {
        number
    } else {
        number.trunc()
    }
}

fn string_to_bigint(input: &str) -> Option<BigInt> {
    let input = input.trim();
    if input.is_empty() {
        return Some(BigInt::from(0));
    }
    let (negative, unsigned) = match input.as_bytes().first() {
        Some(b'+') => (false, &input[1..]),
        Some(b'-') => (true, &input[1..]),
        _ => (false, input),
    };
    if unsigned.is_empty() {
        return None;
    }
    let (radix, digits) = if !negative && !input.starts_with('+') {
        if let Some(digits) = unsigned
            .strip_prefix("0x")
            .or_else(|| unsigned.strip_prefix("0X"))
        {
            (16, digits)
        } else if let Some(digits) = unsigned
            .strip_prefix("0o")
            .or_else(|| unsigned.strip_prefix("0O"))
        {
            (8, digits)
        } else if let Some(digits) = unsigned
            .strip_prefix("0b")
            .or_else(|| unsigned.strip_prefix("0B"))
        {
            (2, digits)
        } else {
            (10, unsigned)
        }
    } else {
        (10, unsigned)
    };
    let magnitude = BigInt::parse_bytes(digits.as_bytes(), radix)?;
    Some(if negative { -magnitude } else { magnitude })
}

fn to_bigint<E: ExecContext>(ctx: &mut E, input: Value) -> Result<(BigInt, Value), Value> {
    let primitive = if value::is_f64(input)
        || value::is_string(input)
        || value::is_undefined(input)
        || value::is_null(input)
        || value::is_bool(input)
        || value::is_bigint(input)
        || value::is_symbol(input)
    {
        input
    } else {
        ctx.to_primitive_hinted(input, wjsm_host::ToPrimitiveHintKind::Number)
    };
    if value::is_exception(primitive) {
        return Err(primitive);
    }
    if let Some(bigint) = ctx.read_bigint(primitive) {
        return Ok((bigint, primitive));
    }
    let bigint = if value::is_bool(primitive) {
        BigInt::from(u8::from(value::decode_bool(primitive)))
    } else if value::is_string(primitive) {
        let string = ctx.read_string_utf8_lossy(primitive);
        let Some(bigint) = string_to_bigint(&string) else {
            return Err(ctx.make_syntax_error("Cannot convert string to a BigInt"));
        };
        bigint
    } else {
        return Err(ctx.make_type_error("Cannot convert value to a BigInt"));
    };
    let converted = ctx.store_bigint(bigint.clone());
    Ok((bigint, converted))
}

fn normalize_operand<E: ExecContext>(
    ctx: &mut E,
    view: &TypedArrayView,
    input: Value,
) -> Result<(i64, Value), Value> {
    if view.element_kind >= 4 {
        let (bigint, converted) = to_bigint(ctx, input)?;
        return Ok((bigint_low_64(&bigint), converted));
    }
    let number = ctx.to_number(input);
    if value::is_exception(number) {
        return Err(number);
    }
    let integer = to_integer_or_infinity(value::decode_f64(number));
    let raw = to_uint_n(integer, u32::from(view.element_size) * 8);
    Ok((raw as i64, value::encode_f64(integer)))
}

fn box_raw<E: ExecContext>(ctx: &mut E, view: &TypedArrayView, raw: i64) -> Value {
    match (view.element_size, view.element_kind) {
        (1, 0) => value::encode_f64(raw as i8 as f64),
        (1, 1) => value::encode_f64(raw as u8 as f64),
        (2, 0) => value::encode_f64(raw as i16 as f64),
        (2, 1) => value::encode_f64(raw as u16 as f64),
        (4, 0) => value::encode_f64(raw as i32 as f64),
        (4, 1) => value::encode_f64(raw as u32 as f64),
        (8, 4) => ctx.store_bigint(BigInt::from(raw)),
        (8, 5) => ctx.store_bigint(BigInt::from(raw as u64)),
        _ => value::encode_undefined(),
    }
}

pub fn load<E: ExecContext>(ctx: &mut E, typed_array: Value, index: Value) -> Value {
    let access = match prepare_access(ctx, typed_array, index) {
        Ok(access) => access,
        Err(exception) => return exception,
    };
    ctx.buffer_atomic_load(&access.view, access.byte_offset)
        .map(|raw| box_raw(ctx, &access.view, raw))
        .unwrap_or_else(value::encode_undefined)
}

pub fn store<E: ExecContext>(ctx: &mut E, typed_array: Value, index: Value, input: Value) -> Value {
    let access = match prepare_access(ctx, typed_array, index) {
        Ok(access) => access,
        Err(exception) => return exception,
    };
    let (raw, converted) = match normalize_operand(ctx, &access.view, input) {
        Ok(value) => value,
        Err(exception) => return exception,
    };
    if ctx
        .buffer_atomic_store(&access.view, access.byte_offset, raw)
        .is_some()
    {
        converted
    } else {
        value::encode_undefined()
    }
}

pub fn rmw<E: ExecContext>(
    ctx: &mut E,
    typed_array: Value,
    index: Value,
    input: Value,
    op: AtomicsRmwOp,
) -> Value {
    let access = match prepare_access(ctx, typed_array, index) {
        Ok(access) => access,
        Err(exception) => return exception,
    };
    let (operand, _) = match normalize_operand(ctx, &access.view, input) {
        Ok(value) => value,
        Err(exception) => return exception,
    };
    ctx.buffer_atomic_rmw(&access.view, access.byte_offset, op, operand)
        .map(|raw| box_raw(ctx, &access.view, raw))
        .unwrap_or_else(value::encode_undefined)
}

pub fn compare_exchange<E: ExecContext>(
    ctx: &mut E,
    typed_array: Value,
    index: Value,
    expected: Value,
    replacement: Value,
) -> Value {
    let access = match prepare_access(ctx, typed_array, index) {
        Ok(access) => access,
        Err(exception) => return exception,
    };
    let (expected, _) = match normalize_operand(ctx, &access.view, expected) {
        Ok(value) => value,
        Err(exception) => return exception,
    };
    let (replacement, _) = match normalize_operand(ctx, &access.view, replacement) {
        Ok(value) => value,
        Err(exception) => return exception,
    };
    ctx.buffer_atomic_compare_exchange(&access.view, access.byte_offset, expected, replacement)
        .map(|raw| box_raw(ctx, &access.view, raw))
        .unwrap_or_else(value::encode_undefined)
}

pub fn is_lock_free(size: Value) -> Value {
    let size = value::decode_f64(size) as u8;
    value::encode_bool(matches!(size, 1 | 2 | 4) || (size == 8 && cfg!(target_has_atomic = "64")))
}

#[inline]
pub fn pause() -> Value {
    value::encode_undefined()
}

fn timeout<E: ExecContext>(ctx: &mut E, input: Value) -> Result<f64, Value> {
    if value::is_undefined(input) {
        return Ok(f64::INFINITY);
    }
    let number = ctx.to_number(input);
    if value::is_exception(number) {
        return Err(number);
    }
    let number = value::decode_f64(number);
    if number.is_nan() {
        Ok(f64::INFINITY)
    } else {
        Ok(number.max(0.0))
    }
}

pub async fn wait<E: ExecContext>(
    ctx: &mut E,
    typed_array: Value,
    index: Value,
    expected: Value,
    timeout_value: Value,
) -> Value {
    let access = match prepare_waitable_access(ctx, typed_array, index) {
        Ok(access) => access,
        Err(exception) => return exception,
    };
    let (expected, _) = match normalize_operand(ctx, &access.view, expected) {
        Ok(value) => value,
        Err(exception) => return exception,
    };
    let timeout = match timeout(ctx, timeout_value) {
        Ok(timeout) => timeout,
        Err(exception) => return exception,
    };
    ctx.atomics_wait_sync(access.view, access.byte_offset, expected, timeout)
        .await
        .unwrap_or_else(|_| value::encode_undefined())
}

pub fn notify<E: ExecContext>(
    ctx: &mut E,
    typed_array: Value,
    index: Value,
    count: Value,
) -> Value {
    let access = match prepare_waitable_access(ctx, typed_array, index) {
        Ok(access) => access,
        Err(exception) => return exception,
    };
    let count = if value::is_undefined(count) {
        None
    } else {
        let count = ctx.to_number(count);
        if value::is_exception(count) {
            return count;
        }
        let count = value::decode_f64(count);
        Some(if count.is_nan() || count <= 0.0 {
            0
        } else if count.is_infinite() {
            u32::MAX
        } else {
            count.trunc().min(u32::MAX as f64) as u32
        })
    };
    value::encode_f64(ctx.atomics_notify(&access.view, access.byte_offset, count) as f64)
}

pub async fn wait_async<E: ExecContext>(
    ctx: &mut E,
    typed_array: Value,
    index: Value,
    expected: Value,
    timeout_value: Value,
) -> Value {
    let access = match prepare_waitable_access(ctx, typed_array, index) {
        Ok(access) => access,
        Err(exception) => return exception,
    };
    let (expected, _) = match normalize_operand(ctx, &access.view, expected) {
        Ok(value) => value,
        Err(exception) => return exception,
    };
    let timeout = match timeout(ctx, timeout_value) {
        Ok(timeout) => timeout,
        Err(exception) => return exception,
    };
    let result = ctx
        .atomics_wait_async_op(access.view, access.byte_offset, expected, timeout)
        .await
        .unwrap_or_else(|_| value::encode_undefined());
    let object = ctx.alloc_object(2);
    let is_async = ctx.is_promise_value(result);
    ctx.define_data_property(object, "async", value::encode_bool(is_async));
    ctx.define_data_property(object, "value", result);
    object
}

pub fn shared_arraybuffer_constructor<E: ExecContext>(
    ctx: &mut E,
    length: Value,
    options: Value,
    target: Value,
) -> Value {
    let byte_length = match to_index(ctx, length, "Invalid array buffer length") {
        Ok(length) => length,
        Err(exception) => return exception,
    };
    let max_byte_length = if value::is_undefined(options) || value::is_null(options) {
        None
    } else {
        if !value::is_js_object(options) {
            return ctx.make_type_error("SharedArrayBuffer options must be an object");
        }
        let max = ctx.read_data_property(options, "maxByteLength");
        if value::is_undefined(max) {
            None
        } else {
            let max = match to_index(ctx, max, "Invalid maxByteLength") {
                Ok(max) => max,
                Err(exception) => return exception,
            };
            if max < byte_length {
                return ctx.make_range_error("maxByteLength must not be less than byte length");
            }
            Some(max)
        }
    };
    ctx.shared_arraybuffer_create_object(target, byte_length, max_byte_length)
}

fn sab_info<E: ExecContext>(
    ctx: &mut E,
    this: Value,
    operation: &str,
) -> Result<(u32, u64, Option<u64>), Value> {
    ctx.shared_arraybuffer_info(this).ok_or_else(|| {
        ctx.make_type_error(&format!(
            "SharedArrayBuffer.prototype.{operation} called on incompatible receiver"
        ))
    })
}

pub fn shared_arraybuffer_byte_length<E: ExecContext>(ctx: &mut E, this: Value) -> Value {
    match sab_info(ctx, this, "byteLength") {
        Ok((_, length, _)) => value::encode_f64(length as f64),
        Err(exception) => exception,
    }
}

pub fn shared_arraybuffer_growable<E: ExecContext>(ctx: &mut E, this: Value) -> Value {
    match sab_info(ctx, this, "growable") {
        Ok((_, _, max)) => value::encode_bool(max.is_some()),
        Err(exception) => exception,
    }
}

pub fn shared_arraybuffer_max_byte_length<E: ExecContext>(ctx: &mut E, this: Value) -> Value {
    match sab_info(ctx, this, "maxByteLength") {
        Ok((_, length, max)) => value::encode_f64(max.unwrap_or(length) as f64),
        Err(exception) => exception,
    }
}

pub fn shared_arraybuffer_grow<E: ExecContext>(
    ctx: &mut E,
    this: Value,
    new_length: Value,
) -> Value {
    let (_, current, max) = match sab_info(ctx, this, "grow") {
        Ok(info) => info,
        Err(exception) => return exception,
    };
    let Some(max) = max else {
        return ctx.make_type_error(
            "SharedArrayBuffer.prototype.grow can only be used with growable SharedArrayBuffers",
        );
    };
    let new_length = match to_index(ctx, new_length, "Invalid array buffer length") {
        Ok(length) => length,
        Err(exception) => return exception,
    };
    if new_length < current {
        return ctx.make_range_error("new length is smaller than the current length");
    }
    if new_length > max {
        return ctx.make_range_error("new length exceeds maxByteLength");
    }
    if ctx.shared_arraybuffer_grow(this, new_length) {
        value::encode_undefined()
    } else {
        ctx.make_type_error("SharedArrayBuffer backing is unavailable")
    }
}

fn relative_index<E: ExecContext>(ctx: &mut E, input: Value, length: u64) -> Result<u64, Value> {
    let number = ctx.to_number(input);
    if value::is_exception(number) {
        return Err(number);
    }
    let number = value::decode_f64(number);
    if number.is_nan() || number == 0.0 {
        return Ok(0);
    }
    if number == f64::NEG_INFINITY {
        return Ok(0);
    }
    if number == f64::INFINITY {
        return Ok(length);
    }
    let integer = number.trunc();
    Ok(if integer < 0.0 {
        (length as f64 + integer).max(0.0) as u64
    } else {
        integer.min(length as f64) as u64
    })
}

pub fn shared_arraybuffer_slice<E: ExecContext>(
    ctx: &mut E,
    this: Value,
    begin: Value,
    end: Value,
) -> Value {
    let (_, length, _) = match sab_info(ctx, this, "slice") {
        Ok(info) => info,
        Err(exception) => return exception,
    };
    let start = match relative_index(ctx, begin, length) {
        Ok(start) => start,
        Err(exception) => return exception,
    };
    let end = if value::is_undefined(end) {
        length
    } else {
        match relative_index(ctx, end, length) {
            Ok(end) => end,
            Err(exception) => return exception,
        }
    };
    let end = end.max(start);
    ctx.shared_arraybuffer_slice(this, start, end)
        .unwrap_or_else(|| ctx.make_type_error("SharedArrayBuffer slice failed"))
}

#[inline]
pub fn shared_arraybuffer_species(this: Value) -> Value {
    this
}
