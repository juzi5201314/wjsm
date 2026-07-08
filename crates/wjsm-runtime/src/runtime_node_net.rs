use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex as AsyncMutex, Notify, mpsc};
use wasmtime::{AsContextMut, Caller, Store};

use crate::runtime_buffer::visible_bytes;
use crate::runtime_encoding::js_string_lossy;
use crate::*;

const READ_CHUNK_LIMIT: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum NetMethodKind {
    Connect,
    Read,
    Write,
    End,
    Destroy,
    ServerListen,
    ServerAccept,
    ServerClose,
    ServerPort,
    ServerAddress,
    SocketLocalPort,
    SocketLocalAddress,
    SocketRemotePort,
    SocketRemoteAddress,
}

impl NetMethodKind {
    pub(crate) fn method(self) -> u8 {
        match self {
            Self::Connect => 0,
            Self::Read => 1,
            Self::Write => 2,
            Self::End => 3,
            Self::Destroy => 4,
            Self::ServerListen => 5,
            Self::ServerAccept => 6,
            Self::ServerClose => 7,
            Self::ServerPort => 8,
            Self::ServerAddress => 9,
            Self::SocketLocalPort => 10,
            Self::SocketLocalAddress => 11,
            Self::SocketRemotePort => 12,
            Self::SocketRemoteAddress => 13,
        }
    }

    pub(crate) fn from_method(method: u8) -> Option<Self> {
        match method {
            0 => Some(Self::Connect),
            1 => Some(Self::Read),
            2 => Some(Self::Write),
            3 => Some(Self::End),
            4 => Some(Self::Destroy),
            5 => Some(Self::ServerListen),
            6 => Some(Self::ServerAccept),
            7 => Some(Self::ServerClose),
            8 => Some(Self::ServerPort),
            9 => Some(Self::ServerAddress),
            10 => Some(Self::SocketLocalPort),
            11 => Some(Self::SocketLocalAddress),
            12 => Some(Self::SocketRemotePort),
            13 => Some(Self::SocketRemoteAddress),
            _ => None,
        }
    }
}

pub(crate) struct NetSocketEntry {
    reader: Arc<AsyncMutex<Option<OwnedReadHalf>>>,
    writer: Arc<AsyncMutex<Option<OwnedWriteHalf>>>,
    local_addr: SocketAddr,
    peer_addr: SocketAddr,
    close_notify: Arc<Notify>,
}

pub(crate) struct NetServerEntry {
    accept_rx: Arc<AsyncMutex<mpsc::UnboundedReceiver<AcceptedTcpStream>>>,
    accept_task: Option<tokio::task::JoinHandle<()>>,
    local_addr: SocketAddr,
    closed: Arc<AtomicBool>,
    close_notify: Arc<Notify>,
}

struct AcceptedTcpStream {
    stream: TcpStream,
    local_addr: SocketAddr,
    peer_addr: SocketAddr,
}

pub(crate) fn create_net_host_object(caller: &mut Caller<'_, RuntimeState>) -> i64 {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let obj = alloc_host_object(caller, &env, 14);
    let temp_root_len = caller.data().push_host_temp_roots([obj]);
    install_net_method(caller, obj, "connect", NetMethodKind::Connect);
    install_net_method(caller, obj, "read", NetMethodKind::Read);
    install_net_method(caller, obj, "write", NetMethodKind::Write);
    install_net_method(caller, obj, "end", NetMethodKind::End);
    install_net_method(caller, obj, "destroy", NetMethodKind::Destroy);
    install_net_method(caller, obj, "serverListen", NetMethodKind::ServerListen);
    install_net_method(caller, obj, "serverAccept", NetMethodKind::ServerAccept);
    install_net_method(caller, obj, "serverClose", NetMethodKind::ServerClose);
    install_net_method(caller, obj, "serverPort", NetMethodKind::ServerPort);
    install_net_method(caller, obj, "serverAddress", NetMethodKind::ServerAddress);
    install_net_method(caller, obj, "socketLocalPort", NetMethodKind::SocketLocalPort);
    install_net_method(caller, obj, "socketLocalAddress", NetMethodKind::SocketLocalAddress);
    install_net_method(caller, obj, "socketRemotePort", NetMethodKind::SocketRemotePort);
    install_net_method(caller, obj, "socketRemoteAddress", NetMethodKind::SocketRemoteAddress);
    caller.data().truncate_host_temp_roots(temp_root_len);
    obj
}

