use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use serde_json::{Value, json};
use sha1::{Digest as _, Sha1};

const WEBSOCKET_GUID: &[u8] = b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
const MAX_HTTP_HEADER_BYTES: usize = 16 * 1024;
const MAX_FRAME_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InspectorConfig {
    pub host: String,
    pub port: u16,
    pub break_on_start: bool,
}

impl InspectorConfig {
    /// 从 `WJSM_INSPECT` / `WJSM_INSPECT_BRK` / `NODE_OPTIONS` 读取。
    pub fn from_environment() -> Result<Option<Self>, String> {
        if let Some(config) = parse_inspect_var("WJSM_INSPECT_BRK", true)? {
            return Ok(Some(config));
        }
        if let Some(config) = parse_inspect_var("WJSM_INSPECT", false)? {
            return Ok(Some(config));
        }
        parse_node_options_inspect()
    }
}

fn parse_inspect_var(name: &str, break_on_start: bool) -> Result<Option<InspectorConfig>, String> {
    let Ok(raw) = std::env::var(name) else {
        return Ok(None);
    };
    let (host, port) = parse_inspect_address(&raw)?;
    Ok(Some(InspectorConfig {
        host,
        port,
        break_on_start,
    }))
}

fn parse_node_options_inspect() -> Result<Option<InspectorConfig>, String> {
    let Ok(options) = std::env::var("NODE_OPTIONS") else {
        return Ok(None);
    };
    let mut found = None;
    for token in options.split_whitespace() {
        if let Some(rest) = token.strip_prefix("--inspect-brk") {
            let raw = rest.strip_prefix('=').unwrap_or("");
            let (host, port) = parse_inspect_address(raw)?;
            found = Some(InspectorConfig {
                host,
                port,
                break_on_start: true,
            });
        } else if let Some(rest) = token.strip_prefix("--inspect")
            && found.as_ref().is_none_or(|config| !config.break_on_start)
        {
            let raw = rest.strip_prefix('=').unwrap_or("");
            let (host, port) = parse_inspect_address(raw)?;
            found = Some(InspectorConfig {
                host,
                port,
                break_on_start: false,
            });
        }
    }
    Ok(found)
}

fn parse_inspect_address(raw: &str) -> Result<(String, u16), String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed == "1" || trimmed.eq_ignore_ascii_case("true") {
        return Ok(("127.0.0.1".to_string(), 9229));
    }
    if trimmed.chars().all(|ch| ch.is_ascii_digit()) {
        let port = trimmed
            .parse::<u16>()
            .map_err(|_| format!("invalid inspect port `{trimmed}`"))?;
        return Ok(("127.0.0.1".to_string(), port));
    }
    if let Some(port_part) = trimmed.strip_prefix(':') {
        let port = port_part
            .parse::<u16>()
            .map_err(|_| format!("invalid inspect address `{trimmed}`"))?;
        return Ok(("127.0.0.1".to_string(), port));
    }
    let (host, port_part) = trimmed.rsplit_once(':').ok_or_else(|| {
        format!("invalid inspect address `{trimmed}` (expected HOST:PORT or PORT)")
    })?;
    if host.is_empty() {
        return Err(format!("invalid inspect address `{trimmed}` (empty host)"));
    }
    let port = port_part
        .parse::<u16>()
        .map_err(|_| format!("invalid inspect port `{port_part}` in `{trimmed}`"))?;
    Ok((host.to_string(), port))
}

#[derive(Clone, Debug)]
pub(super) struct ScriptInfo {
    pub id: String,
    pub url: String,
    pub source: String,
    pub hash: String,
}

#[derive(Debug)]
pub(super) struct InspectorCommand {
    pub id: Value,
    pub method: String,
    pub params: Value,
}

#[derive(Debug)]
pub(super) enum ServerEvent {
    Script(ScriptInfo),
    Protocol(Value),
}

