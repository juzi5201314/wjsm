use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::net::UdpSocket;
use tokio::sync::Notify;
use wasmtime::{AsContextMut, Caller, Store};

use crate::runtime_buffer::visible_bytes;
use crate::runtime_encoding::js_string_lossy;
use crate::*;

/// UDP 数据报最大 payload（IPv4 上 65507 字节）。
const MAX_DATAGRAM_SIZE: usize = 65507;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DgramMethodKind {
    Bind,
    Send,
    Recv,
    Close,
    Address,
    Port,
}

impl DgramMethodKind {
    pub(crate) fn method(self) -> u8 {
        match self {
            Self::Bind => 0,
            Self::Send => 1,
            Self::Recv => 2,
            Self::Close => 3,
            Self::Address => 4,
            Self::Port => 5,
        }
    }

    pub(crate) fn from_method(method: u8) -> Option<Self> {
        match method {
            0 => Some(Self::Bind),
            1 => Some(Self::Send),
            2 => Some(Self::Recv),
            3 => Some(Self::Close),
            4 => Some(Self::Address),
            5 => Some(Self::Port),
            _ => None,
        }
    }
}

struct DatagramPacket {
    data: Vec<u8>,
    remote_addr: SocketAddr,
}

pub(crate) struct DgramSocketEntry {
    socket: Arc<UdpSocket>,
    local_addr: SocketAddr,
    closed: Arc<AtomicBool>,
    close_notify: Arc<Notify>,
}

pub(crate) fn create_dgram_host_object(caller: &mut Caller<'_, RuntimeState>) -> i64 {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let obj = alloc_host_object(caller, &env, 6);
    let temp_root_len = caller.data().push_host_temp_roots([obj]);
    install_dgram_method(caller, obj, "bind", DgramMethodKind::Bind);
    install_dgram_method(caller, obj, "send", DgramMethodKind::Send);
    install_dgram_method(caller, obj, "recv", DgramMethodKind::Recv);
    install_dgram_method(caller, obj, "close", DgramMethodKind::Close);
    install_dgram_method(caller, obj, "address", DgramMethodKind::Address);
    install_dgram_method(caller, obj, "port", DgramMethodKind::Port);
    caller.data().truncate_host_temp_roots(temp_root_len);
    obj
}

pub(crate) fn call_dgram_method(
    caller: &mut Caller<'_, RuntimeState>,
    kind: DgramMethodKind,
    args: &[i64],
) -> i64 {
    match kind {
        DgramMethodKind::Bind => bind(caller, args),
        DgramMethodKind::Send => send(caller, args),
        DgramMethodKind::Recv => recv(caller, args),
        DgramMethodKind::Close => close(caller, args),
        DgramMethodKind::Address => address(caller, args),
        DgramMethodKind::Port => port(caller, args),
    }
}

fn install_dgram_method(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
    name: &str,
    kind: DgramMethodKind,
) {
    let callable = create_native_callable(caller.data(), NativeCallable::DgramMethod { kind });
    let _ = define_host_data_property_from_caller(caller, obj, name, callable);
}

fn bind(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let promise = alloc_promise_from_caller(caller, PromiseEntry::pending());
    let port = match port_arg(caller, args.first().copied(), "bind") {
        Ok(port) => port,
        Err(message) => {
            reject_promise_from_caller(caller, promise, message);
            return promise;
        }
    };
    let host = match host_arg(caller, args.get(1).copied(), "bind") {
        Ok(host) => host,
        Err(message) => {
            reject_promise_from_caller(caller, promise, message);
            return promise;
        }
    };
    let address = format!("{host}:{port}");
    enqueue_async_result(
        caller,
        promise,
        async move {
            let socket = UdpSocket::bind(address)
                .await
                .map_err(|err| format!("bind {host}:{port} failed: {err}"))?;
            let local_addr = socket
                .local_addr()
                .map_err(|err| format!("bind local_addr failed: {err}"))?;
            tokio::task::yield_now().await;
            Ok(make_dgram_socket_entry(socket, local_addr))
        },
        |store, _env, result| match result {
            Ok(entry) => {
                let handle = store.data().dgram_socket_table.alloc(entry);
                PromiseSettlement::Fulfill(value::encode_f64(handle as f64))
            }
            Err(message) => PromiseSettlement::Reject(error_with_env(store, _env, message)),
        },
    );
    promise
}

fn make_dgram_socket_entry(
    socket: UdpSocket,
    local_addr: SocketAddr,
) -> DgramSocketEntry {
    DgramSocketEntry {
        socket: Arc::new(socket),
        local_addr,
        closed: Arc::new(AtomicBool::new(false)),
        close_notify: Arc::new(Notify::new()),
    }
}

