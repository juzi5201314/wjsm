use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::time::Duration;

use wjsm_artifact_format::{ArtifactBuildInput, BuildOptions, ModuleManifest, PortableArtifact};
use wjsm_ir::value;
use wjsm_native_abi::NativeVmContext;

use super::runtime::{fail_dispatch, is_truthy, render_value};
use super::{
    modules,
    structured_clone::{self, SerializedGraph},
};
use crate::{NativeAgentState, NativeCallableKind, NativeRuntime};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum WorkerThreadsMethod {
    CreateMessageChannel,
    PortPostMessage,
    PortStart,
    PortClose,
    PortRef,
    PortUnref,
    ReceiveMessageOnPort,
    CreateWorker,
    WorkerPostMessage,
    WorkerTerminate,
    WorkerRef,
    WorkerUnref,
    WorkerOnLifecycle,
    GetIsMainThread,
    GetThreadId,
    GetWorkerData,
    GetParentPortId,
}

enum WorkerEvent {
    Online(u32),
    Message {
        worker_id: u32,
        value: SerializedGraph,
    },
    Error(u32, String),
    Exit(u32, i32),
    Output(Vec<u8>),
}

enum WorkerCommand {
    Message(SerializedGraph),
    Terminate,
}

struct WorkerControl {
    terminated: AtomicBool,
    command_tx: mpsc::Sender<WorkerCommand>,
}

/// test262 `$262.agent` 的命令（main → agent 线程）。
pub(crate) enum AgentCommand {
    Broadcast(u64),
}

/// 单个 test262 agent 的 control：main agent 通过 command_tx 向其投递命令。
struct AgentControl {
    command_tx: mpsc::Sender<AgentCommand>,
}

/// test262 `$262.agent` 的共享状态（cluster 级）。
///
/// - `agent_reports`：各 agent 通过 `report` 压入的报告队列，main 用 `getReport` 取出。
/// - `broadcasts`：main `broadcast` 压入的待分发广播（seq → backing_id + 可选值）。
/// - `agents`：存活 agent 的 command 通道。
/// - `broadcast_confirmations`：main 阻塞等待所有 agent retrieve 的计数。
pub(crate) struct AgentClusterState {
    agents: Mutex<HashMap<u32, Arc<AgentControl>>>,
    next_agent_id: AtomicU32,
    agent_reports: Mutex<VecDeque<String>>,
    broadcasts: Mutex<HashMap<u64, (u32, Option<i64>)>>,
    next_broadcast_seq: AtomicU64,
    broadcast_confirmations: Mutex<HashMap<u64, u32>>,
    broadcast_condvar: Condvar,
    monotonic_now: std::time::Instant,
}

pub(crate) struct WorkerCluster {
    next_worker_id: AtomicU32,
    next_thread_id: AtomicU32,
    workers: Mutex<HashMap<u32, Arc<WorkerControl>>>,
    /// cluster 级 SAB backing 表：backing_id → (byte_length, max_byte_length)。
    /// backing 本体由 `Arc<Mutex<Vec<u8>>>` 持有，跨同 cluster 的 agent 共享。
    sab_table: Mutex<HashMap<u32, (usize, Option<usize>)>>,
    /// backing 本体：backing_id → Arc<Mutex<Vec<u8>>>。
    sab_backings: Mutex<HashMap<u32, Arc<Mutex<Vec<u8>>>>>,
    next_sab_id: AtomicU32,
    /// SAB wait 队列：location_key = (backing_id << 32) | byte_offset。
    /// 每个 waiting agent 注册一个 `Arc<(Mutex<WaiterStatus>, Condvar)>`；
    /// notify 唤醒匹配位置的前 N 个，其余继续等待。
    waiters: Mutex<HashMap<u64, Vec<WaiterRegistration>>>,
    /// waitAsync 已到期等待项：(backing_id, byte_offset, promise_handle)。
    /// 由后台线程 push，owner loop 在 `poll_external_events` 中 drain 并 settle。
    wait_timeouts: Mutex<VecDeque<(u32, usize, u32)>>,
    /// test262 `$262.agent` 基础设施。
    pub(crate) agent: AgentClusterState,
}

/// 单个 SAB wait 会话：notify 唤醒后置 `Notified`，超时置 `TimedOut`。
struct WaiterRegistration {
    status: Arc<(Mutex<WaiterStatus>, Condvar)>,
    /// waitAsync 的 promise handle；`None` 表示同步 wait。
    promise: Option<u32>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WaiterStatus {
    Waiting,
    Notified,
    TimedOut,
}

impl WorkerCluster {
    fn new() -> Self {
        Self {
            next_worker_id: AtomicU32::new(1),
            next_thread_id: AtomicU32::new(1),
            workers: Mutex::new(HashMap::new()),
            sab_table: Mutex::new(HashMap::new()),
            sab_backings: Mutex::new(HashMap::new()),
            next_sab_id: AtomicU32::new(1),
            waiters: Mutex::new(HashMap::new()),
            wait_timeouts: Mutex::new(VecDeque::new()),
            agent: AgentClusterState {
                agents: Mutex::new(HashMap::new()),
                next_agent_id: AtomicU32::new(1),
                agent_reports: Mutex::new(VecDeque::new()),
                broadcasts: Mutex::new(HashMap::new()),
                next_broadcast_seq: AtomicU64::new(1),
                broadcast_confirmations: Mutex::new(HashMap::new()),
                broadcast_condvar: Condvar::new(),
                monotonic_now: std::time::Instant::now(),
            },
        }
    }

