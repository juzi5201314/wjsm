pub const MASK_SIGN: u64 = 0x8000_0000_0000_0000;
pub const MASK_EXPONENT: u64 = 0x7FF0_0000_0000_0000;
pub const MASK_QUIET_NAN: u64 = 0x0008_0000_0000_0000;
pub const MASK_PAYLOAD: u64 = 0x0007_FFFF_FFFF_FFFF;

pub const TAG_STRING: u64 = 0x1;
pub const TAG_UNDEFINED: u64 = 0x2;
pub const TAG_NULL: u64 = 0x3;
pub const TAG_BOOL: u64 = 0x4;
pub const TAG_EXCEPTION: u64 = 0x5;
pub const TAG_ITERATOR: u64 = 0x6;
pub const TAG_ENUMERATOR: u64 = 0x7;

pub const BOX_BASE: u64 = MASK_EXPONENT | MASK_QUIET_NAN;

pub fn encode_f64(val: f64) -> i64 {
    val.to_bits() as i64
}

pub fn encode_string_ptr(ptr: u32) -> i64 {
    let payload = (TAG_STRING << 32) | (ptr as u64);
    (BOX_BASE | payload) as i64
}

pub fn is_f64(val: i64) -> bool {
    let uval = val as u64;
    (uval & MASK_EXPONENT) != MASK_EXPONENT || (uval & MASK_QUIET_NAN) == 0
}

pub fn is_string(val: i64) -> bool {
    let uval = val as u64;
    (uval & BOX_BASE) == BOX_BASE && ((uval >> 32) & 0x7) == TAG_STRING
}

pub fn decode_string_ptr(val: i64) -> u32 {
    let uval = val as u64;
    (uval & 0xFFFF_FFFF) as u32
}

pub fn encode_undefined() -> i64 {
    (BOX_BASE | (TAG_UNDEFINED << 32)) as i64
}

pub fn is_undefined(val: i64) -> bool {
    let uval = val as u64;
    (uval & BOX_BASE) == BOX_BASE && ((uval >> 32) & 0x7) == TAG_UNDEFINED
}

pub fn encode_null() -> i64 {
    (BOX_BASE | (TAG_NULL << 32)) as i64
}

pub fn is_null(val: i64) -> bool {
    let uval = val as u64;
    (uval & BOX_BASE) == BOX_BASE && ((uval >> 32) & 0x7) == TAG_NULL
}

pub fn encode_bool(val: bool) -> i64 {
    let payload = (TAG_BOOL << 32) | if val { 1 } else { 0 };
    (BOX_BASE | payload) as i64
}

pub fn is_bool(val: i64) -> bool {
    let uval = val as u64;
    (uval & BOX_BASE) == BOX_BASE && ((uval >> 32) & 0x7) == TAG_BOOL
}

pub fn decode_bool(val: i64) -> bool {
    (val as u64 & 1) == 1
}

/// Returns true if the value is `null` or `undefined` (for `??` operator).
pub fn is_nullish(val: i64) -> bool {
    is_null(val) || is_undefined(val)
}

/// Returns true if the value is JavaScript-falsy.
///
/// falsy values: undefined, null, false, +0, -0, NaN, empty string.
pub fn is_falsy(val: i64) -> bool {
    if is_undefined(val) || is_null(val) {
        return true;
    }
    if is_bool(val) {
        return !decode_bool(val);
    }
    if is_f64(val) {
        let f = f64::from_bits(val as u64);
        // +0, -0, NaN
        return f == 0.0 || f.is_nan();
    }
    if is_string(val) {
        // 注意：字符串在内存中以 nul-terminated 方式存储，
        // 编译期无法直接判断是否为空串。
        // 空串的 truthiness 由 backend 的 emit_to_bool_i32 在运行时
        // 通过加载内存首字节来判断（i32.load8_u → eqz → falsy）。
        // 此处 is_falsy 仅用于 IR 层面的分析，保守地返回 false（即视为 truthy）。
        return false;
    }
    // 所有其他 NaN-boxed 类型（exception/iterator/enumerator handle 等）均为 truthy。
    false
}

/// Returns true if the value is JavaScript-truthy.
pub fn is_truthy(val: i64) -> bool {
    !is_falsy(val)
}

/// Encode a handle value (exception, iterator, enumerator) with a given tag.
pub fn encode_handle(tag: u64, handle: u32) -> i64 {
    let payload = (tag << 32) | (handle as u64);
    (BOX_BASE | payload) as i64
}

/// Decode the handle index from a tagged handle value.
pub fn decode_handle(val: i64) -> u32 {
    (val as u64 & 0xFFFF_FFFF) as u32
}

pub fn is_exception(val: i64) -> bool {
    let uval = val as u64;
    (uval & BOX_BASE) == BOX_BASE && ((uval >> 32) & 0x7) == TAG_EXCEPTION
}

pub fn is_iterator(val: i64) -> bool {
    let uval = val as u64;
    (uval & BOX_BASE) == BOX_BASE && ((uval >> 32) & 0x7) == TAG_ITERATOR
}

pub fn is_enumerator(val: i64) -> bool {
    let uval = val as u64;
    (uval & BOX_BASE) == BOX_BASE && ((uval >> 32) & 0x7) == TAG_ENUMERATOR
}
