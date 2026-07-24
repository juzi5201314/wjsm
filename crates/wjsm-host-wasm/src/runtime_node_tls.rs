use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName};
use rustls::{ClientConfig, DigitallySignedStruct, RootCertStore, ServerConfig, SignatureScheme};
use tokio::io::{AsyncReadExt, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex as AsyncMutex, Notify, mpsc};
use tokio_rustls::client::TlsStream as ClientTlsStream;
use tokio_rustls::server::TlsStream as ServerTlsStream;
use tokio_rustls::{TlsAcceptor, TlsConnector, TlsStream};
use wasmtime::{AsContextMut, Caller, Store};

use crate::runtime_buffer::visible_bytes;
use crate::runtime_encoding::js_string_lossy;
use crate::*;

const READ_CHUNK_LIMIT: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TlsMethodKind {
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

impl TlsMethodKind {
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

// TLS 流的读/写半部类型（客户端和服务端共用）
type TlsReadHalf = ReadHalf<TlsStream<TcpStream>>;
type TlsWriteHalf = WriteHalf<TlsStream<TcpStream>>;

#[allow(dead_code)]
pub(crate) struct TlsSocketEntry {
    reader: Arc<AsyncMutex<Option<TlsReadHalf>>>,
    writer: Arc<AsyncMutex<Option<TlsWriteHalf>>>,
    local_addr: SocketAddr,
    peer_addr: SocketAddr,
    close_notify: Arc<Notify>,
    alpn_protocol: Option<String>,
}

pub(crate) struct TlsServerEntry {
    accept_rx: Arc<AsyncMutex<mpsc::UnboundedReceiver<AcceptedTlsStream>>>,
    accept_task: Option<tokio::task::JoinHandle<()>>,
    local_addr: SocketAddr,
    closed: Arc<AtomicBool>,
    close_notify: Arc<Notify>,
}

struct AcceptedTlsStream {
    stream: TlsStream<TcpStream>,
    local_addr: SocketAddr,
    peer_addr: SocketAddr,
    alpn_protocol: Option<String>,
}

// ── 危险的证书验证器（rejectUnauthorized: false） ──────────────────

#[derive(Debug)]
struct NoCertificateVerification;

impl ServerCertVerifier for NoCertificateVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![
            SignatureScheme::RSA_PKCS1_SHA256,
            SignatureScheme::ECDSA_NISTP256_SHA256,
            SignatureScheme::RSA_PSS_SHA256,
            SignatureScheme::RSA_PKCS1_SHA384,
            SignatureScheme::ECDSA_NISTP384_SHA384,
            SignatureScheme::RSA_PSS_SHA384,
            SignatureScheme::RSA_PKCS1_SHA512,
            SignatureScheme::RSA_PSS_SHA512,
            SignatureScheme::ED25519,
        ]
    }
}

// ── host object 创建与 dispatch ─────────────────────────────────────

pub(crate) fn create_tls_host_object(caller: &mut Caller<'_, RuntimeState>) -> i64 {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let obj = alloc_host_object(caller, &env, 14);
    let temp_root_len = caller.data().push_host_temp_roots([obj]);
    install_tls_method(caller, obj, "connect", TlsMethodKind::Connect);
    install_tls_method(caller, obj, "read", TlsMethodKind::Read);
    install_tls_method(caller, obj, "write", TlsMethodKind::Write);
    install_tls_method(caller, obj, "end", TlsMethodKind::End);
    install_tls_method(caller, obj, "destroy", TlsMethodKind::Destroy);
    install_tls_method(caller, obj, "serverListen", TlsMethodKind::ServerListen);
    install_tls_method(caller, obj, "serverAccept", TlsMethodKind::ServerAccept);
    install_tls_method(caller, obj, "serverClose", TlsMethodKind::ServerClose);
    install_tls_method(caller, obj, "serverPort", TlsMethodKind::ServerPort);
    install_tls_method(caller, obj, "serverAddress", TlsMethodKind::ServerAddress);
    install_tls_method(
        caller,
        obj,
        "socketLocalPort",
        TlsMethodKind::SocketLocalPort,
    );
    install_tls_method(
        caller,
        obj,
        "socketLocalAddress",
        TlsMethodKind::SocketLocalAddress,
    );
    install_tls_method(
        caller,
        obj,
        "socketRemotePort",
        TlsMethodKind::SocketRemotePort,
    );
    install_tls_method(
        caller,
        obj,
        "socketRemoteAddress",
        TlsMethodKind::SocketRemoteAddress,
    );
    caller.data().truncate_host_temp_roots(temp_root_len);
    obj
}