pub(crate) fn call_net_method(
    caller: &mut Caller<'_, RuntimeState>,
    kind: NetMethodKind,
    args: &[i64],
) -> i64 {
    match kind {
        NetMethodKind::Connect => connect(caller, args),
        NetMethodKind::Read => read(caller, args),
        NetMethodKind::Write => write(caller, args),
        NetMethodKind::End => end(caller, args),
        NetMethodKind::Destroy => destroy(caller, args),
        NetMethodKind::ServerListen => server_listen(caller, args),
        NetMethodKind::ServerAccept => server_accept(caller, args),
        NetMethodKind::ServerClose => server_close(caller, args),
        NetMethodKind::ServerPort => server_port(caller, args),
        NetMethodKind::ServerAddress => server_address(caller, args),
        NetMethodKind::SocketLocalPort => socket_addr_number(caller, args, false, true),
        NetMethodKind::SocketLocalAddress => socket_addr_string(caller, args, false),
        NetMethodKind::SocketRemotePort => socket_addr_number(caller, args, true, true),
        NetMethodKind::SocketRemoteAddress => socket_addr_string(caller, args, true),
    }
}

pub(crate) fn alloc_socket_entry_from_stream(
    state: &RuntimeState,
    stream: TcpStream,
    local_addr: SocketAddr,
    peer_addr: SocketAddr,
) -> u32 {
    let (reader, writer) = stream.into_split();
    state.net_socket_table.alloc(NetSocketEntry {
        reader: Arc::new(AsyncMutex::new(Some(reader))),
        writer: Arc::new(AsyncMutex::new(Some(writer))),
        local_addr,
        peer_addr,
        close_notify: Arc::new(Notify::new()),
    })
}

fn install_net_method(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
    name: &str,
    kind: NetMethodKind,
) {
    let callable = create_native_callable(caller.data(), NativeCallable::NetMethod { kind });
    let _ = define_host_data_property_from_caller(caller, obj, name, callable);
}

fn connect(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let promise = alloc_promise_from_caller(caller, PromiseEntry::pending());
    let port = match port_arg(caller, args.first().copied(), "connect") {
        Ok(port) => port,
        Err(message) => {
            reject_promise_from_caller(caller, promise, message);
            return promise;
        }
    };
    let host = match host_arg(caller, args.get(1).copied(), "connect") {
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
            let stream = TcpStream::connect(address)
                .await
                .map_err(|err| format!("connect {host}:{port} failed: {err}"))?;
            let local_addr = stream
                .local_addr()
                .map_err(|err| format!("connect local_addr failed: {err}"))?;
            let peer_addr = stream
                .peer_addr()
                .map_err(|err| format!("connect peer_addr failed: {err}"))?;
            Ok((stream, local_addr, peer_addr))
        },
        |store, _env, result| match result {
            Ok((stream, local_addr, peer_addr)) => {
                let handle = alloc_socket_entry_from_stream(store.data(), stream, local_addr, peer_addr);
                PromiseSettlement::Fulfill(value::encode_f64(handle as f64))
            }
            Err(message) => PromiseSettlement::Reject(error_with_env(store, _env, message)),
        },
    );
    promise
}

fn read(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let promise = alloc_promise_from_caller(caller, PromiseEntry::pending());
    let Some(entry) = socket_entry(caller, args.first().copied()) else {
        reject_promise_from_caller(caller, promise, "net.Socket handle is invalid".to_string());
        return promise;
    };
    let reader = Arc::clone(&entry.reader);
    let close_notify = Arc::clone(&entry.close_notify);
    enqueue_async_result(
        caller,
        promise,
        async move {
            let mut guard = reader.lock().await;
            let Some(reader) = guard.as_mut() else {
                return Ok(None);
            };
            let mut buffer = vec![0; READ_CHUNK_LIMIT];
            tokio::select! {
                result = reader.read(&mut buffer) => {
                    let read_len = result.map_err(|err| format!("socket read failed: {err}"))?;
                    if read_len == 0 {
                        *guard = None;
                        Ok(None)
                    } else {
                        buffer.truncate(read_len);
                        Ok(Some(buffer))
                    }
                }
                _ = close_notify.notified() => Ok(None),
            }
        },
        |store, env, result| match result {
            Ok(Some(bytes)) => PromiseSettlement::Fulfill(arraybuffer_with_bytes(store, env, &bytes)),
            Ok(None) => PromiseSettlement::Fulfill(value::encode_null()),
            Err(message) => PromiseSettlement::Reject(error_with_env(store, env, message)),
        },
    );
    promise
}

