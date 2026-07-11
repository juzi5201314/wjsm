//! 异步 `spawn` / `fork`：长生命周期子进程 + IPC + exit 事件。

use std::os::fd::RawFd;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use wasmtime::{Caller, Store};

use crate::runtime_encoding::js_string_lossy;
use crate::runtime_node_data::string_array_from_value;
use crate::runtime_process::ProcessNextTickTask;
use crate::scheduler::AsyncHostCompletion;
use crate::*;

#[cfg(unix)]
use super::child_message_callbacks::{
    drain_child_messages, ipc_payload_to_js, ipc_payload_to_js_caller,
};
#[cfg(unix)]
use super::ipc::{IpcEndpoint, connect_ipc_path, create_parent_ipc};
use super::spawn_sync::{
    make_child_process_error, parse_options, signal_from_status, validate_command_allowed,
};

/// 异步子进程侧表条目。
pub(crate) struct ChildProcessEntry {
    #[allow(dead_code)]
    pub pid: u32,
    /// 可 kill 的子进程句柄。
    child: Mutex<Option<std::process::Child>>,
    #[cfg(unix)]
    pub ipc: Option<super::ipc::ParentIpcHandle>,
    pub killed: AtomicBool,
    pub exited: AtomicBool,
    pub exit_status: Mutex<Option<(Option<i32>, Option<String>)>>,
}

/// 本 Store 上的回调绑定（message / exit / disconnect）。
pub(crate) struct LocalChildBinding {
    pub message_cb: Option<i64>,
    pub exit_cb: Option<i64>,
    pub disconnect_cb: Option<i64>,
    pub ref_guard: Option<crate::scheduler::AsyncOpGuard>,
    #[cfg(unix)]
    pub pending_messages: Vec<super::ipc::IpcMessage>,
    #[cfg(unix)]
    pub message_cb_ready: bool,
}

/// 本进程作为 IPC child 时的通道状态。
#[cfg(unix)]
pub(crate) struct ProcessIpcState {
    /// 延迟连接：snapshot 恢复可能重建 RuntimeState，不能在构造时 connect。
    pub path: String,
    pub endpoint: Mutex<Option<Arc<IpcEndpoint>>>,
    pub message_cb: Mutex<Option<i64>>,
    pub disconnect_cb: Mutex<Option<i64>>,
    pub connected: AtomicBool,
    /// 保持 scheduler 存活直到 disconnect。
    pub ref_guard: Mutex<Option<crate::scheduler::AsyncOpGuard>>,
}

#[cfg(unix)]
impl ProcessIpcState {
    fn ensure_endpoint(&self) -> Result<Arc<IpcEndpoint>, String> {
        let mut guard = self.endpoint.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(ep) = guard.as_ref() {
            return Ok(Arc::clone(ep));
        }
        let ep = connect_ipc_path(&self.path).map_err(|e| format!("ipc connect: {e}"))?;
        *guard = Some(Arc::clone(&ep));
        self.connected.store(true, Ordering::SeqCst);
        Ok(ep)
    }
}

#[cfg(not(unix))]
pub(crate) struct ProcessIpcState;

pub(super) fn spawn_async(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let Some(command) = args.first().copied() else {
        return make_type_error_exception(caller, "spawn command is required");
    };
    let command = js_string_lossy(caller, command);
    let spawn_args = match args.get(1).copied() {
        Some(value) => match string_array_from_value(caller, value) {
            Ok(values) => values,
            Err(error) => return error,
        },
        None => Vec::new(),
    };
    let options = match parse_options(caller, args.get(2).copied()) {
        Ok(options) => options,
        Err(error) => return error,
    };
    let allow_self = options.ipc;
    if let Err(error) = validate_command_allowed(caller, &command, options.shell, allow_self) {
        return make_child_process_error(caller, &error);
    }

    #[cfg(unix)]
    {
        spawn_unix(caller, &command, &spawn_args, &options)
    }
    #[cfg(not(unix))]
    {
        let _ = (command, spawn_args, options);
        make_child_process_error(caller, "async child_process.spawn is only supported on Unix")
    }
}

