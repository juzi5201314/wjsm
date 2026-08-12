use std::collections::HashMap;
use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
#[cfg(unix)]
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};

use wjsm_ir::value;
use wjsm_native_abi::NativeVmContext;

use super::runtime::{fail_dispatch, render_value, to_number};
use crate::NativeAgentState;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum NodeNetMethod {
    Connect,
    Destroy,
    End,
    Read,
    ServerAccept,
    ServerAcceptRawFd,
    ServerAddress,
    ServerClose,
    ServerListen,
    ServerPort,
    SocketFromFd,
    SocketLocalAddress,
    SocketLocalPort,
    SocketRemoteAddress,
    SocketRemotePort,
    Write,
}

#[derive(Default)]
pub(crate) struct NodeNetState {
    pub(crate) bridge: Option<i64>,
    listeners: HashMap<u32, TcpListener>,
    sockets: HashMap<u32, TcpStream>,
    #[cfg(unix)]
    outgoing_fds: HashMap<RawFd, OwnedFd>,
    #[cfg(unix)]
    incoming_fds: HashMap<RawFd, OwnedFd>,
    next_handle: u32,
}

impl NodeNetState {
    fn allocate_handle(&mut self) -> Option<u32> {
        let handle = self.next_handle;
        self.next_handle = self.next_handle.checked_add(1)?;
        Some(handle)
    }
}

pub(crate) fn ensure_bridge(state: &mut NativeAgentState) -> Option<i64> {
    if let Some(bridge) = state.node_net.bridge {
        return Some(bridge);
    }
    let methods = [
        ("connect", NodeNetMethod::Connect),
        ("destroy", NodeNetMethod::Destroy),
        ("end", NodeNetMethod::End),
        ("read", NodeNetMethod::Read),
        ("serverAccept", NodeNetMethod::ServerAccept),
        ("serverAcceptRawFd", NodeNetMethod::ServerAcceptRawFd),
        ("serverAddress", NodeNetMethod::ServerAddress),
        ("serverClose", NodeNetMethod::ServerClose),
        ("serverListen", NodeNetMethod::ServerListen),
        ("serverPort", NodeNetMethod::ServerPort),
        ("socketFromFd", NodeNetMethod::SocketFromFd),
        ("socketLocalAddress", NodeNetMethod::SocketLocalAddress),
        ("socketLocalPort", NodeNetMethod::SocketLocalPort),
        ("socketRemoteAddress", NodeNetMethod::SocketRemoteAddress),
        ("socketRemotePort", NodeNetMethod::SocketRemotePort),
        ("write", NodeNetMethod::Write),
    ];
    let bridge = state.allocate_object(methods.len() as u32, false).ok()?;
    for (name, method) in methods {
        let key = state.intern_text(name.into(), value::TAG_STRING)?;
        let callable = state.native_callable(crate::NativeCallableKind::NodeNet(method))?;
        state
            .heap
            .set_property(
                value::decode_handle(bridge),
                value::decode_handle(key),
                callable as u64,
            )
            .ok()?;
    }
    state.node_net.bridge = Some(bridge);
    Some(bridge)
}

pub(crate) fn call(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    method: NodeNetMethod,
    args: &[i64],
) -> i64 {
    match method {
        NodeNetMethod::Connect => connect(ctx, state, args),
        NodeNetMethod::Destroy => destroy(ctx, state, args),
        NodeNetMethod::End => end(ctx, state, args),
        NodeNetMethod::Read => read(ctx, state, args),
        NodeNetMethod::ServerAccept => server_accept(ctx, state, args, false),
        NodeNetMethod::ServerAcceptRawFd => server_accept(ctx, state, args, true),
        NodeNetMethod::ServerAddress => server_address(ctx, state, args),
        NodeNetMethod::ServerClose => server_close(ctx, state, args),
        NodeNetMethod::ServerListen => server_listen(ctx, state, args),
        NodeNetMethod::ServerPort => server_port(ctx, state, args),
        NodeNetMethod::SocketFromFd => socket_from_fd(ctx, state, args),
        NodeNetMethod::SocketLocalAddress => socket_address(ctx, state, args, true, false),
        NodeNetMethod::SocketLocalPort => socket_address(ctx, state, args, true, true),
        NodeNetMethod::SocketRemoteAddress => socket_address(ctx, state, args, false, false),
        NodeNetMethod::SocketRemotePort => socket_address(ctx, state, args, false, true),
        NodeNetMethod::Write => write(ctx, state, args),
    }
}

