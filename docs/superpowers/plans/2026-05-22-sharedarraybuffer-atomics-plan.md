# SharedArrayBuffer + Atomics — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement ECMAScript `SharedArrayBuffer` and `Atomics`, including test262 agent harness (`$262.agent`).

**Architecture:** SAB data stored as `Arc<RwLock<Vec<u8>>>` on Rust heap, shared across agents via `Arc`. Atomics operations implemented as Rust host functions (aligned → `Atomic*` intrinsics, unaligned → Mutex fallback). wait/notify via `Condvar` + global waiter registry. Agents = independent OS threads, each with own wasmtime Engine/Store/Instance.

**Tech Stack:** Rust, wasmtime, wjsm-ir, wjsm-semantic, wjsm-backend-wasm, wjsm-runtime

**Files to modify/create:**
- `crates/wjsm-ir/src/builtin.rs` — 17 new Builtin variants + Display impl
- `crates/wjsm-semantic/src/builtins.rs` — builtin_from_global_ident mapping
- `crates/wjsm-backend-wasm/src/lib.rs` — import signatures + arities
- `crates/wjsm-backend-wasm/src/compiler_core.rs` — function index mapping + imports
- `crates/wjsm-backend-wasm/src/compiler_builtins.rs` — call argument packing
- `crates/wjsm-runtime/src/lib.rs` — data structures, NativeCallable variants, execute_with_writer signature
- `crates/wjsm-runtime/src/host_imports/collections_buffers.rs` — SAB constructor, global object creation
- `crates/wjsm-runtime/src/host_imports/get_builtin_global_entry.rs` — SAB/Atomics dispatch
- `crates/wjsm-runtime/src/host_imports/typedarray_new_methods.rs` — is_shared dispatch in all TA accessors
- `crates/wjsm-runtime/src/host_imports/atomics.rs` (NEW) — all Atomics host functions
- `crates/wjsm-runtime/src/host_imports/agent.rs` (NEW) — agent harness
- `crates/wjsm-runtime/src/runtime_builtins.rs` — NativeCallable dispatch arms
- `crates/wjsm-test262/src/config.rs` — SUPPORTED_FEATURES additions
- `fixtures/happy/` — 3-4 JS fixtures + .expected
- `fixtures/errors/` — 2 JS fixtures + .expected
- `fixtures/semantic/` — 1 .ir snapshot

**Design decisions:**
- SAB: `SharedArrayBufferEntry { data: Arc<RwLock<Vec<u8>>>, byte_length: u64 }` in `SharedRuntimeState`
- `TypedArrayEntry` gets `is_shared: bool`; all TA accessors branch on it
- Atomics: aligned access via `AtomicI32`/`AtomicI64` pointer cast; unaligned via per-SAB Mutex
- wait/notify: `Condvar`-based waiter registry keyed by `(sab_handle, byte_offset)`
- Agent `start`: `std::thread::spawn` + `catch_unwind`; agent constructs own RuntimeState, shares only `SharedRuntimeState`
- Agent broadcast/receive: busy-wait flag at `SAB[byteLength-1]` + Int32 length + data

---

### Phase 1: IR Layer — Builtin Variants

**Files:** `crates/wjsm-ir/src/builtin.rs`

- [ ] **Step 1: Add 17 new Builtin enum variants**

After `SharedArrayBuffer` stubs (currently nonexistent; add before `ArrayBufferConstructor`):
```rust
// ── SharedArrayBuffer built-in ──
SharedArrayBufferConstructor,
SharedArrayBufferProtoByteLength,
SharedArrayBufferProtoSlice,
SharedArrayBufferProtoSpecies,
// ── Atomics built-in ──
AtomicsLoad,
AtomicsStore,
AtomicsAdd,
AtomicsSub,
AtomicsAnd,
AtomicsOr,
AtomicsXor,
AtomicsExchange,
AtomicsCompareExchange,
AtomicsIsLockFree,
AtomicsWait,
AtomicsNotify,
AtomicsWaitAsync,
```

