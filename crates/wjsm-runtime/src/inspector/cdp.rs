//! CDP JSON-RPC 请求分发。

use super::InspectorHandle;
use super::remote_object::evaluate_simple_expression;
use super::state::{MAIN_SCRIPT_ID, PauseCommand, ResumeAction};
use serde_json::{Value, json};
use tokio::sync::oneshot;

/// 处理单条 CDP 文本消息，返回需要写回会话的 JSON 文本列表。
pub(crate) async fn handle_message(handle: &InspectorHandle, text: &str) -> Vec<String> {
    let Ok(msg) = serde_json::from_str::<Value>(text) else {
        return vec![error_response(None, -32700, "Parse error")];
    };
    let id = msg.get("id").cloned();
    let Some(method) = msg.get("method").and_then(|m| m.as_str()) else {
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
                    Some(script_parsed_event(
                        &inner.debug_info.source_url,
                        inner.debug_info.source_text.len(),
                    ))
                } else {
                    None
                };
                (ev, dbg_count)
            };
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
            let mut inner = handle.inner.lock().await;
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
        "Debugger.setPauseOnExceptions"
        | "Debugger.setAsyncCallStackDepth"
        | "Debugger.setBlackboxPatterns"
        | "Debugger.skipAllPauses"
        | "Debugger.setBreakpointsActive" => vec![ok_response(id, json!({}))],
        "Runtime.enable" => {
            handle.inner.lock().await.runtime_enabled = true;
            vec![ok_response(id, json!({}))]
        }
        "Runtime.disable" => {
            handle.inner.lock().await.runtime_enabled = false;
            vec![ok_response(id, json!({}))]
        }
        "Runtime.runIfWaitingForDebugger" => {
            handle.request_resume(ResumeAction::Continue).await;
            vec![ok_response(id, json!({}))]
        }
        "Runtime.getProperties" => {
            let object_id = params
                .get("objectId")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            match send_pause_command(handle, |reply| PauseCommand::GetProperties {
                object_id: object_id.clone(),
                reply,
            })
            .await
            {
                Ok(result) => vec![ok_response(id, result)],
                Err(_) => {
                    // 未暂停：scope 用缓存 locals 描述。
                    let mut inner = handle.inner.lock().await;
                    let props = offline_scope_props(&mut inner, &object_id);
                    vec![ok_response(id, json!({ "result": props }))]
                }
            }
        }
        "Debugger.evaluateOnCallFrame" => {
            let expression = params
                .get("expression")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let frame_id = params
                .get("callFrameId")
                .and_then(|v| v.as_str())
                .unwrap_or("frame-0")
                .to_string();
            // 未暂停时回退：字面量 / 缓存 locals。
            if !handle.paused.load(std::sync::atomic::Ordering::SeqCst) {
                let mut inner = handle.inner.lock().await;
                return vec![ok_response(
                    id,
                    offline_evaluate(&mut inner, &frame_id, &expression),
                )];
            }
            match send_pause_command(handle, |reply| PauseCommand::EvaluateOnFrame {
                frame_id,
                expression,
                reply,
            })
            .await
            {
                Ok(result) => vec![ok_response(id, result)],
                Err(fallback) => vec![ok_response(id, fallback)],
            }
        }
        "Runtime.evaluate" => {
            let expression = params
                .get("expression")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if handle.paused.load(std::sync::atomic::Ordering::SeqCst)
                && let Ok(result) =
                    send_pause_command(handle, |reply| PauseCommand::EvaluateGlobal {
                        expression: expression.clone(),
                        reply,
                    })
                    .await
            {
                return vec![ok_response(id, result)];
            }
            let mut inner = handle.inner.lock().await;
            match evaluate_simple_expression(&expression, &mut inner.remote_objects) {
                Ok(remote) => vec![ok_response(id, json!({ "result": remote }))],
                Err(msg) => vec![ok_response(id, eval_error_payload(&msg))],
            }
        }
        "Runtime.releaseObject"
        | "Runtime.releaseObjectGroup"
        | "Runtime.compileScript"
        | "Runtime.discardConsoleEntries"
        | "Profiler.enable"
        | "HeapProfiler.enable" => {
            vec![ok_response(id, json!({}))]
        }
        _ => vec![error_response(
            id,
            -32601,
            &format!("Method not found: {method}"),
        )],
    }
}

/// 向暂停循环发送命令；若未暂停则返回缓存回退结果。
async fn send_pause_command<F>(handle: &InspectorHandle, make: F) -> Result<Value, Value>
where
    F: FnOnce(oneshot::Sender<Value>) -> PauseCommand,
{
    let (tx, rx) = oneshot::channel();
    let cmd = make(tx);
    let sent = {
        let inner = handle.inner.lock().await;
        if let Some(cmd_tx) = &inner.pause_cmd_tx {
            cmd_tx.send(cmd).is_ok()
        } else {
            false
        }
    };
    if !sent {
        // 未暂停：scope 走缓存；对象返回空。
        return Err(json!({ "result": [] }));
    }
    match tokio::time::timeout(std::time::Duration::from_secs(30), rx).await {
        Ok(Ok(v)) => Ok(v),
        _ => Err(json!({
            "result": {
                "type": "object",
                "subtype": "error",
                "className": "Error",
                "description": "Inspector command timed out or cancelled",
            },
            "exceptionDetails": {
                "text": "Inspector command timed out or cancelled",
                "exceptionId": 1,
                "lineNumber": 0,
                "columnNumber": 0,
            },
        })),
    }
}

fn offline_scope_props(inner: &mut super::state::InspectorInner, object_id: &str) -> Vec<Value> {
    let Some(frame_id) = object_id.strip_prefix("scope:") else {
        return Vec::new();
    };
    let Some(pairs) = inner.frame_locals.get(frame_id).cloned() else {
        return Vec::new();
    };
    pairs
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
        .collect()
}

fn offline_evaluate(
    inner: &mut super::state::InspectorInner,
    frame_id: &str,
    expression: &str,
) -> Value {
    let expr = expression.trim();
    if let Ok(remote) = evaluate_simple_expression(expr, &mut inner.remote_objects) {
        return json!({ "result": remote });
    }
    if let Some(pairs) = inner.frame_locals.get(frame_id)
        && let Some((_, raw)) = pairs.iter().find(|(n, _)| n == expr)
    {
        let remote = inner.remote_objects.describe(*raw);
        return json!({ "result": remote });
    }
    eval_error_payload(&format!("ReferenceError: {expr} is not defined"))
}

fn eval_error_payload(msg: &str) -> Value {
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

fn script_parsed_event(url: &str, length: usize) -> String {
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
            "length": length,
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