    /// 分配一个新的 cluster 级 SAB backing，返回 backing_id。
    pub(crate) fn allocate_sab(&self, byte_length: usize, max_byte_length: Option<usize>) -> u32 {
        let bytes = vec![0; byte_length];
        self.allocate_sab_bytes(bytes, byte_length, max_byte_length)
    }

    /// 以显式 bytes 分配 SAB backing（slice 拷贝产物）。
    pub(crate) fn allocate_sab_bytes(
        &self,
        bytes: Vec<u8>,
        byte_length: usize,
        max_byte_length: Option<usize>,
    ) -> u32 {
        let mut table = self
            .sab_table
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut backings = self
            .sab_backings
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        loop {
            let id = self.next_sab_id.fetch_add(1, Ordering::Relaxed);
            if table.contains_key(&id) {
                continue;
            }
            table.insert(id, (byte_length, max_byte_length));
            backings.insert(id, Arc::new(Mutex::new(bytes)));
            return id;
        }
    }

    /// 按 backing_id 取回 cluster backing 引用。
    pub(crate) fn sab(&self, backing_id: u32) -> Option<super::sab::SABBacking> {
        let table = self
            .sab_table
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (byte_length, max_byte_length) = table.get(&backing_id).copied()?;
        drop(table);
        let backings = self
            .sab_backings
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let bytes = backings.get(&backing_id).cloned()?;
        Some(super::sab::SABBacking {
            bytes,
            byte_length,
            max_byte_length,
        })
    }

    /// 更新某 backing 的 byte_length（grow 后）。
    pub(crate) fn update_sab_length(&self, backing_id: u32, byte_length: usize) -> bool {
        let mut table = self
            .sab_table
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some((_, max)) = table.get(&backing_id).copied() else {
            return false;
        };
        table.insert(backing_id, (byte_length, max));
        true
    }

    /// 注册一个 wait 会话并返回其状态句柄。`promise` 为 waitAsync 的 promise handle。
    pub(crate) fn wait_register(
        &self,
        backing_id: u32,
        byte_offset: usize,
        promise: Option<u32>,
    ) -> Arc<(Mutex<WaiterStatus>, Condvar)> {
        let registration = Arc::new((Mutex::new(WaiterStatus::Waiting), Condvar::new()));
        let mut waiters = self
            .waiters
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let key = (u64::from(backing_id) << 32) | byte_offset as u64;
        waiters.entry(key).or_default().push(WaiterRegistration {
            status: Arc::clone(&registration),
            promise,
        });
        registration
    }

    /// 阻塞当前线程直至被 notify 唤醒或超时。返回最终状态。
    pub(crate) fn wait_block(
        &self,
        backing_id: u32,
        byte_offset: usize,
        registration: &Arc<(Mutex<WaiterStatus>, Condvar)>,
        timeout: Option<Duration>,
    ) -> WaiterStatus {
        let (lock, condvar) = &**registration;
        let mut status = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if *status != WaiterStatus::Waiting {
            return *status;
        }
        let result = if let Some(timeout) = timeout {
            let (guard, wait_result) = condvar
                .wait_timeout_while(status, timeout, |status| *status == WaiterStatus::Waiting)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            status = guard;
            if wait_result.timed_out() && *status == WaiterStatus::Waiting {
                *status = WaiterStatus::TimedOut;
                // 从队列移除（best-effort）。
                let mut waiters = self
                    .waiters
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let key = (u64::from(backing_id) << 32) | byte_offset as u64;
                waiters
                    .entry(key)
                    .or_default()
                    .retain(|entry| !Arc::ptr_eq(&entry.status, registration));
            }
            *status
        } else {
            let guard = condvar
                .wait_while(status, |status| *status == WaiterStatus::Waiting)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            *guard
        };
        result
    }

    /// 唤醒指定位置的前 `count` 个 waiter（count=None 表示全部）。
    /// 返回被唤醒的 waitAsync promise handles（同步 wait 为 None）。
    pub(crate) fn notify_waiters(
        &self,
        backing_id: u32,
        byte_offset: usize,
        count: Option<u32>,
    ) -> Vec<u32> {
        let key = (u64::from(backing_id) << 32) | byte_offset as u64;
        let mut waiters = self
            .waiters
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(entries) = waiters.get_mut(&key) else {
            return Vec::new();
        };
        let limit = count.unwrap_or(u32::MAX) as usize;
        let mut notified = Vec::new();
        let mut remaining = Vec::new();
        for entry in entries.drain(..) {
            if notified.len() >= limit {
                remaining.push(entry);
                continue;
            }
            let status_arc = Arc::clone(&entry.status);
            let (lock, condvar) = &*status_arc;
            let mut status = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            if *status == WaiterStatus::Waiting {
                *status = WaiterStatus::Notified;
                if let Some(promise) = entry.promise {
                    notified.push(promise);
                }
                drop(status);
                condvar.notify_one();
            } else {
                remaining.push(entry);
            }
        }
        if !remaining.is_empty() {
            waiters.insert(key, remaining);
        } else {
            waiters.remove(&key);
        }
        notified
    }