- [ ] **Step 2: Add Display impl entries**

Add all 17 variants to the `Display` match arm in `impl fmt::Display for Builtin`:
```rust
Self::SharedArrayBufferConstructor => "SharedArrayBuffer",
Self::SharedArrayBufferProtoByteLength => "SharedArrayBuffer.prototype.byteLength",
Self::SharedArrayBufferProtoSlice => "SharedArrayBuffer.prototype.slice",
Self::SharedArrayBufferProtoSpecies => "SharedArrayBuffer.prototype[Symbol.species]",
Self::AtomicsLoad => "Atomics.load",
Self::AtomicsStore => "Atomics.store",
Self::AtomicsAdd => "Atomics.add",
Self::AtomicsSub => "Atomics.sub",
Self::AtomicsAnd => "Atomics.and",
Self::AtomicsOr => "Atomics.or",
Self::AtomicsXor => "Atomics.xor",
Self::AtomicsExchange => "Atomics.exchange",
Self::AtomicsCompareExchange => "Atomics.compareExchange",
Self::AtomicsIsLockFree => "Atomics.isLockFree",
Self::AtomicsWait => "Atomics.wait",
Self::AtomicsNotify => "Atomics.notify",
Self::AtomicsWaitAsync => "Atomics.waitAsync",
```

**Verify:** `cargo check -p wjsm-ir`

---

### Phase 2: Semantic Layer

**Files:** `crates/wjsm-semantic/src/builtins.rs`

- [ ] **Step 1: Map SharedArrayBuffer global identifier**

In `builtin_from_global_ident`, add:
```rust
"SharedArrayBuffer" => Some(Builtin::SharedArrayBufferConstructor),
```

`Atomics` is accessed via member expressions (`Atomics.load(...)`) — not mapped here. The global `Atomics` object is created at runtime.

**Verify:** `cargo check -p wjsm-semantic`

---

### Phase 3: Backend — Import Registration

**Files:**
- `crates/wjsm-backend-wasm/src/lib.rs`
- `crates/wjsm-backend-wasm/src/compiler_core.rs`
- `crates/wjsm-backend-wasm/src/compiler_builtins.rs`

- [ ] **Step 1: Add import signatures (lib.rs)**

In `fn builtin_import_signature` (returns `(&str, u32)`), add entries for all 17 new Builtins:

```rust
Builtin::SharedArrayBufferConstructor => ("SharedArrayBuffer", 1),
Builtin::SharedArrayBufferProtoByteLength => ("SharedArrayBuffer.prototype.byteLength", 1),
Builtin::SharedArrayBufferProtoSlice => ("SharedArrayBuffer.prototype.slice", 3),
Builtin::SharedArrayBufferProtoSpecies => ("SharedArrayBuffer.prototype[Symbol.species]", 1),
Builtin::AtomicsLoad => ("Atomics.load", 2),
Builtin::AtomicsStore => ("Atomics.store", 3),
Builtin::AtomicsAdd => ("Atomics.add", 3),
Builtin::AtomicsSub => ("Atomics.sub", 3),
Builtin::AtomicsAnd => ("Atomics.and", 3),
Builtin::AtomicsOr => ("Atomics.or", 3),
Builtin::AtomicsXor => ("Atomics.xor", 3),
Builtin::AtomicsExchange => ("Atomics.exchange", 3),
Builtin::AtomicsCompareExchange => ("Atomics.compareExchange", 4),
Builtin::AtomicsIsLockFree => ("Atomics.isLockFree", 1),
Builtin::AtomicsWait => ("Atomics.wait", 4),
Builtin::AtomicsNotify => ("Atomics.notify", 3),
Builtin::AtomicsWaitAsync => ("Atomics.waitAsync", 3),
```

- [ ] **Step 2: Register function indices + import entries (compiler_core.rs)**

In the function index mapping (search for `ArrayBufferConstructor`), add all new Builtins with sequential indices. Add corresponding import entries with `"env"` module. Assign consecutive indices after the last existing Builtin.

- [ ] **Step 3: Add call argument packing (compiler_builtins.rs)**

