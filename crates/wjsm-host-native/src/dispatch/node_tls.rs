use std::collections::HashMap;
use std::sync::mpsc::{Receiver, TryRecvError};

use num_traits::ToPrimitive;
use wjsm_ir::value;
use wjsm_native_abi::NativeVmContext;

use super::runtime::{fail_dispatch, render_value, to_number};
use crate::{NativeAgentState, NativeCallableKind};

mod worker;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum NodeTlsMethod {
    Connect,
    Destroy,
    End,
    Read,
    ServerAccept,
    ServerAddress,
    ServerClose,
    ServerListen,
    ServerPort,
    SocketLocalAddress,
    SocketLocalPort,
    SocketRemoteAddress,
    SocketRemotePort,
    Write,
}

struct PendingSocket {
    promise: u32,
    receiver: Receiver<worker::SocketResult>,
}

struct PendingRead {
    promise: u32,
    receiver: Receiver<worker::ReadResult>,
}

#[derive(Default)]
pub(crate) struct NodeTlsState {
    pub(crate) bridge: Option<i64>,
    listeners: HashMap<u32, worker::TlsListenerHandle>,
    sockets: HashMap<u32, worker::TlsSocketEndpoint>,
    pending_sockets: Vec<PendingSocket>,
    pending_reads: Vec<PendingRead>,
    next_handle: u32,
}

impl NodeTlsState {
    fn allocate_handle(&mut self) -> Option<u32> {
        let handle = self.next_handle;
        self.next_handle = self.next_handle.checked_add(1)?;
        Some(handle)
    }
}

pub(crate) fn ensure_bridge(state: &mut NativeAgentState) -> Option<i64> {
    if let Some(bridge) = state.node_tls.bridge {
        return Some(bridge);
    }
    let methods = [
        ("connect", NodeTlsMethod::Connect),
        ("destroy", NodeTlsMethod::Destroy),
        ("end", NodeTlsMethod::End),
        ("read", NodeTlsMethod::Read),
        ("serverAccept", NodeTlsMethod::ServerAccept),
        ("serverAddress", NodeTlsMethod::ServerAddress),
        ("serverClose", NodeTlsMethod::ServerClose),
        ("serverListen", NodeTlsMethod::ServerListen),
        ("serverPort", NodeTlsMethod::ServerPort),
        ("socketLocalAddress", NodeTlsMethod::SocketLocalAddress),
        ("socketLocalPort", NodeTlsMethod::SocketLocalPort),
        ("socketRemoteAddress", NodeTlsMethod::SocketRemoteAddress),
        ("socketRemotePort", NodeTlsMethod::SocketRemotePort),
        ("write", NodeTlsMethod::Write),
    ];
    let capacity = u32::try_from(methods.len()).ok()?;
    let bridge = state.allocate_object(capacity, false).ok()?;
    for (name, method) in methods {
        let callable = state.native_callable(NativeCallableKind::NodeTls(method))?;
        super::modules::set_named_property(state, bridge, name, callable).ok()?;
    }
    state.node_tls.bridge = Some(bridge);
    Some(bridge)
}

pub(crate) fn call(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    method: NodeTlsMethod,
    args: &[i64],
) -> i64 {
    match method {
        NodeTlsMethod::Connect => connect(ctx, state, args),
        NodeTlsMethod::Destroy => destroy(ctx, state, args),
        NodeTlsMethod::End => end(ctx, state, args),
        NodeTlsMethod::Read => read(ctx, state, args),
        NodeTlsMethod::ServerAccept => server_accept(ctx, state, args),
        NodeTlsMethod::ServerAddress => server_address(ctx, state, args, false),
        NodeTlsMethod::ServerClose => server_close(ctx, state, args),
        NodeTlsMethod::ServerListen => server_listen(ctx, state, args),
        NodeTlsMethod::ServerPort => server_address(ctx, state, args, true),
        NodeTlsMethod::SocketLocalAddress => socket_address(ctx, state, args, true, false),
        NodeTlsMethod::SocketLocalPort => socket_address(ctx, state, args, true, true),
        NodeTlsMethod::SocketRemoteAddress => socket_address(ctx, state, args, false, false),
        NodeTlsMethod::SocketRemotePort => socket_address(ctx, state, args, false, true),
        NodeTlsMethod::Write => write(ctx, state, args),
    }
}

pub(crate) fn has_pending(state: &NativeAgentState) -> bool {
    !state.node_tls.pending_sockets.is_empty() || !state.node_tls.pending_reads.is_empty()
}

