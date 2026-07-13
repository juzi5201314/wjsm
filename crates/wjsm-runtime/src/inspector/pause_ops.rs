//! 暂停期间在持有 `Caller` 时执行的 CDP 操作：属性展开、帧上 eval。

use super::remote_object::RemoteObjectTable;
use super::state::PauseCommand;
use crate::runtime_eval::{ScopeRecord, perform_eval_from_caller_async};
use crate::runtime_host_helpers::collect_own_property_names_from_value;
use crate::runtime_render::{read_runtime_string_utf8_lossy, render_value, store_runtime_string};
use crate::runtime_values::{
    read_array_elem, read_array_length, read_object_property_by_name, resolve_array_ptr,
    resolve_handle,
};
use crate::{RuntimeState, value};
use serde_json::{Value, json};
use wasmtime::Caller;

/// 处理一条暂停期命令。
pub(crate) async fn dispatch_pause_command(
    caller: &mut Caller<'_, RuntimeState>,
    frame_locals: &std::collections::HashMap<String, Vec<(String, i64)>>,
    remote: &mut RemoteObjectTable,
    cmd: PauseCommand,
) {
    match cmd {
        PauseCommand::GetProperties { object_id, reply } => {
            let result = get_properties(caller, frame_locals, remote, &object_id);
            let _ = reply.send(json!({ "result": result }));
        }
        PauseCommand::EvaluateOnFrame {
            frame_id,
            expression,
            reply,
        } => {
            let result =
                evaluate_on_frame(caller, frame_locals, remote, &frame_id, &expression).await;
            let _ = reply.send(result);
        }
        PauseCommand::EvaluateGlobal { expression, reply } => {
            let result = evaluate_global(caller, remote, &expression).await;
            let _ = reply.send(result);
        }
    }
}

fn get_properties(
    caller: &mut Caller<'_, RuntimeState>,
    frame_locals: &std::collections::HashMap<String, Vec<(String, i64)>>,
    remote: &mut RemoteObjectTable,
    object_id: &str,
) -> Vec<Value> {
    if let Some(frame_id) = object_id.strip_prefix("scope:") {
        return scope_props(caller, frame_locals, remote, frame_id);
    }
    let Some(raw) = remote.get(object_id) else {
        return Vec::new();
    };
    expand_value_properties(caller, remote, raw)
}

fn scope_props(
    caller: &mut Caller<'_, RuntimeState>,
    frame_locals: &std::collections::HashMap<String, Vec<(String, i64)>>,
    remote: &mut RemoteObjectTable,
    frame_id: &str,
) -> Vec<Value> {
    let Some(pairs) = frame_locals.get(frame_id) else {
        return Vec::new();
    };
    pairs
        .iter()
        .map(|(name, raw)| {
            let value = describe_with_store(caller, remote, *raw);
            property_descriptor(name, value)
        })
        .collect()
}

fn expand_value_properties(
    caller: &mut Caller<'_, RuntimeState>,
    remote: &mut RemoteObjectTable,
    raw: i64,
) -> Vec<Value> {
    let mut props = Vec::new();

    if value::is_string(raw) {
        let s = read_runtime_string_utf8_lossy(caller, raw);
        props.push(property_descriptor(
            "length",
            json!({
                "type": "number",
                "value": s.encode_utf16().count() as f64,
                "description": format!("{}", s.encode_utf16().count()),
            }),
        ));
        return props;
    }

    if value::is_array(raw)
        && let Some(ptr) = resolve_array_ptr(caller, raw)
    {
        let len = read_array_length(caller, ptr).unwrap_or(0);
        props.push(property_descriptor(
            "length",
            json!({
                "type": "number",
                "value": len as f64,
                "description": format!("{len}"),
            }),
        ));
        for i in 0..len {
            if let Some(elem) = read_array_elem(caller, ptr, i) {
                let desc = describe_with_store(caller, remote, elem);
                props.push(property_descriptor(&i.to_string(), desc));
            }
        }
        // 数组命名属性（非索引）。
        let names = collect_own_property_names_from_value(caller, raw, false);
        for name in names {
            if name == "length" || name.chars().all(|c| c.is_ascii_digit()) {
                continue;
            }
            if let Some(v) = read_object_property_by_name(caller, ptr, &name) {
                let desc = describe_with_store(caller, remote, v);
                props.push(property_descriptor(&name, desc));
            }
        }
        return props;
    }

    if let Some(ptr) = resolve_handle(caller, raw) {
        let names = collect_own_property_names_from_value(caller, raw, false);
        for name in names {
            if let Some(v) = read_object_property_by_name(caller, ptr, &name) {
                let desc = describe_with_store(caller, remote, v);
                props.push(property_descriptor(&name, desc));
            }
        }
    }

    props
}

fn property_descriptor(name: &str, value: Value) -> Value {
    json!({
        "name": name,
        "value": value,
        "writable": true,
        "configurable": true,
        "enumerable": true,
        "isOwn": true,
    })
}