pub(crate) fn call_tls_method(
    caller: &mut Caller<'_, RuntimeState>,
    kind: TlsMethodKind,
    args: &[i64],
) -> i64 {
    match kind {
        TlsMethodKind::Connect => connect(caller, args),
        TlsMethodKind::Read => read(caller, args),
        TlsMethodKind::Write => write(caller, args),
        TlsMethodKind::End => end(caller, args),
        TlsMethodKind::Destroy => destroy(caller, args),
        TlsMethodKind::ServerListen => server_listen(caller, args),
        TlsMethodKind::ServerAccept => server_accept(caller, args),
        TlsMethodKind::ServerClose => server_close(caller, args),
        TlsMethodKind::ServerPort => server_port(caller, args),
        TlsMethodKind::ServerAddress => server_address(caller, args),
        TlsMethodKind::SocketLocalPort => socket_addr_number(caller, args, false, true),
        TlsMethodKind::SocketLocalAddress => socket_addr_string(caller, args, false),
        TlsMethodKind::SocketRemotePort => socket_addr_number(caller, args, true, true),
        TlsMethodKind::SocketRemoteAddress => socket_addr_string(caller, args, true),
    }
}

pub(crate) fn alloc_tls_socket_entry_from_stream(
    state: &RuntimeState,
    stream: TlsStream<TcpStream>,
    local_addr: SocketAddr,
    peer_addr: SocketAddr,
    alpn_protocol: Option<String>,
) -> u32 {
    let (reader, writer) = tokio::io::split(stream);
    state.tls_socket_table.alloc(TlsSocketEntry {
        reader: Arc::new(AsyncMutex::new(Some(reader))),
        writer: Arc::new(AsyncMutex::new(Some(writer))),
        local_addr,
        peer_addr,
        close_notify: Arc::new(Notify::new()),
        alpn_protocol,
    })
}

fn install_tls_method(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
    name: &str,
    kind: TlsMethodKind,
) {
    let callable = create_native_callable(caller.data(), NativeCallable::TlsMethod { kind });
    let _ = define_host_data_property_from_caller(caller, obj, name, callable);
}

// ── 客户端 connect ──────────────────────────────────────────────────

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
    // servername（SNI）：默认 = host
    let servername = args
        .get(2)
        .copied()
        .filter(|v| !value::is_undefined(*v) && !value::is_null(*v))
        .map(|v| js_string_lossy(caller, v))
        .unwrap_or_else(|| host.clone());
    // reject_unauthorized：默认 true
    let reject_unauthorized = args
        .get(3)
        .copied()
        .map(|v| {
            if value::is_bool(v) {
                value::decode_bool(v)
            } else if value::is_f64(v) {
                value::decode_f64(v) != 0.0
            } else {
                true
            }
        })
        .unwrap_or(true);
    // alpn_protocols：逗号分隔字符串（如 "h2,http/1.1"）
    let alpn_str = args
        .get(4)
        .copied()
        .filter(|v| !value::is_undefined(*v) && !value::is_null(*v))
        .map(|v| js_string_lossy(caller, v))
        .unwrap_or_default();
    let alpn_protocols: Vec<Vec<u8>> = if alpn_str.is_empty() {
        Vec::new()
    } else {
        alpn_str
            .split(',')
            .map(|s| s.trim().as_bytes().to_vec())
            .collect()
    };

    let address = format!("{host}:{port}");
    enqueue_async_result(
        caller,
        promise,
        async move {
            let tcp_stream = TcpStream::connect(address)
                .await
                .map_err(|err| format!("connect {host}:{port} failed: {err}"))?;
            let local_addr = tcp_stream
                .local_addr()
                .map_err(|err| format!("connect local_addr failed: {err}"))?;
            let peer_addr = tcp_stream
                .peer_addr()
                .map_err(|err| format!("connect peer_addr failed: {err}"))?;

            let server_name = ServerName::try_from(servername.clone())
                .map_err(|err| format!("invalid servername '{servername}': {err}"))?;

            let config = make_client_config(reject_unauthorized, alpn_protocols)?;
            let connector = TlsConnector::from(Arc::new(config));
            let tls_stream = connector
                .connect(server_name, tcp_stream)
                .await
                .map_err(|err| format!("TLS handshake failed: {err}"))?;

            let alpn_protocol = tls_stream
                .get_ref()
                .1
                .alpn_protocol()
                .map(|p| String::from_utf8_lossy(p).to_string());

            Ok((
                TlsStream::Client(tls_stream),
                local_addr,
                peer_addr,
                alpn_protocol,
            ))
        },
        |store, _env, result| match result {
            Ok((stream, local_addr, peer_addr, alpn_protocol)) => {
                let handle = alloc_tls_socket_entry_from_stream(
                    store.data(),
                    stream,
                    local_addr,
                    peer_addr,
                    alpn_protocol,
                );
                PromiseSettlement::Fulfill(value::encode_f64(handle as f64))
            }
            Err(message) => PromiseSettlement::Reject(error_with_env(store, _env, message)),
        },
    );
    promise
}

