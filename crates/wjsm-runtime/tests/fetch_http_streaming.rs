use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use tokio::runtime::Runtime;
use wjsm_runtime::execute_with_writer;

fn compile_source(source: &str) -> Result<Vec<u8>> {
    let module = wjsm_parser::parse_module(source)?;
    let program = wjsm_semantic::lower_module(module, false)?;
    wjsm_backend_wasm::compile(&program)
}

fn run_async(source: &str) -> Result<String> {
    let wasm = compile_source(source)?;
    let rt = Runtime::new()?;
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

#[test]
fn fetch_http_reader_reads_all_chunks() -> Result<()> {
    let url = spawn_chunked_server(vec![
        (b"abc".to_vec(), Duration::ZERO),
        (b"defg".to_vec(), Duration::from_millis(30)),
    ]);
    let output = run_async_or_diag(&format!(
        r#"
        fetch({url:?}).then(resp => resp.body.getReader()).then(reader => {{
          let total = 0;
          let reads = 0;
          function pump() {{
            return reader.read().then(r => {{
              if (r.done) {{
                console.log("reads", reads);
                console.log("total", total);
                return;
              }}
              reads++;
              total += r.value.length;
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
    let url = spawn_chunked_server(vec![
        (b"a".to_vec(), Duration::ZERO),
        (b"b".to_vec(), Duration::from_millis(1500)),
    ]);
    let start = Instant::now();
    let output = run_async_or_diag(&format!(
        r#"
        const resp = await fetch({url:?});
        const reader = resp.body.getReader();
        const r = await reader.read();
        console.log("done", r.done);
        console.log("len", r.value.length);
        "#,
    ));
    assert!(
        start.elapsed() < Duration::from_millis(1200),
        "first read waited for end-of-body: {output:?}"
    );
    assert!(
        output.contains("done false\n"),
        "unexpected output: {output:?}"
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
