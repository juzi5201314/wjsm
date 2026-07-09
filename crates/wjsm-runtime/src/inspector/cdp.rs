//! CDP JSON-RPC 请求分发。

use super::remote_object::evaluate_simple_expression;
use super::state::{MAIN_SCRIPT_ID, ResumeAction};
use super::InspectorHandle;
use serde_json::{Value, json};

/// 处理单条 CDP 文本消息，返回需要写回会话的 JSON 文本列表。
pub(crate) async fn handle_message(handle: &InspectorHandle, text: &str) -> Vec<String> {
    let Ok(msg) = serde_json::from_str::<Value>(text) else {
        return vec![error_response(None, -32700, "Parse error")];
    };
    let id = msg.get("id").cloned();
    let Some(method) = msg.get("method").and_then(|m| m.as_str()) else {
        // 事件回执或无 method 消息忽略。
        return Vec::new();
    };
    let params = msg.get("params").cloned().unwrap_or_else(|| json!({}));

    match method {
        "Debugger.enable" => {
            let mut out = Vec::new();
            let (script_event, debugger_statement_count) = {
                let mut inner = handle.inner.lock().await;
                inner.debugger_enabled = true;
                let dbg_count = if inner.debug_info.has_debugger_pcs() {
                    inner.debug_info.debugger_pcs.len()
                } else {
                    0
                };
                let ev = if !inner.script_parsed_sent {
                    inner.script_parsed_sent = true;
                    Some(script_parsed_event(&inner.debug_info.source_url))
                } else {
                    None
                };
                (ev, dbg_count)
            };
            // debuggerStatementCount 为扩展字段，便于诊断插桩是否生效。
            out.push(ok_response(
                id,
                json!({ "debuggerStatementCount": debugger_statement_count }),
            ));
            if let Some(ev) = script_event {
                out.push(ev);
            }
            out
        }
        "Debugger.disable" => {
            handle.inner.lock().await.debugger_enabled = false;
            vec![ok_response(id, json!({}))]
        }
        "Debugger.setBreakpointByUrl" => {
            let line = params
                .get("lineNumber")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32;
            let column = params
                .get("columnNumber")
                .and_then(|v| v.as_u64())
                .map(|v| v as u32);
            let url = params
                .get("url")
                .and_then(|v| v.as_str())
                .or_else(|| params.get("urlRegex").and_then(|v| v.as_str()))
                .unwrap_or("");
            let mut inner = handle.inner.lock().await;
            // url / urlRegex 任一匹配当前脚本或空（任意）时接受。
            let matches = url.is_empty()
                || inner.debug_info.source_url.contains(url)
                || url.contains(&inner.debug_info.source_url);
            if !matches {
                // 仍记录断点，便于后续 script 对齐。
            }
            let bp_id = inner.set_breakpoint(MAIN_SCRIPT_ID, line, column);
            let locations = json!([{
                "scriptId": MAIN_SCRIPT_ID,
                "lineNumber": line,
                "columnNumber": column.unwrap_or(0),
            }]);
            vec![ok_response(
                id,
                json!({
                    "breakpointId": bp_id,
                    "locations": locations,
                }),
            )]
        }
        "Debugger.removeBreakpoint" => {
            let bp_id = params
                .get("breakpointId")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            handle.inner.lock().await.remove_breakpoint(bp_id);
            vec![ok_response(id, json!({}))]
        }
        "Debugger.resume" => {
            handle.request_resume(ResumeAction::Continue).await;
            vec![ok_response(id, json!({}))]
        }
        "Debugger.stepOver" => {
            handle.request_resume(ResumeAction::StepOver).await;
            vec![ok_response(id, json!({}))]
        }
        "Debugger.stepInto" => {
            handle.request_resume(ResumeAction::StepInto).await;
            vec![ok_response(id, json!({}))]
        }
        "Debugger.stepOut" => {
            handle.request_resume(ResumeAction::StepOut).await;
            vec![ok_response(id, json!({}))]
        }
        "Debugger.getScriptSource" => {
            let source = handle.inner.lock().await.debug_info.source_text.clone();
            vec![ok_response(id, json!({ "scriptSource": source }))]
        }
        "Debugger.setPauseOnExceptions" | "Debugger.setAsyncCallStackDepth"
        | "Debugger.setBlackboxPatterns" | "Debugger.skipAllPauses" => {
            vec![ok_response(id, json!({}))]
        }
        "Runtime.enable" => {
            handle.inner.lock().await.runtime_enabled = true;
            vec![ok_response(id, json!({}))]
        }
        "Runtime.disable" => {
            handle.inner.lock().await.runtime_enabled = false;
            vec![ok_response(id, json!({}))]
        }
        "Runtime.runIfWaitingForDebugger" => {
            // break_on_start 等待连接时，客户端连上后发此方法放行。
            handle.request_resume(ResumeAction::Continue).await;
            vec![ok_response(id, json!({}))]
        }
        "Runtime.getProperties" => {
            let object_id = params
                .get("objectId")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let mut inner = handle.inner.lock().await;
            // scope:frame-N → 返回该帧可见局部（优先 pause 时捕获的真实 NaN-box 值）。
            if let Some(frame_id) = object_id.strip_prefix("scope:") {
                let locals = scope_locals_for_frame(&mut inner, frame_id);
                vec![ok_response(id, json!({ "result": locals }))]
            } else if inner.remote_objects.get(object_id).is_some() {
                // 已注册 remote object：暂无深度展开。
                vec![ok_response(id, json!({ "result": [] }))]
            } else {
                vec![ok_response(id, json!({ "result": [] }))]
            }
        }
        "Debugger.evaluateOnCallFrame" => {
            let expression = params
                .get("expression")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let mut inner = handle.inner.lock().await;
            // 先尝试简单字面量；再从当前帧 locals 按标识符查找。
            match evaluate_simple_expression(expression, &mut inner.remote_objects) {
                Ok(remote) => vec![ok_response(id, json!({ "result": remote }))],
                Err(_) => {
                    let name = expression.trim();
                    let mut found = None;
                    for (_frame, pairs) in &inner.frame_locals {
                        if let Some((_, raw)) = pairs.iter().find(|(n, _)| n == name) {
                            found = Some(*raw);
                            break;
                        }
                    }
                    if let Some(raw) = found {
                        let remote = inner.remote_objects.describe(raw);
                        vec![ok_response(id, json!({ "result": remote }))]
                    } else {
                        vec![ok_response(
                            id,
                            json!({
                                "result": {
                                    "type": "object",
                                    "subtype": "error",
                                    "className": "Error",
                                    "description": format!("ReferenceError: {name} is not defined"),
                                },
                                "exceptionDetails": {
                                    "text": format!("ReferenceError: {name} is not defined"),
                                    "exceptionId": 1,
                                    "lineNumber": 0,
                                    "columnNumber": 0,
                                },
                            }),
                        )]
                    }
                }
            }
        }
        "Runtime.evaluate" => {
            let expression = params
                .get("expression")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let mut inner = handle.inner.lock().await;
            match evaluate_simple_expression(expression, &mut inner.remote_objects) {
                Ok(remote) => vec![ok_response(
                    id,
                    json!({
                        "result": remote,
                    }),
                )],
                Err(msg) => vec![ok_response(
                    id,
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
                    }),
                )],
            }
        }
        "Runtime.releaseObject" | "Runtime.releaseObjectGroup" | "Runtime.compileScript"
        | "Runtime.discardConsoleEntries" | "Profiler.enable" | "HeapProfiler.enable"
        | "Debugger.setBreakpointsActive" => vec![ok_response(id, json!({}))],
        _ => {
            // 未知方法：返回 -32601，避免客户端卡死。
            vec![error_response(id, -32601, &format!("Method not found: {method}"))]
        }
    }
}

