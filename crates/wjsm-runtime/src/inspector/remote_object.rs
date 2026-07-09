//! CDP `Runtime.RemoteObject` 分配与 NaN-box 描述。

use std::collections::HashMap;
use wjsm_ir::value;

/// 远程对象表：`objectId` → 原始 NaN-box 值（及可选文本描述缓存）。
#[derive(Default)]
pub(crate) struct RemoteObjectTable {
    next_id: u64,
    values: HashMap<String, i64>,
}

impl RemoteObjectTable {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn clear(&mut self) {
        self.values.clear();
        self.next_id = 1;
    }

    pub(crate) fn alloc(&mut self, raw: i64) -> String {
        if self.next_id == 0 {
            self.next_id = 1;
        }
        let id = format!("{}", self.next_id);
        self.next_id += 1;
        self.values.insert(id.clone(), raw);
        id
    }

    pub(crate) fn get(&self, object_id: &str) -> Option<i64> {
        self.values.get(object_id).copied()
    }

    /// 将 NaN-box 值描述为 CDP RemoteObject（无需 Store 访问即可处理原始类型）。
    pub(crate) fn describe(&mut self, raw: i64) -> serde_json::Value {
        describe_value(raw, Some(self))
    }
}

fn describe_value(raw: i64, table: Option<&mut RemoteObjectTable>) -> serde_json::Value {
    if value::is_undefined(raw) {
        return serde_json::json!({
            "type": "undefined",
        });
    }
    if value::is_null(raw) {
        return serde_json::json!({
            "type": "object",
            "subtype": "null",
            "value": null,
        });
    }
    if value::is_bool(raw) {
        return serde_json::json!({
            "type": "boolean",
            "value": value::decode_bool(raw),
        });
    }
    if value::is_f64(raw) {
        let n = value::decode_f64(raw);
        if n.is_nan() {
            return serde_json::json!({
                "type": "number",
                "unserializableValue": "NaN",
                "description": "NaN",
            });
        }
        if n.is_infinite() {
            let s = if n.is_sign_positive() {
                "Infinity"
            } else {
                "-Infinity"
            };
            return serde_json::json!({
                "type": "number",
                "unserializableValue": s,
                "description": s,
            });
        }
        return serde_json::json!({
            "type": "number",
            "value": n,
            "description": format_number_desc(n),
        });
    }
    if value::is_string(raw) {
        // 无 memory 时无法读出字符串内容；返回带 objectId 的 string 占位。
        let mut obj = serde_json::json!({
            "type": "string",
            "value": "<string>",
            "description": "<string>",
        });
        if let Some(table) = table {
            let id = table.alloc(raw);
            obj["objectId"] = serde_json::Value::String(id);
        }
        return obj;
    }
    if value::is_bigint(raw) {
        let mut obj = serde_json::json!({
            "type": "bigint",
            "unserializableValue": "0n",
            "description": "0n",
        });
        if let Some(table) = table {
            let id = table.alloc(raw);
            obj["objectId"] = serde_json::Value::String(id);
        }
        return obj;
    }
    if value::is_symbol(raw) {
        let mut obj = serde_json::json!({
            "type": "symbol",
            "description": "Symbol()",
            "className": "Symbol",
        });
        if let Some(table) = table {
            let id = table.alloc(raw);
            obj["objectId"] = serde_json::Value::String(id);
        }
        return obj;
    }
    if value::is_function(raw)
        || value::is_closure(raw)
        || value::is_bound(raw)
        || value::is_native_callable(raw)
    {
        let mut obj = serde_json::json!({
            "type": "function",
            "className": "Function",
            "description": "function () { [wjsm] }",
        });
        if let Some(table) = table {
            let id = table.alloc(raw);
            obj["objectId"] = serde_json::Value::String(id);
        }
        return obj;
    }
    if value::is_array(raw) {
        let mut obj = serde_json::json!({
            "type": "object",
            "subtype": "array",
            "className": "Array",
            "description": "Array",
        });
        if let Some(table) = table {
            let id = table.alloc(raw);
            obj["objectId"] = serde_json::Value::String(id);
        }
        return obj;
    }
    if value::is_object(raw)
        || value::is_proxy(raw)
        || value::is_regexp(raw)
        || value::is_scope_record(raw)
    {
        let (class_name, subtype) = if value::is_regexp(raw) {
            ("RegExp", Some("regexp"))
        } else if value::is_proxy(raw) {
            ("Proxy", Some("proxy"))
        } else {
            ("Object", None)
        };
        let mut obj = serde_json::json!({
            "type": "object",
            "className": class_name,
            "description": class_name,
        });
        if let Some(st) = subtype {
            obj["subtype"] = serde_json::Value::String(st.to_string());
        }
        if let Some(table) = table {
            let id = table.alloc(raw);
            obj["objectId"] = serde_json::Value::String(id);
        }
        return obj;
    }

    // 其它 tagged 值或原始 i64 回退为 number 描述。
    serde_json::json!({
        "type": "number",
        "description": format!("0x{raw:016x}"),
    })
}

fn format_number_desc(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

/// 将 JSON 字面量式简单表达式求值为 RemoteObject（不触及 JS 堆）。
pub(crate) fn evaluate_simple_expression(
    expression: &str,
    table: &mut RemoteObjectTable,
) -> Result<serde_json::Value, String> {
    let expr = expression.trim();
    if expr.is_empty() {
        return Ok(table.describe(value::encode_undefined()));
    }
    if expr == "undefined" {
        return Ok(table.describe(value::encode_undefined()));
    }
    if expr == "null" {
        return Ok(table.describe(value::encode_null()));
    }
    if expr == "true" {
        return Ok(table.describe(value::encode_bool(true)));
    }
    if expr == "false" {
        return Ok(table.describe(value::encode_bool(false)));
    }
    if let Some(s) = parse_js_string_literal(expr) {
        // 字符串无堆分配：用 description 直接返回字面量。
        return Ok(serde_json::json!({
            "type": "string",
            "value": s,
            "description": s,
        }));
    }
    if let Ok(n) = expr.parse::<f64>() {
        return Ok(table.describe(value::encode_f64(n)));
    }
    Err(format!(
        "Runtime.evaluate only supports simple literals in this build: {expr}"
    ))
}

fn parse_js_string_literal(expr: &str) -> Option<String> {
    let bytes = expr.as_bytes();
    if bytes.len() < 2 {
        return None;
    }
    let quote = bytes[0];
    if quote != b'\'' && quote != b'"' {
        return None;
    }
    if *bytes.last()? != quote {
        return None;
    }
    let inner = &expr[1..expr.len() - 1];
    // 简化：不完整处理转义，足够调试字面量。
    Some(inner.replace("\\n", "\n").replace("\\\"", "\"").replace("\\'", "'"))
}
