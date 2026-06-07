# SharedArrayBuffer + Atomics ES 全量实现计划

**日期**: 2026-06-05
**Spec**: [docs/aegis/specs/2026-06-05-sharedarraybuffer-atomics-design.md](../specs/2026-06-05-sharedarraybuffer-atomics-design.md)

## Goal

按已批准设计补齐 ECMAScript Structured Data §25.2 SharedArrayBuffer Objects 与 §25.4 Atomics Object 的完整可观测行为：fixed-length/growable SAB、TypedArray/DataView shared backing、Atomics 全方法、wait/notify/waitAsync、BigInt64Array waitable、agent cluster waiter store。

**状态 (2026-06-07)**:
- Phase C 完成：`Atomics.wait` async host、`Atomics.waitAsync` Promise 路径、`Atomics.notify` 唤醒结算。
- Phase D 完成：DataView `is_shared` 分流 + semantic `dataview_bindings` 直连 `CallBuiltin`（修复 DataView getter `indirect call type mismatch`）。
- Phase E 完成：本 spec/plan/checkpoint 同步。
- **Task 10 完成**：`$262.agent` harness、`happy__atomics_agent_notify`（agent `report('done')` + `Atomics.wait(5000)` + `getReport()`）；`compiler_module.rs::emit_init_module_global_for_js_function` 修复嵌套函数 `$0.$global` 未初始化。
- 已验证 fixtures/commands：
  - `cargo nextest run -E 'test(happy__atomics_wait_async) or test(happy__dataview_sharedarraybuffer)'`
  - `cargo nextest run -E 'test(happy__sharedarraybuffer) or test(happy__sharedarraybuffer_constructor) or test(happy__atomics) or test(happy__atomics_global) or test(happy__atomics_bigint) or test(happy__atomics_wait_async) or test(happy__dataview_sharedarraybuffer) or test(errors__sharedarraybuffer_grow_invalid) or test(errors__atomics_wrong_type) or test(errors__atomics_oob)'`
  - `cargo nextest run -p wjsm-runtime -p wjsm-backend-wasm`
  - `cargo nextest run -E 'test(errors__sharedarraybuffer) or test(errors__atomics) or test(host_import_registry)'`
  - `cargo check -p wjsm-semantic -p wjsm-backend-wasm -p wjsm-runtime`
  - 结果：上述选择器全部通过（2026-06-07）。

## Architecture

```text
JS: new SharedArrayBuffer(length, options)
  → NativeCallable::SharedArrayBufferConstructor / WASM builtin import
  → shared_buffer::construct_shared_array_buffer
  → SharedRuntimeState.sab_table
  → JS host object with SAB internal-slot side table/hidden handle

JS: new Int32Array(sab) / new DataView(sab)
  → resolve_buffer_backing
  → TypedArrayEntry/DataViewEntry { is_shared: true, buffer_handle: sab_handle }
  → shared_buffer read/write helpers

JS: Atomics.*(ta, ...)
  → atomics.rs thin host import / NativeCallable method
  → shared_buffer validation + atomic RMW/load/store/waiter helpers
  → SharedRuntimeState.waiter_lists keyed by (sab_handle, byte_index)
```

**关键决策（已批准）**:
- 新增 runtime 单一 owner：`crates/wjsm-runtime/src/shared_buffer.rs`。
- `runtime_builtins.rs`、`collections_buffers.rs`、`typedarray_new_methods.rs`、`atomics.rs` 只做薄 wiring。
- 删除 SAB/Atomics stub 路径，不保留 duplicate owner。
- normal non-agent execution 也拥有 single-agent shared state；agent harness 共享同一 `SharedRuntimeState`。
- 不建完整 §29 candidate execution graph；用 runtime 锁/condvar/Promise 调度保证 JS 可观测语义。

## Tech Stack

- Rust 2024。
- `wasmtime::Func::wrap` host imports。
- `Arc<Mutex<_>>` / `Arc<RwLock<_>>` / `Condvar` / `AtomicBool` 用于 shared block 和 waiter coordination。
- `wjsm_ir::Builtin` + backend host import registry。
- 现有 Promise/microtask/runtime string helpers。

## Baseline/Authority Refs

- Design Spec: `docs/aegis/specs/2026-06-05-sharedarraybuffer-atomics-design.md`
- ECMAScript §25.1.3, §25.2, §25.4, §29。
- `docs/aegis/specs/2026-05-28-host-import-registry-design.md`
- `docs/aegis/plans/2026-05-27-typedarray-methods-completion.md`
- Current implementation refs:
  - `crates/wjsm-runtime/src/lib.rs`
  - `crates/wjsm-runtime/src/runtime_builtins.rs`
  - `crates/wjsm-runtime/src/host_imports/atomics.rs`
  - `crates/wjsm-runtime/src/host_imports/collections_buffers.rs`
  - `crates/wjsm-runtime/src/host_imports/typedarray_new_methods.rs`
  - `crates/wjsm-runtime/src/host_imports/get_builtin_global_entry.rs`
  - `crates/wjsm-runtime/src/host_imports/mod.rs`
  - `crates/wjsm-ir/src/builtin.rs`
  - `crates/wjsm-semantic/src/builtins.rs`
  - `crates/wjsm-backend-wasm/src/host_import_registry.rs`
  - `crates/wjsm-backend-wasm/src/compiler_builtins.rs`

