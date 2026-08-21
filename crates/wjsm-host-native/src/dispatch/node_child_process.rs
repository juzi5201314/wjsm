#[cfg(unix)]
mod ipc;

use std::process::{Child, Command, Stdio};
use std::time::Duration;

use wjsm_ir::{Builtin, value};
use wjsm_native_abi::NativeVmContext;

use super::{json, modules, node_async_hooks, runtime};
use crate::{NativeAgentState, NativeCallableKind};

#[cfg(unix)]
use std::os::fd::{AsRawFd, RawFd};
#[cfg(not(unix))]
type RawFd = i32;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum NodeChildProcessCallable {
    Spawn,
    Kill,
    Send,
    Disconnect,
    OnMessage,
    OnExit,
    ProcessSend,
    ProcessDisconnect,
    ProcessConnected,
}

#[derive(Clone)]
struct RegisteredCallback {
    callable: i64,
    context: node_async_hooks::AsyncContextSnapshot,
}

struct ChildProcessEntry {
    child: Child,
    #[cfg(unix)]
    ipc: Option<ipc::ParentIpcHandle>,
    message: Option<RegisteredCallback>,
    exit: Option<RegisteredCallback>,
    exit_delivered: bool,
}

#[cfg(unix)]
struct ProcessIpc {
    path: String,
    endpoint: Option<std::sync::Arc<ipc::IpcEndpoint>>,
    message: Option<RegisteredCallback>,
}

pub(crate) struct NodeChildProcessState {
    bridge: Option<i64>,
    children: Vec<Option<ChildProcessEntry>>,
    #[cfg(unix)]
    process: Option<ProcessIpc>,
}

impl Default for NodeChildProcessState {
    fn default() -> Self {
        Self {
            bridge: None,
            children: Vec::new(),
            #[cfg(unix)]
            process: std::env::var("WJSM_IPC_PATH").ok().map(|path| ProcessIpc {
                path,
                endpoint: None,
                message: None,
            }),
        }
    }
}

impl NodeChildProcessState {
    pub(crate) fn reset_agent(&mut self) {
        self.bridge = None;
        shutdown_entries(&mut self.children);
        self.children.clear();
        #[cfg(unix)]
        if let Some(process) = self.process.as_mut() {
            process.message = None;
        }
    }

    pub(crate) fn has_pending(&self) -> bool {
        let child_pending = self
            .children
            .iter()
            .flatten()
            .any(|entry| !entry.exit_delivered);
        #[cfg(unix)]
        {
            child_pending
                || self.process.as_ref().is_some_and(|process| {
                    process.message.is_some()
                        && process
                            .endpoint
                            .as_ref()
                            .is_none_or(|endpoint| !endpoint.is_closed())
                })
        }
        #[cfg(not(unix))]
        {
            child_pending
        }
    }

    pub(crate) fn process_connected(&self) -> bool {
        #[cfg(unix)]
        {
            self.process.as_ref().is_some_and(|process| {
                process
                    .endpoint
                    .as_ref()
                    .is_none_or(|endpoint| !endpoint.is_closed())
            })
        }
        #[cfg(not(unix))]
        {
            false
        }
    }
}

pub(crate) fn ensure_bridge(state: &mut NativeAgentState) -> Option<i64> {
    if let Some(bridge) = state.node_child_process.bridge {
        return Some(bridge);
    }
    let methods = [
        ("spawn", NodeChildProcessCallable::Spawn),
        ("kill", NodeChildProcessCallable::Kill),
        ("send", NodeChildProcessCallable::Send),
        ("disconnect", NodeChildProcessCallable::Disconnect),
        ("onMessage", NodeChildProcessCallable::OnMessage),
        ("onExit", NodeChildProcessCallable::OnExit),
        ("processSend", NodeChildProcessCallable::ProcessSend),
        (
            "processDisconnect",
            NodeChildProcessCallable::ProcessDisconnect,
        ),
        (
            "processConnected",
            NodeChildProcessCallable::ProcessConnected,
        ),
    ];
    let bridge = state.allocate_object(methods.len() as u32, false).ok()?;
    for (name, method) in methods {
        let callable = state.native_callable(NativeCallableKind::NodeChildProcess(method))?;
        modules::set_named_property(state, bridge, name, callable).ok()?;
    }
    state.node_child_process.bridge = Some(bridge);
    Some(bridge)
}

