pub const MASK_SIGN: u64 = 0x8000_0000_0000_0000;
pub const MASK_EXPONENT: u64 = 0x7FF0_0000_0000_0000;
pub const MASK_QUIET_NAN: u64 = 0x0008_0000_0000_0000;
pub const MASK_PAYLOAD: u64 = 0x0007_FFFF_FFFF_FFFF;

pub const TAG_STRING: u64 = 0x1;
pub const TAG_UNDEFINED: u64 = 0x2;

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