## Compatibility Boundary

| 保证 | 说明 |
|---|---|
| ArrayBuffer 不回归 | `ArrayBuffer` constructor/prototype/slice 继续走 `arraybuffer_table` |
| 非 shared TypedArray 不回归 | `TypedArrayEntry.is_shared=false` 路径保持原数组缓冲读写 |
| DataView over ArrayBuffer 不回归 | 只新增 `is_shared` 分流，普通路径不变 |
| BigInt64Array/BigUint64Array 不回归 | 保持 `element_kind` 4/5 语义 |
| Promise/microtask 不回归 | `waitAsync` async promise resolution 使用现有 Promise settling 机制 |
| Host import registry 不回归 | 新 import 只通过 registry 追加/绑定，不写数字索引 |
| Runtime name linking 不变 | 继续 `linker.define("env", name, ...)` |

## Plan Pressure Test

```text
Plan Pressure Test:
- Owner / contract / retirement: shared_buffer.rs becomes SAB/shared-block/waiter owner; old runtime_builtins/atomics SAB stubs retired.
- Verification scope: fixed/growable SAB, views, Atomics operations, waiters, errors, existing regression commands.
- Task executability: tasks are ordered from failing fixtures → owner module → wiring → operations → waiters → cleanup.
- Pressure result: proceed.
```

## Plan-Time Complexity Check

```text
Plan-Time Complexity Check:
- Target files: lib.rs (~2800), collections_buffers.rs (~2400), atomics.rs (~780), typedarray_new_methods.rs (~960), builtins/registry files.
- Existing size / shape signals: runtime files are overloaded; atomics.rs already contains unsafe pointer RMW and SAB stubs.
- Owner fit: SAB allocation/grow/read/write/waiter state belongs in one runtime owner module.
- Add-in-place risk: high if growable/waitAsync/BigInt wait are added inside atomics.rs.
- Better file boundary: create crates/wjsm-runtime/src/shared_buffer.rs; keep host import files as adapters.
- Recommendation: add owner file.
```

## Files

### Create

- `crates/wjsm-runtime/src/shared_buffer.rs`
- New fixtures under `fixtures/happy/` and `fixtures/errors/` listed in tasks.

### Modify

- `crates/wjsm-runtime/src/lib.rs`
- `crates/wjsm-runtime/src/runtime_builtins.rs`
- `crates/wjsm-runtime/src/host_imports/mod.rs`
- `crates/wjsm-runtime/src/host_imports/atomics.rs`
- `crates/wjsm-runtime/src/host_imports/collections_buffers.rs`
- `crates/wjsm-runtime/src/host_imports/typedarray_new_methods.rs`
- `crates/wjsm-runtime/src/host_imports/get_builtin_global_entry.rs`
- `crates/wjsm-ir/src/builtin.rs`
- `crates/wjsm-semantic/src/builtins.rs`
- `crates/wjsm-backend-wasm/src/host_import_registry.rs`
- `crates/wjsm-backend-wasm/src/compiler_builtins.rs`
- `docs/aegis/INDEX.md`

## Repair Track

- **Root cause:** SAB/Atomics were registered across IR/backend/runtime but normal JS constructor/object/method paths were stubs or unreachable.
- **Canonical owner:** `shared_buffer.rs` owns SAB backing state, grow metadata, buffer resolution, shared read/write/RMW, waiter lists.
- **Minimal sufficient stable repair:** remove duplicate SAB constructor stubs and route all public paths through one owner.
- **Compatibility boundary:** ArrayBuffer and non-shared views remain on existing tables; shared paths branch only on explicit SAB internal slot / `is_shared`.
- **Verification:** fixture matrix + crate-specific nextest commands.

## Retirement Track

| Old path | Current status | Plan action | Deletion trigger |
|---|---|---|---|
| `NativeCallable::SharedArrayBufferConstructor => undefined` | active broken normal JS path | replace with construct helper | `new SharedArrayBuffer(16).byteLength === 16` fixture passes |
| `AtomicsGlobal` empty object | active broken global path | install methods or method-dispatch object | `typeof Atomics.load` fixture observes callable behavior |
| SAB stubs in `atomics.rs` | active host imports but incomplete | replace with shared_buffer delegated host functions | constructor/prototype fixture passes |
| `TypedArrayEntry.is_shared=false` fixed write | active broken view path | set from resolved backing | `new Int32Array(sab)` fixture passes Atomics load/store |
| `shared_state None in non-agent` comment | stale documentation | update comment | compile after state model task |

---

## Task 1: Add failing SAB/Atomics fixtures

**Files**: `fixtures/happy/sharedarraybuffer.js`, `fixtures/happy/sharedarraybuffer.expected`, `fixtures/happy/atomics.js`, `fixtures/happy/atomics.expected`, plus new fixture files.

