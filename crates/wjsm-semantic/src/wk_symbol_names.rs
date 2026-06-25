//! `Symbol.xxx` 静态成员名 → well-known symbol 索引（与 `wjsm_ir::wk_symbol` 对齐）。

use wjsm_ir::wk_symbol;

/// 编译期已知的 `Symbol.<name>` 属性名 → well-known 索引。
pub(crate) fn well_known_symbol_index_from_name(prop_name: &str) -> Option<u32> {
    Some(match prop_name {
        "iterator" => wk_symbol::ITERATOR,
        "species" => wk_symbol::SPECIES,
        "toStringTag" => wk_symbol::TO_STRING_TAG,
        "asyncIterator" => wk_symbol::ASYNC_ITERATOR,
        "hasInstance" => wk_symbol::HAS_INSTANCE,
        "toPrimitive" => wk_symbol::TO_PRIMITIVE,
        "dispose" => wk_symbol::DISPOSE,
        "match" => wk_symbol::MATCH,
        "asyncDispose" => wk_symbol::ASYNC_DISPOSE,
        "isConcatSpreadable" => wk_symbol::IS_CONCAT_SPREADABLE,
        "matchAll" => wk_symbol::MATCH_ALL,
        "replace" => wk_symbol::REPLACE,
        "search" => wk_symbol::SEARCH,
        "split" => wk_symbol::SPLIT,
        "unscopables" => wk_symbol::UNSCOPABLES,
        _ => return None,
    })
}