    /// 登记一个已到期的 waitAsync 等待项（后台线程调用）。
    pub(crate) fn push_wait_timeout(&self, backing_id: u32, byte_offset: usize, promise: u32) {
        self.wait_timeouts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push_back((backing_id, byte_offset, promise));
    }

    /// 是否有已到期的 waitAsync 等待项待处理。
    pub(crate) fn has_wait_timeouts(&self) -> bool {
        !self
            .wait_timeouts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_empty()
    }

    /// 取出所有已到期的 waitAsync 等待项。
    pub(crate) fn pop_wait_timeouts(&self) -> Vec<(u32, usize, u32)> {
        self.wait_timeouts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .drain(..)
            .collect()
    }

    /// 从等待队列移除一个注册（超时后调用）。
    pub(crate) fn remove_waiter(
        &self,
        backing_id: u32,
        byte_offset: usize,
        registration: &Arc<(Mutex<WaiterStatus>, Condvar)>,
    ) {
        let key = (u64::from(backing_id) << 32) | byte_offset as u64;
        let mut waiters = self
            .waiters
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(entries) = waiters.get_mut(&key) {
            entries.retain(|entry| !Arc::ptr_eq(&entry.status, registration));
            if entries.is_empty() {
                waiters.remove(&key);
            }
        }
    }
}

/// test262 `$262.agent.start` 启动的 agent 线程上下文。
pub(crate) struct Test262AgentContext {
    pub(crate) cluster: Arc<WorkerCluster>,
    pub(crate) agent_id: u32,
    pub(crate) command_rx: mpsc::Receiver<AgentCommand>,
    pub(crate) runtime_config: crate::NativeRuntimeConfig,
}

impl AgentClusterState {
    /// 注册一个新 agent，返回其 command 通道的接收端。
    pub(crate) fn register_agent(&self) -> (u32, mpsc::Receiver<AgentCommand>) {
        let (command_tx, command_rx) = mpsc::channel();
        let agent_id = loop {
            let id = self.next_agent_id.fetch_add(1, Ordering::Relaxed);
            let mut agents = self
                .agents
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let std::collections::hash_map::Entry::Vacant(entry) = agents.entry(id) {
                entry.insert(Arc::new(AgentControl { command_tx }));
                break id;
            }
        };
        (agent_id, command_rx)
    }

    /// 从 agent 表移除一个 agent（其线程退出时）。
    pub(crate) fn unregister_agent(&self, agent_id: u32) {
        self.agents
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&agent_id);
    }

    /// main 向一个 agent 投递命令。
    pub(crate) fn send_command(&self, agent_id: u32, command: AgentCommand) -> bool {
        self.agents
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&agent_id)
            .map(|control| control.command_tx.send(command).is_ok())
            .unwrap_or(false)
    }

    /// 列出当前存活 agent 的 id（广播投递目标）。
    pub(crate) fn agent_ids(&self) -> Vec<u32> {
        self.agents
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .keys()
            .copied()
            .collect()
    }

    /// 压入一条广播（backing_id, value），返回广播 seq。
    pub(crate) fn register_broadcast(&self, backing_id: u32, value: Option<i64>) -> u64 {
        let seq = self.next_broadcast_seq.fetch_add(1, Ordering::Relaxed);
        self.broadcasts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(seq, (backing_id, value));
        seq
    }

    /// 取出一条广播。返回 None 表示队列空。
    pub(crate) fn pop_broadcast(&self) -> Option<(u64, u32, Option<i64>)> {
        let mut broadcasts = self
            .broadcasts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let seq = broadcasts.keys().copied().min()?;
        let (backing_id, value) = broadcasts.remove(&seq)?;
        Some((seq, backing_id, value))
    }

    /// agent 报告一条消息（main 用 getReport 拉取）。
    pub(crate) fn push_report(&self, message: String) {
        self.agent_reports
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push_back(message);
    }

    /// main 拉取队首报告；无则 None（对应 getReport 返回 null）。
    pub(crate) fn pop_report(&self) -> Option<String> {
        self.agent_reports
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .pop_front()
    }

    /// 单调时钟（ms），作为 `monotonicNow` 的基准。
    pub(crate) fn monotonic_millis(&self) -> f64 {
        self.monotonic_now.elapsed().as_secs_f64() * 1000.0
    }

    /// 登记一个已 retrieve 的 agent 并唤醒等待者。
    pub(crate) fn confirm_broadcast(&self, seq: u64) {
        let mut confirmations = self
            .broadcast_confirmations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *confirmations.entry(seq).or_default() += 1;
        drop(confirmations);
        self.broadcast_condvar.notify_all();
    }

    /// 阻塞直至 `target` 个 agent 全部 retrieve 该 seq。
    pub(crate) fn wait_broadcast(&self, seq: u64, target: u32) {
        let mut confirmations = self
            .broadcast_confirmations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        while confirmations.get(&seq).copied().unwrap_or(0) < target {
            confirmations = self
                .broadcast_condvar
                .wait(confirmations)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        confirmations.remove(&seq);
    }
}

#[derive(Default)]
struct WorkerCallbacks {
    online: Option<i64>,
    message: Option<i64>,
    error: Option<i64>,
    exit: Option<i64>,
    context: super::node_async_hooks::AsyncContextSnapshot,
    active: bool,
    referenced: bool,
}