**Why**: Existing snapshots encode broken `undefined` behavior. Start with observable spec behavior.

**Impact/Compatibility**: Tests fail before implementation; no runtime code changes.

**Verification**:

```bash
cargo nextest run -E 'test(happy__sharedarraybuffer) or test(happy__atomics)'
```

Expected RED: current stdout still contains `undefined` or runtime error.

- [ ] **Write test**: replace `fixtures/happy/sharedarraybuffer.js` with:
  ```js
  var sab = new SharedArrayBuffer(16);
  console.log(sab.byteLength);
  console.log(sab.growable);
  console.log(sab.maxByteLength);
  var view = new Uint8Array(sab);
  view[4] = 99;
  var sliced = sab.slice(4, 8);
  console.log(sliced.byteLength);
  console.log(new Uint8Array(sliced)[0]);
  var growable = new SharedArrayBuffer(4, { maxByteLength: 12 });
  console.log(growable.byteLength);
  console.log(growable.growable);
  console.log(growable.maxByteLength);
  growable.grow(8);
  console.log(growable.byteLength);
  console.log(new Uint8Array(growable)[6]);
  ```
  Replace `fixtures/happy/sharedarraybuffer.expected` with:
  ```text
  exit_code: 0
  --- stdout ---
  16
  false
  16
  4
  99
  4
  true
  12
  8
  0
  --- stderr ---
  ```
  Replace `fixtures/happy/atomics.js` with:
  ```js
  var sab = new SharedArrayBuffer(16);
  var ta = new Int32Array(sab);
  console.log(Atomics.store(ta, 0, 42));
  console.log(Atomics.load(ta, 0));
  console.log(Atomics.add(ta, 0, 8));
  console.log(Atomics.load(ta, 0));
  console.log(Atomics.compareExchange(ta, 0, 50, 7));
  console.log(Atomics.load(ta, 0));
  console.log(Atomics.exchange(ta, 0, 3));
  console.log(Atomics.load(ta, 0));
  console.log(Atomics.isLockFree(4));
  console.log(Atomics.wait(ta, 0, 99, 0));
  console.log(Atomics.wait(ta, 0, 3, 0));
  var result = Atomics.waitAsync(ta, 0, 99, 0);
  console.log(result.async);
  console.log(result.value);
  ```
  Replace `fixtures/happy/atomics.expected` with:
  ```text
  exit_code: 0
  --- stdout ---
  42
  42
  42
  50
  50
  7
  7
  3
  true
  not-equal
  timed-out
  false
  not-equal
  --- stderr ---
  ```
- [ ] **Verify RED**: run the exact command above and confirm failures cite these fixtures.
- [ ] **Minimal code**: none in this task.
- [ ] **Verify GREEN**: defer to later tasks; record that this task intentionally leaves RED fixtures.
- [ ] **Commit**: do not commit yet; commit after first GREEN slice.

---

## Task 2: Add shared_buffer owner module and state model

**Files**: `crates/wjsm-runtime/src/shared_buffer.rs`, `crates/wjsm-runtime/src/lib.rs`.

**Why**: Establish the canonical owner before wiring consumers.

**Impact/Compatibility**: No JS behavior change yet except internal state shape. Must keep existing compile behavior.

**Verification**:

```bash
cargo check -p wjsm-runtime
```

Expected GREEN: runtime crate compiles.

- [ ] **Write test**: no new fixture; this is compile-only owner extraction. Keep Task 1 RED fixtures as acceptance tests.
- [ ] **Verify RED**: run `cargo nextest run -E 'test(happy__sharedarraybuffer)'` and confirm it still fails for behavior, not compile.
- [ ] **Minimal code**: create `crates/wjsm-runtime/src/shared_buffer.rs` with public-in-crate types/helpers:
  ```rust
  use std::collections::{HashMap, VecDeque};
  use std::sync::{Arc, Condvar, Mutex, RwLock};
  use std::sync::atomic::AtomicBool;
  use std::time::Instant;

  use wasmtime::Caller;
  use wjsm_ir::value;

  use crate::{RuntimeState, WasmEnv};
  use crate::{alloc_host_object, define_host_data_property_from_caller, read_object_property_by_name, resolve_handle};

  #[derive(Clone, Debug)]
  pub(crate) struct SharedArrayBufferEntry {
      pub(crate) data: Arc<RwLock<Vec<u8>>>,
      pub(crate) byte_length: u64,
      pub(crate) max_byte_length: Option<u64>,
  }

  impl SharedArrayBufferEntry {
      pub(crate) fn growable(&self) -> bool { self.max_byte_length.is_some() }
      pub(crate) fn max_byte_length(&self) -> u64 { self.max_byte_length.unwrap_or(self.byte_length) }
  }

  pub(crate) struct SharedRuntimeState {
      pub(crate) sab_table: Arc<Mutex<Vec<SharedArrayBufferEntry>>>,
      pub(crate) waiter_lists: Arc<Mutex<HashMap<(u32, u32), WaiterList>>>,
      pub(crate) agent_state: Arc<AgentState>,
  }

  pub(crate) struct AgentState {
      pub(crate) reports: Arc<Mutex<Vec<String>>>,
  }

  pub(crate) struct WaiterList {
      pub(crate) waiters: VecDeque<WaiterRecord>,
  }

  pub(crate) struct WaiterRecord {
      pub(crate) notified: Arc<AtomicBool>,
      pub(crate) condvar: Arc<Condvar>,
      pub(crate) deadline: Option<Instant>,
      pub(crate) promise: Option<i64>,
  }

  #[derive(Clone, Copy, Debug, PartialEq, Eq)]
  pub(crate) enum BufferBacking {
      ArrayBuffer { handle: u32, byte_length: u32 },
      SharedArrayBuffer { handle: u32, byte_length: u32, growable: bool },
  }

  pub(crate) fn new_shared_runtime_state() -> Arc<SharedRuntimeState> {
      Arc::new(SharedRuntimeState {
          sab_table: Arc::new(Mutex::new(Vec::new())),
          waiter_lists: Arc::new(Mutex::new(HashMap::new())),
          agent_state: Arc::new(AgentState { reports: Arc::new(Mutex::new(Vec::new())) }),
      })
  }
  ```
  Then move old `SharedArrayBufferEntry`, `SharedRuntimeState`, `AgentState`, `Waiter` definitions out of `lib.rs` or replace them with `pub(crate) use shared_buffer::{...}`. Add `mod shared_buffer;` near other runtime modules. Update `RuntimeState::new()` to call `shared_buffer::new_shared_runtime_state()`. Update the stale comment on `shared_state` to Chinese: `/// normal execution 拥有单 agent cluster；$262.agent 可共享同一状态。`
