use std::collections::HashMap;
use std::io::ErrorKind;
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::time::Duration;

use mio::{Events, Interest, Poll, Token};
use wjsm_ir::value;
use wjsm_native_abi::NativeVmContext;

use super::{fail_dispatch, modules, node_buffer, promise, runtime::to_number};
use crate::{NativeAgentState, NativeCallableKind};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum NodeDgramMethod {
    Address,
    Bind,
    Close,
    Port,
    Recv,
    Send,
}

#[derive(Default)]
pub(crate) struct NodeDgramState {
    bridge: Option<i64>,
    poll: Option<Poll>,
    sockets: HashMap<u32, mio::net::UdpSocket>,
    pending_receives: HashMap<u32, u32>,
    next_handle: u32,
}

impl NodeDgramState {
    fn allocate_handle(&mut self) -> Option<u32> {
        let handle = self.next_handle;
        self.next_handle = self.next_handle.checked_add(1)?;
        Some(handle)
    }

    fn poll(&mut self) -> std::io::Result<&mut Poll> {
        if self.poll.is_none() {
            self.poll = Some(Poll::new()?);
        }
        Ok(self.poll.as_mut().expect("poll was initialized"))
    }
}

pub(crate) fn ensure_bridge(state: &mut NativeAgentState) -> Option<i64> {
    if let Some(bridge) = state.node_dgram.bridge {
        return Some(bridge);
    }
    let methods = [
        ("address", NodeDgramMethod::Address),
        ("bind", NodeDgramMethod::Bind),
        ("close", NodeDgramMethod::Close),
        ("port", NodeDgramMethod::Port),
        ("recv", NodeDgramMethod::Recv),
        ("send", NodeDgramMethod::Send),
    ];
    let bridge = state.allocate_object(methods.len() as u32, false).ok()?;
    for (name, method) in methods {
        let callable = state.native_callable(NativeCallableKind::NodeDgram(method))?;
        modules::set_named_property(state, bridge, name, callable).ok()?;
    }
    state.node_dgram.bridge = Some(bridge);
    Some(bridge)
}

pub(crate) fn call(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    method: NodeDgramMethod,
    args: &[i64],
) -> i64 {
    match method {
        NodeDgramMethod::Address => socket_address(ctx, state, args, false),
        NodeDgramMethod::Bind => bind(ctx, state, args),
        NodeDgramMethod::Close => close(ctx, state, args),
        NodeDgramMethod::Port => socket_address(ctx, state, args, true),
        NodeDgramMethod::Recv => receive(ctx, state, args),
        NodeDgramMethod::Send => send(ctx, state, args),
    }
}

pub(crate) fn has_pending(state: &NativeAgentState) -> bool {
    !state.node_dgram.pending_receives.is_empty()
}

pub(crate) fn poll_pending(ctx: &mut NativeVmContext, state: &mut NativeAgentState) -> i64 {
    let completions = match poll_completions(&mut state.node_dgram) {
        Ok(completions) => completions,
        Err(error) => {
            let pending = state
                .node_dgram
                .pending_receives
                .drain()
                .map(|(_, promise)| promise)
                .collect::<Vec<_>>();
            for promise in pending {
                settle_error(ctx, state, promise, error.to_string());
            }
            return value::encode_undefined();
        }
    };
    for completion in completions {
        match completion.result {
            Ok((bytes, address)) => {
                let Some(result) = receive_object(state, bytes, address) else {
                    return fail_dispatch(ctx);
                };
                promise::settle_promise(state, completion.promise, result, false);
            }
            Err(message) => settle_error(ctx, state, completion.promise, message),
        }
    }
    value::encode_undefined()
}