pub(crate) fn poll_pending(ctx: &mut NativeVmContext, state: &mut NativeAgentState) -> i64 {
    let socket_completions = take_socket_completions(&mut state.node_tls);
    for (promise, result) in socket_completions {
        match result {
            Ok(socket) => {
                let Some(handle) = state.node_tls.allocate_handle() else {
                    return fail_dispatch(ctx);
                };
                state.node_tls.sockets.insert(handle, socket);
                super::promise::settle_promise(
                    state,
                    promise,
                    value::encode_f64(f64::from(handle)),
                    false,
                );
            }
            Err(message) => settle_error(ctx, state, promise, message),
        }
    }
    let read_completions = take_read_completions(&mut state.node_tls);
    for (promise, result) in read_completions {
        match result {
            Ok(Some(bytes)) => {
                let Some(buffer) = super::node_buffer::from_bytes(state, bytes) else {
                    return fail_dispatch(ctx);
                };
                super::promise::settle_promise(state, promise, buffer, false);
            }
            Ok(None) => {
                super::promise::settle_promise(state, promise, value::encode_null(), false);
            }
            Err(message) => settle_error(ctx, state, promise, message),
        }
    }
    value::encode_undefined()
}

fn server_listen(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let port = args
        .first()
        .and_then(|port| to_number(state, *port))
        .and_then(|port| port.to_u16())
        .unwrap_or(0);
    let host = text_arg(state, args, 1).unwrap_or_else(|| "127.0.0.1".to_owned());
    let cert = text_arg(state, args, 2).unwrap_or_default();
    let key = text_arg(state, args, 3).unwrap_or_default();
    let alpn = text_arg(state, args, 4).unwrap_or_default();
    match worker::listen(host, port, cert, key, alpn) {
        Ok(listener) => {
            let Some(handle) = state.node_tls.allocate_handle() else {
                return fail_dispatch(ctx);
            };
            state.node_tls.listeners.insert(handle, listener);
            resolved(ctx, state, value::encode_f64(f64::from(handle)))
        }
        Err(message) => rejected(ctx, state, message),
    }
}

fn server_accept(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(handle) = handle(state, args.first().copied()) else {
        return rejected(ctx, state, "Invalid TLS server handle".to_owned());
    };
    let Some(listener) = state.node_tls.listeners.get(&handle) else {
        return rejected(ctx, state, "Invalid TLS server handle".to_owned());
    };
    let receiver = match worker::accept(listener) {
        Ok(receiver) => receiver,
        Err(message) => return rejected(ctx, state, message),
    };
    let Some(promise) = super::promise::new_promise(ctx, state) else {
        return fail_dispatch(ctx);
    };
    state.node_tls.pending_sockets.push(PendingSocket {
        promise: value::decode_handle(promise),
        receiver,
    });
    promise
}

fn connect(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(port) = args
        .first()
        .and_then(|port| to_number(state, *port))
        .and_then(|port| port.to_u16())
    else {
        return rejected(ctx, state, "Invalid TLS port".to_owned());
    };
    let host = text_arg(state, args, 1).unwrap_or_else(|| "127.0.0.1".to_owned());
    let server_name = text_arg(state, args, 2).unwrap_or_else(|| host.clone());
    let reject_unauthorized = args
        .get(3)
        .copied()
        .filter(|value| value::is_bool(*value))
        .is_none_or(value::decode_bool);
    let alpn = text_arg(state, args, 4).unwrap_or_default();
    let receiver = worker::connect(host, port, server_name, reject_unauthorized, alpn);
    let Some(promise) = super::promise::new_promise(ctx, state) else {
        return fail_dispatch(ctx);
    };
    state.node_tls.pending_sockets.push(PendingSocket {
        promise: value::decode_handle(promise),
        receiver,
    });
    promise
}

fn read(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(handle) = handle(state, args.first().copied()) else {
        return rejected(ctx, state, "Invalid TLS socket handle".to_owned());
    };
    let Some(socket) = state.node_tls.sockets.get(&handle) else {
        return rejected(ctx, state, "Invalid TLS socket handle".to_owned());
    };
    let receiver = match worker::read(socket) {
        Ok(receiver) => receiver,
        Err(message) => return rejected(ctx, state, message),
    };
    let Some(promise) = super::promise::new_promise(ctx, state) else {
        return fail_dispatch(ctx);
    };
    state.node_tls.pending_reads.push(PendingRead {
        promise: value::decode_handle(promise),
        receiver,
    });
    promise
}

fn write(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(handle) = handle(state, args.first().copied()) else {
        return error_value(ctx, state, "Invalid TLS socket handle".to_owned());
    };
    let Some(socket) = state.node_tls.sockets.get(&handle).cloned() else {
        return error_value(ctx, state, "Invalid TLS socket handle".to_owned());
    };
    let bytes = args
        .get(1)
        .copied()
        .map(|input| render_value(state, input).into_bytes())
        .unwrap_or_default();
    match worker::write(&socket, bytes) {
        Ok(()) => value::encode_undefined(),
        Err(message) => error_value(ctx, state, message),
    }
}

