# Fetch/Streams Streaming Completion Implementation Plan

**Date**: 2026-06-13  
**Spec**: [docs/aegis/specs/2026-06-13-fetch-streams-completion-design.md](../specs/2026-06-13-fetch-streams-completion-design.md)

## Goal

Complete the approved Fetch/Streams streaming scope without writing duplicate owners: HTTP/HTTPS `Response.body` is a real reader-driven byte stream, BYOB byte-stream requests are functional, full-body consumers share the same HTTP body owner, and fetch/streams-specific dead code is retired.

## Architecture

```text
fetch(input, init)
  -> fetch_http::parse_fetch_input using shared Request parsing
  -> fetch_http::perform_http_fetch awaits headers through reqwest
  -> fetch_core creates Response and canonical ReadableStream body
  -> streams_fetch_body::call_fetch_body_reader_read handles reader.read()
  -> take reqwest::Response from http_response_table
  -> tokio worker awaits one chunk while owning Response
  -> AsyncHostCompletion::Materialize puts Response back or marks EOF/error
  -> scheduler owner constructs JS result and settles the read Promise
```

BYOB source reads use the same stream controller table:

```text
reader.read(view) on byte stream with empty queue
  -> create ByobRequestEntry and set controller.active_byob_request
  -> schedule ReadableStreamPull microtask if pull callback exists
  -> controller.byobRequest getter exposes { view, respond(n) }
  -> respond(n) validates, creates result view length n, fulfills read Promise
```

## Tech Stack

- Rust 2024.
- `reqwest = { version = "0.12", features = ["rustls-tls", "stream"] }` already configured.
- Existing Tokio runtime and scheduler owner model.
- Existing `AsyncHostCompletion::Materialize` channel.
- Existing runtime side-table + `NativeCallable` dispatch pattern.

## Baseline/Authority Refs

- Design spec: `docs/aegis/specs/2026-06-13-fetch-streams-completion-design.md`.
- Async authority: `docs/async-scheduler.md`.
- Existing fetch design: `docs/aegis/specs/2026-06-03-fetch-standard-design.md`.
- Existing streams design: `docs/aegis/specs/2026-06-03-streams-standard-design.md`.
- Current code owners:
  - `crates/wjsm-runtime/src/host_imports/fetch.rs`
  - `crates/wjsm-runtime/src/host_imports/fetch_http.rs`
  - `crates/wjsm-runtime/src/host_imports/fetch_core.rs`
  - `crates/wjsm-runtime/src/host_imports/streams_readable.rs`
  - `crates/wjsm-runtime/src/runtime_builtins.rs`
  - `crates/wjsm-runtime/src/lib.rs`

## Compatibility Boundary

| Boundary | Required behavior |
|---|---|
| `data:` fetch | Existing synchronous path remains; fixtures keep passing except BYOB result length correction where directly affected. |
| HTTP body owner | Only `http_response_table` owns fetch-backed body state. No second HTTP stream table. |
| Async scheduler | Workers never touch `Store`, `Caller`, Wasm memory, JS heap, or runtime tables. |
| Backpressure | One reader demand starts at most one network `chunk().await`. |
| BYOB result | Result view length equals bytes written. Oversized network chunks are buffered for the next read. |
| Body disturbance | `getReader()`, `text()`, `json()`, and `arrayBuffer()` reject conflicting second consumption. |
| Dead code | Legacy fetch-only stream method variants are removed after canonical ReadableStream path owns fetch-backed bodies. |

## Plan Basis

- **Fact**: `fetch` is already registered with `func_wrap_async` and HTTP requests already use `reqwest`.
- **Fact**: current `parse_fetch_input` ignores `init` and returns GET/default headers/no body/no signal.
- **Fact**: current HTTP reader path takes `HttpResponseEntry.response` and never puts it back, so only the first chunk can be read.
- **Fact**: current HTTP spawn sites do not use `AsyncOpGuard`, so the scheduler can exit before delayed materialization.
- **Fact**: `controller.byobRequest` is a data property set to `null`.
- **Assumption**: browser-only fetch policy features remain outside runtime scope per approved spec.
- **Unknown to resolve during implementation**: whether all current BYOB tests expect spec-correct returned view length; update only fixtures whose observed behavior intentionally changes.

