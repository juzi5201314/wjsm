use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use anyhow::Result;
use tokio::runtime::Builder;
use wjsm_runtime::{compile_source, execute_with_writer};

fn run_async(source: &str) -> Result<String> {
    let wasm = compile_source(source)?;
    let rt = Builder::new_current_thread().enable_all().build()?;
    let out = rt.block_on(async { execute_with_writer(&wasm, Vec::new()).await })?;
    Ok(String::from_utf8(out)?)
}

#[allow(dead_code)]
fn run_async_or_diag(source: &str) -> String {
    match run_async(source) {
        Ok(s) => s,
        Err(e) => format!("<runtime error: {e:#}>"),
    }
}

fn spawn_chunked_server(chunks: Vec<(Vec<u8>, Duration)>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind local test server");
    let addr = listener.local_addr().expect("local addr");
    thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut request_buf = [0_u8; 2048];
            let _ = stream.read(&mut request_buf);
            let _ = stream.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
            );
            let _ = stream.flush();
            for (chunk, delay_before) in chunks {
                if !delay_before.is_zero() {
                    thread::sleep(delay_before);
                }
                if write!(stream, "{:x}\r\n", chunk.len()).is_err() {
                    break;
                }
                if stream.write_all(&chunk).is_err() {
                    break;
                }
                if stream.write_all(b"\r\n").is_err() {
                    break;
                }
                let _ = stream.flush();
            }
            let _ = stream.write_all(b"0\r\n\r\n");
            let _ = stream.flush();
        }
    });
    format!("http://{}", addr)
}

/// 起一个分块服务器：先立即发出 `first` 块，然后阻塞在 gate 上，
/// 收到 release 信号后才发出 `rest` 块并结束 body。
/// 返回 (url, release_tx)：测试在程序结束后再 release，
/// 从而确定性地证明首个 read 在 body 结束之前就已 resolve（无需依赖墙钟时间）。
fn spawn_gated_server(first: Vec<u8>, rest: Vec<Vec<u8>>) -> (String, mpsc::Sender<()>) {
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind local test server");
    let addr = listener.local_addr().expect("local addr");
    thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut request_buf = [0_u8; 2048];
            let _ = stream.read(&mut request_buf);
            let _ = stream.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
            );
            let _ = stream.flush();
            let _ = write!(stream, "{:x}\r\n", first.len());
            let _ = stream.write_all(&first);
            let _ = stream.write_all(b"\r\n");
            let _ = stream.flush();
            // 阻塞直到测试 release（或 sender 被 drop）才继续发送剩余 body。
            let _ = release_rx.recv();
            for chunk in rest {
                if write!(stream, "{:x}\r\n", chunk.len()).is_err() {
                    break;
                }
                if stream.write_all(&chunk).is_err() {
                    break;
                }
                if stream.write_all(b"\r\n").is_err() {
                    break;
                }
                let _ = stream.flush();
            }
            let _ = stream.write_all(b"0\r\n\r\n");
            let _ = stream.flush();
        }
    });
    (format!("http://{}", addr), release_tx)
}

/// 起一个屏障服务器：每当收到一个连接，就触发一次 `release`（放行被 gate 阻塞的
/// body 服务器），然后回一个最小 200 响应。被测 JS 在读完首块后 `await fetch(barrier)`，
/// 由此确定性地保证后续块只在首块被消费之后才会发出——无需依赖墙钟时间。
fn spawn_barrier_server(release: mpsc::Sender<()>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind barrier server");
    let addr = listener.local_addr().expect("barrier addr");
    thread::spawn(move || {
        for incoming in listener.incoming() {
            let Ok(mut stream) = incoming else { break };
            let mut request_buf = [0_u8; 2048];
            let _ = stream.read(&mut request_buf);
            // 先放行 body 服务器，再回响应：确保 barrier fetch resolve 时后续块已在途。
            let _ = release.send(());
            let _ = stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok");
            let _ = stream.flush();
        }
    });
    format!("http://{}", addr)
}

