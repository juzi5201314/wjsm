//! ECMAScript §6.1.5.1 — `Symbol` 构造器 well-known 静态属性（`Symbol.foo` / `Symbol['foo']`）。
//!
//! NativeCallable 值无堆对象槽，`define_host_data_property` 无法挂属性；用侧表按
//! `native_callable` 索引存储，经 `native_callable_get_property` 与 `reflect_get` 暴露。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use wasmtime::Caller;
use wjsm_ir::wk_symbol;

use crate::RuntimeState;
use crate::value;

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

/// `native_callable` 表索引 → 静态属性名 → 值
pub(crate) type SymbolConstructorStaticProps = Arc<Mutex<HashMap<u32, HashMap<String, i64>>>>;

pub(crate) fn new_symbol_constructor_static_props() -> SymbolConstructorStaticProps {
    Arc::new(Mutex::new(HashMap::new()))
}

pub(crate) fn clear_symbol_constructor_static_props(state: &RuntimeState) {
    state
        .symbol_constructor_static_props
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clear();
}

/// 在 `Symbol` 构造器（NativeCallable 索引）上安装全部 well-known 静态属性。
pub(crate) fn install_well_known_symbols_on_symbol_constructor(
    caller: &mut Caller<'_, RuntimeState>,
    symbol_ctor_native: i64,
) {
    let idx = value::decode_native_callable_idx(symbol_ctor_native);
    let mut map = caller
        .data()
        .symbol_constructor_static_props
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let entry = map.entry(idx).or_default();
    for (name, sym_idx) in WELL_KNOWN_SYMBOL_STATIC_PROPS {
        entry.insert(name.to_string(), value::encode_symbol_handle(*sym_idx));
    }
}

/// 读取 `Symbol` 构造器静态属性（well-known Symbol 等）。
pub(crate) fn native_callable_symbol_constructor_static_property(
    caller: &mut Caller<'_, RuntimeState>,
    native: i64,
    prop_name: &str,
) -> Option<i64> {
    let idx = value::decode_native_callable_idx(native);
    let table = caller
        .data()
        .native_callables
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let is_symbol_ctor = table
        .get(idx as usize)
        .is_some_and(|r| matches!(r, crate::NativeCallable::SymbolConstructor));
    drop(table);
    if !is_symbol_ctor {
        return None;
    }
    let map = caller
        .data()
        .symbol_constructor_static_props
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    map.get(&idx).and_then(|m| m.get(prop_name).copied())
}
