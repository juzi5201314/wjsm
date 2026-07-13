//! 集群级 MessagePort / Worker 状态与 Store 本地绑定。

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use wasmtime::Caller;

use crate::runtime_worker_message::SerializedValue;
use crate::scheduler::{AsyncHostCompletion, AsyncOpGuard};
use crate::*;

/// 集群级 MessagePort 端点（可跨 OS 线程共享）。
pub(crate) struct PortEndpoint {
    pub(crate) id: u32,
    pub(crate) peer_id: u32,
    pub(crate) closed: AtomicBool,
    pub(crate) inbox: Mutex<VecDeque<SerializedValue>>,
    pub(crate) wake_tx: Mutex<Option<tokio::sync::mpsc::UnboundedSender<AsyncHostCompletion>>>,
}

/// 集群级 Worker 控制块。
pub(crate) struct WorkerControl {
    pub(crate) id: u32,
    #[allow(dead_code)]
    pub(crate) thread_id: u32,
    pub(crate) parent_port_id: u32,
    pub(crate) worker_port_id: u32,
    pub(crate) terminated: AtomicBool,
    pub(crate) exit_notified: AtomicBool,
    /// worker 线程 scheduler 的 completion channel（terminate 注入 exit）。
    pub(crate) worker_wake_tx:
        Mutex<Option<tokio::sync::mpsc::UnboundedSender<AsyncHostCompletion>>>,
    /// 父线程 scheduler channel（online/error/exit/message 旁路用）。
    pub(crate) parent_wake_tx:
        Mutex<Option<tokio::sync::mpsc::UnboundedSender<AsyncHostCompletion>>>,
}

/// 跨 agent 共享的 worker_threads 集群状态。
pub(crate) struct WorkerClusterState {
    pub(crate) next_port_id: AtomicU32,
    pub(crate) next_worker_id: AtomicU32,
    pub(crate) next_thread_id: AtomicU32,
    pub(crate) ports: Mutex<HashMap<u32, Arc<PortEndpoint>>>,
    pub(crate) workers: Mutex<HashMap<u32, Arc<WorkerControl>>>,
    pub(crate) max_workers: AtomicU32,
    pub(crate) active_workers: AtomicU32,
}

impl WorkerClusterState {
    pub(crate) fn new() -> Self {
        let max = std::env::var("WJSM_WORKER_THREADS_MAX")
            .ok()
            .and_then(|s| s.parse::<u32>().ok())
            .filter(|n| *n > 0)
            .unwrap_or(32);
        Self {
            next_port_id: AtomicU32::new(1),
            next_worker_id: AtomicU32::new(1),
            next_thread_id: AtomicU32::new(1),
            ports: Mutex::new(HashMap::new()),
            workers: Mutex::new(HashMap::new()),
            max_workers: AtomicU32::new(max),
            active_workers: AtomicU32::new(0),
        }
    }

    pub(crate) fn alloc_port_pair(self: &Arc<Self>) -> (Arc<PortEndpoint>, Arc<PortEndpoint>) {
        let id_a = self.next_port_id.fetch_add(1, Ordering::Relaxed);
        let id_b = self.next_port_id.fetch_add(1, Ordering::Relaxed);
        let a = Arc::new(PortEndpoint {
            id: id_a,
            peer_id: id_b,
            closed: AtomicBool::new(false),
            inbox: Mutex::new(VecDeque::new()),
            wake_tx: Mutex::new(None),
        });
        let b = Arc::new(PortEndpoint {
            id: id_b,
            peer_id: id_a,
            closed: AtomicBool::new(false),
            inbox: Mutex::new(VecDeque::new()),
            wake_tx: Mutex::new(None),
        });
        let mut ports = self.ports.lock().unwrap_or_else(|e| e.into_inner());
        ports.insert(id_a, Arc::clone(&a));
        ports.insert(id_b, Arc::clone(&b));
        (a, b)
    }

    pub(crate) fn port(&self, id: u32) -> Option<Arc<PortEndpoint>> {
        self.ports
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&id)
            .cloned()
    }

    pub(crate) fn worker(&self, id: u32) -> Option<Arc<WorkerControl>> {
        self.workers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&id)
            .cloned()
    }
}

/// Store 本地 MessagePort 绑定（deliver 回调仅本 Store 有效）。
pub(crate) struct LocalPortBinding {
    #[allow(dead_code)]
    pub(crate) global_id: u32,
    pub(crate) deliver_cb: Option<i64>,
    pub(crate) started: bool,
    pub(crate) ref_guard: Option<AsyncOpGuard>,
    pub(crate) scope: Option<crate::CapturedScope>,
}

/// Store 本地 Worker 绑定。
pub(crate) struct LocalWorkerBinding {
    #[allow(dead_code)]
    pub(crate) global_id: u32,
    pub(crate) online_cb: Option<i64>,
    pub(crate) message_cb: Option<i64>,
    pub(crate) error_cb: Option<i64>,
    pub(crate) exit_cb: Option<i64>,
    pub(crate) lifetime_guard: Option<AsyncOpGuard>,
    pub(crate) scope: Option<crate::CapturedScope>,
}

pub(super) fn cluster_of(caller: &Caller<'_, RuntimeState>) -> Option<Arc<WorkerClusterState>> {
    caller
        .data()
        .shared_state
        .as_ref()
        .map(|s| Arc::clone(&s.worker_cluster))
}