pub(crate) fn call(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    callable: NodeChildProcessCallable,
    args: &[i64],
) -> i64 {
    match callable {
        NodeChildProcessCallable::Spawn => spawn(ctx, state, args),
        NodeChildProcessCallable::Kill => kill(ctx, state, args),
        NodeChildProcessCallable::Send => send(ctx, state, args),
        NodeChildProcessCallable::Disconnect => disconnect(ctx, state, args),
        NodeChildProcessCallable::OnMessage => register_callback(ctx, state, args, false),
        NodeChildProcessCallable::OnExit => register_callback(ctx, state, args, true),
        NodeChildProcessCallable::ProcessSend => process_send(ctx, state, args),
        NodeChildProcessCallable::ProcessDisconnect => process_disconnect(state),
        NodeChildProcessCallable::ProcessConnected => {
            value::encode_bool(state.node_child_process.process_connected())
        }
    }
}

pub(crate) fn process_on(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    this_value: i64,
    args: &[i64],
) -> i64 {
    let event = args
        .first()
        .and_then(|event| state.string_owned(*event)).and_then(|text| text.to_utf8())
        .unwrap_or_default();
    if event == "message" {
        let callback = args.get(1).copied().unwrap_or_else(value::encode_undefined);
        if !value::is_callable(callback) {
            return type_error(ctx, state, "process.on requires a callable listener");
        }
        #[cfg(unix)]
        {
            let context = node_async_hooks::capture_context(state);
            let Some(process) = state.node_child_process.process.as_mut() else {
                return this_value;
            };
            process.message = Some(RegisteredCallback {
                callable: callback,
                context,
            });
            if process.endpoint.is_none() {
                match ipc::connect(&process.path) {
                    Ok(endpoint) => process.endpoint = Some(endpoint),
                    Err(error) => {
                        return error_object(ctx, state, &format!("IPC connect failed: {error}"));
                    }
                }
            }
        }
    }
    this_value
}

pub(crate) fn poll(ctx: &mut NativeVmContext, state: &mut NativeAgentState) -> i64 {
    if let Some(event) = next_process_message(state) {
        return deliver_message(ctx, state, event);
    }
    if let Some(event) = next_child_message(state) {
        return deliver_message(ctx, state, event);
    }
    if let Some(event) = next_child_exit(state) {
        return deliver_exit(ctx, state, event);
    }
    if state.node_child_process.has_pending() {
        std::thread::sleep(Duration::from_millis(1));
    }
    value::encode_undefined()
}

pub(crate) fn shutdown(state: &mut NativeAgentState) {
    shutdown_entries(&mut state.node_child_process.children);
}

fn shutdown_entries(entries: &mut [Option<ChildProcessEntry>]) {
    for entry in entries.iter_mut().flatten() {
        if !entry.exit_delivered {
            let _ = entry.child.kill();
            let _ = entry.child.wait();
            entry.exit_delivered = true;
        }
        #[cfg(unix)]
        if let Some(endpoint) = entry.ipc.as_ref().and_then(ipc::ParentIpcHandle::endpoint) {
            endpoint.close();
        }
    }
}

