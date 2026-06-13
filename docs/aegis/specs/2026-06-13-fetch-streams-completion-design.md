# Fetch/Streams Streaming Completion Design

**Date**: 2026-06-13  
**Status**: Approved for planning  
**Scope**: HTTP fetch body streaming, BYOB byte-stream completion, fetch/streams dead-code retirement  
**Spec owner**: `wjsm-runtime` host imports

## 1. TaskIntentDraft

- **Outcome**: `fetch()` for HTTP/HTTPS exposes a real incrementally consumed `ReadableStream` body; byte streams expose functional BYOB requests; fetch/streams-related unused code paths are removed or made active.
- **Goal**: replace the current one-shot or partially detached HTTP body paths with reader-driven streaming that respects the async scheduler contract and Streams backpressure.
- **Success evidence**:
  - HTTP `Response.body.getReader().read()` can resolve after the first network chunk without waiting for end-of-body.
  - Multiple reads consume the same `reqwest::Response` progressively; no read loses the response handle after the first chunk.
  - `Response.text()`, `Response.json()`, and `Response.arrayBuffer()` consume HTTP bodies through the same canonical body owner and reject after the body is already disturbed.
  - `ReadableStream` with `type: "bytes"` supports `controller.byobRequest.view` and `controller.byobRequest.respond(bytesWritten)`.
  - BYOB reads return a view length matching bytes actually supplied, not the original buffer length.
  - Fetch-backed `Response.body` is a byte stream and supports BYOB readers.
  - New tests use a local HTTP server, not external network services.
  - Relevant fetch/streams modules have no dead code introduced or left behind.
- **Stop condition**: targeted runtime tests and affected fixtures pass, and incomplete-marker/dead-code audit for fetch/streams modules is clean.
- **Non-goals**:
  - Browser security policy features: CORS preflight, CSP, Mixed Content, Service Worker interception, Cache API, cookie jar, referrer policy enforcement.
  - Blob/FormData/File APIs, because the runtime does not expose those object models yet.
  - HTTP/2 multiplexing optimization beyond what `reqwest` already performs.
- **Risks**:
  - Holding a mutex guard across `.await` in a spawned task would break `Send` and can deadlock.
  - Missing `AsyncOpGuard` lets the post-main scheduler exit before a spawned HTTP pull materializes.
  - BYOB pending views stored only in side tables can be collected unless GC traces them.

## 2. BaselineReadSetHint

- `docs/async-scheduler.md`: scheduler owner and worker/materialization contract.
- `docs/aegis/specs/2026-06-03-fetch-standard-design.md`: earlier fetch scope and remaining HTTP body work.
- `docs/aegis/specs/2026-06-03-streams-standard-design.md`: earlier streams scope and BYOB surface.
- `crates/wjsm-runtime/src/host_imports/fetch.rs`: `fetch` host import is already `func_wrap_async`.
- `crates/wjsm-runtime/src/host_imports/fetch_http.rs`: HTTP requests use `reqwest`, but `parse_fetch_input` ignores `init`, and `perform_fetch_and_build_response` is unused.
- `crates/wjsm-runtime/src/host_imports/fetch_core.rs`: Response methods consume HTTP bodies with `response.bytes().await` in a spawned task and do not use `AsyncOpGuard`.
- `crates/wjsm-runtime/src/host_imports/streams_readable.rs`: BYOB reads for queued `Uint8Array` chunks work; `byobRequest` is hard-coded `null`; HTTP reader path takes the response out once and never puts it back.
- `crates/wjsm-runtime/src/runtime_builtins.rs`: GC side-table tracing currently traces promises/microtasks/continuations, not reader pending BYOB views or future BYOB request entries.
- WHATWG Fetch Standard: `Response.body` uses Streams, body consumption must respect disturbed/used state.
- WHATWG Streams Standard: byte streams can vend BYOB readers; BYOB readers minimize copies and expose `ReadableStreamBYOBRequest` through the byte-stream controller.

## 3. ImpactStatementDraft

- **Affected layers**:
  - `wjsm-runtime` host imports: fetch parsing, HTTP request execution, Response body methods, ReadableStream/BYOB reader methods.
  - Runtime side tables: HTTP body state, stream controller callbacks, BYOB request roots.
  - Runtime GC tracing: pending BYOB views and BYOB request entries become side-table roots while promises are pending.
  - Tests: runtime async tests and affected fixtures for BYOB value length.
