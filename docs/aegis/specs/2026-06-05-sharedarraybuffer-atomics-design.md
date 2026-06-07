# SharedArrayBuffer + Atomics ES 全量实现设计规格

**日期**: 2026-06-05
**状态**: Implemented (2026-06-07) — Phase C/D/E 完成；单线程 wait 模型已落地
**范围**: ECMAScript Structured Data §25.2 SharedArrayBuffer Objects + §25.4 Atomics Object 当前版本全量行为；包含 fixed-length 与 growable SharedArrayBuffer、TypedArray/DataView shared backing、Atomics 全方法、wait/notify/waitAsync、BigInt64Array waitable 路径、agent cluster waiter store。

---

## 1. 背景与问题

当前仓库已经有 SharedArrayBuffer / Atomics 的部分铺垫，但正常 JS 行为仍是断开的：

1. IR `Builtin` 与 WASM backend import 映射已注册 SAB/Atomics 相关 builtin。
2. runtime 有 `SharedRuntimeState`、`sab_table`、`waiter_lists` 等基础结构。
3. `RuntimeState::new()` 已为 non-agent 执行创建 `shared_state`。
4. 但 `NativeCallable::SharedArrayBufferConstructor` 当前直接返回 `undefined`。
5. `new SharedArrayBuffer(...)` 不走真正的 SAB 分配路径。
6. `typedarray_construct` 只识别 `__arraybuffer_handle__`，不识别 `__sharedarraybuffer_handle__`，并固定写入 `TypedArrayEntry { is_shared: false }`。
7. `AtomicsGlobal` 当前返回空 host object，正常 JS 访问不到完整方法。
8. 现有 `fixtures/happy/sharedarraybuffer.js` 与 `fixtures/happy/atomics.js` 的 `.expected` 记录的是 broken behavior：输出 `undefined`。
9. 2026-06-07 已完成：`shared_buffer.rs` owner、SAB 构造/grow/slice、TypedArray/DataView shared backing、`Atomics.wait`/`waitAsync`/`notify` waiter store、DataView 静态绑定直连 `CallBuiltin`（修复 `indirect call type mismatch`）。

本设计目标不是让这些 fixture 局部变绿，而是把 SharedArrayBuffer + Atomics 补成按 ES 规范可维护的完整实现。

---

## 2. 权威依据

### 2.1 规范依据

- ECMAScript Structured Data §25.1.3 ArrayBuffer abstract operations
  - `ArrayBufferByteLength`
  - `GetValueFromBuffer`
  - `SetValueInBuffer`
  - `GetModifySetValueInBuffer`
  - shared block read/write/RMW 的 ordering 语义
- ECMAScript §25.2 SharedArrayBuffer Objects
  - fixed-length / growable SharedArrayBuffer
  - `AllocateSharedArrayBuffer`
  - `IsSharedArrayBuffer`
  - `IsGrowableSharedArrayBuffer`
  - constructor call restrictions
  - constructor `%Symbol.species%`
  - prototype `byteLength`, `grow`, `growable`, `maxByteLength`, `slice`, `%Symbol.toStringTag%`
- ECMAScript §25.4 Atomics Object
  - integer typed array validation
  - waitable typed array validation
  - atomic access validation and revalidation
  - waiter record / waiter list / critical section model
  - `add`, `and`, `compareExchange`, `exchange`, `isLockFree`, `load`, `or`, `store`, `sub`, `wait`, `waitAsync`, `notify`, `pause`, `xor`
- ECMAScript §29 Memory Model
  - 本实现不构造完整 candidate execution graph，但 runtime 必须保证 JS 可观测的 atomicity、wait/notify ordering、no-tear 整数 shared typed array 行为。

### 2.2 项目依据