fn script_parsed_event(url: &str) -> String {
    json!({
        "method": "Debugger.scriptParsed",
        "params": {
            "scriptId": MAIN_SCRIPT_ID,
            "url": url,
            "startLine": 0,
            "startColumn": 0,
            "endLine": 0,
            "endColumn": 0,
            "executionContextId": 1,
            "hash": "",
            "isModule": false,
            "length": 0,
        }
    })
    .to_string()
}

fn ok_response(id: Option<Value>, result: Value) -> String {
    let mut obj = json!({ "result": result });
    if let Some(id) = id {
        obj.as_object_mut().unwrap().insert("id".to_string(), id);
    }
    obj.to_string()
}

fn error_response(id: Option<Value>, code: i64, message: &str) -> String {
    let mut obj = json!({
        "error": {
            "code": code,
            "message": message,
        }
    });
    if let Some(id) = id {
        obj.as_object_mut().unwrap().insert("id".to_string(), id);
    }
    obj.to_string()
}

/// 编码 CDP 请求（测试辅助）。
#[cfg(test)]
pub(crate) fn encode_request(id: u64, method: &str, params: Value) -> String {
    json!({
        "id": id,
        "method": method,
        "params": params,
    })
    .to_string()
}

/// 根据 callFrameId 构造 scope 属性列表：优先 `frame_locals` 真值，否则回退名字表。
fn scope_locals_for_frame(
    inner: &mut super::state::InspectorInner,
    frame_id: &str,
) -> Vec<Value> {
    if let Some(pairs) = inner.frame_locals.get(frame_id).cloned() {
        return pairs
            .into_iter()
            .map(|(name, raw)| {
                let value = inner.remote_objects.describe(raw);
                json!({
                    "name": name,
                    "value": value,
                    "writable": true,
                    "configurable": true,
                    "enumerable": true,
                    "isOwn": true,
                })
            })
            .collect();
    }

    // 回退：仅名字，值 undefined。
    let mut props = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut funcs: Vec<u32> = inner
        .debug_info
        .local_entries
        .iter()
        .map(|e| e.func_idx)
        .collect();
    funcs.sort_unstable();
    funcs.dedup();
    for func_idx in funcs {
        let mut locals: Vec<_> = inner.debug_info.locals_for_func(func_idx).collect();
        locals.sort_by_key(|l| l.local_idx);
        for local in locals {
            let display = local
                .name
                .rsplit('.')
                .next()
                .unwrap_or(local.name.as_str())
                .to_string();
            if display.starts_with('$') || !seen.insert(display.clone()) {
                continue;
            }
            props.push(json!({
                "name": display,
                "value": { "type": "undefined" },
                "writable": true,
                "configurable": true,
                "enumerable": true,
                "isOwn": true,
            }));
        }
    }
    props
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_request_shape() {
        let s = encode_request(1, "Debugger.enable", json!({}));
        let v: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["id"], 1);
        assert_eq!(v["method"], "Debugger.enable");
    }
}