#[cfg(unix)]
fn spawn_unix(
    caller: &mut Caller<'_, RuntimeState>,
    command: &str,
    spawn_args: &[String],
    options: &super::spawn_sync::CommandOptions,
) -> i64 {
    let parent_ipc_handle = if options.ipc {
        match create_parent_ipc() {
            Ok(h) => Some(h),
            Err(err) => {
                return make_child_process_error(caller, &format!("ipc server failed: {err}"));
            }
        }
    } else {
        None
    };

    let mut cmd = if options.shell {
        let mut shell = Command::new("sh");
        shell.arg("-c").arg(command);
        shell
    } else {
        let mut direct = Command::new(command);
        direct.args(spawn_args);
        direct
    };
    if let Some(cwd) = options
        .cwd
        .as_deref()
        .or(caller.data().process.cwd.as_deref())
    {
        cmd.current_dir(cwd);
    }

    // 环境：默认继承 process.env，再叠 envPairs
    cmd.env_clear();
    for (key, value) in caller.data().process.env.iter() {
        if key == "NODE_CHANNEL_FD"
            || key == "NODE_UNIQUE_ID"
            || key == "WJSM_IPC_PATH"
        {
            continue;
        }
        cmd.env(key, value);
    }
    for (key, value) in &options.env_pairs {
        cmd.env(key, value);
    }

    if let Some(handle) = parent_ipc_handle.as_ref() {
        cmd.env("WJSM_IPC_PATH", handle.path());
        cmd.env("NODE_CHANNEL_FD", "ipc");
    }

    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::inherit());
    cmd.stderr(Stdio::inherit());

    let child = match cmd.spawn() {
        Ok(child) => child,
        Err(err) => {
            return make_child_process_error(caller, &format!("spawn failed: {err}"));
        }
    };

    let pid = child.id();
    let parent_ipc = parent_ipc_handle;

    let entry = ChildProcessEntry {
        pid,
        child: Mutex::new(Some(child)),
        #[cfg(unix)]
        ipc: parent_ipc.clone(),
        killed: AtomicBool::new(false),
        exited: AtomicBool::new(false),
        exit_status: Mutex::new(None),
    };
    let handle = caller.data().child_process_table.alloc(entry);

    // 本地绑定 + 保持 async op 直到 exit（防止 scheduler 提前退出）
    {
        let mut map = caller
            .data()
            .child_bindings
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let ref_guard = caller
            .data()
            .async_op_counter
            .as_ref()
            .map(|c| c.begin());
        map.insert(
            handle,
            LocalChildBinding {
                message_cb: None,
                exit_cb: None,
                disconnect_cb: None,
                ref_guard,
                pending_messages: Vec::new(),
                message_cb_ready: false,
            },
        );
    }

    // 尽早启动 parent IPC reader，不必等第一次 send
    if let Some(ref ipc) = parent_ipc {
        start_parent_ipc_reader(caller, handle, ipc.clone());
    }

    // wait 线程：绝不能在 child.wait() 期间持有 table.inner 锁，
    // 否则 child_send / kill / onExit 等 host 调用会与 wait 死锁。
    {
        let table = Arc::clone(&caller.data().child_process_table);
        let wake_tx = caller.data().host_completion_tx.clone();
        // 在 spawn 调用线程上取出 Child 所有权；wait 线程只持有 Child，不锁表。
        let child_for_wait = {
            let inner = table.inner.lock().unwrap_or_else(|e| e.into_inner());
            inner
                .entries
                .get(handle as usize)
                .and_then(|e| e.as_ref())
                .and_then(|e| e.child.lock().unwrap_or_else(|e| e.into_inner()).take())
        };
        thread::Builder::new()
            .name(format!("wjsm-child-wait-{pid}"))
            .spawn(move || {
                let Some(mut child) = child_for_wait else {
                    return;
                };
                let status = child.wait();
                let (code, signal) = match status {
                    Ok(st) => (st.code(), signal_from_status(&st)),
                    Err(_) => (None, None),
                };
                {
                    let mut inner = table.inner.lock().unwrap_or_else(|e| e.into_inner());
                    if let Some(Some(entry)) = inner.entries.get_mut(handle as usize) {
                        *entry.exit_status.lock().unwrap_or_else(|e| e.into_inner()) =
                            Some((code, signal.clone()));
                        entry.exited.store(true, Ordering::SeqCst);
                    }
                }
                if let Some(tx) = wake_tx {
                    let _ = tx.send(AsyncHostCompletion::HostTask {
                        run: Box::new(move |store, _env| {
                            deliver_exit(store, handle, code, signal);
                        }),
                    });
                }
            })
            .expect("spawn child wait thread");
    }

    // 返回 { id, pid }
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let obj = alloc_host_object(caller, &env, 2);
    let temp = caller.data().push_host_temp_roots([obj]);
    let _ = define_host_data_property_from_caller(
        caller,
        obj,
        "id",
        value::encode_f64(handle as f64),
    );
    let _ = define_host_data_property_from_caller(
        caller,
        obj,
        "pid",
        value::encode_f64(pid as f64),
    );
    caller.data().truncate_host_temp_roots(temp);
    obj
}