fn send(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let Some(entry) = socket_entry(caller, args.first().copied()) else {
        return error_from_caller(caller, "dgram.Socket handle is invalid".to_string());
    };
    let data = match data_arg(caller, args.get(1).copied()) {
        Ok(data) => data,
        Err(message) => return error_from_caller(caller, message),
    };
    let port = match port_arg(caller, args.get(2).copied(), "send") {
        Ok(port) => port,
        Err(message) => return error_from_caller(caller, message),
    };
    let host = match host_arg(caller, args.get(3).copied(), "send") {
        Ok(host) => host,
        Err(message) => return error_from_caller(caller, message),
    };
    let target = format!("{host}:{port}");
    let socket = Arc::clone(&entry.socket);
    let result = tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async move {
            socket
                .send_to(&data, target)
                .await
                .map_err(|err| format!("send failed: {err}"))?;
            Ok::<(), String>(())
        })
    });
    match result {
        Ok(()) => value::encode_undefined(),
        Err(message) => error_from_caller(caller, message),
    }
}

fn recv(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let promise = alloc_promise_from_caller(caller, PromiseEntry::pending());
    let Some(entry) = socket_entry(caller, args.first().copied()) else {
        reject_promise_from_caller(caller, promise, "dgram.Socket handle is invalid".to_string());
        return promise;
    };
    let socket = Arc::clone(&entry.socket);
    let closed = Arc::clone(&entry.closed);
    let close_notify = Arc::clone(&entry.close_notify);
    enqueue_async_result(
        caller,
        promise,
        async move {
            if closed.load(Ordering::SeqCst) {
                return Ok(None);
            }
            let mut buffer = vec![0u8; MAX_DATAGRAM_SIZE];
            tokio::select! {
                result = socket.recv_from(&mut buffer) => {
                    let (len, remote_addr) = result.map_err(|err| format!("recv failed: {err}"))?;
                    buffer.truncate(len);
                    Ok(Some(DatagramPacket { data: buffer, remote_addr }))
                }
                _ = close_notify.notified() => Ok(None),
            }
        },
        |store, env, result| match result {
            Ok(Some(packet)) => {
                let obj = alloc_host_object(store, env, 3);
                let data_val = arraybuffer_with_bytes(store, env, &packet.data);
                let _ = define_host_data_property_with_env(store, env, obj, "data", data_val);
                let addr_val = crate::runtime_render::store_runtime_string_in_state(
                    store.data(), packet.remote_addr.ip().to_string(),
                );
                let _ = define_host_data_property_with_env(store, env, obj, "address", addr_val);
                let _ = define_host_data_property_with_env(
                    store, env, obj, "port",
                    value::encode_f64(f64::from(packet.remote_addr.port())),
                );
                PromiseSettlement::Fulfill(obj)
            }
            Ok(None) => PromiseSettlement::Fulfill(value::encode_null()),
            Err(message) => PromiseSettlement::Reject(error_with_env(store, env, message)),
        },
    );
    promise
}

fn close(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    if let Some(handle) = handle_arg(args.first().copied())
        && let Ok(mut table) = caller.data().dgram_socket_table.inner.lock()
        && let Some(Some(entry)) = table.entries.get_mut(handle as usize)
    {
        entry.closed.store(true, Ordering::SeqCst);
        entry.close_notify.notify_waiters();
    }
    value::encode_undefined()
}

fn address(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    socket_entry(caller, args.first().copied())
        .map(|entry| {
            crate::runtime_render::store_runtime_string(caller, entry.local_addr.ip().to_string())
        })
        .unwrap_or_else(value::encode_undefined)
}

fn port(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    socket_entry(caller, args.first().copied())
        .map(|entry| value::encode_f64(f64::from(entry.local_addr.port())))
        .unwrap_or_else(value::encode_undefined)
}

// ── helpers ────────────────────────────────────────────────────────

struct SocketSnapshot {
    socket: Arc<UdpSocket>,
    local_addr: SocketAddr,
    closed: Arc<AtomicBool>,
    close_notify: Arc<Notify>,
}

fn socket_entry(caller: &mut Caller<'_, RuntimeState>, value_raw: Option<i64>) -> Option<SocketSnapshot> {
    let handle = handle_arg(value_raw)?;
    let table = caller.data().dgram_socket_table.inner.lock().ok()?;
    let entry = table.get(handle as usize)?;
    Some(SocketSnapshot {
        socket: Arc::clone(&entry.socket),
        local_addr: entry.local_addr,
        closed: Arc::clone(&entry.closed),
        close_notify: Arc::clone(&entry.close_notify),
    })
}