In `pack_builtin_args`, add match arms for the new Builtins. All 17 are `call` instructions with 1–4 arguments: pack them using the existing pattern (e.g., `(0, 1)` for 2-arg builtins, `(0, 1, 2)` for 3-arg, `(0, 1, 2, 3)` for 4-arg).

**Verify:** `cargo check -p wjsm-backend-wasm`

---

### Phase 4: Runtime — Data Structures + Shared State

**Files:** `crates/wjsm-runtime/src/lib.rs`

- [ ] **Step 1: Add SharedArrayBufferEntry, SharedRuntimeState, AgentState, Waiter structs**

After `ArrayBufferEntry`:
```rust
#[derive(Clone, Debug)]
struct SharedArrayBufferEntry {
    data: Arc<RwLock<Vec<u8>>>,
    byte_length: u64,
}

struct SharedRuntimeState {
    sab_table: Arc<Mutex<Vec<SharedArrayBufferEntry>>>,
    agent_state: Arc<AgentState>,
}

struct AgentState {
    reports: Arc<Mutex<Vec<String>>>,
    waiters: Arc<Mutex<HashMap<(u32, u32), Vec<Waiter>>>>,
}

struct Waiter {
    condvar: Arc<Condvar>,
    notified: Arc<AtomicBool>,
}
```

Add `use std::sync::{Arc, RwLock, Condvar, atomic::AtomicBool};` and `use std::collections::HashMap;` as needed.

- [ ] **Step 2: Add shared_state field to RuntimeState**

```rust
/// Optional shared state for cross-agent coordination.
/// None in normal (non-agent) execution.
shared_state: Option<Arc<SharedRuntimeState>>,
```

Initialize as `None` in the standard path.

- [ ] **Step 3: Add NativeCallable variants**

Add to `enum NativeCallable`:
```rust
SharedArrayBufferConstructor,
AtomicsLoad,
AtomicsStore,
AtomicsAdd,
AtomicsSub,
AtomicsAnd,
AtomicsOr,
AtomicsXor,
AtomicsExchange,
AtomicsCompareExchange,
AtomicsIsLockFree,
AtomicsWait,
AtomicsNotify,
AtomicsWaitAsync,
AtomicsGlobal,
// Agent harness
AgentStart,
AgentBroadcast,
AgentReceiveBroadcast,
AgentGetReport,
AgentSleep,
AgentMonotonicNow,
```

- [ ] **Step 4: Rename execute_with_writer, add shared_state parameter**

Rename current `execute_with_writer` to `execute_with_writer_shared`, adding `shared_state: Option<Arc<SharedRuntimeState>>` parameter after `writer`. Create a new `execute_with_writer` that calls `execute_with_writer_shared` with `None`.

Same pattern for `execute` → `execute_shared`.

- [ ] **Step 5: TypedArrayEntry — add is_shared field**

```rust
struct TypedArrayEntry {
    buffer_handle: u32,
    byte_offset: u32,
    length: u32,
    element_size: u8,
    element_kind: u8,
    is_shared: bool,  // NEW
}
```

Update all `TypedArrayEntry` construction sites (TypedArrayConstructor) to set `is_shared: false` initially.

**Verify:** `cargo check -p wjsm-runtime`

---

### Phase 5: Runtime — SAB Constructor

**Files:** `crates/wjsm-runtime/src/host_imports/collections_buffers.rs`, `get_builtin_global_entry.rs`, `runtime_builtins.rs`

- [ ] **Step 1: Implement alloc_shared_arraybuffer**

```rust
fn alloc_shared_arraybuffer(
    state: &RuntimeState,
    byte_length: u64,
) -> u32 {
    let shared = state.shared_state.as_ref()
        .expect("SharedArrayBuffer requires shared_state");
    let entry = SharedArrayBufferEntry {
        data: Arc::new(RwLock::new(vec![0u8; byte_length as usize])),
        byte_length,
    };
    let mut table = shared.sab_table.lock().unwrap();
    table.push(entry);
    (table.len() - 1) as u32
}
```