fn bind(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let port = args
        .first()
        .and_then(|port| to_number(state, *port))
        .filter(|port| port.is_finite() && *port >= 0.0 && *port <= f64::from(u16::MAX))
        .map_or(0, |port| port as u16);
    let host = args
        .get(1)
        .and_then(|host| state.string(*host))
        .and_then(|host| host.to_utf8())
        .unwrap_or_else(|| "127.0.0.1".into());
    let result = resolve_address(&host, port).and_then(|address| {
        let socket = UdpSocket::bind(address)?;
        socket.set_nonblocking(true)?;
        Ok(mio::net::UdpSocket::from_std(socket))
    });
    let mut socket = match result {
        Ok(socket) => socket,
        Err(error) => return rejected(ctx, state, error.to_string()),
    };
    let Some(handle) = state.node_dgram.allocate_handle() else {
        return fail_dispatch(ctx);
    };
    let register = state.node_dgram.poll().and_then(|poll| {
        poll.registry()
            .register(&mut socket, token(handle), Interest::READABLE)
    });
    if let Err(error) = register {
        return rejected(ctx, state, error.to_string());
    }
    state.node_dgram.sockets.insert(handle, socket);
    resolved(ctx, state, value::encode_f64(f64::from(handle)))
}

fn receive(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(handle) = handle(state, args.first().copied()) else {
        return rejected(ctx, state, "Invalid UDP handle".into());
    };
    if let Some(promise) = state.node_dgram.pending_receives.get(&handle).copied() {
        return value::encode_handle(value::TAG_OBJECT, promise);
    }
    match try_receive(state.node_dgram.sockets.get_mut(&handle)) {
        Some(Ok((bytes, address))) => {
            let Some(result) = receive_object(state, bytes, address) else {
                return fail_dispatch(ctx);
            };
            resolved(ctx, state, result)
        }
        Some(Err(error)) if error.kind() == ErrorKind::WouldBlock => {
            let Some(promise) = promise::new_promise(ctx, state) else {
                return fail_dispatch(ctx);
            };
            state
                .node_dgram
                .pending_receives
                .insert(handle, value::decode_handle(promise));
            promise
        }
        Some(Err(error)) => rejected(ctx, state, error.to_string()),
        None => rejected(ctx, state, "Invalid UDP handle".into()),
    }
}

fn send(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(handle) = handle(state, args.first().copied()) else {
        return type_error(ctx, state, "Invalid UDP handle");
    };
    let Some(bytes) = args
        .get(1)
        .and_then(|input| node_buffer::bytes(state, *input))
    else {
        return type_error(ctx, state, "UDP payload must be a Buffer");
    };
    let Some(port) = args
        .get(2)
        .and_then(|port| to_number(state, *port))
        .filter(|port| port.is_finite() && *port >= 0.0 && *port <= f64::from(u16::MAX))
        .map(|port| port as u16)
    else {
        return type_error(ctx, state, "Invalid UDP port");
    };
    let host = args
        .get(3)
        .and_then(|host| state.string(*host))
        .and_then(|host| host.to_utf8())
        .unwrap_or_else(|| "127.0.0.1".into());
    let result = resolve_address(&host, port).and_then(|address| {
        state
            .node_dgram
            .sockets
            .get(&handle)
            .ok_or_else(|| std::io::Error::new(ErrorKind::NotFound, "Invalid UDP handle"))?
            .send_to(&bytes, address)
            .map(|_| ())
    });
    match result {
        Ok(()) => value::encode_undefined(),
        Err(error) => error_value(ctx, state, error.to_string()),
    }
}

fn close(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(handle) = handle(state, args.first().copied()) else {
        return type_error(ctx, state, "Invalid UDP handle");
    };
    if let Some(mut socket) = state.node_dgram.sockets.remove(&handle)
        && let Some(poll) = state.node_dgram.poll.as_ref()
    {
        let _ = poll.registry().deregister(&mut socket);
    }
    if let Some(promise) = state.node_dgram.pending_receives.remove(&handle) {
        promise::settle_promise(state, promise, value::encode_null(), false);
    }
    value::encode_undefined()
}

fn socket_address(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
    port: bool,
) -> i64 {
    let Some(handle) = handle(state, args.first().copied()) else {
        return type_error(ctx, state, "Invalid UDP handle");
    };
    let Some(address) = state
        .node_dgram
        .sockets
        .get(&handle)
        .and_then(|socket| socket.local_addr().ok())
    else {
        return type_error(ctx, state, "Invalid UDP handle");
    };
    if port {
        value::encode_f64(f64::from(address.port()))
    } else {
        state
            .intern_text(address.ip().to_string(), value::TAG_STRING)
            .unwrap_or_else(|| fail_dispatch(ctx))
    }
}

