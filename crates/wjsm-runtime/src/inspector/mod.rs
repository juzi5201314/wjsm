//! CDP Inspector 运行时：WebSocket 调试协议 + 异步 `debug_break` 暂停。
//!
//! 混合暂停模型：
//! - 宿主异步 import `env.debug_break(line, col, flags)` 负责断点/步进/debugger 语句暂停；
//! - `guest_debug` 在 inspect 启用时打开，便于 `debug_exit_frames` 读取 FrameHandle 局部变量。

mod cdp;
mod debug_info;
pub(crate) mod pause;
pub(crate) mod pause_ops;
mod remote_object;
mod server;
pub(crate) mod state;

pub(crate) use debug_info::DebugInfo;
pub(crate) use pause::{capture_frame_locals, snapshot_call_frames};
pub(crate) use state::{InspectorInner, PauseReason, ResumeAction, StepMode};

use anyhow::{Context, Result};

/// CDP / Node inspector 监听配置。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InspectConfig {
    /// 监听地址，默认 `127.0.0.1`。
    pub host: String,
    /// 监听端口，默认 `9229`；`0` 表示由 OS 分配临时端口。
    pub port: u16,
    /// 启动时在入口处暂停（等价于 Node `--inspect-brk`）。
    pub break_on_start: bool,
}

impl Default for InspectConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 9229,
            break_on_start: false,
        }
    }
}

impl InspectConfig {
    /// 供 `node:inspector.url()` / `globalThis.__wjsm_inspector_url` 使用的占位 URL。
    ///
    /// 端口为 0（临时端口）时在 bind 前无法确定地址，返回 `None`。
    /// CDP 服务真正启动后应使用 [`InspectorHandle::ws_url`]。
    pub fn provisional_url(&self) -> Option<String> {
        if self.port == 0 {
            return None;
        }
        Some(format!("http://{}:{}", self.host, self.port))
    }
}
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use tokio::sync::{Mutex, Notify, mpsc};

/// 跨 Store/任务共享的 Inspector 句柄（`RuntimeState` 需保持 `Send`）。
#[derive(Clone)]
pub(crate) struct InspectorHandle {
    pub(crate) inner: Arc<Mutex<InspectorInner>>,
    /// 任意 resume 动作时通知等待方。
    pub(crate) resume_notify: Arc<Notify>,
    /// 当前是否处于暂停态。
    pub(crate) paused: Arc<AtomicBool>,
    /// 调试会话 id（WebSocket 路径用）。
    pub(crate) session_id: Arc<String>,
    /// 实际绑定地址（含 ephemeral port 解析后）。
    pub(crate) host: Arc<String>,
    pub(crate) port: Arc<AtomicU32>,
    /// 向所有已连接会话广播 CDP 事件的 fan-out 发送端列表。
    pub(crate) event_txs: Arc<Mutex<Vec<mpsc::UnboundedSender<String>>>>,
    /// 关闭监听循环。
    pub(crate) shutdown: Arc<AtomicBool>,
}

impl InspectorHandle {
    /// 解析 debug info、绑定 TCP、spawn accept 循环，返回可放入 `RuntimeState` 的句柄。
    pub(crate) async fn start(config: &InspectConfig, debug_info: DebugInfo) -> Result<Self> {
        let session_id = uuid::Uuid::new_v4().to_string();
        let bind_addr = format!("{}:{}", config.host, config.port);
        let listener = tokio::net::TcpListener::bind(&bind_addr)
            .await
            .with_context(|| format!("inspector bind failed: {bind_addr}"))?;
        let local = listener
            .local_addr()
            .with_context(|| "inspector local_addr")?;
        let port = local.port();

        let mut inner = InspectorInner::new(debug_info);
        if config.break_on_start {
            // CDP lineNumber 为 0-based；合成入口断点 + 首个 debug_break 强制暂停。
            inner.break_on_start = true;
            inner.breakpoints.insert(
                state::BreakpointKey {
                    script_id: state::MAIN_SCRIPT_ID.to_string(),
                    line: 0,
                },
                state::BreakpointEntry {
                    id: "1".to_string(),
                    column: None,
                },
            );
            inner.next_breakpoint_id = 2;
        }

        let handle = Self {
            inner: Arc::new(Mutex::new(inner)),
            resume_notify: Arc::new(Notify::new()),
            paused: Arc::new(AtomicBool::new(false)),
            session_id: Arc::new(session_id),
            host: Arc::new(config.host.clone()),
            port: Arc::new(AtomicU32::new(port as u32)),
            event_txs: Arc::new(Mutex::new(Vec::new())),
            shutdown: Arc::new(AtomicBool::new(false)),
        };

        let accept_handle = handle.clone();
        tokio::spawn(async move {
            server::accept_loop(listener, accept_handle).await;
        });

        let ws_url = handle.ws_url();
        eprintln!("Debugger listening on {ws_url}");

        Ok(handle)
    }

    pub(crate) fn ws_url(&self) -> String {
        format!(
            "ws://{}:{}/{}",
            self.host,
            self.port.load(Ordering::Relaxed),
            self.session_id
        )
    }

    /// 向所有会话广播 CDP 事件（无 id 字段）。
    pub(crate) async fn broadcast_event(&self, method: &str, params: serde_json::Value) {
        let msg = serde_json::json!({
            "method": method,
            "params": params,
        })
        .to_string();
        let mut txs = self.event_txs.lock().await;
        txs.retain(|tx| tx.send(msg.clone()).is_ok());
    }

    /// 注册会话出站通道。
    pub(crate) async fn register_session(&self, tx: mpsc::UnboundedSender<String>) {
        self.event_txs.lock().await.push(tx);
    }

    /// 由 CDP 方法触发 resume。
    pub(crate) async fn request_resume(&self, action: ResumeAction) {
        let tx = {
            let mut inner = self.inner.lock().await;
            inner.resume_tx.take()
        };
        if let Some(tx) = tx {
            let _ = tx.send(action);
        }
        self.resume_notify.notify_waiters();
    }
}

/// 可选的 wasmtime `DebugHandler`：记录 EpochYield / Breakpoint 事件。
#[derive(Clone)]
pub(crate) struct WjsmDebugHandler {
    pub(crate) inspector: InspectorHandle,
}

impl wasmtime::DebugHandler for WjsmDebugHandler {
    type Data = crate::RuntimeState;

    fn handle(
        &self,
        _store: wasmtime::StoreContextMut<'_, Self::Data>,
        event: wasmtime::DebugEvent<'_>,
    ) -> impl std::future::Future<Output = ()> + Send {
        let _inspector = self.inspector.clone();
        async move {
            match event {
                wasmtime::DebugEvent::Breakpoint => {
                    // 混合模型以 `debug_break` 为主；guest breakpoint 仅作诊断。
                    #[cfg(debug_assertions)]
                    {
                        let _ = &_inspector;
                    }
                }
                wasmtime::DebugEvent::EpochYield
                | wasmtime::DebugEvent::Trap(_)
                | wasmtime::DebugEvent::HostcallError(_)
                | wasmtime::DebugEvent::CaughtExceptionThrown(_)
                | wasmtime::DebugEvent::UncaughtExceptionThrown(_) => {}
            }
        }
    }
}