#[test]
fn fetch_http_reader_reads_all_chunks() -> Result<()> {
    // body 服务器立即发出 "abc"，随后阻塞在 gate 上；只有 barrier 服务器被访问后
    // 才放行 "defg" + body 结束标志。被测 JS 在读完首块后访问一次 barrier，
    // 从而确定性地把两个 chunk 分到两次 read（reads == 2），不依赖墙钟时间或 reqwest
    // 的帧缓冲行为。
    let (url, release) = spawn_gated_server(b"abc".to_vec(), vec![b"defg".to_vec()]);
    let barrier = spawn_barrier_server(release);
    let output = run_async_or_diag(&format!(
        r#"
        fetch({url:?}).then(resp => resp.body.getReader()).then(reader => {{
          let total = 0;
          let reads = 0;
          let released = false;
          function pump() {{
            return reader.read().then(r => {{
              if (r.done) {{
                console.log("reads", reads);
                console.log("total", total);
                return;
              }}
              reads++;
              total += r.value.length;
              if (!released) {{
                // 读完首块后访问 barrier，放行后续块；保证后续块落在下一次 read。
                released = true;
                return fetch({barrier:?}).then(() => pump());
              }}
              return pump();
            }});
          }}
          return pump();
        }});
        "#,
    ));
    assert!(
        output.contains("total 7\n"),
        "unexpected output: {output:?}"
    );
    assert!(
        output.contains("reads 2\n"),
        "response must survive past first chunk: {output:?}"
    );
    Ok(())
}

#[test]
fn fetch_http_first_read_resolves_before_end_of_body() -> Result<()> {
    // 服务器立即发出首块 "a"，随后阻塞在 gate 上；只有测试在程序结束后
    // 才 release 第二块 + body 结束标志。因此若程序能跑完并产出输出，
    // 就确定性地证明首个 read 在 body 结束之前已经 resolve——无需墙钟断言。
    let (url, release) = spawn_gated_server(b"a".to_vec(), vec![b"b".to_vec()]);
    let output = run_async_or_diag(&format!(
        r#"
        const resp = await fetch({url:?});
        const reader = resp.body.getReader();
        const r = await reader.read();
        console.log("done", r.done);
        console.log("len", r.value.length);
        "#,
    ));
    // 程序已结束，放行服务器线程，使其干净退出（写入已关闭的 socket 会被忽略）。
    let _ = release.send(());
    assert!(
        output.contains("done false\n"),
        "first read must resolve before end-of-body: {output:?}"
    );
    assert!(output.contains("len 1\n"), "unexpected output: {output:?}");
    Ok(())
}

#[test]
fn fetch_http_byob_reader_fills_supplied_view() -> Result<()> {
    let url = spawn_chunked_server(vec![(b"hello".to_vec(), Duration::ZERO)]);
    let output = run_async_or_diag(&format!(
        r#"
        const resp = await fetch({url:?});
        const reader = resp.body.getReader({{ mode: "byob" }});
        const view = new Uint8Array(3);
        const r1 = await reader.read(view);
        console.log("done", r1.done);
        console.log("len", r1.value.length);
        console.log("bytes", r1.value[0], r1.value[1], r1.value[2]);
        const r2 = await reader.read(new Uint8Array(8));
        console.log("second", r2.done, r2.value.length, r2.value[0], r2.value[1]);
        "#,
    ));
    assert!(
        output.contains("done false\n"),
        "unexpected output: {output:?}"
    );
    assert!(output.contains("len 3\n"), "unexpected output: {output:?}");
    assert!(
        output.contains("bytes 104 101 108\n"),
        "unexpected output: {output:?}"
    );
    assert!(
        output.contains("second false 2 108 111\n"),
        "overflow bytes must be preserved: {output:?}"
    );
    Ok(())
}

#[test]
fn response_text_after_body_reader_rejects() -> Result<()> {
    let url = spawn_chunked_server(vec![(b"abc".to_vec(), Duration::ZERO)]);
    let output = run_async_or_diag(&format!(
        r#"
        const resp = await fetch({url:?});
        resp.body.getReader();
        resp.text().then(
          () => console.log("unexpected"),
          err => console.log("rejected", err.name || "TypeError")
        );
        "#,
    ));
    assert!(
        output.contains("rejected"),
        "body consumer must reject after getReader: {output:?}"
    );
    Ok(())
}

#[test]
fn response_text_consumes_http_body_once() -> Result<()> {
    let url = spawn_chunked_server(vec![(b"abc".to_vec(), Duration::ZERO)]);
    let output = run_async_or_diag(&format!(
        r#"
        const resp = await fetch({url:?});
        const text = await resp.text();
        console.log(text);
        resp.arrayBuffer().then(
          () => console.log("unexpected"),
          err => console.log("second rejected", err.name || "TypeError")
        );
        "#,
    ));
    assert!(output.contains("abc\n"), "unexpected output: {output:?}");
    assert!(
        output.contains("second rejected"),
        "second consumer must reject: {output:?}"
    );
    Ok(())
}