fn deliver_exit(
    store: &mut Store<RuntimeState>,
    handle: u32,
    code: Option<i32>,
    signal: Option<String>,
) {
    let (exit_cb, disconnect_cb) = {
        let mut map = store
            .data()
            .child_bindings
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        match map.get_mut(&handle) {
            Some(b) => {
                // 释放 ref_guard，允许 scheduler 在无其它 op 时退出
                b.ref_guard = None;
                (b.exit_cb, b.disconnect_cb)
            }
            None => (None, None),
        }
    };
    if let Some(cb) = disconnect_cb {
        store
            .data()
            .next_tick_queue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_back(ProcessNextTickTask {
                callback: cb,
                args: vec![],
            });
    }
    if let Some(cb) = exit_cb {
        let code_val = code
            .map(|c| value::encode_f64(c as f64))
            .unwrap_or_else(value::encode_null);
        let signal_val = match signal {
            Some(s) => store_runtime_string_in_state(store.data(), s),
            None => value::encode_null(),
        };
        store
            .data()
            .next_tick_queue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_back(ProcessNextTickTask {
                callback: cb,
                args: vec![code_val, signal_val],
            });
    }
}

pub(super) fn child_kill(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let Some(handle) = handle_arg(args.first().copied()) else {
        return make_type_error_exception(caller, "childKill: invalid id");
    };
    let signal = args
        .get(1)
        .copied()
        .map(|v| js_string_lossy(caller, v))
        .unwrap_or_else(|| "SIGTERM".to_string());
    let mut inner = caller
        .data()
        .child_process_table
        .inner
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let Some(Some(entry)) = inner.entries.get_mut(handle as usize) else {
        return value::encode_bool(false);
    };
    if entry.exited.load(Ordering::SeqCst) {
        return value::encode_bool(false);
    }
    entry.killed.store(true, Ordering::SeqCst);
    let pid = entry.pid;
    // Child 所有权可能已移交给 wait 线程；用 pid + kill(2) / 可选 Child::kill。
    let mut child_guard = entry.child.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(child) = child_guard.as_mut() {
        #[cfg(unix)]
        {
            let sig = signal_name_to_libc(&signal);
            let _ = unsafe { libc::kill(child.id() as i32, sig) };
        }
        #[cfg(not(unix))]
        {
            let _ = child.kill();
            let _ = signal;
        }
        return value::encode_bool(true);
    }
    drop(child_guard);
    #[cfg(unix)]
    {
        let sig = signal_name_to_libc(&signal);
        let ok = unsafe { libc::kill(pid as i32, sig) } == 0;
        value::encode_bool(ok)
    }
    #[cfg(not(unix))]
    {
        let _ = (pid, signal);
        value::encode_bool(false)
    }
}