- [ ] **Verify GREEN**: run `cargo check -p wjsm-runtime`.
- [ ] **Commit**: commit message `refactor: add shared buffer runtime owner`.

---

## Task 3: Implement SAB construction, slots, fixed/growable metadata

**Files**: `crates/wjsm-runtime/src/shared_buffer.rs`, `crates/wjsm-runtime/src/runtime_builtins.rs`, `crates/wjsm-runtime/src/host_imports/atomics.rs`.

**Why**: Replace `undefined` constructor path with actual SAB object construction.

**Impact/Compatibility**: Enables `new SharedArrayBuffer`; must not affect `ArrayBuffer`.

**Verification**:

```bash
cargo nextest run -E 'test(happy__sharedarraybuffer)'
```

Expected partial GREEN may still fail on TypedArray/slice/grow until later tasks; constructor `byteLength` should no longer be `undefined` when manually run.

- [ ] **Write test**: add `fixtures/happy/sharedarraybuffer_constructor.js`:
  ```js
  var sab = new SharedArrayBuffer(16);
  console.log(sab.byteLength);
  console.log(sab.growable);
  console.log(sab.maxByteLength);
  var growable = new SharedArrayBuffer(4, { maxByteLength: 12 });
  console.log(growable.byteLength);
  console.log(growable.growable);
  console.log(growable.maxByteLength);
  ```
  Add `fixtures/happy/sharedarraybuffer_constructor.expected`:
  ```text
  exit_code: 0
  --- stdout ---
  16
  false
  16
  4
  true
  12
  --- stderr ---
  ```
- [ ] **Verify RED**: run `cargo nextest run -E 'test(happy__sharedarraybuffer_constructor)'`.
- [ ] **Minimal code**: implement in `shared_buffer.rs`:
  - `to_index_from_value(caller, value, error_message) -> Option<u64>` using existing numeric decoding and runtime_error for negative/non-finite values.
  - `construct_shared_array_buffer(caller, length, options, target_obj) -> i64` that reads `maxByteLength` from object options when present, checks `byte_length <= max`, pushes `SharedArrayBufferEntry`, allocates or reuses host object, and defines `__sharedarraybuffer_handle__`, `byteLength`, `growable`, `maxByteLength`.
  - `shared_array_buffer_byte_length(caller, this_val)`, `shared_array_buffer_growable`, `shared_array_buffer_max_byte_length`.
  Replace `NativeCallable::SharedArrayBufferConstructor => Some(value::encode_undefined())` with a call to this helper. Replace `atomics.rs` SAB constructor/byteLength stub bodies with calls to the same helpers.
- [ ] **Verify GREEN**: run `cargo nextest run -E 'test(happy__sharedarraybuffer_constructor)'` and `cargo check -p wjsm-runtime`.
- [ ] **Commit**: commit message `fix: construct shared array buffers`.

---

## Task 4: Add SAB grow/growable/maxByteLength/slice builtins across IR-semantic-backend-runtime

**Files**: `crates/wjsm-ir/src/builtin.rs`, `crates/wjsm-semantic/src/builtins.rs`, `crates/wjsm-backend-wasm/src/host_import_registry.rs`, `crates/wjsm-backend-wasm/src/compiler_builtins.rs`, `crates/wjsm-runtime/src/shared_buffer.rs`, `crates/wjsm-runtime/src/host_imports/atomics.rs`.