fn make_client_config(
    reject_unauthorized: bool,
    alpn_protocols: Vec<Vec<u8>>,
) -> Result<ClientConfig, String> {
    let mut config = if reject_unauthorized {
        let root_store = RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
        };
        ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth()
    } else {
        ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoCertificateVerification))
            .with_no_client_auth()
    };
    config.alpn_protocols = alpn_protocols;
    Ok(config)
}

// ── socket read/write/end/destroy ───────────────────────────────────

fn read(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let promise = alloc_promise_from_caller(caller, PromiseEntry::pending());
    let Some(entry) = socket_entry(caller, args.first().copied()) else {
        reject_promise_from_caller(
            caller,
            promise,
            "tls.TLSSocket handle is invalid".to_string(),
        );
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
                    let read_len = result.map_err(|err| format!("tls read failed: {err}"))?;
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
            Ok(Some(bytes)) => {
                PromiseSettlement::Fulfill(arraybuffer_with_bytes(store, env, &bytes))
            }
            Ok(None) => PromiseSettlement::Fulfill(value::encode_null()),
            Err(message) => PromiseSettlement::Reject(error_with_env(store, env, message)),
        },
    );
    promise
}

fn write(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let Some(entry) = socket_entry(caller, args.first().copied()) else {
        return error_from_caller(caller, "tls.TLSSocket handle is invalid".to_string());
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
                return Err("tls socket is closed".to_string());
            };
            writer
                .write_all(&data)
                .await
                .map_err(|err| format!("tls write failed: {err}"))?;
            writer
                .flush()
                .await
                .map_err(|err| format!("tls flush failed: {err}"))?;
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
        return error_from_caller(caller, "tls.TLSSocket handle is invalid".to_string());
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
                    .map_err(|err| format!("tls shutdown failed: {err}"))?;
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
        && let Ok(mut table) = caller.data().tls_socket_table.inner.lock()
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

// ── server listen/accept/close ──────────────────────────────────────

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
    // cert_pem / key_pem：空时自动生成自签名证书
    let cert_pem = string_arg(caller, args.get(2).copied());
    let key_pem = string_arg(caller, args.get(3).copied());
    // alpn_protocols：逗号分隔
    let alpn_str = string_arg(caller, args.get(4).copied());
    let alpn_protocols: Vec<Vec<u8>> = if alpn_str.is_empty() {
        Vec::new()
    } else {
        alpn_str
            .split(',')
            .map(|s| s.trim().as_bytes().to_vec())
            .collect()
    };

    let address = format!("{host}:{port}");
    enqueue_async_result(
        caller,
        promise,
        async move {
            let (cert_pem, key_pem) = if cert_pem.is_empty() || key_pem.is_empty() {
                generate_self_signed_cert()
                    .map_err(|e| format!("generate self-signed cert failed: {e}"))?
            } else {
                (cert_pem, key_pem)
            };

            let server_config = make_server_config(&cert_pem, &key_pem, alpn_protocols)?;
            let acceptor = TlsAcceptor::from(Arc::new(server_config));

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
                        Ok((tcp_stream, peer_addr)) => {
                            let local_addr = tcp_stream.local_addr().unwrap_or(local_addr);
                            let acceptor = acceptor.clone();
                            let tx = accept_tx.clone();
                            tokio::spawn(async move {
                                if let Ok(tls_stream) = acceptor.accept(tcp_stream).await {
                                    let alpn = tls_stream
                                        .get_ref()
                                        .1
                                        .alpn_protocol()
                                        .map(|p| String::from_utf8_lossy(p).to_string());
                                    let _ = tx.send(AcceptedTlsStream {
                                        stream: TlsStream::Server(tls_stream),
                                        local_addr,
                                        peer_addr,
                                        alpn_protocol: alpn,
                                    });
                                }
                            });
                        }
                        Err(_) => break,
                    }
                }
            });
            Ok(TlsServerEntry {
                accept_rx: Arc::new(AsyncMutex::new(accept_rx)),
                accept_task: Some(accept_task),
                local_addr,
                closed,
                close_notify,
            })
        },
        |store, _env, result| match result {
            Ok(entry) => {
                let handle = store.data().tls_server_table.alloc(entry);
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
        reject_promise_from_caller(caller, promise, "tls.Server handle is invalid".to_string());
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
                let handle = alloc_tls_socket_entry_from_stream(
                    store.data(),
                    accepted.stream,
                    accepted.local_addr,
                    accepted.peer_addr,
                    accepted.alpn_protocol,
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
            reject_promise_from_caller(caller, promise, "tls.Server handle is invalid".to_string());
            return promise;
        }
    };
    let entry = {
        let mut table = caller
            .data()
            .tls_server_table
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
        .map(|entry| {
            crate::runtime_render::store_runtime_string(caller, entry.local_addr.ip().to_string())
        })
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
    let addr = if peer {
        entry.peer_addr
    } else {
        entry.local_addr
    };
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
    let addr = if peer {
        entry.peer_addr
    } else {
        entry.local_addr
    };
    crate::runtime_render::store_runtime_string(caller, addr.ip().to_string())
}

// ── 证书生成与解析 ──────────────────────────────────────────────────

fn generate_self_signed_cert() -> Result<(String, String), String> {
    let rcgen::CertifiedKey { cert, key_pair } =
        rcgen::generate_simple_self_signed(vec!["localhost".to_string(), "127.0.0.1".to_string()])
            .map_err(|e| format!("rcgen generate failed: {e}"))?;
    let cert_pem = cert.pem();
    let key_pem = key_pair.serialize_pem();
    Ok((cert_pem, key_pem))
}

fn make_server_config(
    cert_pem: &str,
    key_pem: &str,
    alpn_protocols: Vec<Vec<u8>>,
) -> Result<ServerConfig, String> {
    let cert_chain: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut cert_pem.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("parse cert PEM failed: {e}"))?;
    if cert_chain.is_empty() {
        return Err("no certificate found in PEM".to_string());
    }
    let key = rustls_pemfile::private_key(&mut key_pem.as_bytes())
        .map_err(|e| format!("parse key PEM failed: {e}"))?
        .ok_or_else(|| "no private key found in PEM".to_string())?;

    let mut config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(cert_chain, key)
        .map_err(|e| format!("server config failed: {e}"))?;
    config.alpn_protocols = alpn_protocols;
    Ok(config)
}

