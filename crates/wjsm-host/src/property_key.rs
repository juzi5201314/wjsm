//! 属性键 `name_id` 纯编码/解码（后端无关）。
//!
//! 上下文相关操作（intern / canonicalize / 读线性内存）留在各后端；
//! 本模块只放位运算、查表与纯判断。

use wjsm_ir::{constants, value};

use crate::Value;

/// 属性槽 `name_id` 的三种存储来源。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodedNameId {
    MemoryString(u32),
    RuntimeString(u32),
    Symbol(u32),
}

#[inline]
pub fn encode_string_name_id(string_idx: u32) -> u32 {
    assert!(string_idx <= constants::NAME_ID_INDEX_MASK);
    string_idx
}

#[inline]
pub fn encode_runtime_string_name_id(index: u32) -> u32 {
    assert!(index <= constants::NAME_ID_INDEX_MASK);
    constants::NAME_ID_RUNTIME_STRING_FLAG | index
}

#[inline]
pub fn encode_symbol_name_id(symbol_idx: u32) -> u32 {
    assert!(symbol_idx <= constants::NAME_ID_INDEX_MASK);
    constants::NAME_ID_SYMBOL_FLAG | symbol_idx
}

#[inline]
pub fn is_symbol_name_id(name_id: u32) -> bool {
    matches!(decode_name_id(name_id), DecodedNameId::Symbol(_))
}

#[inline]
pub fn decode_name_id(name_id: u32) -> DecodedNameId {
    let index = name_id & constants::NAME_ID_INDEX_MASK;
    match name_id & constants::NAME_ID_KIND_MASK {
        constants::NAME_ID_SYMBOL_FLAG => DecodedNameId::Symbol(index),
        constants::NAME_ID_RUNTIME_STRING_FLAG => DecodedNameId::RuntimeString(index),
        _ => DecodedNameId::MemoryString(index),
    }
}

/// Symbol 与 MemoryString 可直接编码；RuntimeString 返回 None，由后端查表并
/// `store_string` 后继续。
#[inline]
pub fn name_id_to_property_key_value(name_id: u32) -> Option<Value> {
    match decode_name_id(name_id) {
        DecodedNameId::MemoryString(index) => Some(value::encode_string_ptr(index)),
        DecodedNameId::RuntimeString(_) => None,
        DecodedNameId::Symbol(index) => Some(value::encode_symbol_handle(index)),
    }
}

/// 将 Symbol 值转为 name_id；非 Symbol 返回 None。
#[inline]
pub fn symbol_value_to_name_id(symbol_val: Value) -> Option<u32> {
    if value::is_symbol(symbol_val) {
        Some(encode_symbol_name_id(value::decode_symbol_handle(
            symbol_val,
        )))
    } else {
        None
    }
}