- `docs/aegis/plans/2026-05-27-typedarray-methods-completion.md`
- `docs/aegis/specs/2026-05-28-host-import-registry-design.md`
- `crates/wjsm-runtime/src/lib.rs`
- `crates/wjsm-runtime/src/runtime_builtins.rs`
- `crates/wjsm-runtime/src/host_imports/atomics.rs`
- `crates/wjsm-runtime/src/host_imports/collections_buffers.rs`
- `crates/wjsm-runtime/src/host_imports/typedarray_new_methods.rs`
- `crates/wjsm-backend-wasm/src/host_import_registry.rs`
- `crates/wjsm-backend-wasm/src/compiler_builtins.rs`
- `crates/wjsm-ir/src/builtin.rs`
- `crates/wjsm-semantic/src/builtins.rs`

---

## 3. 目标

### 3.1 SharedArrayBuffer

实现当前 ECMAScript §25.2 的完整可观测语义：

| 功能 | 目标状态 |
|---|---|
| `new SharedArrayBuffer(length)` | 创建 fixed-length SAB，byteLength 为 `ToIndex(length)` |
| `new SharedArrayBuffer(length, { maxByteLength })` | 创建 growable SAB，初始 length 与 maxByteLength 按规范校验 |
| `SharedArrayBuffer(length)` | 作为普通函数调用抛 `TypeError` |
| `SharedArrayBuffer.prototype.byteLength` | 只接受 SAB receiver；返回 fixed/growable 当前长度 |
| `SharedArrayBuffer.prototype.growable` | fixed 返回 false；growable 返回 true |
| `SharedArrayBuffer.prototype.maxByteLength` | fixed 返回当前 byteLength；growable 返回 maxByteLength |
| `SharedArrayBuffer.prototype.grow(newLength)` | 仅 growable；不可缩小；不可超过 max；成功后新区域零填充 |
| `SharedArrayBuffer.prototype.slice(start, end)` | 返回新的 SAB，复制指定范围；不共享 backing block |
| `SharedArrayBuffer[Symbol.species]` | getter 返回 this value |
| `SharedArrayBuffer.prototype[Symbol.toStringTag]` | 返回 `"SharedArrayBuffer"` |
| SAB instance detach | 不支持 detach；SAB 永不 detached |

### 3.2 TypedArray / DataView shared backing

| 功能 | 目标状态 |
|---|---|
| `new Int32Array(sab)` | 识别 SAB backing，创建 `is_shared=true` view |
| `new BigInt64Array(sab)` | 支持 BigInt shared backing |
| 其他 TypedArray over SAB | 支持普通元素读写；Atomics wait 仅允许 Int32/BigInt64 |
| TypedArray length/byteLength/byteOffset | fixed/growable SAB 下按 witness/cached length 语义返回 |
| TypedArray index access | shared backing 使用 shared-block read/write helpers；整数类型 no-tear |
| DataView over SAB | 读写走 shared-block helpers；bounds 检查按当前 byteLength |
| growable SAB interaction | view 对 grow 后的新长度按规范进行 bounds/witness 检查；fixed-length view 不被错误 detach |

### 3.3 Atomics

实现 §25.4 全方法：

| 方法 | 目标状态 |
|---|---|
| `Atomics.add/and/or/sub/xor/exchange` | 整数 TypedArray 上原子 RMW；返回旧值 |
| `Atomics.compareExchange` | 原子比较交换；返回旧值 |
| `Atomics.load` | SEQ-CST shared/plain integer typed array load |
| `Atomics.store` | SEQ-CST store；返回 coerced value |
| `Atomics.isLockFree` | 至少 `4` 返回 true；其他 size 按 host 能力稳定返回 boolean |
| `Atomics.wait` | 只允许 shared `Int32Array` / `BigInt64Array`；返回 `"not-equal"`、`"timed-out"`、`"ok"` |
| `Atomics.waitAsync` | 返回 `{ async: false, value: string }` 或 `{ async: true, value: Promise }` |
| `Atomics.notify` | 从 waiter list 移除并唤醒最多 count 个 waiter；返回唤醒数量 |
| `Atomics.pause` | hint no-op；返回 undefined |
| `Atomics[Symbol.toStringTag]` | 返回 `"Atomics"` |

