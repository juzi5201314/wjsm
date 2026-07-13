//! Node.js `child_process` host：同步 spawnSync/execSync + 异步 spawn/fork + IPC。

#[cfg(unix)]
mod child_message_callbacks;
#[cfg(unix)]
mod ipc;
mod spawn_async;
mod spawn_sync;

pub(crate) use spawn_async::{
    ChildProcessEntry, LocalChildBinding, ProcessIpcState, kill_all_child_processes,
    try_init_process_ipc_from_env,
};

use wasmtime::Caller;

use crate::*;

#[cfg(unix)]
use child_message_callbacks::child_on_message;
#[cfg(not(unix))]
use spawn_async::child_on_message;
use spawn_async::{
    child_disconnect, child_kill, child_on_exit, child_send, process_connected, process_disconnect,
    process_on_message, process_send, spawn_async,
};
use spawn_sync::{exec_sync, spawn_sync};

/// host 方法判别式（可进 startup snapshot）。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub(crate) enum ChildProcessMethodKind {
    SpawnSync = 0,
    ExecSync = 1,
    Spawn = 2,
    Kill = 3,
    Send = 4,
    Disconnect = 5,
    OnMessage = 6,
    OnExit = 7,
    ProcessSend = 8,
    ProcessDisconnect = 9,
    ProcessOnMessage = 10,
    ProcessConnected = 11,
}

impl ChildProcessMethodKind {
    pub(crate) fn method(self) -> u8 {
        self as u8
    }

    pub(crate) fn from_method(method: u8) -> Option<Self> {
        match method {
            0 => Some(Self::SpawnSync),
            1 => Some(Self::ExecSync),
            2 => Some(Self::Spawn),
            3 => Some(Self::Kill),
            4 => Some(Self::Send),
            5 => Some(Self::Disconnect),
            6 => Some(Self::OnMessage),
            7 => Some(Self::OnExit),
            8 => Some(Self::ProcessSend),
            9 => Some(Self::ProcessDisconnect),
            10 => Some(Self::ProcessOnMessage),
            11 => Some(Self::ProcessConnected),
            _ => None,
        }
    }
}

pub(crate) fn create_child_process_host_object(caller: &mut Caller<'_, RuntimeState>) -> i64 {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let obj = alloc_host_object(caller, &env, 12);
    let temp_root_len = caller.data().push_host_temp_roots([obj]);
    install(caller, obj, "spawnSync", ChildProcessMethodKind::SpawnSync);
    install(caller, obj, "execSync", ChildProcessMethodKind::ExecSync);
    install(caller, obj, "spawn", ChildProcessMethodKind::Spawn);
    install(caller, obj, "kill", ChildProcessMethodKind::Kill);
    install(caller, obj, "send", ChildProcessMethodKind::Send);
    install(
        caller,
        obj,
        "disconnect",
        ChildProcessMethodKind::Disconnect,
    );
    install(caller, obj, "onMessage", ChildProcessMethodKind::OnMessage);
    install(caller, obj, "onExit", ChildProcessMethodKind::OnExit);
    install(
        caller,
        obj,
        "processSend",
        ChildProcessMethodKind::ProcessSend,
    );
    install(
        caller,
        obj,
        "processDisconnect",
        ChildProcessMethodKind::ProcessDisconnect,
    );
    install(
        caller,
        obj,
        "processOnMessage",
        ChildProcessMethodKind::ProcessOnMessage,
    );
    install(
        caller,
        obj,
        "processConnected",
        ChildProcessMethodKind::ProcessConnected,
    );
    caller.data().truncate_host_temp_roots(temp_root_len);
    obj
}

fn install(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
    name: &str,
    kind: ChildProcessMethodKind,
) {
    let callable =
        create_native_callable(caller.data(), NativeCallable::ChildProcessMethod { kind });
    let _ = define_host_data_property_from_caller(caller, obj, name, callable);
}

pub(crate) fn call_child_process_method(
    caller: &mut Caller<'_, RuntimeState>,
    kind: ChildProcessMethodKind,
    args: &[i64],
) -> i64 {
    match kind {
        ChildProcessMethodKind::SpawnSync => spawn_sync(caller, args),
        ChildProcessMethodKind::ExecSync => exec_sync(caller, args),
        ChildProcessMethodKind::Spawn => spawn_async(caller, args),
        ChildProcessMethodKind::Kill => child_kill(caller, args),
        ChildProcessMethodKind::Send => child_send(caller, args),
        ChildProcessMethodKind::Disconnect => child_disconnect(caller, args),
        ChildProcessMethodKind::OnMessage => child_on_message(caller, args),
        ChildProcessMethodKind::OnExit => child_on_exit(caller, args),
        ChildProcessMethodKind::ProcessSend => process_send(caller, args),
        ChildProcessMethodKind::ProcessDisconnect => process_disconnect(caller, args),
        ChildProcessMethodKind::ProcessOnMessage => process_on_message(caller, args),
        ChildProcessMethodKind::ProcessConnected => process_connected(caller, args),
    }
}
