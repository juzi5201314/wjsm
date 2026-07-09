//! Node.js `worker_threads` host：MessagePort registry + Worker OS 线程脚手架。
//!
//! 跨 agent 消息经 `SerializedValue` + `AsyncHostCompletion::HostTask` 投递；
//! JS 回调仅在目标 Store owner 上经 `next_tick_queue` 调用。

mod cluster;
mod port;
mod worker;

pub(crate) use cluster::{LocalPortBinding, LocalWorkerBinding, WorkerClusterState};
pub(crate) use port::auto_ref_port_on_store;
pub(crate) use worker::register_worker_port_wake;

use wasmtime::Caller;

use crate::*;

use port::{
    create_message_channel, port_close, port_post_message, port_ref, port_start, port_unref,
    receive_message_on_port,
};
use worker::{
    create_worker, get_is_main_thread, get_parent_port_id, get_thread_id, get_worker_data,
    worker_on_lifecycle, worker_post_message, worker_ref, worker_terminate, worker_unref,
};

/// `worker_threads` host 方法判别式（无状态，可进 startup snapshot）。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub(crate) enum WorkerThreadsMethodKind {
    CreateMessageChannel = 0,
    PortPostMessage = 1,
    PortStart = 2,
    PortClose = 3,
    PortRef = 4,
    PortUnref = 5,
    ReceiveMessageOnPort = 6,
    CreateWorker = 7,
    WorkerPostMessage = 8,
    WorkerTerminate = 9,
    WorkerRef = 10,
    WorkerUnref = 11,
    WorkerOnLifecycle = 12,
    GetIsMainThread = 13,
    GetThreadId = 14,
    GetWorkerData = 15,
    GetParentPortId = 16,
}

impl WorkerThreadsMethodKind {
    pub(crate) fn method(self) -> u8 {
        self as u8
    }

    pub(crate) fn from_method(method: u8) -> Option<Self> {
        match method {
            0 => Some(Self::CreateMessageChannel),
            1 => Some(Self::PortPostMessage),
            2 => Some(Self::PortStart),
            3 => Some(Self::PortClose),
            4 => Some(Self::PortRef),
            5 => Some(Self::PortUnref),
            6 => Some(Self::ReceiveMessageOnPort),
            7 => Some(Self::CreateWorker),
            8 => Some(Self::WorkerPostMessage),
            9 => Some(Self::WorkerTerminate),
            10 => Some(Self::WorkerRef),
            11 => Some(Self::WorkerUnref),
            12 => Some(Self::WorkerOnLifecycle),
            13 => Some(Self::GetIsMainThread),
            14 => Some(Self::GetThreadId),
            15 => Some(Self::GetWorkerData),
            16 => Some(Self::GetParentPortId),
            _ => None,
        }
    }
}

/// 安装 `__wjsm_node_worker_threads` host 对象上的方法。
pub(crate) fn create_worker_threads_host_object(caller: &mut Caller<'_, RuntimeState>) -> i64 {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let obj = alloc_host_object(caller, &env, 17);
    let temp_root_len = caller.data().push_host_temp_roots([obj]);
    install_all_methods(caller, obj);
    caller.data().truncate_host_temp_roots(temp_root_len);
    obj
}

fn install_all_methods(caller: &mut Caller<'_, RuntimeState>, obj: i64) {
    use WorkerThreadsMethodKind as K;
    install(caller, obj, "createMessageChannel", K::CreateMessageChannel);
    install(caller, obj, "portPostMessage", K::PortPostMessage);
    install(caller, obj, "portStart", K::PortStart);
    install(caller, obj, "portClose", K::PortClose);
    install(caller, obj, "portRef", K::PortRef);
    install(caller, obj, "portUnref", K::PortUnref);
    install(caller, obj, "receiveMessageOnPort", K::ReceiveMessageOnPort);
    install(caller, obj, "createWorker", K::CreateWorker);
    install(caller, obj, "workerPostMessage", K::WorkerPostMessage);
    install(caller, obj, "workerTerminate", K::WorkerTerminate);
    install(caller, obj, "workerRef", K::WorkerRef);
    install(caller, obj, "workerUnref", K::WorkerUnref);
    install(caller, obj, "workerOnLifecycle", K::WorkerOnLifecycle);
    install(caller, obj, "getIsMainThread", K::GetIsMainThread);
    install(caller, obj, "getThreadId", K::GetThreadId);
    install(caller, obj, "getWorkerData", K::GetWorkerData);
    install(caller, obj, "getParentPortId", K::GetParentPortId);
}

fn install(caller: &mut Caller<'_, RuntimeState>, obj: i64, name: &str, kind: WorkerThreadsMethodKind) {
    let callable =
        create_native_callable(caller.data(), NativeCallable::WorkerThreadsMethod { kind });
    let _ = define_host_data_property_from_caller(caller, obj, name, callable);
}

pub(crate) fn call_worker_threads_method(
    caller: &mut Caller<'_, RuntimeState>,
    kind: WorkerThreadsMethodKind,
    args: &[i64],
) -> i64 {
    match kind {
        WorkerThreadsMethodKind::CreateMessageChannel => create_message_channel(caller),
        WorkerThreadsMethodKind::PortPostMessage => port_post_message(caller, args),
        WorkerThreadsMethodKind::PortStart => port_start(caller, args),
        WorkerThreadsMethodKind::PortClose => port_close(caller, args),
        WorkerThreadsMethodKind::PortRef => port_ref(caller, args),
        WorkerThreadsMethodKind::PortUnref => port_unref(caller, args),
        WorkerThreadsMethodKind::ReceiveMessageOnPort => receive_message_on_port(caller, args),
        WorkerThreadsMethodKind::CreateWorker => create_worker(caller, args),
        WorkerThreadsMethodKind::WorkerPostMessage => worker_post_message(caller, args),
        WorkerThreadsMethodKind::WorkerTerminate => worker_terminate(caller, args),
        WorkerThreadsMethodKind::WorkerRef => worker_ref(caller, args),
        WorkerThreadsMethodKind::WorkerUnref => worker_unref(caller, args),
        WorkerThreadsMethodKind::WorkerOnLifecycle => worker_on_lifecycle(caller, args),
        WorkerThreadsMethodKind::GetIsMainThread => get_is_main_thread(caller),
        WorkerThreadsMethodKind::GetThreadId => get_thread_id(caller),
        WorkerThreadsMethodKind::GetWorkerData => get_worker_data(caller),
        WorkerThreadsMethodKind::GetParentPortId => get_parent_port_id(caller),
    }
}