pub(super) struct InspectorServer {
    url: String,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl InspectorServer {
    pub(super) fn start(
        config: InspectorConfig,
    ) -> io::Result<(Self, Receiver<InspectorCommand>, Sender<ServerEvent>)> {
        let listener = TcpListener::bind((config.host.as_str(), config.port))?;
        let port = listener.local_addr()?.port();
        listener.set_nonblocking(true)?;
        let target_id = target_id();
        let url = format!("ws://{}:{port}/{target_id}", display_host(&config.host));
        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let thread_url = url.clone();
        let thread = thread::Builder::new()
            .name("wjsm-inspector".into())
            .spawn(move || {
                server_loop(
                    listener,
                    &target_id,
                    &thread_url,
                    command_tx,
                    event_rx,
                    thread_stop,
                );
            })?;
        Ok((
            Self {
                url,
                stop,
                thread: Some(thread),
            },
            command_rx,
            event_tx,
        ))
    }

    pub(super) fn url(&self) -> &str {
        &self.url
    }
}

impl Drop for InspectorServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[derive(Default)]
struct ServerState {
    script: Option<ScriptInfo>,
    pending: VecDeque<Value>,
}

fn server_loop(
    listener: TcpListener,
    target_id: &str,
    url: &str,
    commands: Sender<InspectorCommand>,
    events: Receiver<ServerEvent>,
    stop: Arc<AtomicBool>,
) {
    let mut state = ServerState::default();
    while !stop.load(Ordering::Acquire) {
        drain_events(&events, &mut state);
        match listener.accept() {
            Ok((mut stream, _)) => {
                let _ = handle_connection(
                    &mut stream,
                    target_id,
                    url,
                    &commands,
                    &events,
                    &mut state,
                    &stop,
                );
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(5));
            }
            Err(_) => break,
        }
    }
}

fn handle_connection(
    stream: &mut TcpStream,
    target_id: &str,
    url: &str,
    commands: &Sender<InspectorCommand>,
    events: &Receiver<ServerEvent>,
    state: &mut ServerState,
    stop: &AtomicBool,
) -> io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_millis(100)))?;
    stream.set_write_timeout(Some(Duration::from_secs(1)))?;
    let request = read_http_request(stream)?;
    let (path, headers) = parse_http_request(&request)?;
    if headers.iter().any(|(name, value)| {
        name.eq_ignore_ascii_case("upgrade") && value.eq_ignore_ascii_case("websocket")
    }) {
        let expected_path = format!("/{target_id}");
        if path != expected_path {
            return write_http(
                stream,
                "404 Not Found",
                "text/plain",
                b"unknown inspector target",
            );
        }
        websocket_handshake(stream, &headers)?;
        return websocket_loop(stream, commands, events, state, stop);
    }
    let body = match path.as_str() {
        "/json" | "/json/list" => serde_json::to_vec(&[target_descriptor(target_id, url)])?,
        "/json/version" => serde_json::to_vec(&json!({
            "Browser": format!("wjsm/{}", env!("CARGO_PKG_VERSION")),
            "Protocol-Version": "1.3",
        }))?,
        _ => {
            return write_http(stream, "404 Not Found", "text/plain", b"not found");
        }
    };
    write_http(stream, "200 OK", "application/json; charset=UTF-8", &body)
}

fn target_descriptor(target_id: &str, url: &str) -> Value {
    json!({
        "description": "wjsm native runtime",
        "devtoolsFrontendUrl": format!("devtools://devtools/bundled/js_app.html?ws={}", &url[5..]),
        "devtoolsFrontendUrlCompat": format!("devtools://devtools/bundled/inspector.html?ws={}", &url[5..]),
        "faviconUrl": "",
        "id": target_id,
        "title": "wjsm",
        "type": "node",
        "url": "file://",
        "webSocketDebuggerUrl": url,
    })
}

fn read_http_request(stream: &mut TcpStream) -> io::Result<Vec<u8>> {
    let mut request = Vec::with_capacity(1024);
    let mut buffer = [0_u8; 1024];
    loop {
        let count = stream.read(&mut buffer)?;
        if count == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "inspector HTTP request ended before headers",
            ));
        }
        request.extend_from_slice(&buffer[..count]);
        if request.windows(4).any(|window| window == b"\r\n\r\n") {
            return Ok(request);
        }
        if request.len() > MAX_HTTP_HEADER_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "inspector HTTP headers exceed limit",
            ));
        }
    }
}

fn parse_http_request(request: &[u8]) -> io::Result<(String, Vec<(String, String)>)> {
    let request = std::str::from_utf8(request)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "HTTP request is not UTF-8"))?;
    let mut lines = request.split("\r\n");
    let first = lines
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing HTTP request line"))?;
    let mut first = first.split_ascii_whitespace();
    if first.next() != Some("GET") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "inspector only accepts GET",
        ));
    }
    let path = first
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing HTTP path"))?
        .to_owned();
    let headers = lines
        .take_while(|line| !line.is_empty())
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_owned(), value.trim().to_owned()))
        .collect();
    Ok((path, headers))
}