#[cfg(unix)]
fn signal_name_to_libc(name: &str) -> i32 {
    match name {
        "SIGKILL" | "KILL" | "9" => libc::SIGKILL,
        "SIGTERM" | "TERM" | "15" => libc::SIGTERM,
        "SIGINT" | "INT" | "2" => libc::SIGINT,
        "SIGHUP" | "HUP" | "1" => libc::SIGHUP,
        _ => libc::SIGTERM,
    }
}

pub(super) fn child_send(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let Some(handle) = handle_arg(args.first().copied()) else {
        return make_type_error_exception(caller, "childSend: invalid id");
    };
    let raw = args
        .get(1)
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let payload = match js_value_to_json_text(caller, raw) {
        Ok(s) => s,
        Err(err) => return make_child_process_error(caller, &err),
    };
    let send_fd = args.get(2).copied().and_then(|v| {
        if value::is_f64(v) {
            Some(value::decode_f64(v) as RawFd)
        } else {
            None
        }
    });

    let handle_ipc = {
        let inner = caller
            .data()
            .child_process_table
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        inner
            .entries
            .get(handle as usize)
            .and_then(|e| e.as_ref())
            .and_then(|e| e.ipc.clone())
    };
    let Some(handle_ipc) = handle_ipc else {
        return make_child_process_error(caller, "child has no IPC channel");
    };
    // 非阻塞 send；若 accept 已完成则立即写，否则排队
    if let Err(err) = handle_ipc.send_nonblocking(payload, send_fd) {
        return make_child_process_error(caller, &format!("childSend failed: {err}"));
    }
    start_parent_ipc_reader(caller, handle, handle_ipc);
    value::encode_bool(true)
}

/// 后台 wait accept 并启动 reader（幂等；ensure_reader 内部有 once 守卫）。
#[cfg(unix)]
fn start_parent_ipc_reader(
    caller: &Caller<'_, RuntimeState>,
    handle: u32,
    handle_ipc: super::ipc::ParentIpcHandle,
) {
    let Some(tx) = caller.data().host_completion_tx.clone() else {
        return;
    };
    thread::spawn(move || {
        if let Ok(endpoint) = handle_ipc.wait_endpoint() {
            endpoint.set_wake_tx(Some(tx));
            let make_wake = Arc::new(move || {
                let id = handle;
                Box::new(move |store: &mut Store<RuntimeState>, env: &WasmEnv| {
                    drain_child_messages(store, env, id);
                })
                    as Box<dyn FnOnce(&mut Store<RuntimeState>, &WasmEnv) + Send>
            });
            endpoint.ensure_reader(make_wake);
        }
    });
}

#[cfg(not(unix))]
fn start_parent_ipc_reader(
    _caller: &Caller<'_, RuntimeState>,
    _handle: u32,
    _handle_ipc: (),
) {
}

pub(super) fn child_disconnect(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let Some(handle) = handle_arg(args.first().copied()) else {
        return make_type_error_exception(caller, "childDisconnect: invalid id");
    };
    let ipc = {
        let inner = caller
            .data()
            .child_process_table
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        inner
            .entries
            .get(handle as usize)
            .and_then(|e| e.as_ref())
            .and_then(|e| e.ipc.clone())
    };
    // 不阻塞 host：仅关闭已就绪的 endpoint；未 accept 的由 drop 清理
    if let Some(ipc) = ipc
        && let Some(ep) = ipc.try_endpoint() {
            ep.close();
        }
    value::encode_undefined()
}

#[cfg(not(unix))]
pub(super) fn child_on_message(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let Some(handle) = handle_arg(args.first().copied()) else {
        return make_type_error_exception(caller, "childOnMessage: invalid id");
    };
    let callback = args
        .get(1)
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let mut bindings = caller
        .data()
        .child_bindings
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(binding) = bindings.get_mut(&handle) {
        binding.message_cb = Some(callback);
    }
    value::encode_undefined()
}

