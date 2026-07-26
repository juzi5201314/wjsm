//! 暂停决策辅助、`Debugger.paused` 事件构造、调用栈快照与 debug_break 暂停循环。

use super::debug_info::DebugInfo;
use super::pause_ops::dispatch_pause_command;
use super::state::{MAIN_SCRIPT_ID, PauseCommand, PauseReason, ResumeAction};
use crate::RuntimeState;
use crate::runtime_source_map::SourceMapInfo;
use std::collections::HashMap;
use tokio::sync::{mpsc, oneshot};
use wasmtime::{Caller, Val, WasmBacktrace};

/// `env.debug_break` 的 wasmtime 暂停循环（无 inspector 时立即返回）。
///
/// 由 [`crate::exec_context_impl::WasmExecContext::debug_break`] 调用；
/// host_imports 仅保留薄注册，不承载此逻辑。
pub(crate) async fn debug_break_body(
    caller: &mut Caller<'_, RuntimeState>,
    line: i32,
    col: i32,
    flags: i32,
) {
    let Some(inspector) = caller.data().inspector.clone() else {
        return;
    };
    let line_u = line.max(0) as u32;
    let col_u = col.max(0) as u32;
    let frame_depth = {
        let n = caller.debug_exit_frames().count();
        n.saturating_sub(1) as u32
    };
    let decision = {
        let mut inner = inspector.inner.lock().await;
        inner.should_pause(line_u, col_u, flags, frame_depth)
    };
    let Some((reason, hit_bps)) = decision else {
        return;
    };
    let debug_info = {
        let inner = inspector.inner.lock().await;
        inner.debug_info.clone()
    };
    let call_frames = snapshot_call_frames(caller, &debug_info, line_u, col_u);
    let frame_locals = capture_frame_locals(caller, &debug_info);
    let pause_depth = call_frames.len().saturating_sub(1) as u32;
    let (resume_tx, mut resume_rx) = oneshot::channel::<ResumeAction>();
    let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<PauseCommand>();
    {
        let mut inner = inspector.inner.lock().await;
        inner.paused = true;
        inner.last_pause_reason = Some(reason);
        inner.cached_call_frames = call_frames.clone();
        inner.frame_locals = frame_locals.clone();
        inner.remote_objects.clear();
        inner.pause_line = line_u;
        inner.pause_col = col_u;
        inner.pause_depth = pause_depth;
        inner.resume_tx = Some(resume_tx);
        inner.pause_cmd_tx = Some(cmd_tx);
    }
    inspector
        .paused
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let params = build_paused_params(reason, call_frames, hit_bps);
    inspector.broadcast_event("Debugger.paused", params).await;
    let action = loop {
        tokio::select! {
            action = &mut resume_rx => { break action.unwrap_or(ResumeAction::Continue); }
            cmd = cmd_rx.recv() => {
                if let Some(cmd) = cmd {
                    let mut remote = {
                        let mut inner = inspector.inner.lock().await;
                        std::mem::take(&mut inner.remote_objects)
                    };
                    dispatch_pause_command(caller, &frame_locals, &mut remote, cmd).await;
                    {
                        let mut inner = inspector.inner.lock().await;
                        inner.remote_objects = remote;
                    }
                }
            }
        }
    };
    {
        let mut inner = inspector.inner.lock().await;
        inner.paused = false;
        inner.resume_tx = None;
        inner.pause_cmd_tx = None;
        inner.apply_resume_action(action);
    }
    inspector
        .paused
        .store(false, std::sync::atomic::Ordering::SeqCst);
    let _ = inspector
        .broadcast_event("Debugger.resumed", serde_json::json!({}))
        .await;
}

/// 构造 CDP `Debugger.paused` params。
pub(crate) fn build_paused_params(
    reason: PauseReason,
    call_frames: Vec<serde_json::Value>,
    hit_breakpoints: Vec<String>,
) -> serde_json::Value {
    let mut params = serde_json::json!({
        "reason": reason.as_cdp(),
        "callFrames": call_frames,
    });
    if !hit_breakpoints.is_empty() {
        params["hitBreakpoints"] = serde_json::Value::Array(
            hit_breakpoints
                .into_iter()
                .map(serde_json::Value::String)
                .collect(),
        );
    }
    params
}