fn spawn(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(command) = args
        .first()
        .and_then(|command| state.string_owned(*command)).and_then(|text| text.to_utf8())
    else {
        return type_error(ctx, state, "spawn command must be a string");
    };
    let spawn_args = match args.get(1).copied() {
        Some(arguments) => match string_array(ctx, state, arguments) {
            Ok(arguments) => arguments,
            Err(exception) => return exception,
        },
        None => Vec::new(),
    };
    let options = args.get(2).copied().unwrap_or_else(value::encode_undefined);
    let shell = modules::named_property(state, options, "shell")
        .is_some_and(|value| runtime::is_truthy(state, value));
    let ipc_enabled = modules::named_property(state, options, "ipc")
        .is_some_and(|value| runtime::is_truthy(state, value));
    if let Err(message) = validate_command(state, &command, shell, ipc_enabled) {
        return error_object(ctx, state, &message);
    }
    let cwd = modules::named_property(state, options, "cwd")
        .filter(|value| !value::is_undefined(*value))
        .and_then(|value| state.string_owned(value)).and_then(|text| text.to_utf8());
    let env_pairs = match modules::named_property(state, options, "envPairs") {
        Some(pairs) => match string_array(ctx, state, pairs) {
            Ok(pairs) => pairs,
            Err(exception) => return exception,
        },
        None => Vec::new(),
    };
    #[cfg(unix)]
    let parent_ipc = if ipc_enabled {
        match ipc::create_parent() {
            Ok(ipc) => Some(ipc),
            Err(error) => return error_object(ctx, state, &format!("IPC server failed: {error}")),
        }
    } else {
        None
    };
    #[cfg(not(unix))]
    if ipc_enabled {
        return error_object(ctx, state, "child process IPC is only supported on Unix");
    }
    let mut process = if shell {
        let mut shell = Command::new("sh");
        shell.arg("-c").arg(&command);
        shell
    } else {
        let mut process = Command::new(&command);
        process.args(&spawn_args);
        process
    };
    process
        .current_dir(cwd.unwrap_or_else(|| state.working_directory.to_string_lossy().into_owned()));
    process.env_clear();
    for (key, value) in &state.environment {
        if !matches!(
            key.as_str(),
            "WJSM_IPC_PATH" | "NODE_CHANNEL_FD" | "NODE_UNIQUE_ID"
        ) {
            process.env(key, value);
        }
    }
    for pair in env_pairs {
        if let Some((key, value)) = pair.split_once('=') {
            process.env(key, value);
        }
    }
    #[cfg(unix)]
    if let Some(parent_ipc) = &parent_ipc {
        process.env("WJSM_IPC_PATH", parent_ipc.path());
        process.env("NODE_CHANNEL_FD", "ipc");
    }
    process
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    let child = match process.spawn() {
        Ok(child) => child,
        Err(error) => return error_object(ctx, state, &format!("spawn failed: {error}")),
    };
    let pid = child.id();
    let id = state
        .node_child_process
        .children
        .iter()
        .position(Option::is_none)
        .unwrap_or(state.node_child_process.children.len());
    let Ok(id_u32) = u32::try_from(id) else {
        return error_object(ctx, state, "child process table is full");
    };
    let entry = ChildProcessEntry {
        child,
        #[cfg(unix)]
        ipc: parent_ipc,
        message: None,
        exit: None,
        exit_delivered: false,
    };
    if id == state.node_child_process.children.len() {
        state.node_child_process.children.push(Some(entry));
    } else {
        state.node_child_process.children[id] = Some(entry);
    }
    id_pair(ctx, state, id_u32, pid)
}

fn kill(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(id) = numeric_id(args.first().copied()) else {
        return type_error(ctx, state, "child kill requires a valid id");
    };
    let Some(entry) = state
        .node_child_process
        .children
        .get_mut(id as usize)
        .and_then(Option::as_mut)
    else {
        return value::encode_bool(false);
    };
    if entry.exit_delivered {
        return value::encode_bool(false);
    }
    value::encode_bool(entry.child.kill().is_ok())
}

fn send(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    #[cfg(unix)]
    {
        let Some(id) = numeric_id(args.first().copied()) else {
            return type_error(ctx, state, "child send requires a valid id");
        };
        let message = args.get(1).copied().unwrap_or_else(value::encode_undefined);
        let payload = match encode_message(ctx, state, message) {
            Ok(payload) => payload,
            Err(exception) => return exception,
        };
        let fd = numeric_fd(args.get(2).copied());
        let transfer = fd.and_then(|fd| super::node_net::take_outgoing_fd(state, fd));
        let wire_fd = transfer.as_ref().map(AsRawFd::as_raw_fd).or(fd);
        let Some(channel) = state
            .node_child_process
            .children
            .get(id as usize)
            .and_then(Option::as_ref)
            .and_then(|entry| entry.ipc.as_ref())
        else {
            return error_object(ctx, state, "child has no IPC channel");
        };
        channel
            .send(payload, wire_fd)
            .map(|()| value::encode_bool(true))
            .unwrap_or_else(|error| {
                error_object(ctx, state, &format!("child send failed: {error}"))
            })
    }
    #[cfg(not(unix))]
    let _ = args;
    #[cfg(not(unix))]
    error_object(ctx, state, "child process IPC is only supported on Unix")
}

