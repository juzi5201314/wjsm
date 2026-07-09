//! Inspector TCP/HTTP/WebSocket 服务端。
//!
//! 兼容 Node inspector 发现协议：
//! - `GET /json`、`GET /json/list`
//! - `GET /json/version`
//! - WebSocket upgrade（任意路径，含 `/{session_id}`）

use super::cdp;
use super::InspectorHandle;
use futures_util::{SinkExt, StreamExt};
use std::sync::atomic::Ordering;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

pub(crate) async fn accept_loop(listener: TcpListener, handle: InspectorHandle) {
    loop {
        if handle.shutdown.load(Ordering::Relaxed) {
            break;
        }
        let accept = listener.accept().await;
        let Ok((stream, _addr)) = accept else {
            continue;
        };
        let handle = handle.clone();
        tokio::spawn(async move {
            if let Err(_err) = handle_connection(stream, handle).await {
                // 客户端断开属于常态。
            }
        });
    }
}

async fn handle_connection(mut stream: TcpStream, handle: InspectorHandle) -> anyhow::Result<()> {
    let mut peek_buf = [0u8; 1024];
    let n = stream.peek(&mut peek_buf).await?;
    if n == 0 {
        return Ok(());
    }
    let head = String::from_utf8_lossy(&peek_buf[..n]);
    let first_line = head.lines().next().unwrap_or("");
    let path = parse_request_path(first_line);

    if is_websocket_upgrade(&head) {
        return serve_websocket(stream, handle).await;
    }

    let mut req_buf = vec![0u8; n.max(4096)];
    let _ = stream.read(&mut req_buf).await?;
    let body = match path.as_str() {
        "/json" | "/json/list" | "/json/list/" => json_list_body(&handle),
        "/json/version" | "/json/version/" => json_version_body(),
        p if p.starts_with("/json/activate") => "{}".to_string(),
        _ => json_list_body(&handle),
    };
    write_http_json(&mut stream, &body).await?;
    Ok(())
}

async fn serve_websocket(stream: TcpStream, handle: InspectorHandle) -> anyhow::Result<()> {
    let ws = tokio_tungstenite::accept_async(stream).await?;
    let (mut sink, mut source) = ws.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    // 同一通道既承载本会话 CDP 响应，也接收广播事件。
    let response_tx = tx.clone();
    handle.register_session(tx).await;

    let write_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if sink.send(Message::Text(msg.into())).await.is_err() {
                break;
            }
        }
    });

    while let Some(msg) = source.next().await {
        let msg = msg?;
        match msg {
            Message::Text(text) => {
                let replies = cdp::handle_message(&handle, text.as_str()).await;
                for reply in replies {
                    if response_tx.send(reply).is_err() {
                        break;
                    }
                }
            }
            Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => {}
            Message::Binary(_) => {}
            Message::Close(_) => break,
        }
    }

    write_task.abort();
    Ok(())
}

fn parse_request_path(first_line: &str) -> String {
    let mut parts = first_line.split_whitespace();
    let _method = parts.next();
    let path = parts.next().unwrap_or("/");
    path.split('?').next().unwrap_or(path).to_string()
}

fn is_websocket_upgrade(head: &str) -> bool {
    let lower = head.to_ascii_lowercase();
    lower.contains("upgrade: websocket")
}

fn json_list_body(handle: &InspectorHandle) -> String {
    let ws = handle.ws_url();
    let id = handle.session_id.as_str();
    let host_port = format!("{}:{}", handle.host, handle.port.load(Ordering::Relaxed));
    let devtools_frontend = format!(
        "devtools://devtools/bundled/js_app.html?experiments=true&v8only=true&ws={host_port}/{id}"
    );
    serde_json::json!([{
        "description": "wjsm",
        "devtoolsFrontendUrl": devtools_frontend,
        "devtoolsFrontendUrlCompat": devtools_frontend,
        "faviconUrl": "https://nodejs.org/static/images/favicons/favicon.png",
        "id": id,
        "title": "wjsm",
        "type": "node",
        "url": "file://",
        "webSocketDebuggerUrl": ws,
    }])
    .to_string()
}

fn json_version_body() -> String {
    serde_json::json!({
        "Browser": "wjsm/0.1",
        "Protocol-Version": "1.3",
        "V8-Version": "0.0.0-wjsm",
        "WebKit-Version": "0.0.0",
    })
    .to_string()
}

async fn write_http_json(stream: &mut TcpStream, body: &str) -> anyhow::Result<()> {
    let resp = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json; charset=UTF-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    stream.write_all(resp.as_bytes()).await?;
    stream.shutdown().await.ok();
    Ok(())
}