- [ ] **Step 2: Wire SAB constructor as host function**

In `collections_buffers.rs`, under the existing `ArrayBufferConstructorGlobal` block, add a `SharedArrayBufferConstructor` host function that:
1. Reads `byte_length` argument (u64 from f64)
2. Calls `alloc_shared_arraybuffer` to get handle
3. Creates host object with `__sharedarraybuffer_handle__` property
4. Defines `byteLength` getter, `slice` method on the object

- [ ] **Step 3: Implement byteLength getter**

`SharedArrayBufferProtoByteLength` host function: resolve handle → read `entry.byte_length` → return as f64.

- [ ] **Step 4: Implement slice method**

`SharedArrayBufferProtoSlice` host function:
1. Resolve this SAB handle + resolve `begin`, `end` indices
2. Clamp to `[0, byteLength]`
3. Copy `data.read()[new_begin..new_end]` to new `Arc<RwLock<Vec<u8>>>`
4. Allocate new SAB entry with copied data
5. Create and return new SAB object

- [ ] **Step 5: Implement Species getter**

`SharedArrayBufferProtoSpecies` → return `this` (the constructor).

- [ ] **Step 6: Wire SharedArrayBuffer in get_builtin_global_entry and create_global_object**

In `get_builtin_global_entry.rs`, change `"SharedArrayBuffer"` from `StubGlobal(())` to:
```rust
"SharedArrayBuffer" => {
    native_callables.push(NativeCallable::SharedArrayBufferConstructor);
    value::encode_native_callable_idx(idx)
}
```

In `collections_buffers.rs` `create_global_object`:
- Replace `StubGlobal` for `"SharedArrayBuffer"` with `NativeCallable::SharedArrayBufferConstructor`

- [ ] **Step 7: Wire NativeCallable dispatch in runtime_builtins.rs**

In `call_native_callable_with_args_from_caller`, add arm for `SharedArrayBufferConstructor` that calls the SAB constructor host function.

**Verify:** `cargo build` — check entire workspace compiles; `cargo run -- run fixtures/happy/sharedarraybuffer.js` (fixture not yet created, but smoke test with inline `new SharedArrayBuffer(16)`)

---

### Phase 6: Runtime — Atomics Host Functions (non-wait)

**Files:** `crates/wjsm-runtime/src/host_imports/atomics.rs` (NEW)

- [ ] **Step 1: Create atomics.rs module**

Declare `mod host_imports::atomics` in `lib.rs`.

- [ ] **Step 2: Implement helper — validate_ta_for_atomics**

```rust
fn validate_ta_for_atomics(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,  // the TypedArray
) -> Result<(usize, usize, u8, u8, usize, &SharedRuntimeState), ()> {
    // 1. Resolve TA handle
    // 2. Check entry.is_shared == true (else TypeError)
    // 3. Return (buf_handle, byte_offset, elem_size, elem_kind, sab_table_index, shared_state)
}
```

- [ ] **Step 3: Implement aligned atomic read/write helpers**

```rust
unsafe fn atomic_load_u8(data: &[u8], offset: usize) -> u8 { ... }
unsafe fn atomic_store_u8(data: &[u8], offset: usize, val: u8) { ... }
unsafe fn atomic_load_i32(data: &[u8], offset: usize) -> i32 {
    let ptr = data.as_ptr().add(offset) as *const AtomicI32;
    (*ptr).load(Ordering::SeqCst)
}
// ... similar for i64, u32, u64, f32, f64
```

For aligned: pointer-cast to `AtomicI32`/`AtomicI64`/`AtomicU32`.
For unaligned/non-standard sizes: Mutex-protected read-modify-write.

- [ ] **Step 4: Implement AtomicsLoad**

1. Validate TA → get SAB data
2. Lock `data.read()`, validate index bounds
3. Atomically read element at `index * element_size + byte_offset`
4. Return as NaN-boxed value

- [ ] **Step 5: Implement AtomicsStore**

Same validation, `data.write()`, atomically write value bytes, return encoded value.