struct LocalPort {
    peer: u32,
    inbox: VecDeque<SerializedGraph>,
    callback: Option<i64>,
    closed: bool,
    referenced: bool,
}

pub(crate) struct WorkerAgentContext {
    cluster: Arc<WorkerCluster>,
    event_tx: mpsc::Sender<WorkerEvent>,
    worker_id: u32,
    thread_id: u32,
    worker_data: SerializedGraph,
    control: Arc<WorkerControl>,
    command_rx: mpsc::Receiver<WorkerCommand>,
    runtime_config: crate::NativeRuntimeConfig,
}

pub(crate) struct NodeWorkerThreadsState {
    bridge: Option<i64>,
    pub(crate) cluster: Arc<WorkerCluster>,
    event_tx: mpsc::Sender<WorkerEvent>,
    event_rx: mpsc::Receiver<WorkerEvent>,
    callbacks: HashMap<u32, WorkerCallbacks>,
    ports: HashMap<u32, LocalPort>,
    next_port_id: u32,
    is_main_thread: bool,
    thread_id: u32,
    parent_worker_id: Option<u32>,
    parent_event_tx: Option<mpsc::Sender<WorkerEvent>>,
    parent_control: Option<Arc<WorkerControl>>,
    parent_command_rx: Option<mpsc::Receiver<WorkerCommand>>,
    parent_message_callback: Option<i64>,
    worker_data: Option<SerializedGraph>,
    materialized_worker_data: Option<i64>,
}

impl NodeWorkerThreadsState {
    pub(crate) fn main() -> Self {
        let (event_tx, event_rx) = mpsc::channel();
        Self {
            bridge: None,
            cluster: Arc::new(WorkerCluster::new()),
            event_tx,
            event_rx,
            callbacks: HashMap::new(),
            ports: HashMap::new(),
            next_port_id: 1,
            is_main_thread: true,
            thread_id: 0,
            parent_worker_id: None,
            parent_event_tx: None,
            parent_control: None,
            parent_command_rx: None,
            parent_message_callback: None,
            worker_data: None,
            materialized_worker_data: None,
        }
    }

    pub(crate) fn worker(context: WorkerAgentContext) -> Self {
        let (event_tx, event_rx) = mpsc::channel();
        Self {
            bridge: None,
            cluster: context.cluster,
            event_tx,
            event_rx,
            callbacks: HashMap::new(),
            ports: HashMap::new(),
            next_port_id: 1,
            is_main_thread: false,
            thread_id: context.thread_id,
            parent_worker_id: Some(context.worker_id),
            parent_event_tx: Some(context.event_tx),
            parent_control: Some(context.control),
            parent_command_rx: Some(context.command_rx),
            parent_message_callback: None,
            worker_data: Some(context.worker_data),
            materialized_worker_data: None,
        }
    }

    pub(crate) fn reset_agent(&mut self) {
        self.bridge = None;
        self.callbacks.clear();
        self.ports.clear();
        self.next_port_id = 1;
        self.parent_message_callback = None;
        self.materialized_worker_data = None;
    }

    pub(crate) fn has_pending(&self) -> bool {
        self.callbacks
            .values()
            .any(|callbacks| callbacks.active && callbacks.referenced)
            || self.ports.values().any(|port| {
                !port.closed && port.referenced && port.callback.is_some() && !port.inbox.is_empty()
            })
            || self.parent_message_callback.is_some()
                && self
                    .parent_control
                    .as_ref()
                    .is_some_and(|control| !control.terminated.load(Ordering::Acquire))
    }
}

pub(crate) fn ensure_bridge(state: &mut NativeAgentState) -> Option<i64> {
    if let Some(bridge) = state.node_worker_threads.bridge {
        return Some(bridge);
    }
    let methods = [
        (
            "createMessageChannel",
            WorkerThreadsMethod::CreateMessageChannel,
        ),
        ("portPostMessage", WorkerThreadsMethod::PortPostMessage),
        ("portStart", WorkerThreadsMethod::PortStart),
        ("portClose", WorkerThreadsMethod::PortClose),
        ("portRef", WorkerThreadsMethod::PortRef),
        ("portUnref", WorkerThreadsMethod::PortUnref),
        (
            "receiveMessageOnPort",
            WorkerThreadsMethod::ReceiveMessageOnPort,
        ),
        ("createWorker", WorkerThreadsMethod::CreateWorker),
        ("workerPostMessage", WorkerThreadsMethod::WorkerPostMessage),
        ("workerTerminate", WorkerThreadsMethod::WorkerTerminate),
        ("workerRef", WorkerThreadsMethod::WorkerRef),
        ("workerUnref", WorkerThreadsMethod::WorkerUnref),
        ("workerOnLifecycle", WorkerThreadsMethod::WorkerOnLifecycle),
        ("getIsMainThread", WorkerThreadsMethod::GetIsMainThread),
        ("getThreadId", WorkerThreadsMethod::GetThreadId),
        ("getWorkerData", WorkerThreadsMethod::GetWorkerData),
        ("getParentPortId", WorkerThreadsMethod::GetParentPortId),
    ];
    let bridge = state.allocate_object(methods.len() as u32, false).ok()?;
    for (name, method) in methods {
        let key = state.intern_text(name.into(), value::TAG_STRING)?;
        let callable = state.native_callable(NativeCallableKind::NodeWorkerThreads(method))?;
        state
            .heap
            .set_property(
                value::decode_handle(bridge),
                value::decode_handle(key),
                callable as u64,
            )
            .ok()?;
    }
    state.node_worker_threads.bridge = Some(bridge);
    Some(bridge)
}