pub(super) fn child_on_exit(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let Some(handle) = handle_arg(args.first().copied()) else {
        return make_type_error_exception(caller, "childOnExit: invalid id");
    };
    let cb = args
        .get(1)
        .copied()
        .unwrap_or_else(value::encode_undefined);
    {
        let mut map = caller
            .data()
            .child_bindings
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(b) = map.get_mut(&handle) {
            b.exit_cb = Some(cb);
        }
    }
    let already = {
        let inner = caller
            .data()
            .child_process_table
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        inner
            .entries
            .get(handle as usize)
            .and_then(|e| e.as_ref())
            .and_then(|e| {
                if e.exited.load(Ordering::SeqCst) {
                    e.exit_status
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .clone()
                } else {
                    None
                }
            })
    };
    if let Some((code, signal)) = already {
        let code_val = code
            .map(|c| value::encode_f64(c as f64))
            .unwrap_or_else(value::encode_null);
        let signal_val = match signal {
            Some(s) => store_runtime_string(caller, s),
            None => value::encode_null(),
        };
        caller
            .data()
            .next_tick_queue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_back(ProcessNextTickTask {
                callback: cb,
                args: vec![code_val, signal_val],
            });
    }
    value::encode_undefined()
}


pub(super) fn handle_arg(raw: Option<i64>) -> Option<u32> {
    let raw = raw?;
    if !value::is_f64(raw) {
        return None;
    }
    let n = value::decode_f64(raw);
    if n.is_finite() && n >= 0.0 && n <= u32::MAX as f64 {
        Some(n as u32)
    } else {
        None
    }
}

// ── process 侧 IPC（worker/cluster child 进程）──────────────────────────

pub(crate) fn try_init_process_ipc_from_env(state: &mut RuntimeState) {
    #[cfg(unix)]
    {
        let path = state
            .process
            .env
            .iter()
            .find(|(k, _)| k == "WJSM_IPC_PATH")
            .map(|(_, v)| v.clone());
        // 从 env 快照中移除，避免再 fork 时错误继承
        let new_env: Vec<_> = state
            .process
            .env
            .iter()
            .filter(|(k, _)| k != "NODE_CHANNEL_FD" && k != "WJSM_IPC_PATH")
            .cloned()
            .collect();
        state.process.env = Arc::from(new_env);

        let Some(path) = path else {
            return;
        };
        // 仅记录 path，真正 connect 延迟到 process.send / process.on('message')，
        // 避免 startup snapshot 重建 RuntimeState 时 drop 掉已建立的连接。
        state.process_ipc = Some(ProcessIpcState {
            path,
            endpoint: Mutex::new(None),
            message_cb: Mutex::new(None),
            disconnect_cb: Mutex::new(None),
            connected: AtomicBool::new(true),
            ref_guard: Mutex::new(None),
        });
    }
    #[cfg(not(unix))]
    {
        let _ = state;
    }
}

pub(super) fn process_send(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let Some(raw) = args.first().copied() else {
        return make_type_error_exception(caller, "process.send requires a message");
    };
    let payload = match js_value_to_json_text(caller, raw) {
        Ok(s) => s,
        Err(err) => return make_child_process_error(caller, &err),
    };
    let send_fd = args.get(1).copied().and_then(|v| {
        if value::is_f64(v) {
            Some(value::decode_f64(v) as RawFd)
        } else {
            None
        }
    });
    let Some(ipc) = caller.data().process_ipc.as_ref() else {
        return make_child_process_error(caller, "process.send: no IPC channel");
    };
    let endpoint = match ipc.ensure_endpoint() {
        Ok(ep) => ep,
        Err(err) => return make_child_process_error(caller, &err),
    };
    // cluster 等在模块加载期 process.on('message') 时 host_completion_tx 可能尚未就绪，
    // reader 未启动。在 send 路径补启动（幂等），确保能收到 primary 回包。
    ensure_process_ipc_reader(caller, &endpoint);
    match endpoint.send(&payload, send_fd) {
        Ok(()) => value::encode_bool(true),
        Err(err) => make_child_process_error(caller, &format!("process.send failed: {err}")),
    }
}

