use wjsm_ir::wk_symbol;

/// `Symbol.<name>` 静态属性名 → `symbol_table` / `symbol_well_known` 索引（与 `wjsm_ir::wk_symbol` 一致）。
pub(crate) fn well_known_symbol_property_index(property: &str) -> Option<u32> {
    Some(match property {
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