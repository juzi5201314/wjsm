//! 属性键的规范64位值。
//!
//! `PropertyKey` 是 ShapeTable、数组副表和属性 IC 共用的唯一键表示：
//! - 低命名空间保存宿主 managed-string handle；
//! - 最高位命名空间保存 Symbol index；
//! - bit 62 命名空间保存 NaN-box ASCII SSO 的完整 payload。
//!
//! 所有键比较都在完整64位值上进行，禁止把 inline key 截断为32位 hash。

use serde::{Deserialize, Serialize};
use wjsm_ir::value;

const SYMBOL_NAMESPACE: u64 = 1 << 63;
const INLINE_NAMESPACE: u64 = 1 << 62;
const NAMESPACE_MASK: u64 = SYMBOL_NAMESPACE | INLINE_NAMESPACE;
const INLINE_PAYLOAD_MASK: u64 = (1_u64 << (value::INLINE_STRING_MARKER_SHIFT + 3)) - 1;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, Serialize, PartialOrd)]
#[repr(transparent)]
pub struct PropertyKey(u64);

impl PropertyKey {
    /// 包装已规范化的 managed-string handle。
    pub const fn from_name_id(name_id: u32) -> Self {
        Self(name_id as u64)
    }

    /// 创建独立 Symbol 命名空间的属性键。
    pub const fn symbol(symbol_idx: u32) -> Self {
        Self(SYMBOL_NAMESPACE | symbol_idx as u64)
    }

    /// 将六码元以内的 ASCII SSO Value 转为无堆分配属性键。
    pub fn inline_string(encoded: i64) -> Option<Self> {
        if !value::is_inline_string(encoded) {
            return None;
        }
        Some(Self(
            INLINE_NAMESPACE | (encoded as u64 & INLINE_PAYLOAD_MASK),
        ))
    }

    /// 返回内部64位编码；只用于持久化、诊断和完整键比较。
    #[inline]
    pub const fn raw(self) -> u64 {
        self.0
    }

    /// 返回 managed-string handle；Symbol 和 inline key 不属于 heap-name 命名空间。
    #[inline]
    pub const fn name_id(self) -> Option<u32> {
        if self.0 & NAMESPACE_MASK == 0 {
            Some(self.0 as u32)
        } else {
            None
        }
    }

    #[inline]
    pub const fn is_symbol(self) -> bool {
        self.0 & NAMESPACE_MASK == SYMBOL_NAMESPACE
    }

    #[inline]
    pub const fn is_inline_string(self) -> bool {
        self.0 & NAMESPACE_MASK == INLINE_NAMESPACE
    }

    /// 恢复为 runtime property API 使用的 Value。
    pub fn to_value(self) -> i64 {
        if self.is_inline_string() {
            value::BOX_BASE as i64 | (self.0 & INLINE_PAYLOAD_MASK) as i64
        } else if self.is_symbol() {
            value::encode_symbol_handle(self.0 as u32)
        } else {
            value::encode_runtime_string_handle(self.0 as u32)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heap_name_and_symbol_namespaces_do_not_collide() {
        let name = PropertyKey::from_name_id(u32::MAX);
        let symbol = PropertyKey::symbol(u32::MAX);
        assert_eq!(name.name_id(), Some(u32::MAX));
        assert!(!name.is_symbol());
        assert!(!name.is_inline_string());
        assert!(symbol.is_symbol());
        assert!(!symbol.is_inline_string());
        assert_eq!(symbol.raw() >> 63, 1);
        assert_ne!(name, symbol);
    }

    #[test]
    fn inline_key_preserves_complete_sso_value() {
        for input in [b"".as_slice(), b"a", b"a\0Z", b"abcdef"] {
            let encoded = value::encode_inline_ascii(input).expect("ASCII SSO");
            let key = PropertyKey::inline_string(encoded).expect("inline property key");
            assert!(key.is_inline_string());
            assert_eq!(key.name_id(), None);
            assert_eq!(key.to_value(), encoded);
            assert_eq!(PropertyKey::inline_string(key.to_value()), Some(key));
        }
    }

    #[test]
    fn inline_latin1_key_preserves_complete_sso_value() {
        for input in [b"".as_slice(), b"\xe9", b"caf\xe9"] {
            let encoded = value::encode_inline_latin1(input).expect("Latin-1 SSO");
            let key = PropertyKey::inline_string(encoded).expect("inline property key");
            assert!(key.is_inline_string());
            assert_eq!(key.name_id(), None);
            assert_eq!(key.to_value(), encoded);
            assert_eq!(PropertyKey::inline_string(key.to_value()), Some(key));
        }
    }

    #[test]
    fn non_inline_values_are_rejected() {
        assert!(PropertyKey::inline_string(value::encode_runtime_string_handle(7)).is_none());
        assert!(PropertyKey::inline_string(value::encode_f64(1.0)).is_none());
    }
}