/// 启动 process IPC reader（幂等）。tx 未就绪时静默跳过。
fn ensure_process_ipc_reader(
    caller: &Caller<'_, RuntimeState>,
    endpoint: &std::sync::Arc<super::ipc::IpcEndpoint>,
) {
    let Some(tx) = caller.data().host_completion_tx.clone() else {
        return;
    };
    endpoint.set_wake_tx(Some(tx));
    let make_wake = Arc::new(move || {
        Box::new(move |store: &mut Store<RuntimeState>, env: &WasmEnv| {
            drain_process_messages(store, env);
        }) as Box<dyn FnOnce(&mut Store<RuntimeState>, &WasmEnv) + Send>
    });
    endpoint.ensure_reader(make_wake);
}

/// 将 JS 值编码为 IPC JSON 文本（string 原样；对象/数组/原始类型走 JSON）。
fn js_value_to_json_text(
    caller: &mut Caller<'_, RuntimeState>,
    raw: i64,
) -> Result<String, String> {
    if value::is_string(raw) {
        // 已是 JSON 文本则原样；否则包一层 JSON string
        let s = js_string_lossy(caller, raw);
        if s.starts_with('{') || s.starts_with('[') || s == "null" || s == "true" || s == "false" {
            return Ok(s);
        }
        return serde_json::to_string(&s).map_err(|e| e.to_string());
    }
    let v = js_to_serde_json(caller, raw, 0)?;
    serde_json::to_string(&v).map_err(|e| e.to_string())
}

fn js_to_serde_json(
    caller: &mut Caller<'_, RuntimeState>,
    raw: i64,
    depth: usize,
) -> Result<serde_json::Value, String> {
    use crate::runtime_values::{read_array_elem, read_array_length, read_object_property_by_name};
    if depth > 32 {
        return Err("json depth exceeded".into());
    }
    if value::is_undefined(raw) || value::is_null(raw) {
        return Ok(serde_json::Value::Null);
    }
    if value::is_bool(raw) {
        return Ok(serde_json::Value::Bool(value::decode_bool(raw)));
    }
    if value::is_f64(raw) {
        let n = value::decode_f64(raw);
        return Ok(serde_json::Number::from_f64(n)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null));
    }
    if value::is_string(raw) {
        return Ok(serde_json::Value::String(js_string_lossy(caller, raw)));
    }
    if value::is_array(raw) {
        let Some(ptr) = resolve_handle(caller, raw) else {
            return Ok(serde_json::Value::Array(vec![]));
        };
        let len = read_array_length(caller, ptr).unwrap_or(0);
        let mut items = Vec::with_capacity(len as usize);
        for i in 0..len {
            let elem = read_array_elem(caller, ptr, i).unwrap_or_else(value::encode_undefined);
            items.push(js_to_serde_json(caller, elem, depth + 1)?);
        }
        return Ok(serde_json::Value::Array(items));
    }
    if value::is_object(raw) {
        let Some(ptr) = resolve_handle(caller, raw) else {
            return Ok(serde_json::json!({}));
        };
        let keys = crate::runtime_values::enumerate_object_keys(caller, raw);
        let mut map = serde_json::Map::new();
        for key in keys {
            if let Some(prop) = read_object_property_by_name(caller, ptr, &key) {
                map.insert(key, js_to_serde_json(caller, prop, depth + 1)?);
            }
        }
        return Ok(serde_json::Value::Object(map));
    }
    Ok(serde_json::Value::String(
        crate::render_value(caller, raw).unwrap_or_default(),
    ))
}

pub(super) fn process_disconnect(caller: &mut Caller<'_, RuntimeState>, _args: &[i64]) -> i64 {
    if let Some(ipc) = caller.data().process_ipc.as_ref() {
        ipc.connected.store(false, Ordering::SeqCst);
        if let Some(ep) = ipc.endpoint.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            ep.close();
        }
        *ipc.ref_guard.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
    value::encode_undefined()
}