- **Canonical owners**:
  - `fetch_http.rs`: request input parsing and HTTP request creation.
  - `fetch_core.rs`: Headers/Request/Response object model and body consumer methods.
  - `streams_readable.rs`: generic ReadableStream/reader/controller semantics.
  - New `streams_fetch_body.rs`: bridge between fetch-owned HTTP response bodies and ReadableStream readers.
  - `runtime_builtins.rs`: GC tracing for runtime side-table roots.
- **Invariants**:
  - Tokio workers never touch `Store`, `Caller`, Wasm memory, JS heap, or RuntimeState tables directly.
  - All JS object allocation and promise settlement materialization happens on the scheduler owner.
  - No `std::sync::MutexGuard` or runtime table borrow is held across `.await`.
  - Existing `data:` fetch path remains synchronous and behavior-compatible except where BYOB spec correction intentionally changes BYOB result view length.
  - Existing custom ReadableStream queue behavior remains compatible; new `pull` and BYOB request paths only activate when there is demand and no queued chunk.
- **Compatibility boundary**:
  - Public JS names remain `fetch`, `Headers`, `Request`, `Response`, `ReadableStream`, `WritableStream`, `TransformStream`, `CountQueuingStrategy`, `ByteLengthQueuingStrategy`.
  - HTTP body consumers reject once body is disturbed by `getReader()` or an existing body method.
  - Runtime-only security omissions remain explicit non-goals rather than hidden partial implementations.
- **ADR signal**: durable owner/contract change. A completion backfill should record that fetch-backed body streaming is owned by a fetch/streams bridge module and must obey the async scheduler materialization boundary.

## 4. Product Risk Lens

- **Value**: users can consume network responses incrementally and use Streams/BYOB APIs with browser-like semantics in wjsm.
- **Non-goals**: browser-origin policy and APIs missing from the runtime object model.
- **Trade-offs**: a local bridge module adds one owner file but avoids growing `streams_readable.rs` and prevents duplicate HTTP body state.
- **Decision needed**: use the existing `http_response_table` as the canonical HTTP body owner, extended with streaming state, instead of adding a second HTTP stream table.

## 5. Plan-Time Complexity Check

- **Target files**:
  - `fetch_core.rs`: 1825 lines.
  - `streams_readable.rs`: 1647 lines.
  - `lib.rs`: 2913 lines.
  - `fetch_http.rs`: 225 lines.
  - `scheduler.rs`: 223 lines.
- **Better file boundary**: add `crates/wjsm-runtime/src/host_imports/streams_fetch_body.rs` for fetch-backed body pull/read helpers. Keep generic stream semantics in `streams_readable.rs`; keep request construction in `fetch_http.rs`; keep Response object methods in `fetch_core.rs`.
- **Recommendation**: extract helper owner file for HTTP body bridge; edit existing owners for their existing contracts; do not introduce a duplicate `http_stream_table`.

## 6. Decision Hygiene Review

### First-principles invariants

- **Non-negotiable goal**: network body data must flow from `reqwest::Response` to JS `ReadableStream` readers incrementally, without buffering the whole body unless a full-body consumer asks for it.
- **Non-negotiable constraints**:
  - Scheduler owner is the only place JS heap objects and promises are materialized.
  - Worker tasks may own `reqwest::Response` while awaiting I/O, but may not own runtime tables or heap handles except as inert `i64` values sent back for owner-side validation.
  - Backpressure means no network `chunk().await` is started without reader demand.
- **Historical assumptions to delete**:
  - `Response.body` can be a thin one-shot adapter that consumes `HttpResponseEntry.response.take()` once.
  - BYOB support is complete if queued `Uint8Array` chunks can be copied into a supplied view.
  - A second side table is harmless; duplicate owners make dead code more likely.

### Owner / retirement matrix

- **New canonical owner**: `streams_fetch_body.rs` owns fetch-backed ReadableStream pull bridging.
- **Old owner**: `streams_readable.rs::call_reader_http_read` owns the incomplete HTTP read branch and is retired into the bridge module.
- **Compat-only carrier**: existing field name `http_response_table` may remain to minimize churn, but the entry becomes the canonical HTTP body stream state with pending/eof/error fields.
- **Delete-first / retirement trigger**: delete `perform_fetch_and_build_response` immediately because `define_fetch` already dispatches `perform_data_url_fetch` and `perform_http_fetch` directly.

### Falsification matrix