pub(crate) fn call(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    method: WorkerThreadsMethod,
    args: &[i64],
) -> i64 {
    match method {
        WorkerThreadsMethod::CreateMessageChannel => create_message_channel(ctx, state),
        WorkerThreadsMethod::PortPostMessage => port_post_message(ctx, state, args),
        WorkerThreadsMethod::PortStart => port_start(ctx, state, args),
        WorkerThreadsMethod::PortClose => port_close(state, args),
        WorkerThreadsMethod::PortRef => port_ref(state, args, true),
        WorkerThreadsMethod::PortUnref => port_ref(state, args, false),
        WorkerThreadsMethod::ReceiveMessageOnPort => receive_message_on_port(ctx, state, args),
        WorkerThreadsMethod::CreateWorker => create_worker(ctx, state, args),
        WorkerThreadsMethod::WorkerPostMessage => worker_post_message(ctx, state, args),
        WorkerThreadsMethod::WorkerTerminate => worker_terminate(state, args),
        WorkerThreadsMethod::WorkerRef => worker_ref(state, args, true),
        WorkerThreadsMethod::WorkerUnref => worker_ref(state, args, false),
        WorkerThreadsMethod::WorkerOnLifecycle => worker_on_lifecycle(state, args),
        WorkerThreadsMethod::GetIsMainThread => {
            value::encode_bool(state.node_worker_threads.is_main_thread)
        }
        WorkerThreadsMethod::GetThreadId => {
            value::encode_f64(f64::from(state.node_worker_threads.thread_id))
        }
        WorkerThreadsMethod::GetWorkerData => get_worker_data(ctx, state),
        WorkerThreadsMethod::GetParentPortId => state
            .node_worker_threads
            .parent_worker_id
            .map_or_else(value::encode_null, |id| value::encode_f64(f64::from(id))),
    }
}

pub(crate) fn poll(ctx: &mut NativeVmContext, state: &mut NativeAgentState) -> i64 {
    if let Some(result) = poll_local_port(ctx, state) {
        return result;
    }
    if state.node_worker_threads.parent_message_callback.is_some()
        && let Some(result) = poll_parent_command(ctx, state)
    {
        return result;
    }
    let event = if state
        .node_worker_threads
        .callbacks
        .values()
        .any(|callbacks| callbacks.active && callbacks.referenced)
    {
        state
            .node_worker_threads
            .event_rx
            .recv_timeout(Duration::from_millis(10))
            .ok()
    } else {
        state.node_worker_threads.event_rx.try_recv().ok()
    };
    event.map_or_else(value::encode_undefined, |event| {
        dispatch_worker_event(ctx, state, event)
    })
}

fn create_message_channel(ctx: &mut NativeVmContext, state: &mut NativeAgentState) -> i64 {
    let port1 = state.node_worker_threads.next_port_id;
    let Some(port2) = port1.checked_add(1) else {
        return fail_dispatch(ctx);
    };
    state.node_worker_threads.next_port_id = port2.saturating_add(1);
    state.node_worker_threads.ports.insert(
        port1,
        LocalPort {
            peer: port2,
            inbox: VecDeque::new(),
            callback: None,
            closed: false,
            referenced: true,
        },
    );
    state.node_worker_threads.ports.insert(
        port2,
        LocalPort {
            peer: port1,
            inbox: VecDeque::new(),
            callback: None,
            closed: false,
            referenced: true,
        },
    );
    id_pair(ctx, state, "port1", port1, "port2", port2)
}

fn port_post_message(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(port_id) = numeric_id(args.first().copied()) else {
        return type_error(ctx, state, "portPostMessage: invalid port id");
    };
    let stored = args.get(1).copied().unwrap_or_else(value::encode_undefined);
    let serialized = match structured_clone::serialize(ctx, state, stored) {
        Ok(serialized) => serialized,
        Err(message) => return structured_clone::data_clone_error(ctx, state, &message),
    };
    if state.node_worker_threads.parent_worker_id == Some(port_id) {
        if let Some(event_tx) = state.node_worker_threads.parent_event_tx.as_ref() {
            let _ = event_tx.send(WorkerEvent::Message {
                worker_id: port_id,
                value: serialized,
            });
        }
        return value::encode_undefined();
    }
    let Some(peer) = state
        .node_worker_threads
        .ports
        .get(&port_id)
        .filter(|port| !port.closed)
        .map(|port| port.peer)
    else {
        return value::encode_undefined();
    };
    if let Some(port) = state.node_worker_threads.ports.get_mut(&peer)
        && !port.closed
    {
        port.inbox.push_back(serialized);
    }
    value::encode_undefined()
}

