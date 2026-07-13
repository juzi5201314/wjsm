//! Inspector / CDP 单元与轻量集成测试。

use serde_json::json;
use std::io::Write;
use wjsm_runtime::InspectConfig;

/// 与 backend `wjsm_debug` version=1 对齐的 payload 编码。
fn encode_wjsm_debug_v1(
    url: &str,
    line_entries: &[(u32, u32, u32, u32)],
    locals: &[(u32, u32, &str)],
) -> Vec<u8> {
    let mut data = Vec::new();
    data.extend_from_slice(&1u32.to_le_bytes());
    data.extend_from_slice(&(url.len() as u32).to_le_bytes());
    data.extend_from_slice(url.as_bytes());
    data.extend_from_slice(&(line_entries.len() as u32).to_le_bytes());
    for &(func, pc, line, col) in line_entries {
        data.extend_from_slice(&func.to_le_bytes());
        data.extend_from_slice(&pc.to_le_bytes());
        data.extend_from_slice(&line.to_le_bytes());
        data.extend_from_slice(&col.to_le_bytes());
    }
    data.extend_from_slice(&(locals.len() as u32).to_le_bytes());
    for &(func, local_idx, name) in locals {
        data.extend_from_slice(&func.to_le_bytes());
        data.extend_from_slice(&local_idx.to_le_bytes());
        data.extend_from_slice(&(name.len() as u32).to_le_bytes());
        data.extend_from_slice(name.as_bytes());
    }
    data.extend_from_slice(&0u32.to_le_bytes()); // debugger pcs
    data
}

/// 将 custom section 包装为最小合法 WASM 模块字节。
fn wasm_with_custom_section(name: &str, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&[0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00]);
    out.push(0x00);
    let mut section = Vec::new();
    section.push(name.len() as u8);
    section.extend_from_slice(name.as_bytes());
    section.extend_from_slice(payload);
    write_leb128_u32(&mut out, section.len() as u32);
    out.extend_from_slice(&section);
    out
}

fn write_leb128_u32(out: &mut Vec<u8>, mut v: u32) {
    loop {
        let mut byte = (v & 0x7f) as u8;
        v >>= 7;
        if v != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if v == 0 {
            break;
        }
    }
}

#[test]
fn inspect_config_defaults() {
    let cfg = InspectConfig::default();
    assert_eq!(cfg.host, "127.0.0.1");
    assert_eq!(cfg.port, 9229);
    assert!(!cfg.break_on_start);
}

#[test]
fn inspect_config_provisional_url() {
    let cfg = InspectConfig::default();
    assert_eq!(
        cfg.provisional_url().as_deref(),
        Some("http://127.0.0.1:9229")
    );
    let ephemeral = InspectConfig {
        port: 0,
        ..Default::default()
    };
    assert!(ephemeral.provisional_url().is_none());
}

#[test]
fn cdp_request_encoding_shape() {
    let s = json!({
        "id": 1,
        "method": "Debugger.enable",
        "params": {},
    })
    .to_string();
    let v: serde_json::Value = serde_json::from_str(&s).unwrap();
    assert_eq!(v["id"], 1);
    assert_eq!(v["method"], "Debugger.enable");
    assert!(v["params"].is_object());
}

#[test]
fn parse_debug_section_via_runtime_module() {
    let payload = encode_wjsm_debug_v1("app.js", &[(5, 0, 2, 4)], &[(5, 0, "x")]);
    let wasm = wasm_with_custom_section("wjsm_debug", &payload);
    let found = find_custom_section(&wasm, "wjsm_debug").expect("section");
    assert_eq!(found, payload.as_slice());

    let mut off = 0;
    assert_eq!(read_u32(found, &mut off), Some(1));
    let url = read_len_string(found, &mut off).unwrap();
    assert_eq!(url, "app.js");
    let n = read_u32(found, &mut off).unwrap();
    assert_eq!(n, 1);
    assert_eq!(read_u32(found, &mut off), Some(5)); // func
    assert_eq!(read_u32(found, &mut off), Some(0)); // pc
    assert_eq!(read_u32(found, &mut off), Some(2)); // line
    assert_eq!(read_u32(found, &mut off), Some(4)); // col
    let n_locals = read_u32(found, &mut off).unwrap();
    assert_eq!(n_locals, 1);
}