---

## 4. 非目标

- 不实现浏览器安全策略中的 cross-origin isolation gating。ECMAScript 允许宿主在无并发访问时省略 `SharedArrayBuffer` 全局属性；wjsm runtime 本次选择提供该全局属性。
- 不实现 Web Worker API。并发 agent 支持限于 runtime/test262 harness 所需的 agent cluster 与 waiter store。
- 不构造完整 §29 candidate execution / chosen values 可检查图。runtime 以 host mutex/condvar/atomic critical section 保证可观测语义。
- 不把 ArrayBuffer resize 作为本设计目标；只处理 SAB growable 相关能力。
- 不引入新的跨 crate 共享依赖或 runtime → backend 反向依赖。

---

## 5. 第一性原则与决策

### 5.1 First-principles invariants

- **Non-negotiable goal:** JS 可观测行为必须符合 ES §25.2/§25.4，而不是仅让 snapshots 变绿。
- **Non-negotiable constraints:** 不允许 stub、部分语义或保留 duplicate owner；共享 backing 数据必须只有一个 canonical owner。
- **Historical assumptions to delete:** `shared_state` 只存在 agent 模式、SAB 只是 `$262.agent` 辅助设施、Atomics 没有真实 waiter 也可接受。

### 5.2 选定方案

采用 **Runtime helper owner + thin wiring**：

1. 新增或抽出单一 runtime owner，管理 shared data block、grow metadata、waiter list、SAB object internal-slot side table。
2. `runtime_builtins.rs`、`collections_buffers.rs`、`typedarray_new_methods.rs`、`atomics.rs` 只做薄 wiring 或调用 helper。
3. backend host import registry 继续作为 import 名字/签名 owner；运行时按名字链接，不引入数字 index 真相源。
4. 删除旧的 `undefined` stub 和只在 `atomics.rs` 局部生效的 SAB constructor stub，避免双 owner。

### 5.3 Owner / retirement matrix

| 项 | 决策 |
|---|---|
| New canonical owner | SAB/shared data block + grow metadata + waiter lists helper module |
| Old owner to retire | `NativeCallable::SharedArrayBufferConstructor => undefined`；`atomics.rs` 内局部 SAB constructor stub |
| Compat-only carrier | Host import names/signatures 保持 registry 派生；实现委托到 runtime owner |
| Delete-first trigger | normal JS `new SharedArrayBuffer`、direct builtin constructor、prototype methods 全部调用同一 owner 后，删除所有 stub 分支 |

---

## 6. 架构设计

### 6.1 Runtime state model

新增或重塑 shared backing 数据结构，使 fixed 与 growable SAB 能共用一个 owner：

```rust
SharedRuntimeState
  sab_table: Vec<SharedArrayBufferEntry>
  waiter_lists: HashMap<(SabHandle, ByteOffset), WaiterList>

SharedArrayBufferEntry
  data: Arc<Mutex<Vec<u8>>>
  byte_length: AtomicU64 or Mutex<u64>
  max_byte_length: Option<u64>
  growable: bool
```

实现可以使用 `Mutex<Vec<u8>>` 作为 first implementation 的 shared block；只要所有 SAB/Atomics/DataView/TypedArray shared 路径都走同一锁域，即可保证 wjsm 可观测的 atomicity。后续若替换为更低层 atomic byte storage，外部 contract 不变。

`RuntimeState::shared_state` 在 normal execution 也必须存在。旧注释 `None in normal (non-agent) execution` 必须更新为：normal execution 拥有单 agent cluster；agent harness 可共享同一 `SharedRuntimeState`。

### 6.2 SAB object representation

SAB JS object 使用 host object + hidden data properties 模拟 internal slots：

| Hidden property | 含义 |
|---|---|
| `__sharedarraybuffer_handle__` | `sab_table` handle |
| `byteLength` or accessor | 当前可观测 byteLength；growable 下从 entry 读取 |
| `maxByteLength` | fixed 下等于 byteLength；growable 下为 max |
| `growable` | boolean |