fn port_start(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(port_id) = numeric_id(args.first().copied()) else {
        return type_error(ctx, state, "portStart: invalid port id");
    };
    let callback = args.get(1).copied().unwrap_or_else(value::encode_undefined);
    if !value::is_callable(callback) {
        return type_error(ctx, state, "portStart: callback must be callable");
    }
    if state.node_worker_threads.parent_worker_id == Some(port_id) {
        state.node_worker_threads.parent_message_callback = Some(callback);
    } else if let Some(port) = state.node_worker_threads.ports.get_mut(&port_id) {
        port.callback = Some(callback);
    }
    value::encode_undefined()
}

fn port_close(state: &mut NativeAgentState, args: &[i64]) -> i64 {
    if let Some(port_id) = numeric_id(args.first().copied()) {
        if state.node_worker_threads.parent_worker_id == Some(port_id) {
            state.node_worker_threads.parent_message_callback = None;
        }
        if let Some(port) = state.node_worker_threads.ports.get_mut(&port_id) {
            port.closed = true;
            port.callback = None;
            port.inbox.clear();
        }
    }
    value::encode_undefined()
}

fn port_ref(state: &mut NativeAgentState, args: &[i64], referenced: bool) -> i64 {
    if let Some(port_id) = numeric_id(args.first().copied())
        && let Some(port) = state.node_worker_threads.ports.get_mut(&port_id)
    {
        port.referenced = referenced;
    }
    value::encode_undefined()
}

fn receive_message_on_port(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let Some(port_id) = numeric_id(args.first().copied()) else {
        return value::encode_undefined();
    };
    let Some(serialized) = state
        .node_worker_threads
        .ports
        .get_mut(&port_id)
        .and_then(|port| port.inbox.pop_front())
    else {
        return value::encode_undefined();
    };
    let message = match structured_clone::deserialize(state, &serialized) {
        Ok(message) => message,
        Err(message) => return structured_clone::data_clone_error(ctx, state, &message),
    };
    let result = match state.allocate_object(1, false) {
        Ok(result) => result,
        Err(_) => return fail_dispatch(ctx),
    };
    if modules::set_named_property(state, result, "message", message).is_err() {
        return fail_dispatch(ctx);
    }
    result
}

fn create_worker(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let filename = args
        .first()
        .and_then(|filename| state.string(*filename))
        .and_then(|filename| filename.to_utf8())
        .unwrap_or_else(|| {
            args.first()
                .map_or_else(String::new, |value| render_value(state, *value))
        });
    let options = args.get(1).copied().unwrap_or_else(value::encode_undefined);
    let is_eval = modules::named_property(state, options, "eval")
        .is_some_and(|value| is_truthy(state, value));
    let worker_data = modules::named_property(state, options, "workerData")
        .unwrap_or_else(value::encode_undefined);
    let worker_data = match structured_clone::serialize(ctx, state, worker_data) {
        Ok(worker_data) => worker_data,
        Err(message) => return structured_clone::data_clone_error(ctx, state, &message),
    };
    let worker_id = state
        .node_worker_threads
        .cluster
        .next_worker_id
        .fetch_add(1, Ordering::Relaxed);
    let thread_id = state
        .node_worker_threads
        .cluster
        .next_thread_id
        .fetch_add(1, Ordering::Relaxed);
    let (command_tx, command_rx) = mpsc::channel();
    let control = Arc::new(WorkerControl {
        terminated: AtomicBool::new(false),
        command_tx,
    });
    state
        .node_worker_threads
        .cluster
        .workers
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(worker_id, Arc::clone(&control));
    let callback_context = super::node_async_hooks::capture_context(state);
    state.node_worker_threads.callbacks.insert(
        worker_id,
        WorkerCallbacks {
            active: true,
            referenced: true,
            context: callback_context,
            ..WorkerCallbacks::default()
        },
    );
    let context = WorkerAgentContext {
        cluster: Arc::clone(&state.node_worker_threads.cluster),
        event_tx: state.node_worker_threads.event_tx.clone(),
        worker_id,
        thread_id,
        worker_data,
        control: Arc::clone(&control),
        command_rx,
        runtime_config: state.runtime_config.child_config(),
    };
    let root = state.working_directory.clone();
    std::thread::Builder::new()
        .name(format!("wjsm-worker-{worker_id}"))
        .spawn(move || run_worker(filename, is_eval, root, context))
        .map(|_| id_pair(ctx, state, "id", worker_id, "threadId", thread_id))
        .unwrap_or_else(|error| type_error(ctx, state, &format!("ERR_WORKER_INIT_FAILED: {error}")))
}

fn run_worker(filename: String, is_eval: bool, root: PathBuf, context: WorkerAgentContext) {
    let worker_id = context.worker_id;
    let control = Arc::clone(&context.control);
    let event_tx = context.event_tx.clone();
    let _ = event_tx.send(WorkerEvent::Online(worker_id));
    let outcome = compile_worker_artifact(&filename, is_eval, &root).and_then(|artifact| {
        let mut runtime = NativeRuntime::new_with_config(context.runtime_config.clone())
            .map_err(|error| error.to_string())?;
        runtime.configure_worker(context);
        runtime
            .execute(&artifact, &root, &root)
            .map_err(|error| error.to_string())
            .map(|execution| execution.stdout)
    });
    let exit_code = match outcome {
        Ok(output) => {
            if !output.is_empty() {
                let _ = event_tx.send(WorkerEvent::Output(output));
            }
            i32::from(control.terminated.load(Ordering::Acquire))
        }
        Err(message) => {
            let message = message
                .strip_prefix("Error: ")
                .unwrap_or(&message)
                .to_string();
            let _ = event_tx.send(WorkerEvent::Error(worker_id, message));
            1
        }
    };
    let _ = event_tx.send(WorkerEvent::Exit(worker_id, exit_code));
}