## Files

### Create

- `crates/wjsm-runtime/src/host_imports/streams_fetch_body.rs`
- `crates/wjsm-runtime/tests/fetch_http_streaming.rs`
- `fixtures/happy/streams_byob_request_respond.js`
- `fixtures/happy/streams_byob_request_respond.expected`

### Modify

- `crates/wjsm-runtime/src/lib.rs`
- `crates/wjsm-runtime/src/host_imports/mod.rs`
- `crates/wjsm-runtime/src/host_imports/fetch.rs`
- `crates/wjsm-runtime/src/host_imports/fetch_http.rs`
- `crates/wjsm-runtime/src/host_imports/fetch_core.rs`
- `crates/wjsm-runtime/src/host_imports/streams_readable.rs`
- `crates/wjsm-runtime/src/host_imports/streams_transform.rs`
- `crates/wjsm-runtime/src/runtime_builtins.rs`
- `crates/wjsm-runtime/src/runtime_heap.rs`
- `fixtures/happy/streams_readable_byob.expected` if BYOB result length changes.
- `AGENTS.md` only after behavior is verified, to update the project status table.

## Plan Pressure Test

- **Owner / contract / retirement**: proceed. The plan keeps one HTTP body owner and retires legacy fetch-only stream callables.
- **Verification scope**: local HTTP runtime tests, affected fixtures, async scheduler regression, runtime check.
- **Task executability**: each task has exact files and commands; tests precede implementation.
- **Pressure result**: proceed.

## Plan-Time Complexity Check

- **Target files**: `fetch_core.rs` 1825 lines, `streams_readable.rs` 1647 lines, `lib.rs` 2913 lines.
- **Existing size / shape signals**: `fetch_core.rs` and `streams_readable.rs` are already large mixed-owner files.
- **Owner fit**: fetch-backed body bridge is cross-cutting, so a new `streams_fetch_body.rs` avoids further bloating generic stream code.
- **Add-in-place risk**: adding HTTP body bridge code directly to `streams_readable.rs` hides fetch-specific state inside generic stream logic.
- **Better file boundary**: `streams_fetch_body.rs` owns only the fetch-backed ReadableStream body bridge.
- **Recommendation**: add owner file for bridge; edit existing files only at dispatch/data-structure seams.

## Risks

| Risk | Mitigation |
|---|---|
| Worker holds lock across await | Use take/spawn/materialize-put-back; no `MutexGuard` crosses `.await`. |
| Scheduler exits early | Move `AsyncOpGuard` into every spawned HTTP body task. |
| BYOB view GC | Trace reader pending views and BYOB request entries as side-table roots. |
| Chunk larger than BYOB view | Add `pending_bytes` queue to `HttpResponseEntry`; never discard overflow. |
| Duplicate stream APIs | Retire legacy `StreamMethod`/`ReaderMethod` after canonical ReadableStream path serves fetch bodies. |
| Test flakiness from external network | Use local `std::net::TcpListener` in runtime tests. |

## Retirement

- Delete `fetch_http::perform_fetch_and_build_response`.
- Remove fetch-only `create_readable_stream_object`, `create_reader_object`, `call_stream_method_from_caller`, and `call_reader_method_from_caller` from `fetch_core.rs` after fetch bodies use canonical ReadableStream object creation.
- Remove `NativeCallable::StreamMethod`, `NativeCallable::ReaderMethod`, `StreamMethodKind`, and `ReaderMethodKind` after no callsites remain.
- Remove `streams_readable::call_reader_http_read` or replace it with a delegating call to `streams_fetch_body::call_fetch_body_reader_read`; keep no duplicate implementation.
- Remove `streams_transform.rs::type_error_exception` if still unused.
- Replace BYOB reserved comments with active semantics comments.

## Tasks

### Task 1: Add RED runtime tests for HTTP streaming