**Why**: Current IR has byteLength/slice/species but lacks grow/growable/maxByteLength and runtime slice is a stub returning `this`.

**Impact/Compatibility**: Adds imports; registry owns names/signatures.

**Verification**:

```bash
cargo nextest run -p wjsm-backend-wasm
cargo nextest run -E 'test(happy__sharedarraybuffer)'
```

- [ ] **Write test**: keep Task 1 `sharedarraybuffer.js` as acceptance. Add error fixture `fixtures/errors/sharedarraybuffer_grow_invalid.js`:
  ```js
  var fixed = new SharedArrayBuffer(4);
  fixed.grow(8);
  ```
  Add expected:
  ```text
  exit_code: 1
  --- stdout ---
  --- stderr ---
  TypeError
  ```
- [ ] **Verify RED**: run `cargo nextest run -E 'test(happy__sharedarraybuffer) or test(errors__sharedarraybuffer_grow_invalid)'`.
- [ ] **Minimal code**:
  - Add `SharedArrayBufferProtoGrow`, `SharedArrayBufferProtoGrowable`, `SharedArrayBufferProtoMaxByteLength`, and `AtomicsPause` to `Builtin` display names.
  - Map static/prototype calls in semantic builtins for `grow`, `growable`, `maxByteLength`, `Atomics.pause`.
  - Add host import registry rows: `sharedarraybuffer_proto_grow` type 2 or suitable two-arg signature, `sharedarraybuffer_proto_growable` type 3, `sharedarraybuffer_proto_max_byte_length` type 3, `atomics_pause` no/one dummy arg as existing ABI requires.
  - Add compiler_builtins dispatch arms matching the signatures.
  - Implement `shared_array_buffer_grow`, `shared_array_buffer_slice`, `shared_array_buffer_species`, `atomics_pause` helpers.
  - Replace `sab_slice_stub` returning `this_val` with real copy to a new SAB.
- [ ] **Verify GREEN**: run both commands above.
- [ ] **Commit**: commit message `feat: add growable shared array buffers`.

---

## Task 5: Resolve buffer backing for TypedArray and DataView

**Files**: `crates/wjsm-runtime/src/shared_buffer.rs`, `crates/wjsm-runtime/src/lib.rs`, `crates/wjsm-runtime/src/host_imports/collections_buffers.rs`, `crates/wjsm-runtime/src/host_imports/typedarray_new_methods.rs`.

**Why**: `new Int32Array(sab)` currently cannot resolve SAB and always records `is_shared=false`.

**Impact/Compatibility**: ArrayBuffer path must remain unchanged.

**Verification**:

```bash
cargo nextest run -E 'test(happy__atomics) or test(happy__sharedarraybuffer)'
cargo nextest run -E 'test(happy__typedarray)'
```

If the second selector has no exact generated test, run the closest existing typed array fixture tests.

- [ ] **Write test**: add `fixtures/happy/sharedarraybuffer_views.js`:
  ```js
  var sab = new SharedArrayBuffer(16);
  var i32 = new Int32Array(sab);
  i32[0] = 123;
  console.log(i32.length);
  console.log(i32.byteLength);
  console.log(i32.byteOffset);
  console.log(new Int32Array(sab)[0]);
  var dv = new DataView(sab);
  dv.setInt32(4, 77);
  console.log(dv.getInt32(4));
  var bi = new BigInt64Array(sab);
  bi[1] = 9n;
  console.log(bi[1]);
  ```
  Add expected:
  ```text
  exit_code: 0
  --- stdout ---
  4
  16
  0
  123
  77
  9
  --- stderr ---
  ```
- [ ] **Verify RED**: run `cargo nextest run -E 'test(happy__sharedarraybuffer_views)'`.
- [ ] **Minimal code**:
  - Implement `resolve_buffer_backing(caller, value) -> Option<BufferBacking>` that checks `__arraybuffer_handle__` then `__sharedarraybuffer_handle__`.
  - Extend `DataViewEntry` in `lib.rs` with `is_shared: bool`.
  - In `typedarray_construct`, carry `(buf_handle, offset, len, byte_len, is_shared)` instead of fixed four-tuple; set `TypedArrayEntry.is_shared = is_shared`.
  - Update `typedarray_element_read/write`, `ta_read/write`, iterator helpers, and prototype methods to use `sab_read/sab_write` or new shared_buffer read/write when `entry.is_shared`.
  - In DataView constructor and get/set macros, branch on `is_shared` and use shared-buffer read/write.
- [ ] **Verify GREEN**: run the verification commands.
- [ ] **Commit**: commit message `fix: support typed views over shared array buffers`.

---

## Task 6: Install complete Atomics global object and method dispatch

**Files**: `crates/wjsm-runtime/src/lib.rs`, `crates/wjsm-runtime/src/runtime_builtins.rs`, `crates/wjsm-runtime/src/host_imports/get_builtin_global_entry.rs`, `crates/wjsm-runtime/src/host_imports/collections_buffers.rs`.

**Why**: `AtomicsGlobal` currently returns an empty object; direct `Atomics.load` may lower to builtin but dynamic property use must also work.