fn end(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(handle) = handle(state, args.first().copied()) else {
        return error_value(ctx, state, "Invalid TLS socket handle".to_owned());
    };
    let Some(socket) = state.node_tls.sockets.get(&handle).cloned() else {
        return error_value(ctx, state, "Invalid TLS socket handle".to_owned());
    };
    match worker::end(&socket) {
        Ok(()) => value::encode_undefined(),
        Err(message) => error_value(ctx, state, message),
    }
}

fn destroy(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(handle) = handle(state, args.first().copied()) else {
        return error_value(ctx, state, "Invalid TLS socket handle".to_owned());
    };
    if let Some(socket) = state.node_tls.sockets.remove(&handle) {
        worker::destroy(socket);
    }
    value::encode_undefined()
}

fn server_close(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(handle) = handle(state, args.first().copied()) else {
        return rejected(ctx, state, "Invalid TLS server handle".to_owned());
    };
    if let Some(listener) = state.node_tls.listeners.remove(&handle) {
        worker::close_listener(listener);
    }
    resolved(ctx, state, value::encode_undefined())
}

fn server_address(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
    port: bool,
) -> i64 {
    let Some(handle) = handle(state, args.first().copied()) else {
        return error_value(ctx, state, "Invalid TLS server handle".to_owned());
    };
    let Some(listener) = state.node_tls.listeners.get(&handle) else {
        return error_value(ctx, state, "Invalid TLS server handle".to_owned());
    };
    if port {
        value::encode_f64(f64::from(listener.address.port()))
    } else {
        state
            .intern_text(listener.address.ip().to_string(), value::TAG_STRING)
            .unwrap_or_else(|| fail_dispatch(ctx))
    }
}

fn socket_address(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
    local: bool,
    port: bool,
) -> i64 {
    let Some(handle) = handle(state, args.first().copied()) else {
        return error_value(ctx, state, "Invalid TLS socket handle".to_owned());
    };
    let Some(socket) = state.node_tls.sockets.get(&handle) else {
        return error_value(ctx, state, "Invalid TLS socket handle".to_owned());
    };
    let address = if local { socket.local } else { socket.remote };
    if port {
        value::encode_f64(f64::from(address.port()))
    } else {
        state
            .intern_text(address.ip().to_string(), value::TAG_STRING)
            .unwrap_or_else(|| fail_dispatch(ctx))
    }
}

fn take_socket_completions(state: &mut NodeTlsState) -> Vec<(u32, worker::SocketResult)> {
    let mut completed = Vec::new();
    let mut index = 0;
    while index < state.pending_sockets.len() {
        let result = match state.pending_sockets[index].receiver.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Disconnected) => Some(Err("TLS worker stopped".to_owned())),
            Err(TryRecvError::Empty) => None,
        };
        if let Some(result) = result {
            let pending = state.pending_sockets.swap_remove(index);
            completed.push((pending.promise, result));
        } else {
            index += 1;
        }
    }
    completed
}

fn take_read_completions(state: &mut NodeTlsState) -> Vec<(u32, worker::ReadResult)> {
    let mut completed = Vec::new();
    let mut index = 0;
    while index < state.pending_reads.len() {
        let result = match state.pending_reads[index].receiver.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Disconnected) => Some(Err("TLS worker stopped".to_owned())),
            Err(TryRecvError::Empty) => None,
        };
        if let Some(result) = result {
            let pending = state.pending_reads.swap_remove(index);
            completed.push((pending.promise, result));
        } else {
            index += 1;
        }
    }
    completed
}

fn handle(state: &NativeAgentState, encoded: Option<i64>) -> Option<u32> {
    encoded
        .and_then(|encoded| to_number(state, encoded))?
        .to_u32()
}

fn text_arg(state: &NativeAgentState, args: &[i64], index: usize) -> Option<String> {
    args.get(index)
        .and_then(|encoded| state.string_owned(*encoded)).and_then(|text| text.to_utf8())
}

fn resolved(ctx: &mut NativeVmContext, state: &mut NativeAgentState, result: i64) -> i64 {
    super::promise::resolved_promise(ctx, state, result)
}

fn rejected(ctx: &mut NativeVmContext, state: &mut NativeAgentState, message: String) -> i64 {
    let error = super::modules::named_error_object(state, "Error", message)
        .unwrap_or_else(|| fail_dispatch(ctx));
    super::promise::rejected_promise(ctx, state, error)
}

fn settle_error(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    promise: u32,
    message: String,
) {
    let error = super::modules::named_error_object(state, "Error", message)
        .unwrap_or_else(|| fail_dispatch(ctx));
    super::promise::settle_promise(state, promise, error, true);
}

fn error_value(ctx: &mut NativeVmContext, state: &mut NativeAgentState, message: String) -> i64 {
    super::modules::named_error_object(state, "Error", message)
        .unwrap_or_else(|| fail_dispatch(ctx))
}