**Files**: create `crates/wjsm-runtime/tests/fetch_http_streaming.rs`  
**Why**: prove current HTTP body code loses the response after one chunk and lacks BYOB body support.  
**Impact/Compatibility**: test-only; no runtime behavior change.  
**Verification**: `cargo nextest run -p wjsm-runtime -E 'test(fetch_http_streaming)'` should fail before implementation.

#### Write test

Create `crates/wjsm-runtime/tests/fetch_http_streaming.rs`:

```rust
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

fn spawn_chunked_server(chunks: Vec<(Vec<u8>, Duration)>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind local test server");
    let addr = listener.local_addr().expect("local addr");
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept request");
        let mut request_buf = [0_u8; 2048];
        let _ = stream.read(&mut request_buf);
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
            )
            .expect("write headers");
        stream.flush().expect("flush headers");
        for (chunk, delay_before) in chunks {
            if !delay_before.is_zero() {
                thread::sleep(delay_before);
            }
            write!(stream, "{:x}\r\n", chunk.len()).expect("write chunk size");
            stream.write_all(&chunk).expect("write chunk");
            stream.write_all(b"\r\n").expect("write chunk terminator");
            stream.flush().expect("flush chunk");
        }
        stream.write_all(b"0\r\n\r\n").expect("write eof");
        stream.flush().expect("flush eof");
    });
    format!("http://{}", addr)
}

#[test]
fn fetch_http_reader_reads_all_chunks() -> Result<()> {
    let url = spawn_chunked_server(vec![
        (b"abc".to_vec(), Duration::ZERO),
        (b"defg".to_vec(), Duration::from_millis(30)),
    ]);
    let output = run_async(&format!(
        r#"
        const resp = await fetch({url:?});
        const reader = resp.body.getReader();
        let total = 0;
        let reads = 0;
        while (true) {{
          const r = await reader.read();
          if (r.done) break;
          reads++;
          total += r.value.length;
        }}
        console.log("reads", reads);
        console.log("total", total);
        "#,
    ))?;
    assert!(output.contains("total 7\n"), "unexpected output: {output:?}");
    assert!(output.contains("reads 2\n"), "response must survive past first chunk: {output:?}");
    Ok(())
}

#[test]
fn fetch_http_first_read_resolves_before_end_of_body() -> Result<()> {
    let url = spawn_chunked_server(vec![
        (b"a".to_vec(), Duration::ZERO),
        (b"b".to_vec(), Duration::from_millis(500)),
    ]);
    let start = Instant::now();
    let output = run_async(&format!(
        r#"
        const resp = await fetch({url:?});
        const reader = resp.body.getReader();
        const r = await reader.read();
        console.log("done", r.done);
        console.log("len", r.value.length);
        "#,
    ))?;
    assert!(start.elapsed() < Duration::from_millis(450), "first read waited for end-of-body: {output:?}");
    assert!(output.contains("done false\n"), "unexpected output: {output:?}");
    assert!(output.contains("len 1\n"), "unexpected output: {output:?}");
    Ok(())
}

#[test]
fn fetch_http_byob_reader_fills_supplied_view() -> Result<()> {
    let url = spawn_chunked_server(vec![(b"hello".to_vec(), Duration::ZERO)]);
    let output = run_async(&format!(
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
    ))?;
    assert!(output.contains("done false\n"), "unexpected output: {output:?}");
    assert!(output.contains("len 3\n"), "unexpected output: {output:?}");
    assert!(output.contains("bytes 104 101 108\n"), "unexpected output: {output:?}");
    assert!(output.contains("second false 2 108 111\n"), "overflow bytes must be preserved: {output:?}");
    Ok(())
}

#[test]
fn response_text_after_body_reader_rejects() -> Result<()> {
    let url = spawn_chunked_server(vec![(b"abc".to_vec(), Duration::ZERO)]);
    let output = run_async(&format!(
        r#"
        const resp = await fetch({url:?});
        resp.body.getReader();
        resp.text().then(
          () => console.log("unexpected"),
          err => console.log("rejected", err.name || "TypeError")
        );
        "#,
    ))?;
    assert!(output.contains("rejected"), "body consumer must reject after getReader: {output:?}");
    Ok(())
}

#[test]
fn response_text_consumes_http_body_once() -> Result<()> {
    let url = spawn_chunked_server(vec![(b"abc".to_vec(), Duration::ZERO)]);
    let output = run_async(&format!(
        r#"
        const resp = await fetch({url:?});
        const text = await resp.text();
        console.log(text);
        resp.arrayBuffer().then(
          () => console.log("unexpected"),
          err => console.log("second rejected", err.name || "TypeError")
        );
        "#,
    ))?;
    assert!(output.contains("abc\n"), "unexpected output: {output:?}");
    assert!(output.contains("second rejected"), "second consumer must reject: {output:?}");
    Ok(())
}
```

