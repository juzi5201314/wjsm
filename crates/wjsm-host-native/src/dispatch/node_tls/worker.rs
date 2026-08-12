use std::collections::VecDeque;
use std::fmt;
use std::io::{BufReader, Cursor, ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;
use std::time::Duration;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{WebPkiSupportedAlgorithms, verify_tls12_signature, verify_tls13_signature};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::{
    ClientConfig, ClientConnection, Connection, DigitallySignedStruct, RootCertStore, ServerConfig,
    ServerConnection, SignatureScheme,
};

const WORKER_POLL_INTERVAL: Duration = Duration::from_millis(1);

#[derive(Clone)]
pub(super) struct TlsSocketEndpoint {
    commands: Sender<SocketCommand>,
    pub(super) local: SocketAddr,
    pub(super) remote: SocketAddr,
}

pub(super) struct TlsListenerHandle {
    commands: Sender<ListenerCommand>,
    pub(super) address: SocketAddr,
}

pub(super) type SocketResult = Result<TlsSocketEndpoint, String>;
pub(super) type ReadResult = Result<Option<Vec<u8>>, String>;

enum ListenerCommand {
    Accept(Sender<SocketResult>),
    Close,
}

enum SocketCommand {
    Destroy,
    End(Sender<Result<(), String>>),
    Read(Sender<ReadResult>),
    Write(Vec<u8>, Sender<Result<(), String>>),
}

pub(super) fn listen(
    host: String,
    port: u16,
    cert_pem: String,
    key_pem: String,
    alpn: String,
) -> Result<TlsListenerHandle, String> {
    let config = server_config(&host, &cert_pem, &key_pem, &alpn)?;
    let listener = TcpListener::bind((host.as_str(), port)).map_err(|error| error.to_string())?;
    listener
        .set_nonblocking(true)
        .map_err(|error| error.to_string())?;
    let address = listener.local_addr().map_err(|error| error.to_string())?;
    let (commands, receiver) = mpsc::channel();
    thread::spawn(move || run_listener(listener, config, receiver));
    Ok(TlsListenerHandle { commands, address })
}

pub(super) fn accept(listener: &TlsListenerHandle) -> Result<Receiver<SocketResult>, String> {
    let (sender, receiver) = mpsc::channel();
    listener
        .commands
        .send(ListenerCommand::Accept(sender))
        .map_err(|_| "TLS server is closed".to_owned())?;
    Ok(receiver)
}

pub(super) fn close_listener(listener: TlsListenerHandle) {
    let _ = listener.commands.send(ListenerCommand::Close);
}

pub(super) fn connect(
    host: String,
    port: u16,
    server_name: String,
    reject_unauthorized: bool,
    alpn: String,
) -> Receiver<SocketResult> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let result = connect_stream(&host, port, &server_name, reject_unauthorized, &alpn);
        match result {
            Ok((connection, stream)) => spawn_socket(connection, stream, sender),
            Err(error) => {
                let _ = sender.send(Err(error));
            }
        }
    });
    receiver
}

pub(super) fn read(socket: &TlsSocketEndpoint) -> Result<Receiver<ReadResult>, String> {
    let (sender, receiver) = mpsc::channel();
    socket
        .commands
        .send(SocketCommand::Read(sender))
        .map_err(|_| "TLS socket is closed".to_owned())?;
    Ok(receiver)
}

pub(super) fn write(socket: &TlsSocketEndpoint, bytes: Vec<u8>) -> Result<(), String> {
    let (sender, receiver) = mpsc::channel();
    socket
        .commands
        .send(SocketCommand::Write(bytes, sender))
        .map_err(|_| "TLS socket is closed".to_owned())?;
    receiver
        .recv()
        .map_err(|_| "TLS socket worker stopped".to_owned())?
}

pub(super) fn end(socket: &TlsSocketEndpoint) -> Result<(), String> {
    let (sender, receiver) = mpsc::channel();
    socket
        .commands
        .send(SocketCommand::End(sender))
        .map_err(|_| "TLS socket is closed".to_owned())?;
    receiver
        .recv()
        .map_err(|_| "TLS socket worker stopped".to_owned())?
}

pub(super) fn destroy(socket: TlsSocketEndpoint) {
    let _ = socket.commands.send(SocketCommand::Destroy);
}