/// 带 Store 的 RemoteObject 描述（可解码字符串内容）。
pub(crate) fn describe_with_store(
    caller: &mut Caller<'_, RuntimeState>,
    remote: &mut RemoteObjectTable,
    raw: i64,
) -> Value {
    if value::is_string(raw) {
        let s = read_runtime_string_utf8_lossy(caller, raw);
        return json!({
            "type": "string",
            "value": s,
            "description": s,
        });
    }
    if value::is_array(raw) {
        let len = resolve_array_ptr(caller, raw)
            .and_then(|p| read_array_length(caller, p))
            .unwrap_or(0);
        let id = remote.alloc(raw);
        return json!({
            "type": "object",
            "subtype": "array",
            "className": "Array",
            "description": format!("Array({len})"),
            "objectId": id,
        });
    }
    if value::is_object(raw) || value::is_function(raw) || value::is_closure(raw) {
        // 用 render_value 生成可读 description（截断保护）。
        let rendered = render_value(caller, raw).unwrap_or_else(|_| "Object".to_string());
        let desc = if rendered.len() > 120 {
            format!("{}…", &rendered[..120])
        } else {
            rendered
        };
        let mut obj = remote.describe(raw);
        obj["description"] = json!(desc);
        return obj;
    }
    remote.describe(raw)
}

async fn evaluate_on_frame(
    caller: &mut Caller<'_, RuntimeState>,
    frame_locals: &std::collections::HashMap<String, Vec<(String, i64)>>,
    remote: &mut RemoteObjectTable,
    frame_id: &str,
    expression: &str,
) -> Value {
    let expr = expression.trim();
    if expr.is_empty() {
        return eval_result_ok(remote.describe(value::encode_undefined()));
    }

    // 优先：完整标识符命中 locals。
    if is_simple_identifier(expr)
        && let Some(pairs) = frame_locals.get(frame_id)
        && let Some((_, raw)) = pairs.iter().find(|(n, _)| n == expr)
    {
        return eval_result_ok(describe_with_store(caller, remote, *raw));
    }

    // 成员访问 a.b / a[0] 快速路径。
    if let Some(v) = try_member_access(caller, frame_locals, frame_id, expr) {
        return eval_result_ok(describe_with_store(caller, remote, v));
    }

    // 完整路径：ScopeRecord + perform_eval_from_caller_async。
    let pairs = frame_locals.get(frame_id).cloned().unwrap_or_default();
    let scope = create_scope_from_locals(caller, &pairs);
    let code = store_runtime_string(caller, expr);
    let result = perform_eval_from_caller_async(caller, code, Some(scope)).await;
    destroy_scope(caller, scope);

    if value::is_exception(result) {
        let msg = render_value(caller, result).unwrap_or_else(|_| "Error".to_string());
        return eval_result_exception(&msg);
    }
    eval_result_ok(describe_with_store(caller, remote, result))
}

async fn evaluate_global(
    caller: &mut Caller<'_, RuntimeState>,
    remote: &mut RemoteObjectTable,
    expression: &str,
) -> Value {
    let expr = expression.trim();
    if expr.is_empty() {
        return eval_result_ok(remote.describe(value::encode_undefined()));
    }
    let code = store_runtime_string(caller, expr);
    let result = perform_eval_from_caller_async(caller, code, None).await;
    if value::is_exception(result) {
        let msg = render_value(caller, result).unwrap_or_else(|_| "Error".to_string());
        return eval_result_exception(&msg);
    }
    eval_result_ok(describe_with_store(caller, remote, result))
}

fn create_scope_from_locals(caller: &mut Caller<'_, RuntimeState>, pairs: &[(String, i64)]) -> i64 {
    let data = caller.data_mut();
    let handle = data.scope_record_next_handle;
    data.scope_record_next_handle += 1;
    let bindings = pairs
        .iter()
        .map(|(n, v)| (n.clone(), *v, true, false))
        .collect();
    data.scope_records.insert(
        handle,
        ScopeRecord {
            bindings,
            home_object: None,
            new_target: None,
            has_arguments_binding: false,
            is_strict: true,
            outer: None,
            object_env: None,
        },
    );
    value::encode_scope_record_handle(handle)
}

fn destroy_scope(caller: &mut Caller<'_, RuntimeState>, scope: i64) {
    if !value::is_scope_record(scope) {
        return;
    }
    let handle = value::decode_scope_record_handle(scope);
    caller.data_mut().scope_records.remove(&handle);
}

fn try_member_access(
    caller: &mut Caller<'_, RuntimeState>,
    frame_locals: &std::collections::HashMap<String, Vec<(String, i64)>>,
    frame_id: &str,
    expr: &str,
) -> Option<i64> {
    let pairs = frame_locals.get(frame_id)?;
    // obj.prop
    if let Some((base, prop)) = expr.split_once('.')
        && is_simple_identifier(base)
        && is_simple_identifier(prop)
    {
        let (_, raw) = pairs.iter().find(|(n, _)| n == base)?;
        let ptr = resolve_handle(caller, *raw)?;
        return read_object_property_by_name(caller, ptr, prop);
    }
    // arr[index]
    if let Some(open) = expr.find('[')
        && expr.ends_with(']')
    {
        let base = expr[..open].trim();
        let idx_src = &expr[open + 1..expr.len() - 1];
        if is_simple_identifier(base)
            && let Ok(idx) = idx_src.trim().parse::<u32>()
        {
            let (_, raw) = pairs.iter().find(|(n, _)| n == base)?;
            let ptr = resolve_array_ptr(caller, *raw)?;
            return read_array_elem(caller, ptr, idx);
        }
    }
    None
}

fn is_simple_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' || c == '$' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
}

fn eval_result_ok(remote: Value) -> Value {
    json!({ "result": remote })
}

fn eval_result_exception(msg: &str) -> Value {
    json!({
        "result": {
            "type": "object",
            "subtype": "error",
            "className": "Error",
            "description": msg,
        },
        "exceptionDetails": {
            "text": msg,
            "exceptionId": 1,
            "lineNumber": 0,
            "columnNumber": 0,
        },
    })
}