#### Verify RED

Run:

```bash
cargo nextest run -p wjsm-runtime -E 'test(fetch_http_streaming)'
```

Expected before implementation: at least `fetch_http_reader_reads_all_chunks` and `fetch_http_byob_reader_fills_supplied_view` fail.

#### Minimal code

No production code in this task.

#### Verify GREEN

Deferred to later tasks.

#### Commit

Commit after Task 5 when the first test group passes.

### Task 2: Add RED BYOB request fixture

**Files**: create `fixtures/happy/streams_byob_request_respond.js`, create `.expected`.  
**Why**: lock expected `controller.byobRequest.respond(n)` semantics before implementation.  
**Impact/Compatibility**: fixture addition only.  
**Verification**: `cargo nextest run -E 'test(happy__streams_byob_request_respond)'` fails before implementation.

#### Write test

Create `fixtures/happy/streams_byob_request_respond.js`:

```javascript
let pulls = 0;
const stream = new ReadableStream({
  type: "bytes",
  pull(controller) {
    pulls++;
    console.log("has request", controller.byobRequest !== null);
    const req = controller.byobRequest;
    const view = req.view;
    console.log("view length", view.length);
    view[0] = 65;
    view[1] = 66;
    req.respond(2);
    controller.close();
  }
});

const reader = stream.getReader({ mode: "byob" });
const buffer = new Uint8Array(8);
const first = await reader.read(buffer);
console.log("done", first.done);
console.log("value length", first.value.length);
console.log("bytes", first.value[0], first.value[1]);
const second = await reader.read(new Uint8Array(4));
console.log("second done", second.done);
console.log("pulls", pulls);
```

Create `fixtures/happy/streams_byob_request_respond.expected`:

```text
exit_code: 0
--- stdout ---
has request true
view length 8
done false
value length 2
bytes 65 66
second done true
pulls 1
--- stderr ---
```

#### Verify RED

Run:

```bash
cargo nextest run -E 'test(happy__streams_byob_request_respond)'
```

Expected before implementation: fixture fails because `controller.byobRequest` is `null` and `pull` is not invoked for pending reads.

#### Minimal code

No production code in this task.

#### Verify GREEN

Deferred to Task 6.

#### Commit

Commit with Task 6.

### Task 3: Complete shared fetch input parsing

**Files**: `crates/wjsm-runtime/src/host_imports/fetch_http.rs`, `crates/wjsm-runtime/src/host_imports/fetch_core.rs`.  
**Why**: `fetch(input, init)` must use headers, method, body, redirect, and signal fields already modeled by Request.  
**Impact/Compatibility**: activates existing Request/Redirect/Abort fields; invalid inputs reject earlier and more correctly.  
**Verification**: existing `fetch_request_init` fixture plus targeted runtime check.

#### Write test

Add or extend a fixture only if existing coverage does not assert method/body/headers. Preferred fixture content for `fixtures/happy/fetch_request_init.js` if missing assertions:

```javascript
const r = new Request("data:text/plain,ignored", {
  method: "POST",
  headers: { "x-test": "1" },
  body: "payload",
  redirect: "manual"
});
console.log(r.method);
console.log(r.headers.get("x-test"));
console.log(r.redirect);
console.log(await r.text());
```

Expected output:

```text
POST
1
manual
payload
```

#### Verify RED

Run:

```bash
cargo nextest run -E 'test(happy__fetch_request_init)'
```

#### Minimal code

Refactor without duplicating parser logic:

1. In `fetch_core.rs`, extract a shared internal function returning the same tuple used by `fetch_http::parse_fetch_input`:

```rust
pub(crate) struct ParsedRequestParts {
    pub method: String,
    pub url: String,
    pub headers_handle: u32,
    pub body: Option<Vec<u8>>,
    pub redirect: RedirectMode,
    pub signal_handle: Option<u32>,
}
```

2. Move the common body of `construct_request` into a helper:

```rust
pub(crate) fn parse_request_parts(
    caller: &mut Caller<'_, RuntimeState>,
    input: i64,
    init: i64,
) -> std::result::Result<ParsedRequestParts, i64>
```

3. `construct_request` calls `parse_request_parts`, then `create_request_object` and `define_request_init_properties`.
4. `fetch_http::parse_fetch_input` calls `parse_request_parts`; on error, it returns an empty URL and lets `define_fetch` reject with the exception message path already used for invalid URL. If preserving exception identity is practical, change `parse_fetch_input` to return `Result<ParsedRequestParts, i64>` and reject with that exception in `define_fetch`.

#### Verify GREEN

Run:

```bash
cargo nextest run -E 'test(happy__fetch_request_init)'
cargo check -p wjsm-runtime
```

#### Commit

Commit message:

```text
fix: share fetch request parsing
```

### Task 4: Extend HTTP body state without duplicate owner

**Files**: `crates/wjsm-runtime/src/lib.rs`.  
**Why**: current `HttpResponseEntry` cannot represent in-flight, EOF, error, or partial BYOB overflow bytes.  
**Impact/Compatibility**: internal side-table shape only.  
**Verification**: `cargo check -p wjsm-runtime` after callsites are updated in Task 5.

#### Write test

Covered by Task 1 tests.

#### Verify RED

Task 1 tests remain red.

#### Minimal code

Replace `HttpResponseEntry` with:

```rust
#[derive(Debug)]
struct HttpResponseEntry {
    response: Option<reqwest::Response>,
    pending_read_promise: Option<i64>,
    pending_bytes: VecDeque<Vec<u8>>,
    eof: bool,
    error: Option<String>,
}
```

Update every construction site to initialize:

```rust
HttpResponseEntry {
    response: Some(response),
    pending_read_promise: None,
    pending_bytes: VecDeque::new(),
    eof: false,
    error: None,
}
```

#### Verify GREEN

Run after Task 5 because this task intentionally breaks current callsites:

```bash
cargo check -p wjsm-runtime
```

#### Commit

Commit with Task 5.

### Task 5: Implement fetch-backed body bridge with AsyncOpGuard

**Files**: create `streams_fetch_body.rs`; modify `host_imports/mod.rs`, `streams_readable.rs`, `fetch_core.rs`.  
**Why**: canonicalize HTTP body reader behavior and fix response loss after first chunk.  
**Impact/Compatibility**: HTTP `Response.body` becomes a canonical ReadableStream byte stream; legacy fetch-only stream wrappers are retired.  
**Verification**: Task 1 runtime tests start passing except BYOB request-specific cases handled in Task 6.

#### Write test

Use Task 1 tests.

#### Verify RED

Run:

```bash
cargo nextest run -p wjsm-runtime -E 'test(fetch_http_reader_reads_all_chunks)'
```

Expected before code: fails because current `response.take()` is not put back.

#### Minimal code

1. In `streams_readable.rs`, make the canonical object helper public inside crate:

```rust
pub(crate) fn create_readable_stream_js_object(
    caller: &mut Caller<'_, RuntimeState>,
    stream_handle: u32,
) -> i64
```

2. In `fetch_core.rs::create_readable_stream_object`, stop building the legacy minimal object. Instead create a `ReadableStreamEntry` with `is_byte_stream: true` and call `streams_readable::create_readable_stream_js_object`.