fn run_listener(
    listener: TcpListener,
    config: Arc<ServerConfig>,
    commands: Receiver<ListenerCommand>,
) {
    let mut pending = VecDeque::new();
    loop {
        match receive_listener_commands(&commands, &mut pending) {
            ListenerState::Open => {}
            ListenerState::Closed => {
                reject_pending_accepts(&mut pending, "TLS server is closed");
                return;
            }
        }
        while let Some(sender) = pending.front() {
            match listener.accept() {
                Ok((stream, _)) => {
                    let sender = pending.pop_front().expect("pending accept exists");
                    match ServerConnection::new(Arc::clone(&config)) {
                        Ok(connection) => {
                            spawn_socket(Connection::Server(connection), stream, sender)
                        }
                        Err(error) => {
                            let _ = sender.send(Err(error.to_string()));
                        }
                    }
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                Err(error) => {
                    let _ = sender.send(Err(error.to_string()));
                    pending.pop_front();
                }
            }
        }
        thread::sleep(WORKER_POLL_INTERVAL);
    }
}

enum ListenerState {
    Open,
    Closed,
}

fn receive_listener_commands(
    commands: &Receiver<ListenerCommand>,
    pending: &mut VecDeque<Sender<SocketResult>>,
) -> ListenerState {
    loop {
        match commands.try_recv() {
            Ok(ListenerCommand::Accept(sender)) => pending.push_back(sender),
            Ok(ListenerCommand::Close) | Err(TryRecvError::Disconnected) => {
                return ListenerState::Closed;
            }
            Err(TryRecvError::Empty) => return ListenerState::Open,
        }
    }
}

fn reject_pending_accepts(pending: &mut VecDeque<Sender<SocketResult>>, message: &str) {
    for sender in pending.drain(..) {
        let _ = sender.send(Err(message.to_owned()));
    }
}

fn spawn_socket(connection: Connection, stream: TcpStream, ready: Sender<SocketResult>) {
    let local = match stream.local_addr() {
        Ok(address) => address,
        Err(error) => {
            let _ = ready.send(Err(error.to_string()));
            return;
        }
    };
    let remote = match stream.peer_addr() {
        Ok(address) => address,
        Err(error) => {
            let _ = ready.send(Err(error.to_string()));
            return;
        }
    };
    if let Err(error) = stream.set_nonblocking(true) {
        let _ = ready.send(Err(error.to_string()));
        return;
    }
    let (commands, receiver) = mpsc::channel();
    let endpoint = TlsSocketEndpoint {
        commands,
        local,
        remote,
    };
    thread::spawn(move || run_socket(connection, stream, endpoint, ready, receiver));
}

fn run_socket(
    mut connection: Connection,
    mut stream: TcpStream,
    endpoint: TlsSocketEndpoint,
    ready: Sender<SocketResult>,
    commands: Receiver<SocketCommand>,
) {
    let mut ready = Some(ready);
    let mut pending_reads = VecDeque::new();
    loop {
        if !process_socket_commands(&mut connection, &commands, &mut pending_reads) {
            return;
        }
        match connection.complete_io(&mut stream) {
            Ok(_) => {}
            Err(error) if error.kind() == ErrorKind::WouldBlock => {}
            Err(error) => {
                fail_socket(&mut ready, &mut pending_reads, error.to_string());
                return;
            }
        }
        if !connection.is_handshaking()
            && let Some(sender) = ready.take()
        {
            let _ = sender.send(Ok(endpoint.clone()));
        }
        drain_plaintext(&mut connection, &mut pending_reads);
        thread::sleep(WORKER_POLL_INTERVAL);
    }
}

fn process_socket_commands(
    connection: &mut Connection,
    commands: &Receiver<SocketCommand>,
    pending_reads: &mut VecDeque<Sender<ReadResult>>,
) -> bool {
    loop {
        match commands.try_recv() {
            Ok(SocketCommand::Destroy) | Err(TryRecvError::Disconnected) => return false,
            Ok(SocketCommand::End(sender)) => {
                connection.send_close_notify();
                let _ = sender.send(Ok(()));
            }
            Ok(SocketCommand::Read(sender)) => pending_reads.push_back(sender),
            Ok(SocketCommand::Write(bytes, sender)) => {
                let result = connection
                    .writer()
                    .write_all(&bytes)
                    .map_err(|error| error.to_string());
                let _ = sender.send(result);
            }
            Err(TryRecvError::Empty) => return true,
        }
    }
}

fn drain_plaintext(connection: &mut Connection, pending_reads: &mut VecDeque<Sender<ReadResult>>) {
    while !pending_reads.is_empty() {
        let mut bytes = vec![0; 16 * 1024];
        match connection.reader().read(&mut bytes) {
            Ok(0) => {
                let sender = pending_reads.pop_front().expect("pending read exists");
                let _ = sender.send(Ok(None));
            }
            Ok(length) => {
                bytes.truncate(length);
                let sender = pending_reads.pop_front().expect("pending read exists");
                let _ = sender.send(Ok(Some(bytes)));
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => break,
            Err(error) => {
                let sender = pending_reads.pop_front().expect("pending read exists");
                let _ = sender.send(Err(error.to_string()));
            }
        }
    }
}

fn fail_socket(
    ready: &mut Option<Sender<SocketResult>>,
    pending_reads: &mut VecDeque<Sender<ReadResult>>,
    message: String,
) {
    if let Some(sender) = ready.take() {
        let _ = sender.send(Err(message.clone()));
    }
    for sender in pending_reads.drain(..) {
        let _ = sender.send(Err(message.clone()));
    }
}

fn connect_stream(
    host: &str,
    port: u16,
    server_name: &str,
    reject_unauthorized: bool,
    alpn: &str,
) -> Result<(Connection, TcpStream), String> {
    let config = client_config(reject_unauthorized, alpn)?;
    let server_name =
        ServerName::try_from(server_name.to_owned()).map_err(|error| error.to_string())?;
    let connection =
        ClientConnection::new(config, server_name).map_err(|error| error.to_string())?;
    let stream = TcpStream::connect((host, port)).map_err(|error| error.to_string())?;
    Ok((Connection::Client(connection), stream))
}

fn server_config(
    host: &str,
    cert_pem: &str,
    key_pem: &str,
    alpn: &str,
) -> Result<Arc<ServerConfig>, String> {
    let (certificates, key) = server_identity(host, cert_pem, key_pem)?;
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut config = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|error| error.to_string())?
        .with_no_client_auth()
        .with_single_cert(certificates, key)
        .map_err(|error| error.to_string())?;
    config.alpn_protocols = parse_alpn(alpn);
    Ok(Arc::new(config))
}

fn server_identity(
    host: &str,
    cert_pem: &str,
    key_pem: &str,
) -> Result<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>), String> {
    if cert_pem.is_empty() && key_pem.is_empty() {
        let names = vec![host.to_owned(), "localhost".to_owned()];
        let rcgen::CertifiedKey { cert, key_pair } =
            rcgen::generate_simple_self_signed(names).map_err(|error| error.to_string())?;
        let key = PrivatePkcs8KeyDer::from(key_pair.serialize_der()).into();
        return Ok((vec![cert.der().clone()], key));
    }
    if cert_pem.is_empty() || key_pem.is_empty() {
        return Err("TLS certificate and private key must be provided together".to_owned());
    }
    let mut certificate_reader = BufReader::new(Cursor::new(cert_pem.as_bytes()));
    let certificates = rustls_pemfile::certs(&mut certificate_reader)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    if certificates.is_empty() {
        return Err("TLS certificate chain is empty".to_owned());
    }
    let mut key_reader = BufReader::new(Cursor::new(key_pem.as_bytes()));
    let key = rustls_pemfile::private_key(&mut key_reader)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "TLS private key is missing".to_owned())?;
    Ok((certificates, key))
}

fn client_config(reject_unauthorized: bool, alpn: &str) -> Result<Arc<ClientConfig>, String> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = ClientConfig::builder_with_provider(Arc::clone(&provider))
        .with_safe_default_protocol_versions()
        .map_err(|error| error.to_string())?;
    let mut config = if reject_unauthorized {
        let roots = RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.into(),
        };
        builder.with_root_certificates(roots).with_no_client_auth()
    } else {
        builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoCertificateVerification {
                algorithms: provider.signature_verification_algorithms,
            }))
            .with_no_client_auth()
    };
    config.alpn_protocols = parse_alpn(alpn);
    Ok(Arc::new(config))
}

fn parse_alpn(alpn: &str) -> Vec<Vec<u8>> {
    alpn.split(',')
        .filter(|protocol| !protocol.is_empty())
        .map(|protocol| protocol.as_bytes().to_vec())
        .collect()
}

struct NoCertificateVerification {
    algorithms: WebPkiSupportedAlgorithms,
}

impl fmt::Debug for NoCertificateVerification {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("NoCertificateVerification")
    }
}

impl ServerCertVerifier for NoCertificateVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(message, cert, dss, &self.algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(message, cert, dss, &self.algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}
