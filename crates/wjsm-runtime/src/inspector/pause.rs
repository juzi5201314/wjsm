//! 暂停决策辅助与 `Debugger.paused` 事件构造；调用栈快照。

use super::debug_info::DebugInfo;
use super::state::{MAIN_SCRIPT_ID, PauseReason};
use crate::RuntimeState;
use crate::runtime_source_map::SourceMapInfo;
use std::collections::HashMap;
use wasmtime::{Caller, Val, WasmBacktrace};

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
        && !frames.is_empty() {
            return frames;
        }

    if let Some(frames) = try_wasm_backtrace_frames(caller, debug_info, line, col)
        && !frames.is_empty() {
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
        let func_idx_pc = handle.wasm_function_index_and_pc(&mut *caller).ok().flatten();
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
        && let Some(dot) = rest.find('.') {
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