fn disconnect(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(id) = numeric_id(args.first().copied()) else {
        return type_error(ctx, state, "child disconnect requires a valid id");
    };
    #[cfg(not(unix))]
    let _ = id;
    #[cfg(unix)]
    if let Some(endpoint) = state
        .node_child_process
        .children
        .get(id as usize)
        .and_then(Option::as_ref)
        .and_then(|entry| entry.ipc.as_ref())
        .and_then(ipc::ParentIpcHandle::endpoint)
    {
        endpoint.close();
    }
    value::encode_undefined()
}

fn register_callback(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
    exit: bool,
) -> i64 {
    let Some(id) = numeric_id(args.first().copied()) else {
        return type_error(ctx, state, "child callback requires a valid id");
    };
    let callback = args.get(1).copied().unwrap_or_else(value::encode_undefined);
    if !value::is_callable(callback) {
        return type_error(ctx, state, "child callback must be callable");
    }
    let registered = RegisteredCallback {
        callable: callback,
        context: node_async_hooks::capture_context(state),
    };
    let Some(entry) = state
        .node_child_process
        .children
        .get_mut(id as usize)
        .and_then(Option::as_mut)
    else {
        return type_error(ctx, state, "child callback id does not exist");
    };
    if exit {
        entry.exit = Some(registered);
    } else {
        entry.message = Some(registered);
    }
    value::encode_undefined()
}

fn process_send(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    #[cfg(unix)]
    {
        let message = args
            .first()
            .copied()
            .unwrap_or_else(value::encode_undefined);
        let payload = match encode_message(ctx, state, message) {
            Ok(payload) => payload,
            Err(exception) => return exception,
        };
        let fd = numeric_fd(args.get(1).copied());
        let transfer = fd.and_then(|fd| super::node_net::take_outgoing_fd(state, fd));
        let wire_fd = transfer.as_ref().map(AsRawFd::as_raw_fd).or(fd);
        let Some(process) = state.node_child_process.process.as_mut() else {
            return error_object(ctx, state, "process has no IPC channel");
        };
        if process.endpoint.is_none() {
            match ipc::connect(&process.path) {
                Ok(endpoint) => process.endpoint = Some(endpoint),
                Err(error) => {
                    return error_object(ctx, state, &format!("IPC connect failed: {error}"));
                }
            }
        }
        process
            .endpoint
            .as_ref()
            .expect("endpoint initialized")
            .send(&payload, wire_fd)
            .map(|()| value::encode_bool(true))
            .unwrap_or_else(|error| {
                error_object(ctx, state, &format!("process send failed: {error}"))
            })
    }
    #[cfg(not(unix))]
    let _ = args;
    #[cfg(not(unix))]
    error_object(ctx, state, "process IPC is only supported on Unix")
}

fn process_disconnect(state: &mut NativeAgentState) -> i64 {
    #[cfg(not(unix))]
    let _ = state;
    #[cfg(unix)]
    if let Some(endpoint) = state
        .node_child_process
        .process
        .as_ref()
        .and_then(|process| process.endpoint.as_ref())
    {
        endpoint.close();
    }
    value::encode_undefined()
}

struct MessageEvent {
    callback: RegisteredCallback,
    payload: String,
    fd: Option<RawFd>,
}

struct ExitEvent {
    callback: Option<RegisteredCallback>,
    code: Option<i32>,
    signal: Option<String>,
}
fn next_process_message(state: &mut NativeAgentState) -> Option<MessageEvent> {
    #[cfg(unix)]
    {
        let process = state.node_child_process.process.as_ref()?;
        let callback = process.message.clone()?;
        let message = process.endpoint.as_ref()?.pop()?;
        Some(MessageEvent {
            callback,
            payload: message.payload,
            fd: message.fd,
        })
    }
    #[cfg(not(unix))]
    {
        let _ = state;
        None
    }
}