fn compile_worker_artifact(
    filename: &str,
    is_eval: bool,
    root: &Path,
) -> Result<PortableArtifact, String> {
    let program = if is_eval {
        let module =
            wjsm_parser::parse_script_as_module(filename).map_err(|error| error.to_string())?;
        let module_id = wjsm_ir::ModuleId(0);
        let input = wjsm_semantic::ModuleLoweringInput {
            id: module_id,
            ast: module,
            metadata: wjsm_semantic::ModuleMetadata {
                filename: "[worker eval]".into(),
                dirname: root.display().to_string(),
                url: "worker:eval".into(),
                kind: wjsm_semantic::ModuleKind::CommonJs,
            },
            source: Some(Arc::<str>::from(filename)),
        };
        wjsm_semantic::lower_modules(
            vec![input],
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
        )
        .map_err(|error| error.to_string())?
    } else {
        let path = PathBuf::from(filename);
        let path = if path.is_absolute() {
            path
        } else {
            root.join(path)
        };
        let module_root = path.parent().unwrap_or(root);
        wjsm_module::lower_bundle_cached_with_options(
            &path,
            module_root,
            wjsm_module::ResolutionOptions::default(),
        )
        .map_err(|error| error.to_string())?
    };
    PortableArtifact::from_input(&ArtifactBuildInput::new(
        program,
        ModuleManifest::single("worker:entry", true),
        BuildOptions::default(),
    ))
    .map_err(|error| error.to_string())
}

fn worker_post_message(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let Some(worker_id) = numeric_id(args.first().copied()) else {
        return type_error(ctx, state, "workerPostMessage: invalid worker id");
    };
    let stored = args.get(1).copied().unwrap_or_else(value::encode_undefined);
    let message = match structured_clone::serialize(ctx, state, stored) {
        Ok(message) => message,
        Err(message) => return structured_clone::data_clone_error(ctx, state, &message),
    };
    let control = state
        .node_worker_threads
        .cluster
        .workers
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(&worker_id)
        .cloned();
    if let Some(control) = control {
        let _ = control.command_tx.send(WorkerCommand::Message(message));
    }
    value::encode_undefined()
}

fn worker_terminate(state: &mut NativeAgentState, args: &[i64]) -> i64 {
    if let Some(worker_id) = numeric_id(args.first().copied())
        && let Some(control) = state
            .node_worker_threads
            .cluster
            .workers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&worker_id)
            .cloned()
    {
        control.terminated.store(true, Ordering::Release);
        let _ = control.command_tx.send(WorkerCommand::Terminate);
    }
    value::encode_undefined()
}

fn worker_ref(state: &mut NativeAgentState, args: &[i64], referenced: bool) -> i64 {
    if let Some(worker_id) = numeric_id(args.first().copied())
        && let Some(callbacks) = state.node_worker_threads.callbacks.get_mut(&worker_id)
    {
        callbacks.referenced = referenced;
    }
    value::encode_undefined()
}

fn worker_on_lifecycle(state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(worker_id) = numeric_id(args.first().copied()) else {
        return value::encode_undefined();
    };
    let handlers = args.get(1).copied().unwrap_or_else(value::encode_undefined);
    let online = modules::named_property(state, handlers, "online")
        .filter(|value| value::is_callable(*value));
    let message = modules::named_property(state, handlers, "message")
        .filter(|value| value::is_callable(*value));
    let error = modules::named_property(state, handlers, "error")
        .filter(|value| value::is_callable(*value));
    let exit =
        modules::named_property(state, handlers, "exit").filter(|value| value::is_callable(*value));
    if let Some(callbacks) = state.node_worker_threads.callbacks.get_mut(&worker_id) {
        callbacks.online = online;
        callbacks.message = message;
        callbacks.error = error;
        callbacks.exit = exit;
    }
    value::encode_undefined()
}

fn get_worker_data(ctx: &mut NativeVmContext, state: &mut NativeAgentState) -> i64 {
    if let Some(worker_data) = state.node_worker_threads.materialized_worker_data {
        return worker_data;
    }
    let Some(serialized) = state.node_worker_threads.worker_data.clone() else {
        return value::encode_undefined();
    };
    match structured_clone::deserialize(state, &serialized) {
        Ok(worker_data) => {
            state.node_worker_threads.materialized_worker_data = Some(worker_data);
            worker_data
        }
        Err(message) => structured_clone::data_clone_error(ctx, state, &message),
    }
}

fn poll_parent_command(ctx: &mut NativeVmContext, state: &mut NativeAgentState) -> Option<i64> {
    let command = state
        .node_worker_threads
        .parent_command_rx
        .as_ref()?
        .recv_timeout(Duration::from_millis(10))
        .ok()?;
    match command {
        WorkerCommand::Terminate => {
            if let Some(control) = &state.node_worker_threads.parent_control {
                control.terminated.store(true, Ordering::Release);
            }
            Some(value::encode_undefined())
        }
        WorkerCommand::Message(serialized) => {
            let callback = state.node_worker_threads.parent_message_callback?;
            let message = match structured_clone::deserialize(state, &serialized) {
                Ok(message) => message,
                Err(error) => return Some(structured_clone::data_clone_error(ctx, state, &error)),
            };
            Some(
                state
                    .invoke_callable(ctx, callback, value::encode_undefined(), &[message])
                    .unwrap_or_else(|| fail_dispatch(ctx)),
            )
        }
    }
}