struct Completion {
    promise: u32,
    result: Result<(Vec<u8>, SocketAddr), String>,
}

fn poll_completions(state: &mut NodeDgramState) -> std::io::Result<Vec<Completion>> {
    let mut events = Events::with_capacity(state.pending_receives.len().max(1));
    state.poll()?.poll(&mut events, Some(Duration::ZERO))?;
    let mut completions = Vec::new();
    for event in &events {
        if !event.is_readable() {
            continue;
        }
        let Ok(handle) = u32::try_from(event.token().0) else {
            continue;
        };
        let Some(promise) = state.pending_receives.get(&handle).copied() else {
            continue;
        };
        match try_receive(state.sockets.get_mut(&handle)) {
            Some(Ok(received)) => {
                state.pending_receives.remove(&handle);
                completions.push(Completion {
                    promise,
                    result: Ok(received),
                });
            }
            Some(Err(error)) if error.kind() == ErrorKind::WouldBlock => {}
            Some(Err(error)) => {
                state.pending_receives.remove(&handle);
                completions.push(Completion {
                    promise,
                    result: Err(error.to_string()),
                });
            }
            None => {
                state.pending_receives.remove(&handle);
                completions.push(Completion {
                    promise,
                    result: Err("Invalid UDP handle".into()),
                });
            }
        }
    }
    Ok(completions)
}

fn try_receive(
    socket: Option<&mut mio::net::UdpSocket>,
) -> Option<std::io::Result<(Vec<u8>, SocketAddr)>> {
    let socket = socket?;
    let mut bytes = vec![0; 65_536];
    Some(socket.recv_from(&mut bytes).map(|(length, address)| {
        bytes.truncate(length);
        (bytes, address)
    }))
}

fn receive_object(
    state: &mut NativeAgentState,
    bytes: Vec<u8>,
    address: SocketAddr,
) -> Option<i64> {
    let object = state.allocate_object(3, false).ok()?;
    let data = node_buffer::from_bytes(state, bytes)?;
    let address_text = state.intern_text(address.ip().to_string(), value::TAG_STRING)?;
    for (name, stored) in [
        ("data", data),
        ("address", address_text),
        ("port", value::encode_f64(f64::from(address.port()))),
    ] {
        modules::set_named_property(state, object, name, stored).ok()?;
    }
    Some(object)
}

fn resolve_address(host: &str, port: u16) -> std::io::Result<SocketAddr> {
    (host, port)
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| std::io::Error::new(ErrorKind::AddrNotAvailable, "Address not found"))
}

fn token(handle: u32) -> Token {
    Token(handle as usize)
}

fn handle(state: &NativeAgentState, encoded: Option<i64>) -> Option<u32> {
    encoded
        .and_then(|encoded| to_number(state, encoded))
        .filter(|number| number.is_finite() && *number >= 0.0 && number.fract() == 0.0)
        .map(|number| number as u32)
}

fn resolved(ctx: &mut NativeVmContext, state: &mut NativeAgentState, result: i64) -> i64 {
    let Some(promise) = promise::new_promise(ctx, state) else {
        return fail_dispatch(ctx);
    };
    promise::settle_promise(state, value::decode_handle(promise), result, false);
    promise
}

fn rejected(ctx: &mut NativeVmContext, state: &mut NativeAgentState, message: String) -> i64 {
    let error = error_value(ctx, state, message);
    let Some(promise) = promise::new_promise(ctx, state) else {
        return fail_dispatch(ctx);
    };
    promise::settle_promise(state, value::decode_handle(promise), error, true);
    promise
}

fn settle_error(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    promise: u32,
    message: String,
) {
    let error = error_value(ctx, state, message);
    promise::settle_promise(state, promise, error, true);
}

fn error_value(ctx: &mut NativeVmContext, state: &mut NativeAgentState, message: String) -> i64 {
    modules::named_error_object(state, "Error", message).unwrap_or_else(|| fail_dispatch(ctx))
}

fn type_error(ctx: &mut NativeVmContext, state: &mut NativeAgentState, message: &str) -> i64 {
    modules::named_error_object(state, "TypeError", message.into())
        .and_then(|error| state.create_exception(error))
        .unwrap_or_else(|| fail_dispatch(ctx))
}