fn websocket_handshake(stream: &mut TcpStream, headers: &[(String, String)]) -> io::Result<()> {
    let key = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("sec-websocket-key"))
        .map(|(_, value)| value.as_str())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing websocket key"))?;
    let mut hasher = Sha1::new();
    hasher.update(key.as_bytes());
    hasher.update(WEBSOCKET_GUID);
    let accept = base64::engine::general_purpose::STANDARD.encode(hasher.finalize());
    write!(
        stream,
        "HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Accept: {accept}\r\n\r\n"
    )?;
    stream.flush()
}

fn websocket_loop(
    stream: &mut TcpStream,
    commands: &Sender<InspectorCommand>,
    events: &Receiver<ServerEvent>,
    state: &mut ServerState,
    stop: &AtomicBool,
) -> io::Result<()> {
    let mut input = Vec::new();
    while !stop.load(Ordering::Acquire) {
        drain_protocol_events(stream, events, state)?;
        let mut buffer = [0_u8; 8192];
        match stream.read(&mut buffer) {
            Ok(0) => return Ok(()),
            Ok(count) => input.extend_from_slice(&buffer[..count]),
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) => {}
            Err(error) => return Err(error),
        }
        while let Some(frame) = take_frame(&mut input)? {
            match frame.opcode {
                0x1 => {
                    let message: Value =
                        serde_json::from_slice(&frame.payload).map_err(|error| {
                            io::Error::new(io::ErrorKind::InvalidData, error.to_string())
                        })?;
                    handle_protocol_message(stream, message, commands, state)?;
                }
                0x8 => {
                    write_frame(stream, 0x8, &[])?;
                    return Ok(());
                }
                0x9 => write_frame(stream, 0xA, &frame.payload)?,
                0xA => {}
                _ => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "unsupported websocket frame",
                    ));
                }
            }
        }
    }
    Ok(())
}

fn handle_protocol_message(
    stream: &mut TcpStream,
    message: Value,
    commands: &Sender<InspectorCommand>,
    state: &mut ServerState,
) -> io::Result<()> {
    let id = message.get("id").cloned().unwrap_or(Value::Null);
    let method = message
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let params = message.get("params").cloned().unwrap_or_else(|| json!({}));
    match method {
        "Runtime.enable" => {
            write_json(stream, &json!({"id": id, "result": {}}))?;
            write_json(
                stream,
                &json!({
                    "method": "Runtime.executionContextCreated",
                    "params": {"context": {
                        "id": 1,
                        "origin": "",
                        "name": "wjsm",
                        "uniqueId": "wjsm-main",
                        "auxData": {"isDefault": true, "type": "default", "frameId": ""},
                    }}
                }),
            )?;
        }
        "Debugger.enable" => {
            write_json(
                stream,
                &json!({"id": id, "result": {"debuggerId": "wjsm-native"}}),
            )?;
            if let Some(script) = &state.script {
                write_json(stream, &script_parsed(script))?;
            }
            while let Some(pending) = state.pending.pop_front() {
                write_json(stream, &pending)?;
            }
        }
        "Runtime.disable"
        | "Debugger.disable"
        | "Profiler.enable"
        | "Profiler.disable"
        | "Debugger.setAsyncCallStackDepth"
        | "Runtime.setCustomObjectFormatterEnabled" => {
            write_json(stream, &json!({"id": id, "result": {}}))?;
        }
        "Debugger.getScriptSource" => {
            let source = state
                .script
                .as_ref()
                .map_or("", |script| script.source.as_str());
            write_json(
                stream,
                &json!({"id": id, "result": {"scriptSource": source}}),
            )?;
        }
        "Schema.getDomains" => {
            write_json(
                stream,
                &json!({"id": id, "result": {"domains": [
                    {"name": "Runtime", "version": "1.3"},
                    {"name": "Debugger", "version": "1.3"},
                ]}}),
            )?;
        }
        _ => {
            commands
                .send(InspectorCommand {
                    id,
                    method: method.to_owned(),
                    params,
                })
                .map_err(|_| {
                    io::Error::new(io::ErrorKind::BrokenPipe, "native runtime has exited")
                })?;
        }
    }
    Ok(())
}