**Impact/Compatibility**: Only affects global `Atomics` object.

**Verification**:

```bash
cargo nextest run -E 'test(happy__atomics_global)'
```

- [ ] **Write test**: add `fixtures/happy/atomics_global.js`:
  ```js
  console.log(typeof Atomics.load);
  console.log(typeof Atomics.store);
  console.log(typeof Atomics.waitAsync);
  console.log(Atomics[Symbol.toStringTag]);
  var f = Atomics.load;
  var sab = new SharedArrayBuffer(4);
  var ta = new Int32Array(sab);
  Atomics.store(ta, 0, 11);
  console.log(f(ta, 0));
  ```
  Add expected:
  ```text
  exit_code: 0
  --- stdout ---
  function
  function
  function
  Atomics
  11
  --- stderr ---
  ```
- [ ] **Verify RED**: run the command above.
- [ ] **Minimal code**:
  - Add `NativeCallable::AtomicsMethod { kind: AtomicsMethodKind }` and `AtomicsMethodKind` variants for every method.
  - In `AtomicsGlobal`, allocate object and define `load`, `store`, `add`, `sub`, `and`, `or`, `xor`, `exchange`, `compareExchange`, `isLockFree`, `wait`, `notify`, `waitAsync`, `pause`, and `Symbol.toStringTag` if symbol-property helper exists; otherwise define string fallback consistent with current object model.
  - Dispatch `NativeCallable::AtomicsMethod` to the same helper functions used by host imports.
- [ ] **Verify GREEN**: run `cargo nextest run -E 'test(happy__atomics_global)'`.
- [ ] **Commit**: commit message `feat: expose atomics methods on global object`.

---

## Task 7: Replace unsafe pointer Atomics with owner-backed validation and RMW helpers

**Files**: `crates/wjsm-runtime/src/shared_buffer.rs`, `crates/wjsm-runtime/src/host_imports/atomics.rs`.

**Why**: Current Atomics casts `Vec<u8>` bytes to atomic pointers, lacks bounds checks, rejects BigInt64Array, and returns `undefined` instead of TypeError/RangeError.

**Impact/Compatibility**: Non-wait Atomics must work on integer TypedArray over ArrayBuffer and SAB per spec. wait/waitAsync still require SAB.

**Verification**:

```bash
cargo nextest run -E 'test(happy__atomics) or test(happy__atomics_bigint)'
cargo nextest run -E 'test(errors__atomics_invalid)'
```

- [ ] **Write test**: add `fixtures/happy/atomics_bigint.js`:
  ```js
  var sab = new SharedArrayBuffer(16);
  var ta = new BigInt64Array(sab);
  console.log(Atomics.store(ta, 0, 5n));
  console.log(Atomics.load(ta, 0));
  console.log(Atomics.add(ta, 0, 3n));
  console.log(Atomics.load(ta, 0));
  console.log(Atomics.compareExchange(ta, 0, 8n, 1n));
  console.log(Atomics.load(ta, 0));
  ```
  Add expected stdout: `5`, `5`, `5`, `8`, `8`, `1`. Add `fixtures/errors/atomics_invalid.js`:
  ```js
  var sab = new SharedArrayBuffer(16);
  Atomics.load(new Float32Array(sab), 0);
  ```
  Expected stderr contains `TypeError`.
- [ ] **Verify RED**: run both selectors above.
- [ ] **Minimal code**:
  - Implement `validate_integer_typed_array`, `validate_waitable_typed_array`, and `validate_atomic_access` in `shared_buffer.rs`.
  - Implement byte-based helpers: `atomic_load`, `atomic_store`, `atomic_rmw`, `atomic_compare_exchange`. Use the shared block lock to guarantee atomicity; do not cast unaligned `u8` pointers to `AtomicI32`.
  - Support element sizes 1, 2, 4, and 8; element_kind 0/1 for Number integer, 4/5 for BigInt integer. Reject float and clamped for Atomics.
  - Return proper Number/BigInt values using existing `bigint_table` helpers.
  - Update `atomics.rs` host imports to delegate to helpers.
- [ ] **Verify GREEN**: run the verification commands and `cargo check -p wjsm-runtime`.
- [ ] **Commit**: commit message `fix: implement spec atomics operations`.

---

## Task 8: Implement Atomics.wait/notify/waitAsync waiter store

**Files**: `crates/wjsm-runtime/src/shared_buffer.rs`, `crates/wjsm-runtime/src/host_imports/atomics.rs`, `crates/wjsm-runtime/src/runtime_builtins.rs`.

**Why**: Current wait/waitAsync always time out when equal and notify always returns 0.

**Impact/Compatibility**: Adds blocking wait and Promise-based async wait. Must avoid hanging tests; fixtures use timeout 0 or agent-driven notify.

**Verification**:

```bash
cargo nextest run -E 'test(happy__atomics_wait_notify) or test(happy__atomics_wait_async)'
```

