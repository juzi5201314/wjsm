use std::io::{ErrorKind, Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use anyhow::Result;
use tokio::runtime::Builder;
use wjsm_runtime::{
    RuntimeCompiler, RuntimeOptions, compile_source, execute_with_writer_with_options,
};

fn spawn_http_server() -> (String, mpsc::Sender<()>, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind perf_hooks fetch server");
    listener
        .set_nonblocking(true)
        .expect("configure perf_hooks fetch server");
    let address = listener.local_addr().expect("perf_hooks fetch address");
    let (shutdown_tx, shutdown_rx) = mpsc::channel();
    let server = thread::spawn(move || {
        let mut served = 0;
        while served < 6 {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    served += 1;
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
                    respond_to_request(&mut stream);
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => {
                    if shutdown_rx.try_recv().is_ok() {
                        return;
                    }
                    thread::sleep(Duration::from_millis(2));
                }
                Err(_) => return,
            }
        }
    });
    (format!("http://{address}"), shutdown_tx, server)
}

fn respond_to_request(stream: &mut std::net::TcpStream) {
    let mut request = [0_u8; 2048];
    let read = stream.read(&mut request).unwrap_or(0);
    let request = String::from_utf8_lossy(&request[..read]);
    let path = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/");
    let response = match path {
        "/empty" => "HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n",
        "/error" => concat!(
            "HTTP/1.1 200 OK\r\n",
            "Content-Length: 20\r\n",
            "Connection: close\r\n\r\n",
            "short"
        ),
        _ => concat!(
            "HTTP/1.1 200 OK\r\n",
            "Content-Length: 5\r\n",
            "Connection: close\r\n\r\n",
            "hello"
        ),
    };
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

fn run_source(source: &str, base_url: &str) -> Result<(String, String)> {
    let wasm = compile_source(source)?;
    let runtime = Builder::new_current_thread().enable_all().build()?;
    let options = RuntimeOptions {
        compiler: Some(RuntimeCompiler::Winch),
        env: vec![("WJSM_TEST_URL".to_string(), base_url.to_string())],
        ..RuntimeOptions::default()
    };
    let (stdout, diagnostics) = runtime
        .block_on(async { execute_with_writer_with_options(&wasm, Vec::new(), options).await })?;
    Ok((String::from_utf8(stdout)?, String::from_utf8(diagnostics)?))
}

#[test]
fn host_fetch_owns_resource_timing_completion() -> Result<()> {
    let (base_url, shutdown, server) = spawn_http_server();
    let result = run_source(
        r#"
const perfHost = globalThis.__wjsm_node_perf_hooks;
const entries = [];

function drainEntries() {
  let entry = perfHost.drainNativeEntry();
  while (entry !== undefined) {
    entries.push(entry);
    entry = perfHost.drainNativeEntry();
  }
}

function nextImmediate() {
  return new Promise((resolve) => setImmediate(resolve));
}

function drainReader(reader) {
  return reader.read().then((result) => {
    if (result.done) return;
    return drainReader(reader);
  });
}

const baseUrl = process.env.WJSM_TEST_URL;
perfHost.setObserverState(64, drainEntries);

const clonedResponse = await fetch(baseUrl + '/clone');
const clone = clonedResponse.clone();
await clone.text();
await nextImmediate();
const cloneCount = entries.length;
const originalBody = await clonedResponse.text();
await nextImmediate();
console.log(cloneCount === 1 && entries.length === 1 && originalBody === 'hello');

const readerResponse = await fetch(baseUrl + '/reader');
const reader = readerResponse.body.getReader();
await drainReader(reader);
await nextImmediate();
const readerTiming = entries[1].detail.timingInfo;
console.log(
  entries.length === 2 &&
  entries[1].name === baseUrl + '/reader' &&
  readerTiming.encodedBodySize === 5 &&
  readerTiming.decodedBodySize === 5
);

const cancelledResponse = await fetch(baseUrl + '/cancel');
await cancelledResponse.body.cancel();
await nextImmediate();
console.log(entries.length === 3 && entries[2].name === baseUrl + '/cancel');

const emptyResponse = await fetch(baseUrl + '/empty');
await nextImmediate();
console.log(
  emptyResponse.body === null &&
  entries.length === 4 &&
  entries[3].detail.responseStatus === 204
);

const failedResponse = await fetch(baseUrl + '/error');
try {
  await failedResponse.text();
} catch {}
await nextImmediate();
console.log(entries.length === 5 && entries[4].name === baseUrl + '/error');

const beforeInternal = entries.length;
const internalResponse = await fetch(baseUrl + '/internal', {
  __wjsm_internal_no_resource_timing: true,
});
await internalResponse.text();
await nextImmediate();
console.log(entries.length === beforeInternal);
perfHost.setObserverState(0, undefined);
"#,
        &base_url,
    );
    let _ = shutdown.send(());
    server.join().expect("join perf_hooks fetch server");
    let (stdout, stderr) = result?;
    assert_eq!(stdout, "true\ntrue\ntrue\ntrue\ntrue\ntrue\n");
    assert!(stderr.is_empty(), "unexpected stderr: {stderr:?}");
    Ok(())
}
