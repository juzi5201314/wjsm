//! Inspector 宿主 import：`env.debug_break(line, col, flags)`。
//!
//! 始终注册；`RuntimeOptions.inspect == None` 时立即返回。

use anyhow::Result;
use wasmtime::{Caller, Linker, Store};

use crate::inspector::snapshot_call_frames;
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
        |mut caller: Caller<'_, RuntimeState>,
         (line, col, flags): (i32, i32, i32)| {
            Box::new(async move {
                let Some(inspector) = caller.data().inspector.clone() else {
                    return;
                };

                let line_u = line.max(0) as u32;
                let col_u = col.max(0) as u32;

                let decision = {
                    let mut inner = inspector.inner.lock().await;
                    inner.should_pause(line_u, col_u, flags)
                };
                let Some((reason, hit_bps)) = decision else {
                    return;
                };

                let debug_info = {
                    let inner = inspector.inner.lock().await;
                    inner.debug_info.clone()
                };
                let call_frames =
                    snapshot_call_frames(&mut caller, &debug_info, line_u, col_u);
                let frame_locals =
                    crate::inspector::capture_frame_locals(&mut caller, &debug_info);

                // 清空并重建远程对象表与帧局部缓存（暂停点语义）。
                {
                    let mut inner = inspector.inner.lock().await;
                    inner.remote_objects.clear();
                    inner.frame_locals = frame_locals;
                }

                let _action = inspector
                    .enter_pause_and_wait(reason, call_frames, hit_bps)
                    .await;
            })
        },
    )?;
    Ok(())
}