fn drain_events(events: &Receiver<ServerEvent>, state: &mut ServerState) {
    loop {
        match events.try_recv() {
            Ok(ServerEvent::Script(script)) => state.script = Some(script),
            Ok(ServerEvent::Protocol(message)) => state.pending.push_back(message),
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
        }
    }
}

fn drain_protocol_events(
    stream: &mut TcpStream,
    events: &Receiver<ServerEvent>,
    state: &mut ServerState,
) -> io::Result<()> {
    while let Ok(event) = events.try_recv() {
        match event {
            ServerEvent::Script(script) => {
                state.script = Some(script.clone());
                write_json(stream, &script_parsed(&script))?;
            }
            ServerEvent::Protocol(message) => write_json(stream, &message)?,
        }
    }
    Ok(())
}

fn script_parsed(script: &ScriptInfo) -> Value {
    json!({
        "method": "Debugger.scriptParsed",
        "params": {
            "scriptId": script.id,
            "url": script.url,
            "startLine": 0,
            "startColumn": 0,
            "endLine": script.source.lines().count(),
            "endColumn": 0,
            "executionContextId": 1,
            "hash": script.hash,
            "isLiveEdit": false,
            "sourceMapURL": "",
            "hasSourceURL": false,
            "isModule": true,
            "length": script.source.len(),
        }
    })
}

struct Frame {
    opcode: u8,
    payload: Vec<u8>,
}

fn take_frame(input: &mut Vec<u8>) -> io::Result<Option<Frame>> {
    if input.len() < 2 {
        return Ok(None);
    }
    let opcode = input[0] & 0x0F;
    let masked = input[1] & 0x80 != 0;
    let mut payload_len = usize::from(input[1] & 0x7F);
    let mut header_len: usize = 2;
    if payload_len == 126 {
        if input.len() < 4 {
            return Ok(None);
        }
        payload_len = usize::from(u16::from_be_bytes([input[2], input[3]]));
        header_len = 4;
    } else if payload_len == 127 {
        if input.len() < 10 {
            return Ok(None);
        }
        payload_len = usize::try_from(u64::from_be_bytes(
            input[2..10].try_into().expect("checked websocket header"),
        ))
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "websocket frame too large"))?;
        header_len = 10;
    }
    if payload_len > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "websocket frame exceeds inspector limit",
        ));
    }
    let mask_len: usize = if masked { 4 } else { 0 };
    let frame_len = header_len
        .checked_add(mask_len)
        .and_then(|len| len.checked_add(payload_len))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "websocket length overflow"))?;
    if input.len() < frame_len {
        return Ok(None);
    }
    let mask = masked.then(|| {
        let start = header_len;
        [
            input[start],
            input[start + 1],
            input[start + 2],
            input[start + 3],
        ]
    });
    let payload_start = header_len + mask_len;
    let mut payload = input[payload_start..frame_len].to_vec();
    if let Some(mask) = mask {
        for (index, byte) in payload.iter_mut().enumerate() {
            *byte ^= mask[index % 4];
        }
    }
    input.drain(..frame_len);
    Ok(Some(Frame { opcode, payload }))
}

fn write_json(stream: &mut TcpStream, message: &Value) -> io::Result<()> {
    write_frame(stream, 0x1, &serde_json::to_vec(message)?)
}

fn write_frame(stream: &mut TcpStream, opcode: u8, payload: &[u8]) -> io::Result<()> {
    let mut header = Vec::with_capacity(10);
    header.push(0x80 | opcode);
    match payload.len() {
        0..=125 => header.push(payload.len() as u8),
        126..=65535 => {
            header.push(126);
            header.extend_from_slice(&(payload.len() as u16).to_be_bytes());
        }
        _ => {
            header.push(127);
            header.extend_from_slice(&(payload.len() as u64).to_be_bytes());
        }
    }
    stream.write_all(&header)?;
    stream.write_all(payload)?;
    stream.flush()
}

fn write_http(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &[u8],
) -> io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body)?;
    stream.flush()
}

fn display_host(host: &str) -> &str {
    match host {
        "0.0.0.0" => "127.0.0.1",
        "::" => "[::1]",
        other => other,
    }
}

fn target_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{:x}-{nanos:x}", std::process::id())
}