3. Create `streams_fetch_body.rs` with these public functions:

```rust
pub(crate) fn call_fetch_body_reader_read(
    caller: &mut Caller<'_, RuntimeState>,
    reader_handle: u32,
    http_handle: u32,
    byob_view: Option<i64>,
) -> Option<i64>;

pub(crate) fn consume_fetch_body_to_bytes(
    caller: &mut Caller<'_, RuntimeState>,
    http_handle: u32,
    promise: i64,
    kind: ResponseMethodKind,
) -> Option<()>;
```

4. `call_fetch_body_reader_read` must:
   - return done if `eof` and no `pending_bytes`;
   - serve `pending_bytes` before starting a network pull;
   - return existing pending promise if `pending_read_promise` is set;
   - take `response` out of the table, set pending, and spawn one worker;
   - move an `AsyncOpGuard` into the worker;
   - on `Ok(Some(chunk))`, Materialize writes BYOB or creates `Uint8Array`, stores overflow in `pending_bytes`, puts `response` back, clears pending, and fulfills;
   - on `Ok(None)`, Materialize marks EOF and fulfills done;
   - on `Err(e)`, Materialize stores error and rejects.

5. Update `streams_readable.rs::call_default_reader_method_from_caller` HTTP branch to call `call_fetch_body_reader_read(caller, handle, http_handle, byob_view)`.

6. Update `fetch_core.rs::call_response_method_from_caller` HTTP full-body consumer branch to use `consume_fetch_body_to_bytes` and `AsyncOpGuard`, with no direct `tokio::spawn` left in `fetch_core.rs` for HTTP body consumption.

7. Export bridge functions from `host_imports/mod.rs`.

#### Verify GREEN

Run:

```bash
cargo nextest run -p wjsm-runtime -E 'test(fetch_http_reader_reads_all_chunks) or test(fetch_http_first_read_resolves_before_end_of_body) or test(response_text_consumes_http_body_once) or test(response_text_after_body_reader_rejects)'
cargo check -p wjsm-runtime
```

#### Commit

Commit message:

```text
fix: stream HTTP response bodies incrementally
```

### Task 6: Complete BYOB request semantics

**Files**: `lib.rs`, `streams_readable.rs`, `runtime_builtins.rs`, `runtime_heap.rs`.  
**Why**: `controller.byobRequest` is currently always `null`; full byte streams need request/view/respond behavior.  
**Impact/Compatibility**: BYOB result view length changes to bytes-written length; update affected expected output.  
**Verification**: Task 2 fixture and Task 1 BYOB HTTP test pass.

#### Write test

Use Task 2 fixture and Task 1 `fetch_http_byob_reader_fills_supplied_view`.

#### Verify RED

Run:

```bash
cargo nextest run -E 'test(happy__streams_byob_request_respond)'
cargo nextest run -p wjsm-runtime -E 'test(fetch_http_byob_reader_fills_supplied_view)'
```

#### Minimal code

1. Add side-table structs and fields in `lib.rs`:

```rust
#[derive(Clone, Debug)]
struct ByobRequestEntry {
    controller_handle: u32,
    reader_handle: u32,
    view: i64,
    promise: i64,
    responded: bool,
}
```

Add to `RuntimeState` and clone/new:

```rust
byob_request_table: Arc<Mutex<Vec<ByobRequestEntry>>>,
```

Extend `StreamControllerEntry`:

```rust
underlying_source: Option<i64>,
pull_callback: Option<i64>,
cancel_callback: Option<i64>,
active_byob_request: Option<u32>,
```

2. Add native callable variants:

```rust
ReadableStreamByobRequestMethod { handle: u32, kind: ReadableStreamByobRequestMethodKind },
```

and kind enum:

```rust
#[derive(Clone, Copy)]
enum ReadableStreamByobRequestMethodKind {
    GetView,
    Respond,
}
```

3. In `construct_readable_stream`, store `source`, callable `pull`, and callable `cancel` in the controller entry.

4. Change `controller.byobRequest` from data property to accessor getter using `define_host_accessor_property_with_env`.