- [ ] **Write test**: add `fixtures/happy/atomics_wait_async.js`:
  ```js
  var sab = new SharedArrayBuffer(4);
  var ta = new Int32Array(sab);
  console.log(Atomics.wait(ta, 0, 1, 0));
  console.log(Atomics.wait(ta, 0, 0, 0));
  var r = Atomics.waitAsync(ta, 0, 1, 0);
  console.log(r.async);
  console.log(r.value);
  var r2 = Atomics.waitAsync(ta, 0, 0, 0);
  console.log(r2.async);
  console.log(r2.value);
  console.log(Atomics.notify(ta, 0, 1));
  ```
  Add expected stdout: `not-equal`, `timed-out`, `false`, `not-equal`, `false`, `timed-out`, `0`.
- [ ] **Verify RED**: run `cargo nextest run -E 'test(happy__atomics_wait_async)'`.
- [ ] **Minimal code**:
  - Implement waiter key `(sab_handle, byte_index)`, `enter/remove/notify` helpers in `shared_buffer.rs`.
  - `Atomics.wait`: only Int32Array/BigInt64Array over SAB; if mismatch return `not-equal`; if timeout 0 return `timed-out`; otherwise wait on condvar until notified/deadline.
  - `Atomics.waitAsync`: immediate cases return `{ async: false, value: string }`; non-immediate creates Promise and stores async waiter; timeout/notify resolves via existing Promise helper/microtask path.
  - `Atomics.notify`: remove FIFO waiters up to count, wake blocking waiters, settle async waiters, return removed count.
  - Use exact `ToIntegerOrInfinity(count)` semantics for count; negative count becomes 0.
- [ ] **Verify GREEN**: run verification command. Also run `cargo nextest run -E 'test(happy__atomics)'`.
- [ ] **Commit**: commit message `feat: implement atomics waiter lists`.

---

## Task 9: Add complete error fixtures and fix error classes

**Files**: `fixtures/errors/*.js`, `fixtures/errors/*.expected`, `crates/wjsm-runtime/src/shared_buffer.rs`, `crates/wjsm-runtime/src/host_imports/atomics.rs`.

**Why**: Spec-complete behavior requires wrong receivers/types/bounds to fail with the right error class, not return `undefined`.

**Impact/Compatibility**: Error output format must match existing fixture harness style.

**Verification**:

```bash
cargo nextest run -E 'test(errors__sharedarraybuffer) or test(errors__atomics)'
```

- [ ] **Write test**: add fixtures:
  - `fixtures/errors/sharedarraybuffer_call_without_new.js`: `SharedArrayBuffer(1);` → stderr contains `TypeError`.
  - `fixtures/errors/sharedarraybuffer_bad_receiver.js`: `SharedArrayBuffer.prototype.slice.call({}, 0, 1);` → `TypeError`.
  - `fixtures/errors/sharedarraybuffer_grow_range.js`: `new SharedArrayBuffer(4, { maxByteLength: 8 }).grow(16);` → `RangeError`.
  - `fixtures/errors/atomics_wait_non_shared.js`: `Atomics.wait(new Int32Array(new ArrayBuffer(4)), 0, 0, 0);` → `TypeError`.
  - `fixtures/errors/atomics_wait_wrong_typedarray.js`: `Atomics.wait(new Uint32Array(new SharedArrayBuffer(4)), 0, 0, 0);` → `TypeError`.
  - `fixtures/errors/atomics_index_oob.js`: `Atomics.load(new Int32Array(new SharedArrayBuffer(4)), 2);` → `RangeError`.
  - `fixtures/errors/atomics_bigint_number_value.js`: `Atomics.store(new BigInt64Array(new SharedArrayBuffer(8)), 0, 1);` → `TypeError`.
- [ ] **Verify RED**: run the selector above and confirm failures before error fixes.
- [ ] **Minimal code**: ensure every validation helper calls existing runtime error/exception path with correct class. Replace `return value::encode_undefined()` on invalid Atomics/SAB receiver paths with TypeError/RangeError runtime_error.
- [ ] **Verify GREEN**: run error selector and then `cargo nextest run -E 'test(errors__)'` if duration is acceptable; otherwise run all newly added exact generated tests.
- [ ] **Commit**: commit message `fix: enforce sab atomics error semantics`.

---

## Task 10: Wire agent harness shared state for notify integration

**Files**: `crates/wjsm-runtime/src/runtime_builtins.rs`, `crates/wjsm-runtime/src/shared_buffer.rs`, relevant `$262.agent` code paths.

**Why**: Spec includes agent cluster waiter store; test262 SAB/Atomics relies on `$262.agent` broadcast/report when available.

**Impact/Compatibility**: Existing `$262.agent.getReport` behavior must remain.

**Verification**:

```bash
cargo nextest run -E 'test(happy__atomics_agent_notify)'
```