- **Dependency-removal test**: if `AsyncOpGuard` is omitted, a local delayed-chunk test can complete early or hang because the scheduler exits before materialization.
- **Counterexample scenario**: holding `Arc<Mutex<Option<Response>>>` locked across `response.chunk().await` makes the spawned future non-`Send` or serializes I/O behind a blocking mutex.
- **Must fail / degrade / remain correct cases**:
  - `reader.read()` after EOF must fulfill `{ done: true }` without spawning.
  - Two simultaneous `reader.read()` calls on the same reader must not spawn two pulls; the active pending promise remains the only pull.
  - `controller.byobRequest.respond(n)` with `n > view.byteLength` rejects with TypeError.
  - `Response.text()` after `Response.body.getReader()` rejects.

### Verdict

- **Adopt**: extend the existing HTTP response side table and use take/spawn/materialize-put-back.
- **Reject**: a new `http_stream_table` with `Arc<Mutex<Option<Response>>>`, because it duplicates ownership and risks holding locks across await.
- **Reject**: background prefetch task with an internal channel, because it weakens reader-driven backpressure.
- **Next evidence**: local HTTP streaming tests and BYOB request tests before implementation is declared complete.

## 7. Options

### Option A — Extend existing `http_response_table` with take/spawn/put-back

- **Shape**: `HttpResponseEntry` stores `response: Option<reqwest::Response>`, `pending_read_promise`, `eof`, and `error`. On read, owner takes `response` out, sets pending, spawns a task owning `Response`, and Materialize puts `Response` back after `chunk().await` when there is another chunk.
- **Pros**:
  - One canonical HTTP body owner.
  - No lock held across await.
  - Fits scheduler Materialize contract.
  - Minimal state churn in existing Response entries.
- **Cons**: requires careful put-back on every success/error/eof branch.
- **Decision**: recommended and selected.

### Option B — Add `http_stream_table` with `Arc<Mutex<Option<Response>>>`

- **Shape**: new table owns response stream state; spawned tasks lock the response arc and call `chunk().await`.
- **Pros**: superficially isolates streaming from old `http_response_table`.
- **Cons**:
  - Duplicate HTTP body owner.
  - High risk of holding a blocking mutex guard across await.
  - More retirement work and more dead-code surface.
- **Decision**: rejected.

### Option C — Background prefetch loop with a channel of chunks

- **Shape**: fetch spawns a task that continuously reads response chunks into a channel; `reader.read()` drains the channel.
- **Pros**: simpler reads after the task starts.
- **Cons**:
  - Violates reader-driven backpressure.
  - Can buffer unbounded network data if JS stops reading.
  - More cancellation and memory accounting work.
- **Decision**: rejected.

## 8. Detailed Design

### 8.1 HTTP body state

Update `HttpResponseEntry` instead of adding a second HTTP body table:

```rust
#[derive(Debug)]
struct HttpResponseEntry {
    response: Option<reqwest::Response>,
    pending_read_promise: Option<i64>,
    eof: bool,
    error: Option<String>,
}
```

Rules:

1. `response.is_some()` and `pending_read_promise.is_none()` means idle and readable.
2. `response.is_none()` and `pending_read_promise.is_some()` means a worker owns the response and a pull is in flight.
3. `eof == true` means all future reads fulfill done.
4. `error.is_some()` means all future reads reject until the JS stream is canceled or garbage-collected.
5. No worker may mutate this table. Workers send all mutations as a Materialize closure.

### 8.2 HTTP read algorithm

Move the fetch-backed reader branch from `streams_readable.rs` into `streams_fetch_body.rs`:

```rust
pub(crate) fn call_fetch_body_reader_read(
    caller: &mut Caller<'_, RuntimeState>,
    reader_handle: u32,
    http_handle: u32,
    byob_view: Option<i64>,
) -> Option<i64>
```

Algorithm:

1. Allocate a pending promise only after confirming there is no active pending read.
2. Under the table lock:
   - return done if `eof`;
   - reject if `error`;
   - return the existing promise if `pending_read_promise` is set;
   - take `response` out of the entry;
   - store `pending_read_promise = Some(promise)`.
3. Store `byob_view` in the reader entry if this is a BYOB read.
4. Create `AsyncOpGuard` from `RuntimeState.async_op_counter` and move it into the spawned task.
5. Spawn a worker that owns the taken `reqwest::Response` and awaits one `chunk()`.
6. Send `AsyncHostCompletion::Materialize` with the promise and a closure that:
   - clears `pending_read_promise`;
   - puts `response` back on `Ok(Some(chunk))`;
   - sets `eof = true` and drops `response` on `Ok(None)`;
   - sets `error` and drops `response` on `Err`;
   - constructs the JS reader result on the scheduler owner.

### 8.3 AsyncOpGuard requirement

Every spawned HTTP body task must use:

```rust
let guard = caller
    .data()
    .async_op_counter
    .as_ref()
    .map(|counter| counter.begin());
```

The guard is moved into the async task and dropped only after the `tx.send(...)` attempt. This keeps `run_post_main_scheduler_async` waiting while the network pull is in flight. Existing HTTP body spawn sites in `fetch_core.rs` and `streams_readable.rs` are not safe enough without this guard.

### 8.4 Response full-body consumers

`Response.text()`, `Response.json()`, and `Response.arrayBuffer()` use the same canonical `http_response_table`:

1. If body is already used or disturbed, reject.
2. Take the response out of `HttpResponseEntry` and mark the entry pending.
3. Spawn `response.bytes().await` with `AsyncOpGuard`.
4. Materialize the final JS value on the scheduler owner.
5. Mark the HTTP body entry `eof = true`, clear pending, and leave `response = None`.

This removes the split between stream reads and full-body consumers and prevents a body method from racing a reader.

### 8.5 Fetch input parsing

Replace the current incomplete `parse_fetch_input` with a shared parser that mirrors `construct_request` behavior:

- string input: URL from the string, default method `GET`, empty headers, no body, redirect `follow`.
- Request input: copy method, URL, headers, body, redirect, signal, cache, credentials, integrity, keepalive fields from `FetchRequestEntry` and observable properties where already defined.
- init object: override method, headers, body, redirect, cache, credentials, integrity, keepalive, signal.
- reject invalid method and forbidden method (`CONNECT`, `TRACE`, `TRACK`).
- reject body with `GET`/`HEAD`.
- reject URL credentials.

To avoid two parsers drifting, extract the shared logic into `fetch_core.rs` and have both `construct_request` and `parse_fetch_input` call it.

### 8.6 BYOB request state

Add a BYOB request side table:

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

Add fields to `StreamControllerEntry`:

```rust
underlying_source: Option<i64>,
pull_callback: Option<i64>,
cancel_callback: Option<i64>,
active_byob_request: Option<u32>,
```

Rules:

1. `active_byob_request` exists only while a BYOB reader has a pending `read(view)` with no queued chunk.
2. `controller.byobRequest` is an accessor getter, not a data property.
3. The getter returns `null` when there is no active request; otherwise it returns a host object with `view` getter and `respond(bytesWritten)` method.
4. `respond(bytesWritten)` validates the view and byte count, creates a result view of length `bytesWritten`, fulfills the pending promise, clears `active_byob_request`, and marks the request responded.
5. BYOB request entries and pending BYOB views are GC roots until settled.

### 8.7 Underlying source `pull` and `cancel`

`construct_readable_stream` currently stores only the `start` callback behavior. Complete byte-stream behavior requires source callbacks:

1. During construction, read `source.pull` and `source.cancel` if callable and store them in `StreamControllerEntry`.
2. When `reader.read()` finds no queued chunk and stream is not closed:
   - store the pending read promise;
   - for BYOB, create a `ByobRequestEntry` and expose it through `controller.byobRequest`;
   - enqueue a microtask to invoke `pull(controller)` if a pull callback exists.
3. Add a `Microtask::ReadableStreamPull` variant handled by both sync and async drains. The async drain calls `call_host_function_with_args_async`; the sync drain keeps the existing sync path for remaining sync-only execution helpers.
4. `cancel(reason)` invokes stored `cancel` callback when present, clears queues, rejects pending reads, and transitions to closed.

### 8.8 BYOB result view

The current queued-chunk BYOB helper fulfills with the original supplied view. Complete behavior returns a view whose length equals bytes actually written. Add a helper that creates a typed-array view over the same `ArrayBufferEntry`:

```rust
fn create_typedarray_view_from_existing_buffer(
    caller_or_store: &mut impl AsContextMut<Data = RuntimeState>,
    env: &WasmEnv,
    source_view: i64,
    byte_length: usize,
) -> Result<i64, i64>
```

For `Uint8Array`, the new entry uses the same `buffer_handle`, the same `byte_offset`, `length = byte_length`, `element_size = 1`, and `element_kind = Uint`. This avoids copying and makes `result.value.length` spec-correct.

### 8.9 HTTP body BYOB reads

Fetch-backed bodies are byte streams. `call_fetch_body_reader_read` handles `byob_view` by:

1. validating the supplied view before spawn;
2. storing the view in `ReaderEntry.pending_byob_view` as a GC root;
3. after the chunk arrives, writing at most `view.byteLength` bytes into the view;
4. storing overflow bytes at the front of the HTTP body entry's pending byte queue or a small `VecDeque<Vec<u8>>` field before reading more network data;
5. fulfilling with a result view sized to bytes written.