fn next_child_message(state: &mut NativeAgentState) -> Option<MessageEvent> {
    #[cfg(unix)]
    {
        for entry in state.node_child_process.children.iter().flatten() {
            let Some(callback) = entry.message.clone() else {
                continue;
            };
            let Some(endpoint) = entry.ipc.as_ref().and_then(ipc::ParentIpcHandle::endpoint) else {
                continue;
            };
            let Some(message) = endpoint.pop() else {
                continue;
            };
            return Some(MessageEvent {
                callback,
                payload: message.payload,
                fd: message.fd,
            });
        }
        None
    }
    #[cfg(not(unix))]
    {
        let _ = state;
        None
    }
}

fn next_child_exit(state: &mut NativeAgentState) -> Option<ExitEvent> {
    for entry in state.node_child_process.children.iter_mut().flatten() {
        if entry.exit_delivered {
            continue;
        }
        let status = match entry.child.try_wait() {
            Ok(Some(status)) => status,
            Ok(None) => continue,
            Err(_) => {
                entry.exit_delivered = true;
                return Some(ExitEvent {
                    callback: entry.exit.clone(),
                    code: None,
                    signal: None,
                });
            }
        };
        entry.exit_delivered = true;
        return Some(ExitEvent {
            callback: entry.exit.clone(),
            code: status.code(),
            signal: signal_from_status(&status),
        });
    }
    None
}

fn deliver_message(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    event: MessageEvent,
) -> i64 {
    #[cfg(unix)]
    if let Some(fd) = event.fd {
        super::node_net::register_incoming_fd(state, fd);
    }
    let message = match decode_message(ctx, state, &event.payload) {
        Ok(message) => message,
        Err(exception) => {
            #[cfg(unix)]
            if let Some(fd) = event.fd {
                super::node_net::discard_incoming_fd(state, fd);
            }
            return exception;
        }
    };
    let fd = event.fd.map_or_else(value::encode_undefined, |fd| {
        value::encode_f64(f64::from(fd))
    });
    let result = invoke_registered(ctx, state, event.callback, &[message, fd]);
    #[cfg(unix)]
    if let Some(fd) = event.fd {
        super::node_net::discard_incoming_fd(state, fd);
    }
    result
}

fn deliver_exit(ctx: &mut NativeVmContext, state: &mut NativeAgentState, event: ExitEvent) -> i64 {
    let Some(callback) = event.callback else {
        return value::encode_undefined();
    };
    let code = event.code.map_or_else(value::encode_null, |code| {
        value::encode_f64(f64::from(code))
    });
    let signal = event
        .signal
        .and_then(|signal| state.intern_text(signal, value::TAG_STRING))
        .unwrap_or_else(value::encode_null);
    invoke_registered(ctx, state, callback, &[code, signal])
}

fn invoke_registered(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    callback: RegisteredCallback,
    args: &[i64],
) -> i64 {
    let previous = node_async_hooks::enter_context(state, callback.context);
    let result = state
        .invoke_callable(ctx, callback.callable, value::encode_undefined(), args)
        .unwrap_or_else(|| runtime::fail_dispatch(ctx));
    node_async_hooks::restore_context(state, previous);
    result
}

#[cfg(unix)]
fn encode_message(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    message: i64,
) -> Result<String, i64> {
    let encoded = json::dispatch_json(ctx, state, Builtin::JsonStringify, &[message])
        .ok_or_else(|| runtime::fail_dispatch(ctx))?;
    if value::is_exception(encoded) {
        return Err(encoded);
    }
    state.string_owned(encoded)
        .map(|text| text.to_utf8_lossy())
        .ok_or_else(|| type_error(ctx, state, "IPC message is not JSON serializable"))
}

fn decode_message(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    payload: &str,
) -> Result<i64, i64> {
    let payload = state
        .intern_text(payload.into(), value::TAG_STRING)
        .ok_or_else(|| runtime::fail_dispatch(ctx))?;
    let decoded = json::dispatch_json(ctx, state, Builtin::JsonParse, &[payload])
        .ok_or_else(|| runtime::fail_dispatch(ctx))?;
    if value::is_exception(decoded) {
        Err(decoded)
    } else {
        Ok(decoded)
    }
}