fn write(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let Some(entry) = socket_entry(caller, args.first().copied()) else {
        return error_from_caller(caller, "net.Socket handle is invalid".to_string());
    };
    let data = match data_arg(caller, args.get(1).copied()) {
        Ok(data) => data,
        Err(message) => return error_from_caller(caller, message),
    };
    let writer = Arc::clone(&entry.writer);
    let result = tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async move {
            let mut guard = writer.lock().await;
            let Some(writer) = guard.as_mut() else {
                return Err("socket is closed".to_string());
            };
            writer
                .write_all(&data)
                .await
                .map_err(|err| format!("socket write failed: {err}"))?;
            writer
                .flush()
                .await
                .map_err(|err| format!("socket flush failed: {err}"))?;
            Ok(())
        })
    });
    match result {
        Ok(()) => value::encode_undefined(),
        Err(message) => error_from_caller(caller, message),
    }
}

fn end(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let Some(entry) = socket_entry(caller, args.first().copied()) else {
        return error_from_caller(caller, "net.Socket handle is invalid".to_string());
    };
    entry.close_notify.notify_waiters();
    let writer = Arc::clone(&entry.writer);
    let result = tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async move {
            let mut guard = writer.lock().await;
            if let Some(writer) = guard.as_mut() {
                writer
                    .shutdown()
                    .await
                    .map_err(|err| format!("socket shutdown failed: {err}"))?;
            }
            *guard = None;
            Ok(())
        })
    });
    match result {
        Ok(()) => value::encode_undefined(),
        Err(message) => error_from_caller(caller, message),
    }
}

fn destroy(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    if let Some(handle) = handle_arg(args.first().copied())
        && let Ok(mut table) = caller.data().net_socket_table.inner.lock()
        && let Some(Some(entry)) = table.entries.get_mut(handle as usize)
    {
        if let Ok(mut reader) = entry.reader.try_lock() {
            *reader = None;
        }
        entry.close_notify.notify_waiters();
        if let Ok(mut writer) = entry.writer.try_lock() {
            *writer = None;
        }
    }
    value::encode_undefined()
}