Add `pending_bytes: VecDeque<Vec<u8>>` to `HttpResponseEntry` so partial chunks are not lost when a BYOB view is smaller than the network chunk.

### 8.10 GC tracing

Update `trace_runtime_side_table_roots_fixed_point` to mark:

- `ReaderEntry.pending_byob_view` when present.
- `ByobRequestEntry.view` and `ByobRequestEntry.promise`.
- `StreamControllerEntry.underlying_source`, `pull_callback`, `cancel_callback`, and any active controller object handle if stored.
- `HttpResponseEntry.pending_read_promise` when present.

This makes pending BYOB views and promises safe across GC cycles triggered before materialization.

### 8.11 Dead-code retirement

Remove or repurpose these fetch/streams-specific dead paths:

- Delete `perform_fetch_and_build_response` from `fetch_http.rs`.
- Remove duplicate HTTP read implementation from `streams_readable.rs` after `streams_fetch_body.rs` owns it.
- Retire legacy fetch-only `NativeCallable::StreamMethod`, `NativeCallable::ReaderMethod`, `StreamMethodKind`, `ReaderMethodKind`, `fetch_core::call_stream_method_from_caller`, and `fetch_core::call_reader_method_from_caller` once fetch-backed bodies use the canonical `ReadableStreamMethod` / `ReadableStreamDefaultReaderMethod` path.
- Remove unused `streams_transform.rs::type_error_exception` if still unused after BYOB additions.
- Replace comments that say BYOB fields are reserved with comments describing active semantics.
- Do not silence relevant dead-code warnings with `#[allow(dead_code)]` unless the item is a documented standards enum outside the changed module and is not fetch/streams implementation scaffolding.

## 9. Verification Strategy

### 9.1 Runtime tests with local HTTP server

Use `crates/wjsm-runtime/tests/fetch_http_streaming.rs` with a real local `std::net::TcpListener` served from a test thread. Tests must not depend on external services. The server sends valid chunked HTTP responses and can delay the final chunk to prove first read is not eager full-body consumption.

Required tests:

1. `fetch_http_first_read_resolves_before_end_of_body`: first `reader.read()` resolves before delayed end-of-body.
2. `fetch_http_reader_reads_all_chunks`: total bytes and order are correct.
3. `fetch_http_byob_reader_fills_supplied_view`: BYOB reader on `Response.body` fills a supplied `Uint8Array` and returns the correct result view length.
4. `response_text_after_body_reader_rejects`: body disturbed by `getReader()` rejects full-body consumer.
5. `response_text_consumes_http_body_once`: repeated full-body consumers reject.

### 9.2 Fixture updates

- Add `fixtures/happy/streams_byob_request_respond.js` and expected output.
- Update `fixtures/happy/streams_readable_byob.expected` if result view length changes from supplied view length to bytes-read length.
- Keep `fixtures/happy/fetch_data_url.js`, `streams_fetch_body_data_url.js`, `streams_fetch_body_used.js`, and `streams_fetch_clone_shared.js` passing.

### 9.3 Targeted commands

- `cargo nextest run -p wjsm-runtime -E 'test(fetch_http_streaming)'`
- `cargo nextest run -E 'test(happy__streams_readable_byob) or test(happy__streams_byob_request_respond) or test(happy__streams_fetch_body_data_url) or test(happy__fetch_data_url)'`
- `cargo nextest run -p wjsm-runtime -E 'test(async_scheduler) or test(async_reentry)'`
- `cargo check -p wjsm-runtime`

## 10. Spec Self-Review

- **Incomplete-marker scan**: no incomplete requirements are intentionally left in this spec.
- **Internal consistency**: the spec uses one canonical HTTP body owner, not a new duplicate table.
- **Scope check**: the work is bounded to fetch body streaming, BYOB byte streams, request parsing required by fetch, scheduler-safe materialization, and relevant dead-code retirement.
- **Ambiguity check**: selected approach is explicit; rejected alternatives and reasons are recorded.
- **Boundary check**: non-goals, compatibility boundaries, owners, GC roots, and ADR signals are marked.
- **Risk reflection**: the rejected `Arc<Mutex<Option<Response>>>` design is explicitly excluded because it can encode the wrong async ownership boundary.

## 11. User Review Gate

The user explicitly instructed: "不需要我审查，你直接按照流程写入spec和plan，然后自审". This spec records that review gate as waived for this turn, so planning may proceed immediately after self-review.