实现必须避免让用户可枚举/可写这些 hidden properties 破坏 internal slot 语义。若当前 object helper 只能定义 data property，则实现计划必须优先补齐 non-enumerable/non-writable hidden data property 或改为 side-table lookup，不把规范 internal slot 暴露给 JS 枚举。

### 6.3 Constructor and prototype wiring

`SharedArrayBuffer` 的正常 JS path 与 builtin host import path 必须收敛：

```text
new SharedArrayBuffer(length, options)
  → NativeCallable::SharedArrayBufferConstructor
  → shared_buffer_construct(length, options, new_target)
  → SharedRuntimeState.sab_table push
  → JS host object with SAB hidden slot
```

prototype methods：

```text
sab.byteLength / sab.growable / sab.maxByteLength / sab.grow() / sab.slice()
  → NativeCallable or Builtin host import
  → shared_buffer_* helper
```

`SharedArrayBuffer(length)` 普通调用必须抛 TypeError。若当前 NativeCallable dispatch 没有 `new.target` 区分能力，实施计划必须先建立 constructor-call marker，而不是把普通调用也当 constructor。

### 6.4 TypedArray shared backing

`typedarray_construct` 必须把 backing buffer resolution 抽成 helper：

```text
resolve_buffer_backing(value)
  → ArrayBuffer(handle, byteLength)
  → SharedArrayBuffer(handle, byteLength, growable)
  → NotBuffer
```

创建 view 时：

- ArrayBuffer: `TypedArrayEntry { buffer_handle, is_shared: false }`
- SharedArrayBuffer: `TypedArrayEntry { buffer_handle: sab_handle, is_shared: true }`

`ta_read` / `ta_write` / indexed access / prototype methods 必须基于 `entry.is_shared` 分流：

- `false`: 继续使用 `arraybuffer_table`
- `true`: 使用 shared-buffer owner 读写

BigInt64Array / BigUint64Array already use `element_kind` 4/5 in the typedarray completion plan. Atomics waitable path 只允许 `element_kind == 0 && elem_size == 4` 或 `element_kind == 4 && elem_size == 8`。

### 6.5 DataView shared backing

DataView constructor 也必须使用 `resolve_buffer_backing`。`DataViewEntry` 需要记录 `is_shared`，否则 DataView over SAB 会错误读取 `arraybuffer_table`。

DataView getter/setter 的 shared path 使用同一个 shared-buffer helper，并按 element size 做 bounds check。DataView 操作不是 Atomics 操作，但在 shared block 上读写仍必须不可访问越界、不可读 stale table。

### 6.6 Atomics object wiring

`AtomicsGlobal` 必须返回带完整方法的 object，而不是空 object。每个方法用 `NativeCallable::AtomicsMethod { kind }` 或等价枚举分发到 `atomics.rs` owner。

Atomics validation helper：

```text
validate_integer_typed_array(ta, waitable)
  → TypedArrayEntry
  → reject non-typedarray
  → reject float / Uint8Clamped for Atomics operations
  → if waitable: only Int32Array or BigInt64Array

validate_atomic_access(entry, index)
  → ToIndex(index)
  → bounds by current view length
  → byteIndex = byteOffset + index * elementSize
```

Atomics read/write/RMW：

- Must revalidate access after value coercion.
- Number typed arrays coerce with `ToIntegerOrInfinity` then element conversion.
- BigInt typed arrays coerce with `ToBigInt` / `ToBigInt64` semantics.
- Return old value for RMW/compareExchange, stored value for store.
- Plain ArrayBuffer-backed integer typed arrays remain valid for non-wait Atomics operations per spec algorithms; wait/waitAsync require SAB.

### 6.7 Waiter list and agent cluster

`SharedRuntimeState.waiter_lists` 是 agent cluster 级 owner，key 为 `(sab_handle, byte_index)`。Waiter list 必须包含：