/// 从 `Caller` 快照调用栈：优先 guest_debug FrameHandle，回退 WasmBacktrace + sourcemap。
pub(crate) fn snapshot_call_frames(
    caller: &mut Caller<'_, RuntimeState>,
    debug_info: &DebugInfo,
    line: u32,
    col: u32,
) -> Vec<serde_json::Value> {
    if let Some(frames) = try_guest_debug_frames(caller, debug_info, line, col)
        && !frames.is_empty()
    {
        return frames;
    }

    if let Some(frames) = try_wasm_backtrace_frames(caller, debug_info, line, col)
        && !frames.is_empty()
    {
        return frames;
    }

    vec![synthetic_top_frame(debug_info, line, col)]
}

fn try_guest_debug_frames(
    caller: &mut Caller<'_, RuntimeState>,
    debug_info: &DebugInfo,
    line: u32,
    col: u32,
) -> Option<Vec<serde_json::Value>> {
    let handles: Vec<_> = caller.debug_exit_frames().collect();
    if handles.is_empty() {
        return None;
    }
    let mut frames = Vec::new();
    for (depth, handle) in handles.into_iter().enumerate() {
        let func_idx_pc = handle
            .wasm_function_index_and_pc(&mut *caller)
            .ok()
            .flatten();
        let (func_name, loc_line, loc_col) = match func_idx_pc {
            Some((idx, pc)) => {
                // DefinedFuncIndex Debug 形如 `DefinedFuncIndex(N)`；N 与 backend wasm func 索引对齐时
                // 可走 lookup_pc，否则回退顶层 line/col。
                let name = format!("wasm_func_{idx:?}");
                let idx_num = format!("{idx:?}")
                    .chars()
                    .filter(|c| c.is_ascii_digit())
                    .collect::<String>()
                    .parse::<u32>()
                    .ok();
                let (l, c) = if depth == 0 {
                    (line, col)
                } else if let Some(fi) = idx_num {
                    debug_info
                        .lookup_pc(fi, pc)
                        .or_else(|| debug_info.lookup_func(fi))
                        .unwrap_or((1, 0))
                } else {
                    (1, 0)
                };
                (name, l, c)
            }
            None => (
                "<anonymous>".to_string(),
                if depth == 0 { line } else { 1 },
                if depth == 0 { col } else { 0 },
            ),
        };

        frames.push(cdp_call_frame(
            &format!("frame-{depth}"),
            &func_name,
            debug_info,
            loc_line,
            loc_col,
        ));
        if depth >= 31 {
            break;
        }
    }
    Some(frames)
}

fn try_wasm_backtrace_frames(
    caller: &mut Caller<'_, RuntimeState>,
    debug_info: &DebugInfo,
    line: u32,
    col: u32,
) -> Option<Vec<serde_json::Value>> {
    let bt = WasmBacktrace::capture(&caller);
    let wasm_frames = bt.frames();
    if wasm_frames.is_empty() {
        return None;
    }
    let sm = caller.data().source_map.as_ref();
    let mut out = Vec::with_capacity(wasm_frames.len());
    for (i, frame) in wasm_frames.iter().enumerate() {
        let func_name = frame.func_name().unwrap_or("<anonymous>");
        let func_idx = frame.func_index();
        let (loc_line, loc_col) = if i == 0 {
            (line, col)
        } else {
            lookup_line_col(debug_info, sm, func_idx).unwrap_or((1, 0))
        };
        out.push(cdp_call_frame(
            &format!("frame-{i}"),
            func_name,
            debug_info,
            loc_line,
            loc_col,
        ));
    }
    Some(out)
}