- [ ] **Step 6: Implement AtomicsAdd, Sub, And, Or, Xor**

Read-modify-write: atomic load → apply operation → atomic store → return old value.

- [ ] **Step 7: Implement AtomicsExchange**

Atomic swap: return old value.

- [ ] **Step 8: Implement AtomicsCompareExchange**

Atomic CAS: compare `expected` → if match, replace with `replacement` → return old value.

- [ ] **Step 9: Implement AtomicsIsLockFree**

Return `true` for sizes 1, 2, 4 (and 8 for BigInt arrays); `false` otherwise.

- [ ] **Step 10: Wire Atomics builtins as host functions**

Register all Atomics host functions as `env` imports in `collections_buffers.rs`. Wire into `runtime_builtins.rs` dispatch.

- [ ] **Step 11: Create Atomics global object**

In `create_global_object` (`collections_buffers.rs`), create an `Atomics` object with all 13 method properties:
```rust
("Atomics", NativeCallable::AtomicsGlobal),
```
Then in `create_global_object_fn`, create a plain object and attach all Atomics methods as `NativeCallable`-wrapped functions.

**Verify:** `cargo build`; `cargo run -- run -e "Atomics.load(new Int32Array(new SharedArrayBuffer(4)), 0)"`

---

### Phase 7: Runtime — Atomics Wait/Notify

**Files:** `crates/wjsm-runtime/src/host_imports/atomics.rs`

- [ ] **Step 1: Implement waiter registry helpers**

```rust
fn register_waiter(
    state: &RuntimeState,
    sab_handle: u32,
    byte_offset: u32,
) -> Arc<(Condvar, AtomicBool)> { ... }

fn wake_waiters(
    state: &RuntimeState,
    sab_handle: u32,
    byte_offset: u32,
    count: i64,
) -> u32 { ... }
```

- [ ] **Step 2: Implement AtomicsWait**

1. Validate TA (must be Int32Array or BigInt64Array — `element_kind` check)
2. Check not detached (verify SAB entry exists)
3. Agent can't wait on main thread's SAB without shared_state → error if agent_state is None
4. Atomic load value at index
5. If `value != expected` → return `"not-equal"` (as tagged string)
6. Register waiter with timeout
7. `condvar.wait_timeout(&mut guard, timeout_duration)`
8. If `notified.load(Ordering::SeqCst)` → return `"ok"`
9. Else (timeout) → return `"timed-out"`

- [ ] **Step 3: Implement AtomicsNotify**

1. Same validation
2. Look up waiters for (sab_handle, byte_offset)
3. Wake up to `count` waiters (FIFO): set notified flag, condvar.notify_one()
4. Return number woken as f64

- [ ] **Step 4: Implement AtomicsWaitAsync (simplified)**

1. Validate
2. Atomic load
3. If `value != expected` → return `{ async: false, value: "not-equal" }`
4. Else → return `{ async: false, value: "timed-out" }` (no true async implementation)

**Verify:** `cargo run -- run fixtures/happy/atomics_wait_notify.js`

---

### Phase 8: TypedArray Integration — is_shared Dispatch

**Files:** `crates/wjsm-runtime/src/host_imports/typedarray_new_methods.rs`

- [ ] **Step 1: Update ta_resolve to return is_shared**

Change return type of `ta_resolve` to include `is_shared: bool`.

- [ ] **Step 2: Add sab_read / sab_write helpers**

Mirror `ta_read` / `ta_write` but access `sab_table` via `state.shared_state` instead of `arraybuffer_table`. Use `data.read().unwrap()` / `data.write().unwrap()` (RwLock — non-atomic is fine for TA methods).

- [ ] **Step 3: Branch on is_shared in all TA access methods**

In every function that calls `ta_read` or `ta_write` (`TypedArrayProtoLength`, `set`, `slice`, `subarray`, `fill`, `reverse`, `indexOf`, `lastIndexOf`, `includes`, `join`, `toString`, `copyWithin`, `at`, `forEach`, `map`, `filter`, `reduce`, `reduceRight`, `find`, `findIndex`, `some`, `every`, `sort`, `entries`, `keys`, `values`):

