//! NaN-boxing 适配器：集中所有 i64 ↔ Rust 原生类型的转换操作
//!
//! 此模块是运行时代码中所有 NaN-boxing 操作的唯一入口。
//! 所有 `f64::from_bits` 和 `nanbox_to_*` 辅助函数都应通过此模块调用。
//!
//! ## 安全性
//!
//! - `as_f64`: 假设输入已经是 f64 类型（通过 `value::is_f64` 验证）
//! - `to_number`: 将任意 NaN-boxed 值转换为 f64（处理 bool/null/undefined）
//! - `to_usize/u32/bool`: 从 shadow stack 参数转换为整数/布尔值

use wjsm_ir::value;

/// 从已知是 f64 的 NaN-boxed 值提取 f64
///
/// # Safety
/// 调用者必须确保 `val` 已通过 `value::is_f64(val)` 验证
#[allow(dead_code)]
#[inline(always)]
pub fn as_f64(val: i64) -> f64 {
    value::decode_f64(val)
}

/// 将任意 NaN-boxed 值转换为 f64（Number 语义）
///
/// 遵循 ECMAScript ToNumber 抽象操作：
/// - f64: 直接返回
/// - bool: true → 1.0, false → 0.0
/// - null: 0.0
/// - undefined: NaN
/// - 其他类型: NaN（保守处理）
#[allow(dead_code)]
#[inline]
pub fn to_number(val: i64) -> f64 {
    if value::is_f64(val) {
        value::decode_f64(val)
    } else if value::is_bool(val) {
        if value::decode_bool(val) { 1.0 } else { 0.0 }
    } else if value::is_null(val) {
        0.0
    } else {
        f64::NAN
    }
}

/// 从 shadow stack 参数转换为 usize（用于索引、长度等）
///
/// 处理 bool（true → 1, false → 0）和 f64（截断为整数）
#[allow(dead_code)]
#[inline]
pub fn to_usize(val: i64) -> usize {
    if value::is_bool(val) {
        if value::decode_bool(val) { 1 } else { 0 }
    } else if value::is_f64(val) {
        value::decode_f64(val) as usize
    } else {
        0
    }
}

/// 从 shadow stack 参数转换为 u32
#[allow(dead_code)]
#[inline(always)]
pub fn to_u32(val: i64) -> u32 {
    to_usize(val) as u32
}

/// 从 shadow stack 参数转换为 bool
///
/// 遵循 ECMAScript ToBoolean：
/// - bool: 直接返回
/// - f64: 非零且非 NaN 为 true
/// - 其他: false（保守处理）
#[allow(dead_code)]
#[inline]
pub fn to_bool(val: i64) -> bool {
    if value::is_bool(val) {
        value::decode_bool(val)
    } else if value::is_f64(val) {
        let f = value::decode_f64(val);
        f != 0.0 && !f.is_nan()
    } else {
        false
    }
}

/// 从 shadow stack 参数转换为 i32（用于有符号整数参数）
#[allow(dead_code)]
#[inline]
pub fn to_i32(val: i64) -> i32 {
    if value::is_f64(val) {
        value::decode_f64(val) as i32
    } else if value::is_bool(val) {
        if value::decode_bool(val) { 1 } else { 0 }
    } else {
        0
    }
}

/// 从 shadow stack 参数转换为 u64（用于大整数参数）
#[allow(dead_code)]
#[inline]
pub fn to_u64(val: i64) -> u64 {
    if value::is_f64(val) {
        value::decode_f64(val) as u64
    } else if value::is_bool(val) {
        if value::decode_bool(val) { 1 } else { 0 }
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_to_number_f64() {
        assert_eq!(to_number(value::encode_f64(3.25)), 3.25);
        assert_eq!(to_number(value::encode_f64(-0.0)), -0.0);
        assert!(to_number(value::encode_f64(f64::NAN)).is_nan());
    }

    #[test]
    fn test_to_number_bool() {
        assert_eq!(to_number(value::encode_bool(true)), 1.0);
        assert_eq!(to_number(value::encode_bool(false)), 0.0);
    }

    #[test]
    fn test_to_number_null_undefined() {
        assert_eq!(to_number(value::encode_null()), 0.0);
        assert!(to_number(value::encode_undefined()).is_nan());
    }

    #[test]
    fn test_to_usize() {
        assert_eq!(to_usize(value::encode_f64(42.0)), 42);
        assert_eq!(to_usize(value::encode_f64(42.9)), 42); // 截断
        assert_eq!(to_usize(value::encode_bool(true)), 1);
        assert_eq!(to_usize(value::encode_bool(false)), 0);
    }

    #[test]
    fn test_to_bool() {
        assert!(to_bool(value::encode_bool(true)));
        assert!(!to_bool(value::encode_bool(false)));
        assert!(to_bool(value::encode_f64(1.0)));
        assert!(!to_bool(value::encode_f64(0.0)));
        assert!(!to_bool(value::encode_f64(f64::NAN)));
    }

    #[test]
    fn test_as_f64() {
        let val = value::encode_f64(2.5);
        assert_eq!(as_f64(val), 2.5);
    }
}
