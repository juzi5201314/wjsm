//! Inspector 宿主 import：`env.debug_break(line, col, flags)`。
//!
//! 始终注册；`RuntimeOptions.inspect == None` 时立即返回。
//! 暂停期间循环处理 CDP 命令（getProperties / evaluate），持有 `Caller`。

use anyhow::Result;
use tokio::sync::{mpsc, oneshot};
use wasmtime::{Caller, Linker, Store};

use crate::inspector::pause_ops::dispatch_pause_command;
use crate::inspector::state::{PauseCommand, ResumeAction};
use crate::inspector::{capture_frame_locals, snapshot_call_frames};
use crate::*;

pub(crate) fn define_inspector_host(
    linker: &mut Linker<RuntimeState>,
    _store: &mut Store<RuntimeState>,
) -> Result<()> {
    // debug_break(line: i32, col: i32, flags: i32) -> ()
    // flags&1 == 无条件 debugger; 语句。
    linker.func_wrap_async(
        "env",
        "debug_break",
        |mut caller: Caller<'_, RuntimeState>, (line, col, flags): (i32, i32, i32)| {
            Box::new(async move {
                let Some(inspector) = caller.data().inspector.clone() else {
                    return;
                };

                let line_u = line.max(0) as u32;
                let col_u = col.max(0) as u32;

                // 栈深：guest_debug frames 数量；无则 0。
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
                let call_frames = snapshot_call_frames(&mut caller, &debug_info, line_u, col_u);
                let frame_locals = capture_frame_locals(&mut caller, &debug_info);
                let pause_depth = call_frames.len().saturating_sub(1) as u32;

                let (resume_tx, mut resume_rx) = oneshot::channel::<ResumeAction>();
                let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<PauseCommand>();

                // 进入暂停态并广播 Debugger.paused。
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

                let params =
                    crate::inspector::pause::build_paused_params(reason, call_frames, hit_bps);
                inspector.broadcast_event("Debugger.paused", params).await;

                // 暂停循环：resume 或处理需要 Caller 的 CDP 命令。
                let action = loop {
                    tokio::select! {
                        action = &mut resume_rx => {
                            break action.unwrap_or(ResumeAction::Continue);
                        }
                        cmd = cmd_rx.recv() => {
                            match cmd {
                                Some(cmd) => {
                                    // 从 shared state 取 remote_objects，命令处理后写回。
                                    let mut remote = {
                                        let mut inner = inspector.inner.lock().await;
                                        std::mem::take(&mut inner.remote_objects)
                                    };
                                    dispatch_pause_command(
                                        &mut caller,
                                        &frame_locals,
                                        &mut remote,
                                        cmd,
                                    )
                                    .await;
                                    {
                                        let mut inner = inspector.inner.lock().await;
                                        inner.remote_objects = remote;
                                    }
                                }
                                None => {
                                    // 所有 cmd_tx 已 drop，仅等 resume。
                                }
                            }
                        }
                    }
                };

                // 结束暂停。
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
            })
        },
    )?;
    Ok(())
}