```rust
if is_shared {
    sab_read(caller, buf_handle, byte_offset, elem_size, element_kind, index)
} else {
    ta_read(caller, buf_handle, byte_offset, elem_size, element_kind, index)
}
```

- [ ] **Step 4: Update TA constructor for SAB detection**

In `TypedArrayConstructor` host function, detect if the buffer argument is a SharedArrayBuffer (check for `__sharedarraybuffer_handle__` property) and set `is_shared: true` + store handle in `sab_table`.

**Verify:** `cargo build`; run all existing TypedArray fixtures to confirm no regression

---

### Phase 9: Agent Harness

**Files:** `crates/wjsm-runtime/src/host_imports/agent.rs` (NEW)

- [ ] **Step 1: Create agent.rs module, implement AgentState**

```rust
use std::sync::{Arc, Mutex, Condvar, atomic::{AtomicBool, Ordering}};
use std::thread::{self, Thread};
use std::time::{Duration, Instant};

struct AgentState {
    reports: Arc<Mutex<Vec<String>>>,
    broadcast_signals: Arc<Mutex<HashMap<u32, Arc<(Mutex<bool>, Condvar)>>>>,
}
```

- [ ] **Step 2: Implement agent_start**

```rust
fn agent_start(script: String, shared_state: Arc<SharedRuntimeState>) {
    thread::spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // Compile script
            let module = wjsm_parser::parse_module(&script)?;
            let program = wjsm_semantic::lower_module(module)?;
            let wasm_bytes = wjsm_backend_wasm::compile(&program)?;
            // Execute with shared state
            let mut report = Vec::new();
            execute_with_writer_shared(&wasm_bytes, &mut report, Some(shared_state.clone()))?;
            Ok::<_, anyhow::Error>(String::from_utf8_lossy(&report).to_string())
        }));
        let report_str = match result {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => format!("agent error: {e}"),
            Err(panic) => format!("agent panic: {panic:?}"),
        };
        shared_state.agent_state.reports.lock().unwrap().push(report_str);
    });
}
```

- [ ] **Step 3: Implement agent_broadcast**

Write data to SAB tail:
1. Lock byte at `byteLength - 1` — busy-wait until 0, then set to 1
2. Write data length (Int32, elements count) at `byteLength - 8` to `byteLength - 5`
3. Write data elements at `byteLength - 4` onwards (limited by space)
4. Set lock byte back to 0

- [ ] **Step 4: Implement agent_receiveBroadcast**

Read from SAB tail:
1. Busy-wait lock byte == 1 (with yield + timeout)
2. Read data length
3. Read data elements
4. Set lock byte to 0

- [ ] **Step 5: Implement agent_getReport, sleep, monotonicNow**

```rust
// getReport: pop from reports vector
// sleep: thread::sleep(Duration::from_millis(ms))
// monotonicNow: Instant::now().elapsed().as_millis() as f64
```

- [ ] **Step 6: Wire $262 object**

In `create_global_object` or `get_builtin_global_entry`:
- When `name == "$262"`, create object with `agent` property that has `start`, `broadcast`, `receiveBroadcast`, `getReport`, `sleep`, `monotonicNow` methods.

**Verify:** `cargo build`

---

### Phase 10: test262 Integration

**Files:** `crates/wjsm-test262/src/config.rs`

- [ ] **Step 1: Add SUPPORTED_FEATURES**

```rust
"SharedArrayBuffer",
"Atomics",
"Atomics.waitAsync",
```

- [ ] **Step 2: Handle agent tests**

Agent tests use `$262.agent.*`. The test262 runner needs to:
1. Check if `$262.agent` exists in the global scope
2. If missing, skip the test (the harness creation in runtime handles this)

No test262 runner changes needed if harness properly creates `$262` object.

- [ ] **Step 3: Run test262 evaluation**