fn handle_arg(value_raw: Option<i64>) -> Option<u32> {
    let value_raw = value_raw?;
    if value::is_f64(value_raw) {
        let number = value::decode_f64(value_raw);
        if number.is_finite() && number >= 0.0 && number <= f64::from(u32::MAX) {
            return Some(number as u32);
        }
    }
    None
}

fn port_arg(
    caller: &mut Caller<'_, RuntimeState>,
    value_raw: Option<i64>,
    syscall: &str,
) -> Result<u16, String> {
    let Some(value_raw) = value_raw else {
        return Err(format!("{syscall} requires a port"));
    };
    let port = if value::is_f64(value_raw) {
        value::decode_f64(value_raw)
    } else {
        js_string_lossy(caller, value_raw)
            .parse::<f64>()
            .unwrap_or(f64::NAN)
    };
    if !port.is_finite() || port < 0.0 || port > f64::from(u16::MAX) {
        return Err(format!("{syscall} port must be between 0 and 65535"));
    }
    Ok(port as u16)
}

fn host_arg(
    caller: &mut Caller<'_, RuntimeState>,
    value_raw: Option<i64>,
    syscall: &str,
) -> Result<String, String> {
    let host = value_raw
        .filter(|v| !value::is_undefined(*v) && !value::is_null(*v))
        .map(|v| js_string_lossy(caller, v))
        .filter(|host| !host.is_empty())
        .unwrap_or_else(|| "127.0.0.1".to_string());
    if matches!(host.as_str(), "127.0.0.1" | "localhost" | "::1") {
        Ok(host)
    } else {
        Err(format!(
            "{syscall} host '{host}' is not allowed by wjsm network sandbox"
        ))
    }
}

fn data_arg(caller: &mut Caller<'_, RuntimeState>, value_raw: Option<i64>) -> Result<Vec<u8>, String> {
    let value_raw = value_raw.unwrap_or_else(value::encode_undefined);
    if value::is_undefined(value_raw) || value::is_null(value_raw) {
        return Ok(Vec::new());
    }
    if let Some(bytes) = visible_bytes(caller, value_raw) {
        return Ok(bytes);
    }
    Ok(js_string_lossy(caller, value_raw).into_bytes())
}

fn enqueue_async_result<T, Fut, Materialize>(
    caller: &mut Caller<'_, RuntimeState>,
    promise: i64,
    future: Fut,
    materialize: Materialize,
) where
    T: Send + 'static,
    Fut: Future<Output = Result<T, String>> + Send + 'static,
    Materialize: FnOnce(&mut Store<RuntimeState>, &WasmEnv, Result<T, String>) -> PromiseSettlement
        + Send
        + 'static,
{
    let Some(tx) = caller.data().host_completion_tx.clone() else {
        reject_promise_from_caller(caller, promise, "async network runtime is not available".to_string());
        return;
    };
    let Some(counter) = caller.data().async_op_counter.clone() else {
        reject_promise_from_caller(caller, promise, "async network runtime is not available".to_string());
        return;
    };
    let guard = counter.begin();
    tokio::spawn(async move {
        let result = future.await;
        let _ = tx.send(crate::scheduler::AsyncHostCompletion::Materialize {
            promise,
            materialize: Box::new(move |store, env| materialize(store, env, result)),
        });
        drop(guard);
    });
}

fn reject_promise_from_caller(caller: &mut Caller<'_, RuntimeState>, promise: i64, message: String) {
    let msg_val = crate::runtime_render::store_runtime_string(caller, message.clone());
    let error = create_error_object(caller, "Error", msg_val, value::encode_undefined());
    settle_promise(caller.data(), promise, PromiseSettlement::Reject(error));
}

fn error_with_env<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    message: String,
) -> i64 {
    crate::runtime_heap::alloc_error_object_with_env(ctx, env, "Error", message, None)
}

fn error_from_caller(caller: &mut Caller<'_, RuntimeState>, message: String) -> i64 {
    let msg_val = crate::runtime_render::store_runtime_string(caller, message);
    create_error_object(caller, "Error", msg_val, value::encode_undefined())
}

fn arraybuffer_with_bytes<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    bytes: &[u8],
) -> i64 {
    let ab = alloc_host_object(ctx, env, 1);
    let handle = {
        let mut ctx_mut = ctx.as_context_mut();
        let mut table = ctx_mut
            .data_mut()
            .arraybuffer_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let handle = table.len() as u32;
        table.push(ArrayBufferEntry {
            data: bytes.to_vec(),
        });
        handle
    };
    let _ = define_host_data_property_with_env(
        ctx,
        env,
        ab,
        "__arraybuffer_handle__",
        value::encode_f64(handle as f64),
    );
    let _ = define_host_data_property_with_env(
        ctx,
        env,
        ab,
        "byteLength",
        value::encode_f64(bytes.len() as f64),
    );
    ab
}