fn string_array(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    array: i64,
) -> Result<Vec<String>, i64> {
    if !value::is_array(array) {
        return Err(type_error(
            ctx,
            state,
            "child process arguments must be an array",
        ));
    }
    let handle = value::decode_handle(array);
    let length = state
        .gc
        .heap()
        .array_length(handle)
        .map_err(|_| runtime::fail_dispatch(ctx))?;
    let mut strings = Vec::with_capacity(length as usize);
    for index in 0..length {
        let item = state
            .gc
            .heap()
            .get_element(handle, index)
            .map_err(|_| runtime::fail_dispatch(ctx))?
            .map(|item| item as i64)
            .unwrap_or_else(value::encode_undefined);
        strings.push(runtime::to_string_coerced(ctx, state, item)?);
    }
    Ok(strings)
}

fn id_pair(ctx: &mut NativeVmContext, state: &mut NativeAgentState, id: u32, pid: u32) -> i64 {
    let object = match state.allocate_object(2, false) {
        Ok(object) => object,
        Err(_) => return runtime::fail_dispatch(ctx),
    };
    if modules::set_named_property(state, object, "id", value::encode_f64(f64::from(id))).is_err()
        || modules::set_named_property(state, object, "pid", value::encode_f64(f64::from(pid)))
            .is_err()
    {
        return runtime::fail_dispatch(ctx);
    }
    object
}

fn validate_command(
    state: &NativeAgentState,
    command: &str,
    shell: bool,
    allow_self: bool,
) -> Result<(), String> {
    if allow_self
        && std::env::current_exe()
            .ok()
            .is_some_and(|path| path == std::path::Path::new(command))
    {
        return Ok(());
    }
    let allowed = state
        .environment
        .get("WJSM_CHILD_PROCESS_ALLOW")
        .map(String::as_str)
        .unwrap_or_default();
    let command = if shell {
        command.split_whitespace().next().unwrap_or(command)
    } else {
        command
    };
    if allowed
        .split(',')
        .flat_map(std::env::split_paths)
        .any(|allowed| {
            allowed == std::path::Path::new("*")
                || allowed == std::path::Path::new(command)
                || allowed.file_name() == std::path::Path::new(command).file_name()
        })
    {
        return Ok(());
    }
    Err(format!(
        "child_process execution is disabled for '{command}'; set WJSM_CHILD_PROCESS_ALLOW"
    ))
}

fn numeric_id(value: Option<i64>) -> Option<u32> {
    let value = value
        .filter(|value| value::is_f64(*value))
        .map(value::decode_f64)?;
    (value.is_finite() && value >= 0.0 && value <= f64::from(u32::MAX)).then_some(value as u32)
}

#[cfg(unix)]
fn numeric_fd(value: Option<i64>) -> Option<RawFd> {
    let value = value
        .filter(|value| value::is_f64(*value))
        .map(value::decode_f64)?;
    (value.is_finite() && value >= 0.0 && value <= f64::from(i32::MAX)).then_some(value as RawFd)
}

#[cfg(unix)]
fn signal_from_status(status: &std::process::ExitStatus) -> Option<String> {
    use std::os::unix::process::ExitStatusExt;
    status.signal().map(|signal| format!("SIG{signal}"))
}

#[cfg(not(unix))]
fn signal_from_status(_status: &std::process::ExitStatus) -> Option<String> {
    None
}

fn type_error(ctx: &mut NativeVmContext, state: &mut NativeAgentState, message: &str) -> i64 {
    modules::named_error_object(state, "TypeError", message.into())
        .and_then(|error| state.create_exception(error))
        .unwrap_or_else(|| runtime::fail_dispatch(ctx))
}

fn error_object(ctx: &mut NativeVmContext, state: &mut NativeAgentState, message: &str) -> i64 {
    modules::named_error_object(state, "Error", message.into())
        .and_then(|error| state.create_exception(error))
        .unwrap_or_else(|| runtime::fail_dispatch(ctx))
}