- [ ] **Write test**: add `fixtures/happy/atomics_agent_notify.js` only if current harness can execute `$262.agent.start`; use:
  ```js
  var sab = new SharedArrayBuffer(4);
  var ta = new Int32Array(sab);
  $262.agent.start(`
    $262.agent.receiveBroadcast(function(sab) {
      var ta = new Int32Array(sab);
      Atomics.store(ta, 0, 1);
      Atomics.notify(ta, 0, 1);
      $262.agent.report('done');
    });
  `);
  $262.agent.broadcast(sab);
  console.log($262.agent.getReport());
  console.log(Atomics.load(ta, 0));
  ```
  Expected stdout: `done`, `1`. If `$262.agent.start` is still intentionally no-op, do not add this fixture yet; instead document harness limitation in implementation notes and keep waiter store covered by direct notify tests.
- [ ] **Verify RED**: run selector if fixture added; otherwise run `cargo check -p wjsm-runtime` and note that no fixture was added because harness cannot execute agents yet.
- [ ] **Minimal code**: if harness is active, share the same `Arc<SharedRuntimeState>` across sub-agent RuntimeState clones and route broadcast SAB handles by reference, not copied bytes. If harness remains no-op, do not fake behavior; keep waiter owner ready and leave agent execution for a separate `$262.agent` plan.
- [ ] **Verify GREEN**: run exact fixture if added, plus `cargo check -p wjsm-runtime`.
- [ ] **Commit**: commit message `feat: share sab state across agents` if code changed; otherwise no commit.

---

## Task 11: Cleanup, regression, and ADR/backfill notes

**Files**: `docs/aegis/INDEX.md`, optional ADR/baseline note if project convention requires after implementation.

**Why**: Retire obsolete paths and preserve durable architecture decision signals.

**Impact/Compatibility**: No behavior change except removing dead stubs/comments.

**Verification**:

```bash
cargo nextest run -E 'test(happy__sharedarraybuffer) or test(happy__atomics)'
cargo nextest run -p wjsm-runtime
cargo nextest run -p wjsm-backend-wasm
```

- [ ] **Write test**: no new tests; use full SAB/Atomics fixture set from earlier tasks.
- [ ] **Verify RED**: search for old paths using built-in search, not shell: `NativeCallable::SharedArrayBufferConstructor => Some(value::encode_undefined())`, `AtomicsGlobal => Some({ alloc_host_object`, `SharedArrayBuffer stubs`, `None in normal (non-agent) execution`. Any hit is RED.
- [ ] **Minimal code**: delete obsolete stubs/comments; ensure all SAB/Atomics public paths call shared_buffer owner. Add ADR/baseline follow-up note if implementation changed owner contracts.
- [ ] **Verify GREEN**: run all commands above and exact generated tests for every new fixture.
- [ ] **Commit**: commit message `docs: record sab atomics architecture cleanup` if docs changed; otherwise `refactor: retire sab atomics stubs`.

---

## Final Verification

Run after all tasks:

```bash
cargo nextest run -E 'test(happy__sharedarraybuffer) or test(happy__sharedarraybuffer_constructor) or test(happy__sharedarraybuffer_views) or test(happy__atomics) or test(happy__atomics_global) or test(happy__atomics_bigint) or test(happy__atomics_wait_async)'
cargo nextest run -E 'test(errors__sharedarraybuffer) or test(errors__atomics)'
cargo nextest run -p wjsm-runtime
cargo nextest run -p wjsm-backend-wasm
```

Task 10 已覆盖：`fixtures/happy/atomics_agent_notify.js` 验证 `$262.agent.start` / `broadcast` / `receiveBroadcast` 与主 agent 共享 SAB waiter/backing。

## Risks

| Risk | Mitigation |
|---|---|
| Hidden properties leak internal slots | Prefer side-table lookup; if hidden properties remain, define as non-enumerable/non-writable where helper supports it |
| `wait` can hang tests | Fixtures use timeout 0 unless agent notify is verified; real wait uses timeout/deadline |
| `waitAsync` Promise resolution crosses agent/store boundary | Use existing Promise/microtask owner; do not touch Store from worker thread |
| Unaligned atomic pointer casts are UB | Use owner lock + byte conversion, not pointer casts |
| growable SAB with active views | Recompute current byteLength via backing owner before access; no detach behavior |
| host import ABI mismatch | Add registry entries and backend tests before runtime behavior claims |

## ADR / Baseline Sync Signal

After implementation is verified, record an ADR or baseline sync note covering:

1. `shared_buffer.rs` as canonical owner for SAB/shared-block/waiter store.
2. Decision to model §29 via JS-observable atomicity instead of full candidate execution graph.
3. Decision to expose `SharedArrayBuffer` in normal wjsm runtime.
4. Retirement of SAB/Atomics stub paths.

## Plan Self-Review

- Spec coverage: every §25.2/§25.4 surface in the approved spec maps to at least one task.
- Placeholder scan: no TBD/TODO placeholders; tasks name exact files and fixtures.
- Type consistency: plan uses existing `i64` NaN-box ABI, `TypedArrayEntry.element_kind`, runtime side tables, and host import registry.
- Compatibility: ArrayBuffer/non-shared TypedArray/DataView/Promise/import registry boundaries are explicit.
- Complexity: new owner file avoids worsening `atomics.rs` and `collections_buffers.rs`.
- Verification: exact commands and fixture contents are included.
- Dual-track: repair and retirement tracks are preserved.
