//! JSON 中间值表示（后端无关 DTO）。
//!
//! `wjsm-builtins::json` 的解析器产出此结构；由各后端的
//! `ExecContext::json_materialize` 物化为 JS 值。

use crate::RuntimeString;

/// JSON.parse 的中间解析结果。
#[derive(Debug, Clone, PartialEq)]
pub enum JsonValue {
    Null,
    Bool(bool),
    Number(f64),
    String(RuntimeString),
    Array(Vec<JsonValue>),
    Object(Vec<(RuntimeString, JsonValue)>),
}