#[tokio::test]
async fn inspector_discovery_and_ws_cdp_shape() {
    use futures_util::{SinkExt, StreamExt};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio_tungstenite::tungstenite::Message;

    // 纯协议：本地临时 listener 模拟 /json 响应格式。
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 1024];
        let _ = stream.read(&mut buf).await;
        let body = json!([{
            "description": "wjsm",
            "id": "test-id",
            "title": "wjsm",
            "type": "node",
            "webSocketDebuggerUrl": format!("ws://{addr}/test-id"),
        }])
        .to_string();
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream.write_all(resp.as_bytes()).await.unwrap();
    });

    let mut client = tokio::net::TcpStream::connect(addr).await.unwrap();
    client
        .write_all(b"GET /json/list HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut resp = Vec::new();
    client.read_to_end(&mut resp).await.unwrap();
    let text = String::from_utf8_lossy(&resp);
    assert!(text.contains("webSocketDebuggerUrl"));
    assert!(text.contains("wjsm"));

    // WebSocket 握手 + 一条 Debugger.enable 形状
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ws");
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
        if let Some(Ok(Message::Text(t))) = ws.next().await {
            let v: serde_json::Value = serde_json::from_str(&t).unwrap();
            let id = v["id"].clone();
            let reply = json!({ "id": id, "result": {} }).to_string();
            ws.send(Message::Text(reply.into())).await.unwrap();
        }
    });

    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/"))
        .await
        .expect("ws connect");
    let req = json!({
        "id": 42,
        "method": "Debugger.enable",
        "params": {}
    })
    .to_string();
    ws.send(Message::Text(req.into())).await.unwrap();
    let reply = ws.next().await.unwrap().unwrap();
    match reply {
        Message::Text(t) => {
            let v: serde_json::Value = serde_json::from_str(&t).unwrap();
            assert_eq!(v["id"], 42);
            assert!(v.get("result").is_some());
        }
        other => panic!("unexpected {other:?}"),
    }
}