fn lookup_line_col(
    debug_info: &DebugInfo,
    sm: Option<&SourceMapInfo>,
    func_idx: u32,
) -> Option<(u32, u32)> {
    if let Some(lc) = debug_info.lookup_func(func_idx) {
        return Some(lc);
    }
    sm.and_then(|m| m.lookup(func_idx))
}

fn synthetic_top_frame(debug_info: &DebugInfo, line: u32, col: u32) -> serde_json::Value {
    cdp_call_frame("frame-0", "<anonymous>", debug_info, line, col)
}

/// 在 guest_debug 可用时读取各帧局部变量（NaN-box i64）。
pub(crate) fn capture_frame_locals(
    caller: &mut Caller<'_, RuntimeState>,
    debug_info: &DebugInfo,
) -> HashMap<String, Vec<(String, i64)>> {
    let mut out = HashMap::new();
    let handles: Vec<_> = caller.debug_exit_frames().collect();
    for (depth, handle) in handles.into_iter().enumerate() {
        let frame_id = format!("frame-{depth}");
        let mut pairs = Vec::new();
        let func_idx = handle
            .wasm_function_index_and_pc(&mut *caller)
            .ok()
            .flatten()
            .and_then(|(idx, _)| parse_entity_index_u32(idx));
        let num_locals = handle.num_locals(&mut *caller).unwrap_or(0);
        for local_i in 0..num_locals {
            let Ok(val) = handle.local(&mut *caller, local_i) else {
                continue;
            };
            let Val::I64(raw) = val else {
                continue;
            };
            let name = func_idx
                .and_then(|fi| {
                    debug_info
                        .local_entries
                        .iter()
                        .find(|e| e.func_idx == fi && e.local_idx == local_i)
                        .map(|e| e.name.clone())
                })
                .unwrap_or_else(|| format!("${local_i}"));
            if let Some(display) = display_local_name(&name) {
                pairs.push((display, raw));
            }
        }
        out.insert(frame_id, pairs);
        if depth >= 31 {
            break;
        }
    }
    out
}

/// 从 wasmtime entity index 的 Debug 表示中解析 `N`（如 `DefinedFuncIndex(3)`）。
fn parse_entity_index_u32(idx: impl std::fmt::Debug) -> Option<u32> {
    format!("{idx:?}")
        .chars()
        .filter(|c| c.is_ascii_digit())
        .collect::<String>()
        .parse()
        .ok()
}

/// 将 IR 槽名 `$N.x` / `$this` 转为用户可见名；过滤内部槽。
fn display_local_name(name: &str) -> Option<String> {
    if name == "$env" || name.ends_with(".$env") {
        return None;
    }
    if name == "$this" || name.ends_with(".$this") {
        return Some("this".to_string());
    }
    if name == "$0.$global" || name.ends_with(".$global") {
        return Some("globalThis".to_string());
    }
    if let Some(rest) = name.strip_prefix('$')
        && let Some(dot) = rest.find('.')
    {
        let user = &rest[dot + 1..];
        if user.is_empty() || user.starts_with('$') {
            return None;
        }
        return Some(user.to_string());
    }
    if name.starts_with('$') {
        return None;
    }
    Some(name.to_string())
}

fn cdp_call_frame(
    call_frame_id: &str,
    function_name: &str,
    debug_info: &DebugInfo,
    line: u32,
    col: u32,
) -> serde_json::Value {
    // CDP：lineNumber / columnNumber 均为 0-based。
    let line_number = line.saturating_sub(1);
    let column_number = col.saturating_sub(1);
    serde_json::json!({
        "callFrameId": call_frame_id,
        "functionName": function_name,
        "location": {
            "scriptId": MAIN_SCRIPT_ID,
            "lineNumber": line_number,
            "columnNumber": column_number,
        },
        "url": debug_info.source_url,
        "scopeChain": [{
            "type": "local",
            "object": {
                "type": "object",
                "className": "Object",
                "description": "Object",
                "objectId": format!("scope:{call_frame_id}"),
            },
        }],
        "this": {
            "type": "undefined",
        },
    })
}