fn poll_local_port(ctx: &mut NativeVmContext, state: &mut NativeAgentState) -> Option<i64> {
    let port_id = state
        .node_worker_threads
        .ports
        .iter()
        .find(|(_, port)| !port.closed && port.callback.is_some() && !port.inbox.is_empty())
        .map(|(id, _)| *id)?;
    let port = state.node_worker_threads.ports.get_mut(&port_id)?;
    let callback = port.callback?;
    let serialized = port.inbox.pop_front()?;
    let message = match structured_clone::deserialize(state, &serialized) {
        Ok(message) => message,
        Err(error) => return Some(structured_clone::data_clone_error(ctx, state, &error)),
    };
    Some(
        state
            .invoke_callable(ctx, callback, value::encode_undefined(), &[message])
            .unwrap_or_else(|| fail_dispatch(ctx)),
    )
}

fn dispatch_worker_event(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    event: WorkerEvent,
) -> i64 {
    let (callback, arguments, context) = match event {
        WorkerEvent::Online(worker_id) => {
            let (callback, context) = state
                .node_worker_threads
                .callbacks
                .get(&worker_id)
                .map(|callbacks| (callbacks.online, callbacks.context.clone()))
                .unwrap_or_default();
            (callback, Vec::new(), context)
        }
        WorkerEvent::Message { worker_id, value } => {
            let message = match structured_clone::deserialize(state, &value) {
                Ok(message) => message,
                Err(error) => return structured_clone::data_clone_error(ctx, state, &error),
            };
            let (callback, context) = state
                .node_worker_threads
                .callbacks
                .get(&worker_id)
                .map(|callbacks| (callbacks.message, callbacks.context.clone()))
                .unwrap_or_default();
            (callback, vec![message], context)
        }
        WorkerEvent::Error(worker_id, message) => {
            let error = modules::named_error_object(state, "Error", message)
                .unwrap_or_else(value::encode_undefined);
            let (callback, context) = state
                .node_worker_threads
                .callbacks
                .get(&worker_id)
                .map(|callbacks| (callbacks.error, callbacks.context.clone()))
                .unwrap_or_default();
            (callback, vec![error], context)
        }
        WorkerEvent::Exit(worker_id, code) => {
            if let Some(callbacks) = state.node_worker_threads.callbacks.get_mut(&worker_id) {
                callbacks.active = false;
            }
            let control = state
                .node_worker_threads
                .cluster
                .workers
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(&worker_id);
            let code = if control.is_some_and(|control| control.terminated.load(Ordering::Acquire))
            {
                1
            } else {
                code
            };
            let (callback, context) = state
                .node_worker_threads
                .callbacks
                .get(&worker_id)
                .map(|callbacks| (callbacks.exit, callbacks.context.clone()))
                .unwrap_or_default();
            (callback, vec![value::encode_f64(f64::from(code))], context)
        }
        WorkerEvent::Output(output) => {
            state.output.borrow_mut().extend_from_slice(&output);
            return value::encode_undefined();
        }
    };
    let Some(callback) = callback else {
        return value::encode_undefined();
    };
    let previous = super::node_async_hooks::enter_context(state, context);
    let result = state
        .invoke_callable(ctx, callback, value::encode_undefined(), &arguments)
        .unwrap_or_else(|| fail_dispatch(ctx));
    super::node_async_hooks::restore_context(state, previous);
    result
}

fn id_pair(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    first_name: &str,
    first: u32,
    second_name: &str,
    second: u32,
) -> i64 {
    let Ok(result) = state.allocate_object(2, false) else {
        return fail_dispatch(ctx);
    };
    if modules::set_named_property(
        state,
        result,
        first_name,
        value::encode_f64(f64::from(first)),
    )
    .is_err()
        || modules::set_named_property(
            state,
            result,
            second_name,
            value::encode_f64(f64::from(second)),
        )
        .is_err()
    {
        return fail_dispatch(ctx);
    }
    result
}

fn numeric_id(encoded: Option<i64>) -> Option<u32> {
    let encoded = encoded?;
    value::is_f64(encoded).then(|| value::decode_f64(encoded) as u32)
}

fn type_error(ctx: &mut NativeVmContext, state: &mut NativeAgentState, message: &str) -> i64 {
    modules::named_error_object(state, "TypeError", message.into())
        .and_then(|error| state.create_exception(error))
        .unwrap_or_else(|| fail_dispatch(ctx))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn worker_file_uses_cached_builtin_segment() {
        let fixture_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/modules/async_local_worker");
        let worker = fixture_root.join("worker.js");
        let artifact = compile_worker_artifact(
            worker.to_str().expect("fixture 路径必须是 UTF-8"),
            false,
            &fixture_root,
        )
        .expect("worker fixture 必须可以编译");

        assert!(
            artifact.program().split_builtin_segment().is_some(),
            "worker 文件必须走 builtin 分段缓存路径"
        );
    }
}