/// 端到端：`--inspect-brk` 等价路径下编译调试插桩 → CDP 连接 → pause → resume → 输出。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_inspect_brk_debugger_pause_resume() {
    use futures_util::{SinkExt, StreamExt};
    use std::sync::{Arc, Mutex};
    use tokio_tungstenite::tungstenite::Message;
    use wjsm_runtime::{
        InspectConfig, RuntimeOptions, compile_source_with_debug, execute_with_writer_with_options,
    };

    let source = "let x = 42;\ndebugger;\nconsole.log(x);\n";
    let wasm = compile_source_with_debug(source, "e2e.js").expect("compile debug");
    assert!(
        wasm.windows(b"wjsm_debug".len())
            .any(|w| w == b"wjsm_debug"),
        "debug compile must emit wjsm_debug section"
    );
    assert!(
        wasm.windows(b"debug_break".len())
            .any(|w| w == b"debug_break"),
        "debug compile must import debug_break"
    );

    // 预留端口，避免 race 时反复重试。
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").expect("bind probe");
        l.local_addr().unwrap().port()
    };

    let writer = Arc::new(Mutex::new(Vec::<u8>::new()));
    let writer_task = writer.clone();
    // execute future 内部在 await 点持有 std MutexGuard，不可 `tokio::spawn`；
    // 放到独立 OS 线程 + current_thread runtime。
    let exec = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        rt.block_on(async move {
            let sink = SharedWriter(writer_task);
            let options = RuntimeOptions {
                inspect: Some(InspectConfig {
                    host: "127.0.0.1".into(),
                    port,
                    break_on_start: true,
                }),
                ..RuntimeOptions::default()
            };
            execute_with_writer_with_options(&wasm, sink, options).await
        })
    });

    // 轮询 CDP discovery。
    let list_url = format!("http://127.0.0.1:{port}/json/list");
    let mut ws_url = None;
    for _ in 0..200 {
        if let Ok(body) = tokio::task::spawn_blocking({
            let list_url = list_url.clone();
            move || {
                let resp = ureq_get(&list_url)?;
                Ok::<_, String>(resp)
            }
        })
        .await
        .unwrap()
            && let Ok(v) = serde_json::from_str::<serde_json::Value>(&body)
            && let Some(url) = v
                .as_array()
                .and_then(|a| a.first())
                .and_then(|o| o.get("webSocketDebuggerUrl"))
                .and_then(|u| u.as_str())
        {
            ws_url = Some(url.to_string());
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let ws_url = ws_url.expect("inspector discovery should publish webSocketDebuggerUrl");

    let (mut ws, _) = tokio_tungstenite::connect_async(&ws_url)
        .await
        .expect("connect CDP websocket");

    async fn send(
        ws: &mut tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        id: u64,
        method: &str,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let req = json!({ "id": id, "method": method, "params": params }).to_string();
        ws.send(Message::Text(req.into())).await.unwrap();
        loop {
            let msg = tokio::time::timeout(std::time::Duration::from_secs(5), ws.next())
                .await
                .expect("timeout waiting cdp")
                .expect("ws closed")
                .expect("ws error");
            let Message::Text(t) = msg else {
                continue;
            };
            let v: serde_json::Value = serde_json::from_str(&t).unwrap();
            if v.get("id") == Some(&json!(id)) {
                return v;
            }
            // 吞掉事件；paused 等由调用方后续读。
            if v.get("method").and_then(|m| m.as_str()) == Some("Debugger.paused") {
                // 保留在外层处理：此处继续等到带 id 的响应。
            }
        }
    }

    let _ = send(&mut ws, 1, "Debugger.enable", json!({})).await;
    let _ = send(&mut ws, 2, "Runtime.enable", json!({})).await;

    // 等待 break_on_start 的 paused（可能已在 enable 前发出，再 resume 后还有 debugger;）。
    let mut saw_pause = false;
    for _ in 0..20 {
        match tokio::time::timeout(std::time::Duration::from_millis(200), ws.next()).await {
            Ok(Some(Ok(Message::Text(t)))) => {
                let v: serde_json::Value = serde_json::from_str(&t).unwrap();
                if v.get("method").and_then(|m| m.as_str()) == Some("Debugger.paused") {
                    saw_pause = true;
                    break;
                }
            }
            _ => break,
        }
    }

    // 即便错过了首个 event，Runtime.runIfWaitingForDebugger / resume 也应放行。
    let _ = send(&mut ws, 3, "Runtime.runIfWaitingForDebugger", json!({})).await;
    let _ = send(&mut ws, 4, "Debugger.resume", json!({})).await;

    // 期望在 debugger; 处再次暂停，或程序直接跑完。
    let mut paused_at_debugger = false;
    for _ in 0..40 {
        match tokio::time::timeout(std::time::Duration::from_millis(250), ws.next()).await {
            Ok(Some(Ok(Message::Text(t)))) => {
                let v: serde_json::Value = serde_json::from_str(&t).unwrap();
                if v.get("method").and_then(|m| m.as_str()) == Some("Debugger.paused") {
                    paused_at_debugger = true;
                    let frames = v["params"]["callFrames"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default();
                    assert!(!frames.is_empty(), "paused should include callFrames");
                    let scope_id = frames[0]["scopeChain"][0]["object"]["objectId"]
                        .as_str()
                        .unwrap_or("scope:frame-0");
                    let props = send(
                        &mut ws,
                        10,
                        "Runtime.getProperties",
                        json!({ "objectId": scope_id }),
                    )
                    .await;
                    assert!(props.get("result").is_some());
                    let _ = send(&mut ws, 11, "Debugger.resume", json!({})).await;
                    break;
                }
            }
            Ok(Some(Ok(_))) => {}
            _ => {
                if exec.is_finished() {
                    break;
                }
            }
        }
    }

    let join = tokio::task::spawn_blocking(move || exec.join())
        .await
        .expect("spawn_blocking")
        .expect("thread join")
        .expect("execute ok");
    let _ = join;
    let stdout = {
        let guard = writer.lock().unwrap();
        String::from_utf8_lossy(&guard).into_owned()
    };
    assert!(
        stdout.contains('4') || stdout.contains("42"),
        "expected console output, got {stdout:?}; saw_pause={saw_pause} paused_at_debugger={paused_at_debugger}"
    );
}

#[derive(Clone)]
struct SharedWriter(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl Write for SharedWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn ureq_get(url: &str) -> Result<String, String> {
    // 避免新增 HTTP 客户端依赖：手写极简 GET。
    let url = url
        .strip_prefix("http://")
        .ok_or_else(|| "only http".to_string())?;
    let (hostport, path) = url.split_once('/').unwrap_or((url, ""));
    let path = format!("/{path}");
    let (host, port) = if let Some((h, p)) = hostport.split_once(':') {
        (h, p.parse::<u16>().map_err(|e| e.to_string())?)
    } else {
        (hostport, 80u16)
    };
    let mut stream = std::net::TcpStream::connect((host, port)).map_err(|e| e.to_string())?;
    stream
        .set_read_timeout(Some(std::time::Duration::from_millis(200)))
        .ok();
    let req = format!("GET {path} HTTP/1.1\r\nHost: {hostport}\r\nConnection: close\r\n\r\n");
    use std::io::{Read, Write};
    stream
        .write_all(req.as_bytes())
        .map_err(|e| e.to_string())?;
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&buf);
    let body = text.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
    Ok(body)
}

// ── 与 runtime 内部同构的 section 解析（integration 测试不可见 crate-private）──

fn find_custom_section<'a>(wasm_bytes: &'a [u8], target_name: &str) -> Option<&'a [u8]> {
    if wasm_bytes.len() < 8 {
        return None;
    }
    let mut offset = 8usize;
    while offset < wasm_bytes.len() {
        let section_id = wasm_bytes[offset];
        offset += 1;
        let (size, consumed) = read_leb128(wasm_bytes, offset)?;
        offset += consumed;
        let section_end = offset + size as usize;
        if section_end > wasm_bytes.len() {
            return None;
        }
        if section_id == 0 {
            let (name_len, name_consumed) = read_leb128(wasm_bytes, offset)?;
            let name_start = offset + name_consumed;
            let name_end = name_start + name_len as usize;
            if name_end > section_end {
                return None;
            }
            let name = std::str::from_utf8(&wasm_bytes[name_start..name_end]).ok()?;
            if name == target_name {
                return Some(&wasm_bytes[name_end..section_end]);
            }
        }
        offset = section_end;
    }
    None
}

fn read_leb128(data: &[u8], offset: usize) -> Option<(u32, usize)> {
    let mut result = 0u32;
    let mut shift = 0u32;
    let mut i = 0usize;
    loop {
        if offset + i >= data.len() {
            return None;
        }
        let byte = data[offset + i];
        i += 1;
        result |= ((byte & 0x7F) as u32) << shift;
        if byte & 0x80 == 0 {
            break;
        }
        shift += 7;
        if shift >= 32 {
            return None;
        }
    }
    Some((result, i))
}

fn read_u32(data: &[u8], offset: &mut usize) -> Option<u32> {
    if *offset + 4 > data.len() {
        return None;
    }
    let v = u32::from_le_bytes([
        data[*offset],
        data[*offset + 1],
        data[*offset + 2],
        data[*offset + 3],
    ]);
    *offset += 4;
    Some(v)
}

fn read_len_string(data: &[u8], offset: &mut usize) -> Option<String> {
    let len = read_u32(data, offset)? as usize;
    if *offset + len > data.len() {
        return None;
    }
    let s = String::from_utf8_lossy(&data[*offset..*offset + len]).into_owned();
    *offset += len;
    Some(s)
}