```bash
cargo run -p wjsm-test262 -- run --suite test/built-ins/SharedArrayBuffer --all
cargo run -p wjsm-test262 -- run --suite test/built-ins/Atomics --all
```

**Verify:** Non-agent tests pass at >90%. Agent tests skip or pass depending on harness maturity.

---

### Phase 11: Fixtures

**Files to create:**
- `fixtures/happy/sharedarraybuffer.js` + `.expected`
- `fixtures/happy/atomics.js` + `.expected`
- `fixtures/happy/atomics_wait_notify.js` + `.expected`
- `fixtures/errors/sab_detached.js` + `.expected`
- `fixtures/errors/atomics_bad_ta.js` + `.expected`
- `fixtures/semantic/sharedarraybuffer.ir`

- [ ] **Step 1: sharedarraybuffer.js**

```js
var sab = new SharedArrayBuffer(16);
console.log(sab.byteLength);          // 16
console.log(sab instanceof SharedArrayBuffer);  // true
var sliced = sab.slice(4, 8);
console.log(sliced.byteLength);       // 4
```

- [ ] **Step 2: atomics.js**

```js
var sab = new SharedArrayBuffer(16);
var ta = new Int32Array(sab);
Atomics.store(ta, 0, 42);
console.log(Atomics.load(ta, 0));     // 42
Atomics.add(ta, 0, 8);
console.log(Atomics.load(ta, 0));     // 50
console.log(Atomics.compareExchange(ta, 0, 50, 100));  // 50
console.log(Atomics.load(ta, 0));     // 100
console.log(Atomics.isLockFree(4));   // true
```

- [ ] **Step 3: atomics_wait_notify.js**

```js
var sab = new SharedArrayBuffer(16);
var ta = new Int32Array(sab);
Atomics.store(ta, 0, 0);
// wait on value that doesn't match → not-equal
console.log(Atomics.wait(ta, 0, 1, 100));  // "not-equal"
// wait on value that matches, with timeout
console.log(Atomics.wait(ta, 0, 0, 100));  // "timed-out"
```

- [ ] **Step 4: Error fixtures**

`sab_detached.js`: detach pattern (if supported) + Atomics access → TypeError
`atomics_bad_ta.js`: pass non-TypedArray or non-Int32Array to wait → TypeError

- [ ] **Step 5: Update snapshots**

```bash
WJSM_UPDATE_FIXTURES=1 cargo test
```

- [ ] **Step 6: IR snapshot**

Create `fixtures/semantic/sharedarraybuffer.ir` with expected lowering output.

**Verify:** `cargo test` — all fixtures pass

---

### Phase 12: Final Integration Test

- [ ] **Step 1: Run full test suite**

```bash
cargo test
cargo nextest run
```

- [ ] **Step 2: Run test262 SAB + Atomics suites**

```bash
cargo run -p wjsm-test262 -- run --suite test/built-ins/SharedArrayBuffer --all
cargo run -p wjsm-test262 -- run --suite test/built-ins/Atomics --all
```

- [ ] **Step 3: Verify no regressions**

```bash
cargo run -p wjsm-test262  # full suite
```

- [ ] **Step 4: Update todo.md**

Mark `SharedArrayBuffer + Atomics` as completed.

---

### Implementation Order Summary

```
Phase 1  (IR)           → check
Phase 2  (Semantic)     → check
Phase 3  (Backend)      → check
Phase 4  (Data structs) → check
Phase 5  (SAB ctor)     → build + smoke test
Phase 6  (Atomics ops)  → build + smoke test
Phase 7  (wait/notify)  → build + smoke test
Phase 8  (TA integ)     → cargo test (fixtures)
Phase 9  (Agent)        → build
Phase 10 (test262 cfg)  → run test262 SAB/Atomics
Phase 11 (Fixtures)     → WJSM_UPDATE_FIXTURES=1 cargo test
Phase 12 (Final)        → full test suite + test262
```

Phases 1–4 are pure struct/enum additions (no behavioral change). Phases 5–9 introduce the feature incrementally. Phase 10–12 verify end-to-end.
