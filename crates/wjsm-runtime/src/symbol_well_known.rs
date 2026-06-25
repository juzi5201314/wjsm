//! ECMAScript §6.1.5.1 — 在 `Symbol` 构造器上安装 well-known 静态属性（供 `Symbol.foo` / `Symbol['foo']` / `get_prop`）。

use wasmtime::Caller;
use wjsm_ir::wk_symbol;

use crate::runtime_heap::define_host_data_property_from_caller;
use crate::value;
use crate::RuntimeState;

const WELL_KNOWN_SYMBOL_STATIC_PROPS: &[(&str, u32)] = &[
    ("iterator", wk_symbol::ITERATOR),
    ("species", wk_symbol::SPECIES),
    ("toStringTag", wk_symbol::TO_STRING_TAG),
    ("asyncIterator", wk_symbol::ASYNC_ITERATOR),
    ("hasInstance", wk_symbol::HAS_INSTANCE),
    ("toPrimitive", wk_symbol::TO_PRIMITIVE),
    ("dispose", wk_symbol::DISPOSE),
    ("match", wk_symbol::MATCH),
    ("asyncDispose", wk_symbol::ASYNC_DISPOSE),
    ("isConcatSpreadable", wk_symbol::IS_CONCAT_SPREADABLE),
    ("matchAll", wk_symbol::MATCH_ALL),
    ("replace", wk_symbol::REPLACE),
    ("search", wk_symbol::SEARCH),
    ("split", wk_symbol::SPLIT),
    ("unscopables", wk_symbol::UNSCOPABLES),
];

/// 将 `symbol_table` 中预分配的 well-known symbol 挂到 `Symbol` 函数对象上。
pub(crate) fn install_well_known_symbols_on_symbol_constructor(
    caller: &mut Caller<'_, RuntimeState>,
    symbol_ctor: i64,
) {
    for (name, idx) in WELL_KNOWN_SYMBOL_STATIC_PROPS {
        let sym = value::encode_symbol_handle(*idx);
        let _ = define_host_data_property_from_caller(caller, symbol_ctor, name, sym);
    }
}