```rust
WaiterList
  mutex/critical_section
  waiters: VecDeque<WaiterRecord>
  condvar or notification channel

WaiterRecord
  agent_signifier
  mode: Blocking | AsyncPromise
  timeout_deadline
  result: ok | timed-out
  promise_handle: Option<i64>
```

`Atomics.wait`：

1. validate waitable typed array。
2. require backing buffer is SAB。
3. read current value under waiter critical section。
4. mismatch → return `"not-equal"`。
5. timeout == 0 → return `"timed-out"`。
6. otherwise enqueue blocking waiter and suspend current agent until notify or timeout。
7. return `"ok"` or `"timed-out"`。

`Atomics.waitAsync`：

1. Same validation and initial compare.
2. mismatch → `{ async: false, value: "not-equal" }`。
3. timeout == 0 → `{ async: false, value: "timed-out" }`。
4. otherwise create Promise, enqueue async waiter, return `{ async: true, value: promise }`。
5. notify/timeout resolves promise through existing Promise/microtask machinery in the target agent。

`Atomics.notify`：

1. validate waitable typed array and access。
2. If buffer is not SAB, return `+0` per spec。
3. count undefined → infinity; else `ToIntegerOrInfinity(count)` and clamp negative to 0。
4. remove up to count waiters in FIFO order。
5. blocking waiter wakes condvar; async waiter schedules promise resolution.
6. return number removed.

### 6.7.1 单线程 wait 模型（wjsm 当前实现）

wjsm 当前是单 agent cluster 执行模型，不启动真实 OS 线程阻塞 JS 主执行。`Atomics.wait` / `Atomics.waitAsync` 因此采用 **async host import + tokio + Promise/microtask** 组合，而不是 pthread 级阻塞：

1. `Atomics.wait` 使用 `linker.func_wrap_async("env", "atomics_wait", ...)`。
2. 值不匹配或 `timeout <= 0` 时同步返回 `"not-equal"` / `"timed-out"` 字符串。
3. 值匹配且 `timeout > 0` 时：
   - `shared_buffer::enter_waiter((sab_handle, byte_offset), promise=None)` 入队；
   - async future 通过 `tokio::sync::Notify` 等待 `Atomics.notify` 唤醒，或与 `tokio::time::sleep_until(deadline)` 竞争；
   - 唤醒返回 `"ok"`，超时返回 `"timed-out"` 并 `remove_waiter`。
4. `Atomics.waitAsync` 在 `timeout > 0` 且值匹配时：
   - 分配 pending Promise；
   - `enter_waiter(..., Some(promise))`；
   - 返回 `{ async: true, value: promise }`；
   - `Atomics.notify` 通过 `notify_waiters_with_promises` FIFO 弹出 waiter，并对 async waiter Promise `settle_promise(Fulfill("ok"))`；
   - 超时路径通过 `AsyncHostCompletion::Materialize` 在 host completion channel 上结算 `"timed-out"`。
5. microtask 排空仍走现有 runtime Promise/microtask owner；worker 线程只发送可 `Send` 的 completion，不直接触碰 `Store`。

该模型保证单线程 fixture 可观测语义与 ES §25.4 一致，同时避免测试挂死；多 agent `$262.agent` harness 仍共享同一 `SharedRuntimeState.waiter_lists`。

### 6.8 Growable SAB

Growable SAB implementation may use `Vec<u8>` resize under lock. Requirements:

- construction with `{ maxByteLength }` allocates or reserves max-capable storage according to host feasibility; observable bytes beyond current length are inaccessible until grow。
- `grow(newLength)` may only increase length。
- `newLength > maxByteLength` throws RangeError。
- growing zero-fills new bytes before publishing new length。
- concurrent grow calls are serialized by entry lock; serialization must not allow shrink。
- fixed SAB lacks grow slot and `grow()` throws TypeError。

### 6.9 Error propagation

Runtime errors must use existing `runtime_error` / exception path consistently. Required messages need not match V8 byte-for-byte, but error class must match spec:

| Case | Error class |
|---|---|
| `SharedArrayBuffer()` without `new` | TypeError |
| invalid length / maxByteLength conversion | RangeError or propagated conversion error |
| byteLength/grow/slice receiver without SAB internal slot | TypeError |
| grow fixed SAB | TypeError |
| grow smaller or beyond max | RangeError |
| Atomics on non-typedarray | TypeError |
| Atomics on non-integer typedarray | TypeError |
| wait/waitAsync on non-SAB backing | TypeError |
| wait/waitAsync on non-Int32/BigInt64 typedarray | TypeError |
| atomic index out of range | RangeError |
| BigInt Atomics with Number value | TypeError |

---

## 7. Layer impact

| Layer | Required changes |
|---|---|
| `wjsm-ir` | Ensure all SAB/Atomics Builtin variants exist; add missing `AtomicsPause`, growable SAB builtins if absent |
| `wjsm-semantic` | Map `SharedArrayBuffer` constructor/prototype and `Atomics.*` methods to native callable / builtin consistently |
| `wjsm-backend-wasm` | Add missing host import registry entries/signatures; no hard-coded import indices |
| `wjsm-runtime` | Add shared-buffer owner; retire SAB/Atomics stubs; wire NativeCallable dispatch; implement waiter store |
| `tests/fixtures` | Replace broken snapshots; add happy/error fixtures for all public behavior categories |
| `test262` | Enable relevant SAB/Atomics subsets once harness can provide agent cluster APIs |

---

## 8. Compatibility boundary

Must not break:

- Existing ArrayBuffer constructor/prototype/slice behavior.
- Existing non-shared TypedArray constructors, index access, methods, BigInt typed arrays.
- Existing DataView over ArrayBuffer behavior.
- Existing Promise/microtask ordering.
- Host import registry single-owner invariant.
- Runtime name-based linking.

Must retire:

- `NativeCallable::SharedArrayBufferConstructor => Some(value::encode_undefined())`。
- `AtomicsGlobal` returning an empty object。
- SAB constructor implementation that exists only as a host import stub and is unreachable from normal JS。
- Comments claiming `shared_state` is `None` in normal non-agent execution。

---

## 9. Verification matrix

### 9.1 Happy fixtures

Add or update fixtures covering:

1. fixed SAB construction: `byteLength`, `toStringTag`, `slice`。
2. growable SAB: `growable`, `maxByteLength`, successful `grow`, zero-filled grown bytes。
3. TypedArray over SAB: `new Int32Array(sab)`, index read/write, length/byteLength/byteOffset。
4. BigInt64Array over SAB: read/write BigInt values。
5. DataView over SAB: `getInt32`/`setInt32`。
6. Atomics load/store。
7. Atomics RMW: add/sub/and/or/xor/exchange。
8. Atomics compareExchange success/failure。
9. Atomics isLockFree and pause。
10. Atomics wait immediate `not-equal` and timeout `timed-out`。
11. Atomics waitAsync immediate sync result and async Promise result。
12. Atomics notify wakes waiters and returns count。
13. Agent harness shared state: two agents observing the same SAB backing。

### 9.2 Error fixtures

Add fixtures covering:

1. `SharedArrayBuffer(1)` throws TypeError。
2. `SharedArrayBuffer.prototype.byteLength` on non-SAB throws TypeError。
3. `sab.grow()` on fixed SAB throws TypeError。
4. grow smaller / beyond max throws RangeError。
5. `Atomics.load(new Float32Array(...), 0)` throws TypeError。
6. `Atomics.wait(new Int32Array(new ArrayBuffer(4)), 0, 0)` throws TypeError。
7. `Atomics.wait(new Uint32Array(new SharedArrayBuffer(4)), 0, 0)` throws TypeError。
8. atomic index out of bounds throws RangeError。
9. BigInt Atomics with Number value throws TypeError。
10. SAB slice species returning ArrayBuffer throws TypeError。