pub(super) fn process_on_message(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let cb = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let Some(ipc) = caller.data().process_ipc.as_ref() else {
        return value::encode_undefined();
    };
    *ipc.message_cb.lock().unwrap_or_else(|e| e.into_inner()) = Some(cb);
    // 持有 async op，避免 main 返回后 scheduler 立即退出
    if ipc
        .ref_guard
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .is_none()
        && let Some(counter) = caller.data().async_op_counter.clone() {
            *ipc.ref_guard.lock().unwrap_or_else(|e| e.into_inner()) = Some(counter.begin());
        }
    let endpoint = match ipc.ensure_endpoint() {
        Ok(ep) => ep,
        Err(_) => return value::encode_undefined(),
    };
    ensure_process_ipc_reader(caller, &endpoint);
    drain_process_messages_from_caller(caller);
    value::encode_undefined()
}

pub(super) fn process_connected(caller: &mut Caller<'_, RuntimeState>, _args: &[i64]) -> i64 {
    let connected = caller
        .data()
        .process_ipc
        .as_ref()
        .map(|i| {
            i.connected.load(Ordering::SeqCst)
                && i.endpoint
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .as_ref()
                    .is_some_and(|ep| !ep.is_closed())
        })
        .unwrap_or(false);
    // 有 path 即视为 connected（延迟 connect）
    let has_ipc = caller.data().process_ipc.is_some();
    value::encode_bool(connected || has_ipc)
}

fn drain_process_messages_from_caller(caller: &mut Caller<'_, RuntimeState>) {
    let Some(ipc) = caller.data().process_ipc.as_ref() else {
        return;
    };
    let messages = ipc
        .endpoint
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map(|ep| ep.drain_inbox())
        .unwrap_or_default();
    let cb = *ipc.message_cb.lock().unwrap_or_else(|e| e.into_inner());
    let Some(cb) = cb else {
        return;
    };
    for msg in messages {
        let payload = ipc_payload_to_js_caller(caller, &msg.payload);
        let fd_val = msg
            .fd
            .map(|fd| value::encode_f64(fd as f64))
            .unwrap_or_else(value::encode_undefined);
        caller
            .data()
            .next_tick_queue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_back(ProcessNextTickTask {
                callback: cb,
                args: vec![payload, fd_val],
            });
    }
}

pub(crate) fn drain_process_messages(store: &mut Store<RuntimeState>, env: &WasmEnv) {
    let Some(ipc) = store.data().process_ipc.as_ref() else {
        return;
    };
    let (messages, closed) = {
        let guard = ipc.endpoint.lock().unwrap_or_else(|e| e.into_inner());
        match guard.as_ref() {
            Some(ep) => (ep.drain_inbox(), ep.is_closed()),
            None => (Vec::new(), false),
        }
    };
    let cb = *ipc.message_cb.lock().unwrap_or_else(|e| e.into_inner());
    let disconnect_cb = *ipc.disconnect_cb.lock().unwrap_or_else(|e| e.into_inner());
    if closed {
        *ipc.ref_guard.lock().unwrap_or_else(|e| e.into_inner()) = None;
        ipc.connected.store(false, Ordering::SeqCst);
        if let Some(dcb) = disconnect_cb {
            store
                .data()
                .next_tick_queue
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push_back(ProcessNextTickTask {
                    callback: dcb,
                    args: vec![],
                });
        }
    }
    let Some(cb) = cb else {
        return;
    };
    for msg in messages {
        let payload = ipc_payload_to_js(store, env, &msg.payload);
        let fd_val = msg
            .fd
            .map(|fd| value::encode_f64(fd as f64))
            .unwrap_or_else(value::encode_undefined);
        store
            .data()
            .next_tick_queue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_back(ProcessNextTickTask {
                callback: cb,
                args: vec![payload, fd_val],
            });
    }
}


