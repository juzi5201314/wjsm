use wasmtime::*;
use wjsm_ir::value;

use crate::types::*;
use crate::runtime::*;

pub(crate) fn create_host_functions(store: &mut Store<RuntimeState>) -> Vec<(usize, Func)> {
    let symbol_create_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, desc: i64| -> i64 {
            let description = if value::is_undefined(desc) {
                None
            } else {
                let s = render_value(&mut caller, desc).unwrap_or_default();
                // 去掉符号描述可能有的额外引号
                Some(s.trim_matches('"').to_string())
            };
            let mut table = caller
                .data()
                .symbol_table
                .lock()
                .expect("symbol_table mutex");
            let handle = table.len() as u32;
            table.push(SymbolEntry {
                description,
                global_key: None,
            });
            value::encode_symbol_handle(handle)
        },
    );

    // ── Import 106: symbol_for(i64) → i64 ─────────────────────────────
    // 全局 symbol 注册表（static 变量，与 RuntimeState 生命周期相同）
    // Symbol.for(key) 返回全局注册表中的 symbol

    let symbol_for_fn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, key: i64| -> i64 {
            let key_str = if value::is_string(key) {
                read_value_string_bytes(&mut caller, key)
                    .map(|b| String::from_utf8_lossy(&b).to_string())
            } else {
                render_value(&mut caller, key).ok()
            }
            .unwrap_or_default();
            let key_str = key_str.trim_end_matches('\0').to_string();
            let mut table = caller
                .data()
                .symbol_table
                .lock()
                .expect("symbol_table mutex");
            // 查找是否已有同 key 的 symbol
            for (idx, entry) in table.iter().enumerate() {
                if entry.global_key.as_deref() == Some(&key_str) {
                    return value::encode_symbol_handle(idx as u32);
                }
            }
            // 创建新 symbol
            let handle = table.len() as u32;
            table.push(SymbolEntry {
                description: Some(key_str.clone()),
                global_key: Some(key_str),
            });
            value::encode_symbol_handle(handle)
        },
    );

    // ── Import 107: symbol_key_for(i64) → i64 ─────────────────────────

    let symbol_key_for_fn = Func::wrap(
        &mut *store,
        |caller: Caller<'_, RuntimeState>, sym: i64| -> i64 {
            if !value::is_symbol(sym) {
                return value::encode_undefined();
            }
            let handle = value::decode_symbol_handle(sym) as usize;
            let table = caller
                .data()
                .symbol_table
                .lock()
                .expect("symbol_table mutex");
            let key_to_return = table.get(handle).and_then(|entry| entry.global_key.clone());
            drop(table);
            if let Some(key) = key_to_return {
                return store_runtime_string(&caller, key);
            }
            value::encode_undefined()
        },
    );

    // ECMAScript § 6.1.5.1 Well-Known Symbols
    // 返回预分配的 well-known symbol（id=0..7）

    let symbol_well_known_fn = Func::wrap(
        &mut *store,
        |caller: Caller<'_, RuntimeState>, id: i32| -> i64 {
            if id < 0 || id > 7 {
                return value::encode_undefined();
            }
            let table = caller
                .data()
                .symbol_table
                .lock()
                .expect("symbol_table mutex");
            if (id as usize) < table.len() {
                value::encode_symbol_handle(id as u32)
            } else {
                value::encode_undefined()
            }
        },
    );

    // ── Import 109: regex_create(i32, i32, i32, i32) → i64 ──────────────────────

    vec![
        (105, symbol_create_fn),
        (106, symbol_for_fn),
        (107, symbol_key_for_fn),
        (108, symbol_well_known_fn),
    ]
}