// ── helpers ────────────────────────────────────────────────────────

struct SocketSnapshot {
    reader: Arc<AsyncMutex<Option<TlsReadHalf>>>,
    writer: Arc<AsyncMutex<Option<TlsWriteHalf>>>,
    local_addr: SocketAddr,
    peer_addr: SocketAddr,
    close_notify: Arc<Notify>,
}

struct ServerSnapshot {
    accept_rx: Arc<AsyncMutex<mpsc::UnboundedReceiver<AcceptedTlsStream>>>,
    local_addr: SocketAddr,
    closed: Arc<AtomicBool>,
    close_notify: Arc<Notify>,
}

fn socket_entry(
    caller: &mut Caller<'_, RuntimeState>,
    value_raw: Option<i64>,
) -> Option<SocketSnapshot> {
    let handle = handle_arg(value_raw)?;
    let table = caller.data().tls_socket_table.inner.lock().ok()?;
    let entry = table.get(handle as usize)?;
    Some(SocketSnapshot {
        reader: Arc::clone(&entry.reader),
        writer: Arc::clone(&entry.writer),
        local_addr: entry.local_addr,
        peer_addr: entry.peer_addr,
        close_notify: Arc::clone(&entry.close_notify),
    })
}

fn server_entry(
    caller: &mut Caller<'_, RuntimeState>,
    value_raw: Option<i64>,
) -> Option<ServerSnapshot> {
    let handle = handle_arg(value_raw)?;
    let table = caller.data().tls_server_table.inner.lock().ok()?;
    let entry = table.get(handle as usize)?;
    Some(ServerSnapshot {
        accept_rx: Arc::clone(&entry.accept_rx),
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

fn string_arg(caller: &mut Caller<'_, RuntimeState>, value_raw: Option<i64>) -> String {
    value_raw
        .filter(|v| !value::is_undefined(*v) && !value::is_null(*v))
        .map(|v| js_string_lossy(caller, v))
        .unwrap_or_default()
}

fn data_arg(
    caller: &mut Caller<'_, RuntimeState>,
    value_raw: Option<i64>,
) -> Result<Vec<u8>, String> {
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
        reject_promise_from_caller(
            caller,
            promise,
            "async network runtime is not available".to_string(),
        );
        return;
    };
    let Some(counter) = caller.data().async_op_counter.clone() else {
        reject_promise_from_caller(
            caller,
            promise,
            "async network runtime is not available".to_string(),
        );
        return;
    };
    let guard = counter.begin();
    let scope = crate::scheduler::capture_completion_scope_from_caller(caller);
    tokio::spawn(async move {
        let result = future.await;
        let _ = tx.send(crate::scheduler::AsyncHostCompletion::Materialize {
            promise,
            materialize: Box::new(move |store, env| materialize(store, env, result)),
            scope,
        });
        drop(guard);
    });
}

fn reject_promise_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    promise: i64,
    message: String,
) {
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

// 抑制未使用类型导入警告（ClientTlsStream/ServerTlsStream 通过 TlsStream 别名使用）
#[allow(dead_code)]
type _UnusedClientTlsStream = ClientTlsStream<TcpStream>;
#[allow(dead_code)]
type _UnusedServerTlsStream = ServerTlsStream<TcpStream>;