fn server_listen(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let port = args
        .first()
        .and_then(|port| to_number(state, *port))
        .map_or(0, |port| port as u16);
    let host = args
        .get(1)
        .and_then(|host| state.string(*host))
        .and_then(|host| host.to_utf8())
        .unwrap_or_else(|| "127.0.0.1".into());
    let result = TcpListener::bind((host.as_str(), port)).and_then(|listener| {
        listener.set_nonblocking(true)?;
        Ok(listener)
    });
    match result {
        Ok(listener) => {
            let Some(handle) = state.node_net.allocate_handle() else {
                return fail_dispatch(ctx);
            };
            state.node_net.listeners.insert(handle, listener);
            resolved(ctx, state, value::encode_f64(f64::from(handle)))
        }
        Err(error) => rejected(ctx, state, error.to_string()),
    }
}

fn server_accept(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
    raw_fd: bool,
) -> i64 {
    let stream = match accept_stream(state, args) {
        Ok(Some(stream)) => stream,
        Ok(None) => return resolved(ctx, state, value::encode_null()),
        Err(error) => return rejected(ctx, state, error),
    };
    if raw_fd {
        #[cfg(unix)]
        {
            let owned: OwnedFd = stream.into();
            let fd = owned.as_raw_fd();
            state.node_net.outgoing_fds.insert(fd, owned);
            return resolved(ctx, state, value::encode_f64(f64::from(fd)));
        }
        #[cfg(not(unix))]
        return rejected(ctx, state, "raw socket transfer is unavailable".into());
    }
    let Some(socket) = state.node_net.allocate_handle() else {
        return fail_dispatch(ctx);
    };
    state.node_net.sockets.insert(socket, stream);
    resolved(ctx, state, value::encode_f64(f64::from(socket)))
}