5. In BYOB `reader.read(view)` with empty queue:
   - create a pending promise;
   - set `reader.pending_read_promise` and `reader.pending_byob_view`;
   - create `ByobRequestEntry`;
   - set `controller.active_byob_request`;
   - schedule a `ReadableStreamPull` microtask if `pull_callback` exists.

6. Add `Microtask::ReadableStreamPull { callback, this_val, controller, read_promise }` and handle it in both `drain_microtasks` and `drain_microtasks_async`. The async branch calls `call_host_function_with_args_async`.

7. Implement `ReadableStreamBYOBRequest.view` getter and `respond(bytesWritten)`:
   - validate request exists and not responded;
   - validate `bytesWritten` is an integer in `[0, view.byteLength]`;
   - create result view over the same buffer with length `bytesWritten`;
   - fulfill promise with `{ done: false, value: resultView }`;
   - clear `controller.active_byob_request`, `reader.pending_byob_view`, and `reader.pending_read_promise`.

8. Update `fulfill_byob_read` to return a bytes-written-length result view and preserve overflow chunks.

#### Verify GREEN

Run:

```bash
cargo nextest run -E 'test(happy__streams_byob_request_respond) or test(happy__streams_readable_byob)'
cargo nextest run -p wjsm-runtime -E 'test(fetch_http_byob_reader_fills_supplied_view)'
cargo check -p wjsm-runtime
```

#### Commit

Commit message:

```text
feat: complete BYOB byte stream requests
```

### Task 7: Add GC roots for pending stream state

**Files**: `runtime_builtins.rs`.  
**Why**: pending BYOB views, BYOB request promises, pull callbacks, and HTTP pending read promises are side-table roots.  
**Impact/Compatibility**: prevents GC from collecting live objects during pending reads.  
**Verification**: targeted GC regression fixture plus existing tests.

#### Write test

Add `fixtures/happy/streams_byob_gc_pending_view.js`:

```javascript
let savedController;
const stream = new ReadableStream({
  type: "bytes",
  start(controller) { savedController = controller; }
});
const reader = stream.getReader({ mode: "byob" });
const view = new Uint8Array(4);
const p = reader.read(view).then(r => {
  console.log("done", r.done);
  console.log("len", r.value.length);
  console.log("byte0", r.value[0]);
});
for (let i = 0; i < 2000; i++) ({ i });
const req = savedController.byobRequest;
req.view[0] = 88;
req.respond(1);
await p;
```

Expected:

```text
exit_code: 0
--- stdout ---
done false
len 1
byte0 88
--- stderr ---
```

#### Verify RED

Run:

```bash
cargo nextest run -E 'test(happy__streams_byob_gc_pending_view)'
```

#### Minimal code

In `trace_runtime_side_table_roots_fixed_point`, add snapshots and mark:

- `ReaderEntry.pending_byob_view`.
- `ByobRequestEntry.view` and `ByobRequestEntry.promise`.
- `StreamControllerEntry.underlying_source`, `pull_callback`, `cancel_callback`.
- `HttpResponseEntry.pending_read_promise`.

Use snapshots to avoid holding table locks while recursively marking.

#### Verify GREEN

Run:

```bash
cargo nextest run -E 'test(happy__streams_byob_gc_pending_view)'
cargo nextest run -p wjsm-runtime -E 'test(fetch_http_streaming)'
```

#### Commit

Commit message:

```text
fix: trace pending stream side-table roots
```

### Task 8: Retire legacy fetch/streams dead code

**Files**: `lib.rs`, `runtime_builtins.rs`, `runtime_heap.rs`, `fetch_http.rs`, `fetch_core.rs`, `streams_readable.rs`, `streams_transform.rs`, `host_imports/mod.rs`.  
**Why**: user explicitly required no dead/unused code in relevant modules.  
**Impact/Compatibility**: removes duplicate and stale fetch-only stream API paths.  
**Verification**: no relevant dead-code warnings; affected tests pass.

#### Write test

No new behavior test. Use compiler and targeted behavior tests.

#### Verify RED

Run:

```bash
cargo check -p wjsm-runtime
```

Record existing relevant warnings before deletion: `perform_fetch_and_build_response`, `streams_transform::type_error_exception`, and any legacy stream variants that become unused after Task 5.

#### Minimal code

Remove all fetch/streams-specific unused items whose callsites are gone:

- `fetch_http::perform_fetch_and_build_response`.
- `fetch_core::create_reader_object` if no longer used.
- `fetch_core::call_stream_method_from_caller`.
- `fetch_core::call_reader_method_from_caller`.
- `NativeCallable::StreamMethod` and `NativeCallable::ReaderMethod`.
- `StreamMethodKind` and `ReaderMethodKind`.
- runtime builtins dispatch branches for removed variants.
- runtime heap tracing branches for removed variants.
- `streams_transform.rs::type_error_exception` if unused.

Update comments around BYOB fields to describe active behavior.

#### Verify GREEN

Run:

```bash
cargo check -p wjsm-runtime
cargo nextest run -p wjsm-runtime -E 'test(fetch_http_streaming) or test(async_scheduler) or test(async_reentry)'
cargo nextest run -E 'test(happy__streams_readable_byob) or test(happy__streams_byob_request_respond) or test(happy__streams_fetch_body_data_url) or test(happy__fetch_data_url)'
```

#### Commit

Commit message:

```text
refactor: retire legacy fetch stream paths
```

### Task 9: Update project status documentation after verification

**Files**: `AGENTS.md`, possibly `docs/async-scheduler.md`.  
**Why**: authority docs currently describe fetch as data-url-only/non-conformant and BYOB as partial in places.  
**Impact/Compatibility**: documentation only after behavior is verified.  
**Verification**: docs mention the verified behavior and keep non-goals explicit.

#### Write test

No code test.

#### Verify RED

Not applicable.

#### Minimal code

Update only factual status lines:

- In `AGENTS.md`, change fetch note from data-url-only/non-Promise Response to HTTP/HTTPS streaming body supported within runtime non-browser policy limits.
- In `AGENTS.md`, change BYOB note to include `ReadableStreamBYOBRequest` and fetch-backed body BYOB support.
- In `docs/async-scheduler.md`, add a short subsection under Worker boundary describing HTTP body pull workers and the requirement to use `AsyncOpGuard`.

#### Verify GREEN

Run:

```bash
cargo nextest run -p wjsm-runtime -E 'test(fetch_http_streaming)'
```

#### Commit

Commit message:

```text
docs: update fetch streams status
```

## Final Verification

Run the focused checks first:

```bash
cargo nextest run -p wjsm-runtime -E 'test(fetch_http_streaming) or test(async_scheduler) or test(async_reentry)'
cargo nextest run -E 'test(happy__streams_readable_byob) or test(happy__streams_byob_request_respond) or test(happy__streams_byob_gc_pending_view) or test(happy__streams_fetch_body_data_url) or test(happy__fetch_data_url) or test(happy__fetch_request_init)'
cargo check -p wjsm-runtime
```

After focused checks pass, run the broader fixture suite:

```bash
cargo nextest run -E 'test(happy__fetch) or test(happy__streams)'
```

Use full workspace only after focused runtime/fixture checks pass:

```bash
cargo nextest run --workspace
```

## Plan Self-Review

- **Spec coverage**: HTTP streaming, BYOB request, BYOB HTTP body, request parsing, GC roots, and dead-code retirement each have tasks.
- **Incomplete-marker scan**: no incomplete requirements are intentionally left in the plan.
- **Type consistency**: plan uses existing `http_response_table`; no duplicate `http_stream_table` appears.
- **Compatibility**: data URL path remains guarded; BYOB result length change is marked as spec-correct behavior.
- **Plan-time complexity**: bridge owner file avoids growing generic stream file further.
- **Verification**: each major slice has exact commands and expected red/green behavior.
- **Dual-track and retirement**: repair tasks and retirement tasks are both present.
- **ADR/baseline signal**: completion should backfill an ADR or baseline note for fetch-backed body bridge ownership and scheduler boundary.