### 9.3 Commands

Implementation plan must verify at least:

```bash
cargo nextest run -E 'test(happy__sharedarraybuffer)'
cargo nextest run -E 'test(happy__atomics)'
cargo nextest run -p wjsm-runtime
cargo nextest run -p wjsm-backend-wasm
```

If new fixture names differ, run the exact generated fixture tests for every new/updated SAB/Atomics fixture.

---

## 10. ADR signal

This design touches durable architecture surfaces:

- canonical owner for shared backing state。
- public JS built-in contract。
- host import / NativeCallable ownership boundary。
- agent cluster shared-state contract。
- retirement of old stubs and comments。

After implementation passes verification, create or update an ADR/baseline sync record covering:

1. SAB/shared-block owner.
2. Atomics waiter store ownership.
3. decision not to model full §29 event graph internally.
4. reason wjsm exposes `SharedArrayBuffer` in normal runtime.

---

## 11. Work artifacts appendix

### TaskIntentDraft

- **Outcome:** `SharedArrayBuffer` / `Atomics` 按当前 ECMAScript Structured Data §25.2/§25.4 全量补齐。
- **Success evidence:** `new SharedArrayBuffer` 创建真实共享数据块；TypedArray/DataView 可视图化 SAB；Atomics 所有方法按规范做类型/边界/BigInt/等待队列/Promise 语义；原 broken fixtures 改为规范输出；新增 E2E/错误 fixtures 覆盖 fixed/growable SAB、RMW、wait/notify/waitAsync。
- **Stop condition:** 设计规格批准后转入 implementation plan；本规格阶段不写实现代码。
- **Non-goals:** 不实现浏览器跨域隔离 gating；不实现真实浏览器 Worker API；`$262.agent` 仅作为 test262 agent harness/agent cluster 载体。
- **Risks:** 当前 `RuntimeState` 把 SAB state 和 agent state 混在 `SharedRuntimeState`；`typedarray_construct` 当前只识别 ArrayBuffer；`NativeCallable::SharedArrayBufferConstructor` 是 `undefined` stub；`AtomicsGlobal` 返回空对象；`atomics.rs` 有 host import path 但 JS normal path 没接上。

### BaselineReadSetHint

- **Authority:** ECMAScript §25.2 SharedArrayBuffer Objects；§25.4 Atomics Object；§25.1.3 buffer abstract ops；§29 Memory Model（只实现宿主可观测一致性，不建完整 spec event graph）。
- **Existing project refs:** TypedArray completion plan、host import registry spec、现有 `typedarray_new_methods.rs` / `collections_buffers.rs` / `runtime_builtins.rs` / `atomics.rs`。
- **Gaps:** 没有当前 SAB/Atomics 设计规格；现有 fixtures 是 broken-behavior 记录。

### ImpactStatementDraft

- **Affected layers:** IR Builtin / semantic builtin mapping / backend import signatures / runtime object construction / buffer side tables / typed array & DataView access / Atomics waiter store / fixtures。
- **Canonical runtime owner:** buffer backing/state 在 focused SAB helper module；Atomics wait/RMW 在 `host_imports/atomics.rs` 薄入口后委托 owner；TypedArray view construction remains `typedarray_construct` owner but delegates buffer resolution。
- **Compatibility boundaries:** 不破坏 ArrayBuffer/DataView/TypedArray 非共享路径；不改变现有 host import indices except registry-compatible additions；retire stub paths instead of keeping duplicate SAB owners。

---

## 12. Spec self-review result

- Placeholder scan: no placeholders retained.
- Internal consistency: selected owner model, non-goals, verification matrix, and retirement list align.
- Scope check: high-complexity but single implementation plan; no unrelated Worker/browser platform features included.
- Ambiguity check: growable SAB, waitAsync, BigInt waitable, and non-agent shared_state behavior explicitly included.
- Boundary check: invariants, compatibility boundaries, owners, non-goals, and ADR signal recorded.
