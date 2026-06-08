use wjsm_ir::{constants, value};

#[inline]
pub(crate) fn encode_string_name_id(string_idx: u32) -> u32 {
    debug_assert!(string_idx < constants::NAME_ID_SYMBOL_FLAG);
    string_idx
}

#[inline]
pub(crate) fn encode_symbol_name_id(symbol_idx: u32) -> u32 {
    debug_assert!(symbol_idx < constants::NAME_ID_SYMBOL_FLAG);
    constants::NAME_ID_SYMBOL_FLAG | symbol_idx
}

#[inline]
pub(crate) fn is_symbol_name_id(name_id: u32) -> bool {
    (name_id & constants::NAME_ID_SYMBOL_FLAG) != 0
}

#[inline]
pub(crate) fn decode_name_id(name_id: u32) -> (bool, u32) {
    (
        is_symbol_name_id(name_id),
        name_id & constants::NAME_ID_INDEX_MASK,
    )
}

#[inline]
pub(crate) fn name_id_to_property_key_value(name_id: u32) -> Option<i64> {
    let (is_symbol, index) = decode_name_id(name_id);
    if is_symbol {
        Some(value::encode_symbol_handle(index))
    } else {
        None
    }
}

#[inline]
pub(crate) fn symbol_value_to_name_id(symbol_val: i64) -> Option<u32> {
    if value::is_symbol(symbol_val) {
        Some(encode_symbol_name_id(value::decode_symbol_handle(
            symbol_val,
        )))
    } else {
        None
    }
}