fn accept_stream(state: &NativeAgentState, args: &[i64]) -> Result<Option<TcpStream>, String> {
    let Some(handle) = handle(state, args.first().copied()) else {
        return Err("Invalid server handle".into());
    };
    let Some(listener) = state.node_net.listeners.get(&handle) else {
        return Err("Invalid server handle".into());
    };
    match listener.accept() {
        Ok((stream, _)) => {
            stream
                .set_nonblocking(true)
                .map_err(|error| error.to_string())?;
            Ok(Some(stream))
        }
        Err(error) if error.kind() == ErrorKind::WouldBlock => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

fn socket_from_fd(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    #[cfg(unix)]
    {
        let Some(fd) = args
            .first()
            .and_then(|fd| to_number(state, *fd))
            .filter(|fd| fd.is_finite() && *fd >= 0.0 && fd.fract() == 0.0)
            .and_then(|fd| i32::try_from(fd as i64).ok())
        else {
            return type_error(ctx, state, "Invalid transferred socket fd");
        };
        let Some(owned) = state.node_net.incoming_fds.remove(&fd) else {
            return type_error(ctx, state, "Socket fd was not received through IPC");
        };
        let stream = TcpStream::from(owned);
        if let Err(error) = stream.set_nonblocking(true) {
            return type_error(ctx, state, &error.to_string());
        }
        let Some(socket) = state.node_net.allocate_handle() else {
            return fail_dispatch(ctx);
        };
        state.node_net.sockets.insert(socket, stream);
        return value::encode_f64(f64::from(socket));
    }
    #[cfg(not(unix))]
    {
        let _ = args;
        type_error(ctx, state, "socketFromFd is unavailable")
    }
}

#[cfg(unix)]
pub(crate) fn take_outgoing_fd(state: &mut NativeAgentState, fd: RawFd) -> Option<OwnedFd> {
    state.node_net.outgoing_fds.remove(&fd)
}

#[cfg(unix)]
pub(crate) fn register_incoming_fd(state: &mut NativeAgentState, fd: RawFd) {
    // SAFETY: fd 由 recvmsg(SCM_RIGHTS) 创建，所有权在此唯一转入 side table。
    let owned = unsafe { OwnedFd::from_raw_fd(fd) };
    state.node_net.incoming_fds.insert(fd, owned);
}

#[cfg(unix)]
pub(crate) fn discard_incoming_fd(state: &mut NativeAgentState, fd: RawFd) {
    state.node_net.incoming_fds.remove(&fd);
}

fn connect(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let port = args
        .first()
        .and_then(|port| to_number(state, *port))
        .map_or(0, |port| port as u16);
    let host = args
        .get(1)
        .and_then(|host| state.string(*host))
        .and_then(|host| host.to_utf8())
        .unwrap_or_else(|| "127.0.0.1".into());
    match TcpStream::connect((host.as_str(), port)) {
        Ok(stream) => {
            if let Err(error) = stream.set_nonblocking(true) {
                return rejected(ctx, state, error.to_string());
            }
            let Some(handle) = state.node_net.allocate_handle() else {
                return fail_dispatch(ctx);
            };
            state.node_net.sockets.insert(handle, stream);
            super::node_perf_hooks::emit_net_entry(ctx, state, &host, port);
            resolved(ctx, state, value::encode_f64(f64::from(handle)))
        }
        Err(error) => rejected(ctx, state, error.to_string()),
    }
}

fn read(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(handle) = handle(state, args.first().copied()) else {
        return rejected(ctx, state, "Invalid socket handle".into());
    };
    let mut bytes = vec![0; 64 * 1024];
    let result = state
        .node_net
        .sockets
        .get_mut(&handle)
        .map(|stream| stream.read(&mut bytes));
    match result {
        Some(Ok(0)) => resolved(ctx, state, value::encode_null()),
        Some(Ok(length)) => {
            bytes.truncate(length);
            let Some(buffer) = super::node_buffer::from_bytes(state, bytes) else {
                return fail_dispatch(ctx);
            };
            resolved(ctx, state, buffer)
        }
        Some(Err(error)) if error.kind() == ErrorKind::WouldBlock => {
            resolved(ctx, state, value::encode_null())
        }
        Some(Err(error)) => rejected(ctx, state, error.to_string()),
        None => rejected(ctx, state, "Invalid socket handle".into()),
    }
}

fn write(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(handle) = handle(state, args.first().copied()) else {
        return type_error(ctx, state, "Invalid socket handle");
    };
    let input = args.get(1).copied().unwrap_or_else(value::encode_undefined);
    let bytes = super::node_buffer::bytes(state, input).unwrap_or_else(|| {
        state
            .string(input)
            .and_then(|text| text.to_utf8())
            .unwrap_or_else(|| render_value(state, input))
            .into_bytes()
    });
    match state
        .node_net
        .sockets
        .get_mut(&handle)
        .map(|stream| stream.write_all(&bytes))
    {
        Some(Ok(())) => value::encode_undefined(),
        Some(Err(error)) => error_value(ctx, state, error.to_string()),
        None => type_error(ctx, state, "Invalid socket handle"),
    }
}

fn end(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(handle) = handle(state, args.first().copied()) else {
        return type_error(ctx, state, "Invalid socket handle");
    };
    match state
        .node_net
        .sockets
        .get(&handle)
        .map(|stream| stream.shutdown(Shutdown::Write))
    {
        Some(Ok(())) => value::encode_undefined(),
        Some(Err(error)) => error_value(ctx, state, error.to_string()),
        None => type_error(ctx, state, "Invalid socket handle"),
    }
}

fn destroy(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(handle) = handle(state, args.first().copied()) else {
        return type_error(ctx, state, "Invalid socket handle");
    };
    if let Some(stream) = state.node_net.sockets.remove(&handle) {
        let _ = stream.shutdown(Shutdown::Both);
    }
    value::encode_undefined()
}

fn server_close(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(handle) = handle(state, args.first().copied()) else {
        return type_error(ctx, state, "Invalid server handle");
    };
    state.node_net.listeners.remove(&handle);
    value::encode_undefined()
}

fn server_port(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(handle) = handle(state, args.first().copied()) else {
        return type_error(ctx, state, "Invalid server handle");
    };
    state
        .node_net
        .listeners
        .get(&handle)
        .and_then(|listener| listener.local_addr().ok())
        .map(|address| value::encode_f64(f64::from(address.port())))
        .unwrap_or_else(|| type_error(ctx, state, "Invalid server handle"))
}

fn server_address(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(handle) = handle(state, args.first().copied()) else {
        return type_error(ctx, state, "Invalid server handle");
    };
    let Some(address) = state
        .node_net
        .listeners
        .get(&handle)
        .and_then(|listener| listener.local_addr().ok())
    else {
        return type_error(ctx, state, "Invalid server handle");
    };
    state
        .intern_text(address.ip().to_string(), value::TAG_STRING)
        .unwrap_or_else(|| fail_dispatch(ctx))
}

fn socket_address(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
    local: bool,
    port: bool,
) -> i64 {
    let Some(handle) = handle(state, args.first().copied()) else {
        return type_error(ctx, state, "Invalid socket handle");
    };
    let address = state.node_net.sockets.get(&handle).and_then(|stream| {
        if local {
            stream.local_addr().ok()
        } else {
            stream.peer_addr().ok()
        }
    });
    let Some(address) = address else {
        return type_error(ctx, state, "Invalid socket handle");
    };
    if port {
        value::encode_f64(f64::from(address.port()))
    } else {
        state
            .intern_text(address.ip().to_string(), value::TAG_STRING)
            .unwrap_or_else(|| fail_dispatch(ctx))
    }
}

fn handle(state: &NativeAgentState, encoded: Option<i64>) -> Option<u32> {
    encoded
        .and_then(|encoded| to_number(state, encoded))
        .filter(|number| number.is_finite() && *number >= 0.0 && number.fract() == 0.0)
        .map(|number| number as u32)
}

fn resolved(ctx: &mut NativeVmContext, state: &mut NativeAgentState, result: i64) -> i64 {
    let Some(promise) = super::promise::new_promise(ctx, state) else {
        return fail_dispatch(ctx);
    };
    super::promise::settle_promise(state, value::decode_handle(promise), result, false);
    promise
}

fn rejected(ctx: &mut NativeVmContext, state: &mut NativeAgentState, message: String) -> i64 {
    let error = error_value(ctx, state, message);
    let Some(promise) = super::promise::new_promise(ctx, state) else {
        return fail_dispatch(ctx);
    };
    super::promise::settle_promise(state, value::decode_handle(promise), error, true);
    promise
}

fn error_value(ctx: &mut NativeVmContext, state: &mut NativeAgentState, message: String) -> i64 {
    super::modules::named_error_object(state, "Error", message)
        .unwrap_or_else(|| fail_dispatch(ctx))
}

fn type_error(ctx: &mut NativeVmContext, state: &mut NativeAgentState, message: &str) -> i64 {
    super::modules::named_error_object(state, "TypeError", message.into())
        .and_then(|error| state.create_exception(error))
        .unwrap_or_else(|| fail_dispatch(ctx))
}
