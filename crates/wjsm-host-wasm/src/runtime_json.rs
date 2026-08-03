//! JSON.parse 薄委托层。
//!
//! 解析器与算法已迁至 `wjsm-builtins::json`（后端无关，`<E: ExecContext>` 单态化）；
//! 本文件保留原调用路径签名（`Caller` / `Store` 形态），内部委托 builtins。

use crate::exec_context_impl::WasmExecContext;
use crate::*;
use wasmtime::{AsContextMut, Caller};

pub(crate) use wjsm_builtins::json::{JsonParser, parse_json_text};
pub(crate) use wjsm_host::JsonValue;

/// JSON 中间值 → JS 值（无 Caller 上下文的 Store/env 调用方专用）。
pub(crate) fn build_wasm_value_with_env<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    json_value: &JsonValue,
) -> i64 {
    match json_value {
        JsonValue::Null => value::encode_null(),
        JsonValue::Bool(b) => value::encode_bool(*b),
        JsonValue::Number(n) => value::encode_f64(*n),
        JsonValue::String(s) => store_runtime_string_with_env(ctx, env, s.clone()),
        JsonValue::Array(elements) => {
            let arr = alloc_array_with_env(ctx, env, elements.len() as u32);
            if let Some(ptr) = resolve_array_ptr_with_env(ctx, env, arr) {
                for (i, elem) in elements.iter().enumerate() {
                    let elem_val = build_wasm_value_with_env(ctx, env, elem);
                    write_array_elem_with_env(ctx, env, ptr, i as u32, elem_val);
                }
                write_array_length_with_env(ctx, env, ptr, elements.len() as u32);
            }
            arr
        }
        JsonValue::Object(properties) => {
            let obj = alloc_host_object(ctx, env, properties.len() as u32);
            for (key, val) in properties {
                let val_encoded = build_wasm_value_with_env(ctx, env, val);
                let key_lossy = key.to_utf8_lossy();
                let _ = define_host_data_property_with_env(ctx, env, obj, &key_lossy, val_encoded);
            }
            obj
        }
    }
}

/// JSON.parse 输入的同步 ToString（`value_to_key_string` 等冷路径用）。
pub(crate) fn json_parse_to_string(
    caller: &mut Caller<'_, RuntimeState>,
    value: i64,
) -> Result<String, i64> {
    let mut ctx = WasmExecContext::new(caller);
    wjsm_builtins::json::json_parse_to_string_impl(&mut ctx, value)
}

/// ES §24.5.1 JSON.parse(text, reviver) — async 完整路径。
pub async fn json_parse_to_wasm_async(
    caller: &mut Caller<'_, RuntimeState>,
    text: i64,
    reviver: i64,
) -> i64 {
    let mut ctx = WasmExecContext::new(caller);
    wjsm_builtins::json::json_parse_impl(&mut ctx, text, reviver).await
}