fn server_listen(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let promise = alloc_promise_from_caller(caller, PromiseEntry::pending());
    let port = match port_arg(caller, args.first().copied(), "listen") {
        Ok(port) => port,
        Err(message) => {
            reject_promise_from_caller(caller, promise, message);
            return promise;
        }
    };
    let host = match host_arg(caller, args.get(1).copied(), "listen") {
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
            let listener = TcpListener::bind(address)
                .await
                .map_err(|err| format!("listen {host}:{port} failed: {err}"))?;
            let local_addr = listener
                .local_addr()
                .map_err(|err| format!("listen local_addr failed: {err}"))?;
            let (accept_tx, accept_rx) = mpsc::unbounded_channel();
            let closed = Arc::new(AtomicBool::new(false));
            let task_closed = Arc::clone(&closed);
            let close_notify = Arc::new(Notify::new());
            let accept_task = tokio::spawn(async move {
                loop {
                    if task_closed.load(Ordering::SeqCst) {
                        break;
                    }
                    match listener.accept().await {
                        Ok((stream, peer_addr)) => {
                            let local_addr = stream.local_addr().unwrap_or(local_addr);
                            if accept_tx
                                .send(AcceptedTcpStream {
                                    stream,
                                    local_addr,
                                    peer_addr,
                                })
                                .is_err()
                            {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
            });
            Ok(NetServerEntry {
                accept_rx: Arc::new(AsyncMutex::new(accept_rx)),
                accept_task: Some(accept_task),
                local_addr,
                closed,
                close_notify,
            })
        },
        |store, _env, result| match result {
            Ok(entry) => {
                let handle = store.data().net_server_table.alloc(entry);
                PromiseSettlement::Fulfill(value::encode_f64(handle as f64))
            }
            Err(message) => PromiseSettlement::Reject(error_with_env(store, _env, message)),
        },
    );
    promise
}

fn server_accept(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let promise = alloc_promise_from_caller(caller, PromiseEntry::pending());
    let Some(entry) = server_entry(caller, args.first().copied()) else {
        reject_promise_from_caller(caller, promise, "net.Server handle is invalid".to_string());
        return promise;
    };
    let accept_rx = Arc::clone(&entry.accept_rx);
    let closed = Arc::clone(&entry.closed);
    let close_notify = Arc::clone(&entry.close_notify);
    enqueue_async_result(
        caller,
        promise,
        async move {
            if closed.load(Ordering::SeqCst) {
                return Ok(None);
            }
            let mut rx = accept_rx.lock().await;
            tokio::select! {
                accepted = rx.recv() => Ok(accepted),
                _ = close_notify.notified() => Ok(None),
            }
        },
        |store, _env, result| match result {
            Ok(Some(accepted)) => {
                let handle = alloc_socket_entry_from_stream(
                    store.data(),
                    accepted.stream,
                    accepted.local_addr,
                    accepted.peer_addr,
                );
                PromiseSettlement::Fulfill(value::encode_f64(handle as f64))
            }
            Ok(None) => PromiseSettlement::Fulfill(value::encode_null()),
            Err(message) => PromiseSettlement::Reject(error_with_env(store, _env, message)),
        },
    );
    promise
}

fn server_close(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let promise = alloc_promise_from_caller(caller, PromiseEntry::pending());
    let handle = match handle_arg(args.first().copied()) {
        Some(handle) => handle,
        None => {
            reject_promise_from_caller(caller, promise, "net.Server handle is invalid".to_string());
            return promise;
        }
    };
    let entry = {
        let mut table = caller
            .data()
            .net_server_table
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        table
            .entries
            .get_mut(handle as usize)
            .and_then(Option::take)
    };
    if let Some(mut entry) = entry {
        entry.closed.store(true, Ordering::SeqCst);
        entry.close_notify.notify_waiters();
        if let Some(task) = entry.accept_task.take() {
            task.abort();
        }
    }
    settle_promise(
        caller.data(),
        promise,
        PromiseSettlement::Fulfill(value::encode_undefined()),
    );
    promise
}

fn server_port(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    server_entry(caller, args.first().copied())
        .map(|entry| value::encode_f64(f64::from(entry.local_addr.port())))
        .unwrap_or_else(value::encode_undefined)
}

fn server_address(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    server_entry(caller, args.first().copied())
        .map(|entry| crate::runtime_render::store_runtime_string(caller, entry.local_addr.ip().to_string()))
        .unwrap_or_else(value::encode_undefined)
}

fn socket_addr_number(
    caller: &mut Caller<'_, RuntimeState>,
    args: &[i64],
    peer: bool,
    port: bool,
) -> i64 {
    let Some(entry) = socket_entry(caller, args.first().copied()) else {
        return value::encode_undefined();
    };
    let addr = if peer { entry.peer_addr } else { entry.local_addr };
    if port {
        value::encode_f64(f64::from(addr.port()))
    } else {
        value::encode_undefined()
    }
}

fn socket_addr_string(caller: &mut Caller<'_, RuntimeState>, args: &[i64], peer: bool) -> i64 {
    let Some(entry) = socket_entry(caller, args.first().copied()) else {
        return value::encode_undefined();
    };
    let addr = if peer { entry.peer_addr } else { entry.local_addr };
    crate::runtime_render::store_runtime_string(caller, addr.ip().to_string())
}

fn socket_entry(caller: &mut Caller<'_, RuntimeState>, value_raw: Option<i64>) -> Option<SocketSnapshot> {
    let handle = handle_arg(value_raw)?;
    let table = caller.data().net_socket_table.inner.lock().ok()?;
    let entry = table.get(handle as usize)?;
    Some(SocketSnapshot {
        reader: Arc::clone(&entry.reader),
        writer: Arc::clone(&entry.writer),
        local_addr: entry.local_addr,
        peer_addr: entry.peer_addr,
        close_notify: Arc::clone(&entry.close_notify),
    })
}

fn server_entry(caller: &mut Caller<'_, RuntimeState>, value_raw: Option<i64>) -> Option<ServerSnapshot> {
    let handle = handle_arg(value_raw)?;
    let table = caller.data().net_server_table.inner.lock().ok()?;
    let entry = table.get(handle as usize)?;
    Some(ServerSnapshot {
        accept_rx: Arc::clone(&entry.accept_rx),
        local_addr: entry.local_addr,
        closed: Arc::clone(&entry.closed),
        close_notify: Arc::clone(&entry.close_notify),
    })
}

struct SocketSnapshot {
    reader: Arc<AsyncMutex<Option<OwnedReadHalf>>>,
    writer: Arc<AsyncMutex<Option<OwnedWriteHalf>>>,
    local_addr: SocketAddr,
    peer_addr: SocketAddr,
    close_notify: Arc<Notify>,
}

struct ServerSnapshot {
    accept_rx: Arc<AsyncMutex<mpsc::UnboundedReceiver<AcceptedTcpStream>>>,
    local_addr: SocketAddr,
    closed: Arc<AtomicBool>,
    close_notify: Arc<Notify>,
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
