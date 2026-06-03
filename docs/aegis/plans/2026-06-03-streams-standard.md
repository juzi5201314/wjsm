# WHATWG Streams Standard 实现计划

**日期**: 2026-06-03
**Spec**: [docs/aegis/specs/2026-06-03-streams-standard-design.md](../specs/2026-06-03-streams-standard-design.md)

---

## Goal

为 wjsm runtime 实现完整 WHATWG Streams Standard：ReadableStream + WritableStream + TransformStream + pipeTo + pipeThrough + BYOB reader + QueuingStrategy + Symbol.asyncIterator + fetch 集成修复。

---

## Architecture

```
JS: new ReadableStream({ start(controller) { controller.enqueue(chunk); } })
  → WASM call native_callable (constructor dispatch)
  → host function 中执行 call_wasm_callback(start, controller)
  → controller.enqueue(chunk) 通过 NativeCallable::ReadableStreamDefaultControllerMethod 分发
  → chunk 推入 StreamControllerEntry.chunk_queue
  → reader.read() 通过 NativeCallable::ReadableStreamDefaultReaderMethod 分发
  → 如果 queue 有 chunk，立即 resolve；否则 pending，等待 enqueue
```

**关键决策**（已批准）：
- 侧表 + NativeCallable + `func_wrap_async` 模式（与现有 fetch 一致）
- `StreamControllerEntry` 统一侧表（用 `ControllerKind` 区分 Default/ByteStream/Writable/Transform）
- `ReaderEntry` 新增 `pending_read_promise` 用于 enqueue 时同步 resolve
- `call_wasm_callback` 同步调用 JS 回调（start/pull/write/flush）
- 文件拆分：`streams_readable.rs` + `streams_writable.rs` + `streams_transform.rs` + `streams_queuing.rs` + `streams_fetch.rs`

---

## Tech Stack

- 已有 `func_wrap_async` + `tokio::spawn` + `Materialize` 闭包
- 已有 `NativeCallable` 侧表 + `call_native_callable_with_args_from_caller`
- 已有 `call_wasm_callback` / `call_wasm_callback_async` — 调用 JS 函数
- 已有 `read_object_property_by_name` — 读取 JS 对象属性
- 新增侧表：`writable_stream_table` + `writer_table` + `transform_stream_table` + `stream_controller_table`

---

## Baseline/Authority Refs

- `docs/aegis/specs/2026-06-03-streams-standard-design.md` — 本设计规格
- `docs/aegis/specs/2026-06-03-fetch-standard-design.md` — Fetch 设计规格（已有实现）
- `crates/wjsm-runtime/src/host_imports/fetch_core.rs` — 现有 stream/reader/abort 实现
- `crates/wjsm-runtime/src/host_imports/reentrant_async.rs` — async host function 模式
- `crates/wjsm-runtime/src/lib.rs` — RuntimeState + NativeCallable + Promise
- `WHATWG Streams Standard` — https://streams.spec.whatwg.org/

---

## Compatibility Boundary

| 保证 | 说明 |
|---|---|
| 现有 fetch fixture `.expected` 零变更 | 无回归（除 data: URL body 从 null 变为 ReadableStream） |
| `data:` URL 行为不变 | 同步处理，Promise 同步 settle；仅 body 从 null 变为 ReadableStream |
| 现有 Response.text()/json()/arrayBuffer() | 行为不变，路径不变 |
| 现有 NativeCallable dispatch 路径 | 不变，新增变体 |
| 现有 `AbortController` | 不变，WritableStream 复用 |
| **Map.size / Set.size 从方法变为 getter** | 行为变更：`m.size` 不再需要 `()`。现有 fixture 需要更新 `.expected`。这是 spec 合规性修复。 |
---

## Plan Pressure Test

```
Plan Pressure Test:
- Owner / contract / retirement: streams_readable.rs (owner: ReadableStream/Reader/Controller),
  streams_writable.rs (owner: WritableStream/Writer), streams_transform.rs (owner: TransformStream/pipe),
  streams_queuing.rs (owner: QueuingStrategy), streams_fetch.rs (owner: fetch 集成)
- Verification scope: 16+ 新增 fixtures + 470+ 现有 fixtures 零回归
- Task executability: 所有任务有精确文件路径和代码
- Pressure result: proceed
```

---

## Plan-Time Complexity Check

```
Plan-Time Complexity Check:
- Target files: fetch_core.rs (1645 行), lib.rs (2604 行)
- Existing size signals: lib.rs 2604 行即将膨胀（新增 5+ 侧表字段 + 20+ NativeCallable 变体）
- Owner fit: 拆分后 streams_*.rs 各 ~600-1200 行，owner 清晰
- Add-in-place risk: 不拆分 fetch_core.rs 会到 ~3000 行
- Better file boundary: 按 stream 类型拆分（readable/writable/transform/queuing/fetch）
- Recommendation: extract helper — 拆分为 5 个新文件
```

---

## 任务清单

### Phase 0: Accessor Property 基础设施（前置）

**目标**: 启用 getter/accessor property 支持，解决 `locked`/`desiredSize`/`ready`/`closed`/`signal` 的 spec 合规性

**文件**: `crates/wjsm-runtime/src/runtime_heap.rs`, `crates/wjsm-runtime/src/host_imports/collections_buffers.rs`

- [ ] **Task 0.1**: 新增 `define_host_accessor_property_from_caller` 工具函数
  - `runtime_heap.rs` 中实现，与 `define_host_data_property_from_caller` 平行
  - slot 布局：`flags = FLAG_CONFIGURABLE | FLAG_ENUMERABLE | FLAG_IS_ACCESSOR`，offset 8 = undefined，offset 16 = getter，offset 24 = setter
  - 底层 `$obj_get`/`$obj_set` WASM helper 已有 accessor 分支，无需修改
  - 新增 `define_host_accessor_property_with_env` 泛型版本（与其他 define_host_* 函数一致）

- [ ] **Task 0.2**: 修复 Map.size / Set.size 为 getter
  - `collections_buffers.rs`：将 `size` 从 `define_host_data_property_from_caller(obj, "size", size_fn)` 改为 `define_host_accessor_property_from_caller(obj, "size", size_fn, undefined)`
  - Map/Set `size` NativeCallable 仍存在，但作为 accessor getter 而非 data property 方法
  - 验证：`m.size` 无需 `()` 即返回数值，`m.size()` 应抛 TypeError（不再是方法）

- [ ] **Task 0.3**: 回归验证
  - `cargo nextest run --workspace` 零失败（Map/Set fixture 可能需要更新 `.expected`）

### Phase 1: ReadableStream 核心（构造函数 + DefaultController + DefaultReader + locked + cancel + fetch body 修复）

**目标**: ReadableStream 构造函数可工作，fetch 的 Response.body 完整

**文件**: `crates/wjsm-runtime/src/host_imports/streams_readable.rs`, `streams_fetch.rs`

- [ ] **Task 1.1**: 扩展 `RuntimeState` — 新增侧表字段
  - `lib.rs` 新增：`writable_stream_table` + `writer_table` + `transform_stream_table` + `stream_controller_table`
  - `RuntimeState::new()` 初始化空 Vec
  - 注意：`stream_chunk_queue` 不需要单独侧表，chunk 存储在 `StreamControllerEntry.chunk_queue` 中

- [ ] **Task 1.2**: 扩展 `NativeCallable` — 新增 ReadableStream 相关变体
  - `ReadableStreamConstructor`
  - `ReadableStreamMethod` (GetReader, GetLocked, Cancel, Tee, AsyncIterator)
  - `ReadableStreamDefaultReaderMethod` (Read, ReleaseLock, GetClosed)
  - `ReadableStreamDefaultControllerMethod` (Enqueue, Close, Error, GetDesiredSize)
  - `call_native_callable` dispatch 分支
  - **注意**：`GetLocked`/`GetDesiredSize`/`GetClosed` 等变体作为 accessor property 的 getter 调用（Phase 0 已启用），不再作为 data property 方法暴露

- [ ] **Task 1.3**: 扩展 `ReaderEntry` — 新增字段
  - `pending_read_promise: Option<i64>` — 存储 controller.enqueue 时的 pending read Promise
  - `closed_promise: Option<i64>` — ReadableStreamDefaultReader.closed 的 Promise
  - `lib.rs` struct 修改

- [ ] **Task 1.4**: 实现 `construct_readable_stream` — ReadableStream 构造函数
  - 解析 underlyingSource + strategy
  - 创建 StreamControllerEntry + ReadableStreamEntry
  - 调用 `start(controller)` 通过 `call_wasm_callback`
  - 创建 controller JS 对象（带 enqueue/close/error 方法 + desiredSize getter 的 NativeCallable）
  - 构造 ReadableStream JS 对象：
    - `locked` → `define_host_accessor_property_from_caller(obj, "locked", getter, undefined)` — 真正的 getter
    - `getReader` / `cancel` / `tee` → `define_host_data_property_from_caller` — 方法
    - `Symbol.asyncIterator` → data property
  - **pull 回调**：`underlyingSource.pull(controller)` 在 `reader.read()` 且 chunk_queue 为空时调用（异步路径通过 `call_wasm_callback_async`）

- [ ] **Task 1.5**: 实现 `controller_enqueue` — 推入 chunk + resolve pending read
  - 检查 `close_requested`
  - 推入 `chunk_queue`（VecDeque）
  - 检查 `reader_table` 中 `pending_read_promise` 并 resolve

- [ ] **Task 1.6**: 实现 `controller_close` — 标记 close + resolve pending read with done
  - 设置 `close_requested = true`
  - 检查 pending read 并 resolve `{done: true}`
  - 更新 stream state 为 `Closed`

- [ ] **Task 1.7**: 实现 `controller_error` — 标记 error + reject pending read
  - 设置 stream state 为 `Errored`
  - 检查 pending read 并 reject

- [ ] **Task 1.8**: 实现 `call_readable_stream_method` — 分发 ReadableStream 方法
  - `GetLocked`：从 `readable_stream_table` 读取 `locked` 字段返回 bool
  - `GetReader`：创建 reader + 更新 `locked = true` + `disturbed = true`
  - `Cancel`：调用 underlyingSource.cancel + 更新 state
  - `Tee`：创建两个新 stream + clone chunk queue
  - `AsyncIterator`：返回 `{ next, return }` 对象

- [ ] **Task 1.9**: 强化 `call_reader_method` — `read()` 支持自定义流
  - 先检查 `chunk_queue`（VecDeque pop_front），有 chunk 立即 resolve
  - 无 chunk 时检查 `close_requested` / `stream.state`
  - 否则调用 `pull(controller)` + pending，存储 `pending_read_promise`
  - `GetClosed`：返回 `closed_promise`

- [ ] **Task 1.10**: 实现 `streams_fetch.rs` — data: URL Response.body 返回 ReadableStream
  - `create_data_url_response_with_body` — 创建 controller + 推入 bytes + close + 设置 body
  - `response_clone_with_shared_body` — clone 共享底层 stream

- [ ] **Task 1.11**: 修复 `bodyUsed` — `getReader()` 时更新
  - 在 `call_readable_stream_method` 的 `GetReader` 分支中更新 `bodyUsed`

- [ ] **Task 1.12**: Semantic + Backend — 新增 `ReadableStreamConstructor` builtin
  - `wjsm-ir/src/builtin.rs` 新增变体
  - `wjsm-semantic/src/builtins.rs` 新增解析
  - `wjsm-backend-wasm/src/host_import_registry.rs` 新增 host import spec

- [ ] **Task 1.13**: Fixture — 新增 6 个 ReadableStream 核心 fixture
  - `streams_readable_constructor.js` — 构造函数
  - `streams_readable_enqueue_close.js` — enqueue/close
  - `streams_readable_locked.js` — locked getter（`stream.locked` 无括号）
  - `streams_fetch_body_data_url.js` — data: URL body
  - `streams_fetch_clone_shared.js` — clone body 共享
  - `streams_fetch_body_used.js` — bodyUsed

- [ ] **Task 1.14**: 回归验证
  - `cargo nextest run --workspace` 零失败

### Phase 2: ReadableStream 高级（tee + Symbol.asyncIterator + QueuingStrategy）

**文件**: `streams_readable.rs`, `streams_queuing.rs`

- [ ] **Task 2.1**: 实现 `readable_stream_tee`
  - 创建两个新 controller + 两个新 stream
  - Clone chunk queue
  - 返回 `[stream1, stream2]`

- [ ] **Task 2.2**: 实现 `Symbol.asyncIterator`
  - 返回 `{ next: () => reader.read() }` 对象
  - `return()` 调用 `reader.releaseLock()`

- [ ] **Task 2.3**: 实现 `CountQueuingStrategy` + `ByteLengthQueuingStrategy`
  - 构造函数 + `size` 方法
  - `CountQueuingStrategy` size = 1
  - `ByteLengthQueuingStrategy` size = chunk.byteLength

- [ ] **Task 2.4**: Semantic + Backend — 新增 QueuingStrategy builtin
  - `CountQueuingStrategyConstructor` / `ByteLengthQueuingStrategyConstructor`

- [ ] **Task 2.5**: Fixture — 新增 4 个 fixture
  - `streams_readable_tee.js`
  - `streams_readable_async_iter.js`
  - `streams_queuing_strategy.js`

- [ ] **Task 2.6**: 回归验证

### Phase 3: BYOB Reader + ByteStreamController

**文件**: `streams_readable.rs`

- [ ] **Task 3.1**: 扩展 `ReadableStreamEntry` — `is_byte_stream` 字段

- [ ] **Task 3.2**: 实现 `ReadableStreamByteStreamController`
  - `byobRequest` 属性
  - `enqueue(chunk)` 支持 ArrayBufferView
  - `close()` / `error(e)`

- [ ] **Task 3.3**: 实现 `ReadableStreamBYOBReader`
  - `read(view)` — 写入 ArrayBufferView
  - `releaseLock()` / `closed`

- [ ] **Task 3.4**: NativeCallable 扩展 — BYOB 相关变体

- [ ] **Task 3.5**: Fixture — `streams_readable_byob.js`

- [ ] **Task 3.6**: 回归验证

### Phase 4: WritableStream

**文件**: `streams_writable.rs`

- [ ] **Task 4.1**: 实现 `construct_writable_stream`
  - 解析 underlyingSink + strategy
  - 创建 StreamControllerEntry (Writable) + WritableStreamEntry（含 `abort_signal` 字段）
  - 如果构造时传入 signal options，引用该 AbortSignal；否则创建内部 AbortController
  - 调用 `start(controller)`
  - 构造 JS 对象：
    - `locked` → `define_host_accessor_property_from_caller(obj, "locked", getter, undefined)` — 真正的 getter
    - `getWriter` / `abort` → `define_host_data_property_from_caller` — 方法

- [ ] **Task 4.2**: 实现 `WritableStreamDefaultWriter`
  - `write(chunk)` — 返回 Promise
  - `close()` — 返回 Promise
  - `abort(reason)` — 返回 Promise
  - `releaseLock()`
  - `closed` → `define_host_accessor_property_from_caller(writer, "closed", getter, undefined)` — getter 返回 closed_promise
  - `desiredSize` → `define_host_accessor_property_from_caller(writer, "desiredSize", getter, undefined)` — getter
  - `ready` → `define_host_accessor_property_from_caller(writer, "ready", getter, undefined)` — getter 返回 ready_promise

- [ ] **Task 4.3**: 实现 `WritableStreamDefaultController`
  - `error(e)` — data property 方法
  - `signal` → `define_host_accessor_property_from_caller(ctrl, "signal", getter, undefined)` — getter 返回 WritableStreamEntry.abort_signal

- [ ] **Task 4.4**: NativeCallable 扩展 — WritableStream 变体
  - `WritableStreamMethod` (GetWriter, GetLocked, Abort)
  - `WritableStreamDefaultWriterMethod` (Write, Close, Abort, ReleaseLock, GetClosed, GetDesiredSize, GetReady)
  - `WritableStreamDefaultControllerMethod` (Error, GetSignal)

- [ ] **Task 4.5**: Semantic + Backend — `WritableStreamConstructor` builtin

- [ ] **Task 4.6**: Fixture — 新增 4 个 fixture
  - `streams_writable_constructor.js`
  - `streams_writable_writer.js` — 测试 `writer.closed`/`writer.ready`/`writer.desiredSize` 作为 getter
  - `streams_writable_locked.js` — 测试 `ws.locked` 作为 getter
  - `streams_writable_controller_signal.js` — 测试 `controller.signal` 作为 getter

- [ ] **Task 4.7**: 回归验证

### Phase 5: TransformStream

**文件**: `streams_transform.rs`

- [ ] **Task 5.1**: 实现 `construct_transform_stream`
  - 解析 transformer + writableStrategy + readableStrategy
  - 创建 ReadableStream + WritableStream + TransformStreamEntry
  - 调用 `start(controller)`
  - 构造 JS 对象（readable / writable 属性）

- [ ] **Task 5.2**: 实现 `TransformStreamDefaultController`
  - `enqueue(chunk)` — 推入 readable queue
  - `error(e)` — error 两端
  - `terminate()` — close readable

- [ ] **Task 5.3**: 实现 `transform` 回调
  - `write(chunk)` 时调用 `transformer.transform(chunk, controller)`
  - `flush()` 时调用 `transformer.flush(controller)`

- [ ] **Task 5.4**: NativeCallable 扩展 — TransformStream 变体

- [ ] **Task 5.5**: Semantic + Backend — `TransformStreamConstructor` builtin

- [ ] **Task 5.6**: Fixture — `streams_transform.js`

- [ ] **Task 5.7**: 回归验证

### Phase 6: pipeTo + pipeThrough

**文件**: `streams_transform.rs`

- [ ] **Task 6.1**: 实现 `readable_stream_pipe_to`
  - 获取 reader + writer
  - 循环：read() -> write() -> close()
  - 处理 abort / signal
  - 返回 Promise

- [ ] **Task 6.2**: 实现 `readable_stream_pipe_through`
  - 获取 readable + writable
  - 调用 `pipeTo(writable)`
  - 返回 `{ readable, writable }`

- [ ] **Task 6.3**: NativeCallable 扩展 — `ReadableStreamPipeTo`

- [ ] **Task 6.4**: Fixture — 新增 2 个 fixture
  - `streams_pipe_to.js`
  - `streams_pipe_through.js`

- [ ] **Task 6.5**: 回归验证

### Phase 7: 最终回归 + 测试

- [ ] **Task 7.1**: 全 workspace `cargo nextest run --workspace`
- [ ] **Task 7.2**: `cargo nextest run -E 'test(streams__)'` 验证所有新增 fixture
- [ ] **Task 7.3**: `cargo nextest run -E 'test(fetch__)'` 验证 fetch 回归
- [ ] **Task 7.4**: `cargo nextest run -E 'test(happy__)'` 验证 happy 回归

---

## Risks

### R1: `define_host_accessor_property_from_caller` 与 `$obj_get`/`$obj_set` 兼容性

**根因**：新增的 `define_host_accessor_property_from_caller` 写入 slot 格式必须与 `$obj_get`/`$obj_set` WASM helper 的读取逻辑严格匹配。

**代码证据**（已验证）：
- `$obj_get`（compiler_helpers.rs:398-428）：检测 `flags & FLAG_IS_ACCESSOR != 0` → 加载 offset 16 作为 getter → `emit_resolve_callable_for_helper` + `call_indirect type 12` 调用
- `$obj_set`（compiler_helpers.rs:765-812）：检测 `flags & FLAG_IS_ACCESSOR != 0` → 加载 offset 24 作为 setter → 同样 `call_indirect type 12` 调用
- slot 布局：offset 0-3=name_id, 4-7=flags, 8-15=value, 16-23=getter, 24-31=setter

**解决**：
```rust
pub(crate) fn define_host_accessor_property_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
    name: &str,
    getter: i64,
    setter: i64,
) -> Option<()> {
    // 复用 define_host_data_property_with_env 的 slot 分配逻辑
    // 但写入不同的 flags 和布局：
    let flags = constants::FLAG_CONFIGURABLE | constants::FLAG_ENUMERABLE | constants::FLAG_IS_ACCESSOR;
    // offset 8  = undefined（accessor 不使用 value 字段）
    // offset 16 = getter
    // offset 24 = setter
}
```
与 `define_host_data_property_with_env`（runtime_host_helpers.rs:490-538）唯一的区别：`flags` 包含 `FLAG_IS_ACCESSOR`，offset 8 写 undefined，offset 16 写 getter，offset 24 写 setter。

**验证**：Phase 0 Task 0.2 — Map.size 改为 getter 后，`m.size` 返回数值、`m.size()` 抛 TypeError。如果不兼容，Map fixture 立即失败，零风险漏过。

---

### R2: Map.size/Set.size 从方法变为 getter 导致 fixture 变更

**根因**：当前 `m.size` 是 data property 方法（`m.size()`），改为 getter 后 `m.size` 无括号即返回值，`m.size()` 抛 TypeError。

**影响范围**：所有使用 `m.size()` 或 `s.size()` 的 fixture 和用户代码。

**解决**：
1. `WJSM_UPDATE_FIXTURES=1 cargo nextest run` 自动更新所有 `.expected` 文件
2. 手动检查 `git diff fixtures/` — 确认变更仅限 `size()` → `size`，无意外副作用
3. 如果 fixture 中的 JS 源码也使用了 `m.size()`，需要同步修改 `.js` 文件（但大多数 fixture 可能只用 `m.size`）

**验证**：`cargo nextest run -E 'test(happy__map)'` + `cargo nextest run -E 'test(happy__set)'` 全部通过。

---

### R3: NativeCallable getter 的 `this` 绑定

**根因**：`$obj_get` 调用 getter 时传 `this_val = local 0`（即被读属性的对象）。NativeCallable getter 需要从 `this_val` 中提取 stream/writer/controller handle。

**代码证据**（已验证）：
- `$obj_get` 第418行：`LocalGet(0)` — this_val 就是 boxed object（i64）
- Type 12 签名：`(env_obj: i64, this_val: i64, args_base: i32, args_count: i32) -> i64`
- `call_native_callable_with_args_from_caller` 接收 `this_val` 作为第二个参数

**解决**：Getter 实现中从 `this_val` 读取内部 handle 属性：
```rust
// 例如 ReadableStream.locked getter
NativeCallable::ReadableStreamMethod { handle, kind: GetLocked }
// 但 handle 在构造时已硬编码到 NativeCallable 中，无需从 this_val 读取
// this_val 仅用于需要动态解析的场景
```

实际上更简单的方案：**handle 在构造时已绑定到 NativeCallable 变体中**（`ReadableStreamMethod { handle: stream_handle, kind: GetLocked }`），getter 实现直接使用 `handle` 字段，无需从 `this_val` 解析。`this_val` 传入但不使用。这与现有 NativeCallable 的模式一致（如 `StreamMethod { handle }`）。

**验证**：Phase 1 Task 1.8 — `stream.locked` 返回正确值。如果 `this` 绑定有问题，`stream.locked` 会返回错误值或抛异常，fixture 立即捕获。

---

### R4: controller.enqueue() 同步 resolve reader.read()

**根因**：Streams Standard 要求 `controller.enqueue(chunk)` 立即 resolve 等待中的 `reader.read()` promise。但 `enqueue` 在 JS 回调（`start()`/`pull()`）中同步调用，而 `read()` 可能已返回一个 pending promise。

**解决**：
1. `ReaderEntry.pending_read_promise: Option<i64>` — 存储 `read()` 创建的未 resolve 的 promise
2. `controller_enqueue` 逻辑：
   a. 推入 `chunk_queue`
   b. 遍历 `reader_table` 找到 `stream_handle` 匹配且 `pending_read_promise.is_some()` 的 reader
   c. 从 `chunk_queue` 前端 pop chunk
   d. `settle_promise(pending_read_promise, Fulfill({value: chunk, done: false}))`
   e. 清除 `pending_read_promise = None`
3. `controller_close` 逻辑：类似，但 resolve `{done: true, value: undefined}`
4. `controller_error` 逻辑：类似，但 reject promise

**边界情况**：
- `start()` 中先 `enqueue` 再 `close`：`enqueue` resolve pending read，`close` 标记 `close_requested = true`，下次 `read()` 返回 `{done: true}`
- `start()` 中 `close` 无 `enqueue`：`close` resolve pending read 为 `{done: true}`，无需 `enqueue`
- 多次 `enqueue` 无 `read`：chunk 累积在 queue，下次 `read()` 立即返回队首

**验证**：Fixture `streams_readable_enqueue_close.js` — 测试 `start` 中 enqueue + close 组合。

---

### R5: pipeTo 循环与 wasmtime epoch yielding

**根因**：`pipeTo` 是无限循环（`loop { read().await; write().await; }`），wasmtime 默认有 epoch-based interruption，可能中断长时间运行的 WASM。

**解决**：使用 `func_wrap_async` + `tokio::spawn`，与现有 HTTP reader.read() 路径一致。每次 `read().await` 和 `write().await` 都会 yield 回 tokio runtime，wasmtime 的 epoch 检查在 yield 点之间执行，不会中断。循环在 `read()` 返回 `{done: true}` 时 break。

**验证**：Fixture `streams_pipe_to.js` — pipe 10+ chunks，确认无超时或中断。已有 HTTP reader.read() 的 async 路径证明此模式可行。

---

### R6: tee() 两个 reader 同时消费

**根因**：`tee()` 返回两个独立的 ReadableStream，各自有独立的 reader。两个 reader 可能同时调用 `read()`，但底层共享同一个 source stream 的 chunk。

**解决**：
1. `tee()` 创建两个新的 `StreamControllerEntry`，各持有独立的 `chunk_queue`
2. 原始 stream 的 `controller.enqueue` 被替换为一个**分发 controller**：每次 `enqueue` 时同时推入两个子 controller 的 queue
3. 两个子 stream 的 `pull` 回调都委托给原始 stream 的 `pull`
4. 任一子 stream `cancel` 时，另一个继续消费；两者都 `cancel` 时，调用原始 stream 的 `cancel`

**实现细节**：
- 在 `ReadableStreamEntry` 上标记 `is_tee_branch: bool` + `tee_source_handle: Option<u32>`
- 分发 controller 是一个特殊的 `StreamControllerEntry`（kind 仍为 `ReadableDefault`），但 `chunk_queue` 不直接使用——`enqueue` 被拦截后推入两个子 controller
- 或者更简单：`tee()` 后原始 stream 被标记为 `disturbed = true` + `locked = true`，两个子 stream 各自有独立的 controller 和 chunk queue（clone 原始 queue 的当前内容），后续 chunk 通过拦截原始 controller 的 `enqueue` 分发到两个子 controller

**验证**：Fixture `streams_readable_tee.js` — 两个 reader 独立读取，互不干扰。

---

### R7: BYOB reader 需要 ArrayBuffer 直接写入

**根因**：`ReadableStreamBYOBReader.read(view)` 要求将 chunk 数据直接写入用户提供的 `ArrayBufferView`，而非创建新的 `Uint8Array`。需要直接操作 WASM 线性内存中的 ArrayBuffer 数据区域。

**解决**：
1. `read(view)` 接收一个 TypedArray 对象（NaN-boxed i64）
2. 从 TypedArray 对象读取 `__typedarray_handle__` → `TypedArrayEntry` → buffer_ptr + byte_offset + byte_length
3. 将 chunk 数据（来自 HTTP response 或 controller queue）memcpy 到 buffer_ptr + byte_offset
4. 返回 `{done: false, value: view}`（同一个 view 对象，数据已被写入）

**边界情况**：
- view 太小装不下 chunk → 取 min(chunk.len(), view.byteLength) 写入，剩余 chunk 放回 queue
- view 已 detached → 抛 TypeError
- stream 已关闭 → 返回 `{done: true, value: view}`

**验证**：Fixture `streams_readable_byob.js` — 用 `Uint8Array` 作为 view 读取。

---

### R8: 侧表数量增加导致 RuntimeState 过大

**根因**：新增 4 个侧表（`writable_stream_table` + `writer_table` + `transform_stream_table` + `stream_controller_table`），RuntimeState 字段从 ~20 个增长到 ~24 个。

**解决**：`StreamControllerEntry` 合并 4 种 controller（`ControllerKind::ReadableDefault/ReadableByteStream/Writable/Transform`），而非 4 个独立侧表。合并后：
- 1 个 `stream_controller_table` 替代 4 个独立侧表
- `ControllerKind` 区分类型，字段中不适用的部分为 `None`/`false`/`0`
- 内存开销：每个空 `StreamControllerEntry` 约 80 字节（VecDeque + Option + bool），vs 4 个独立 struct 各约 40 字节但需要 4 个 `Arc<Mutex<Vec<>>>` wrapper

**验证**：`cargo nextest run --workspace` — 如果 Mutex 死锁或侧表冲突，测试立即失败。

---

### R9: 流式 body 与 `Response.text()` 同时消费

**根因**：`Response.body.getReader()` 和 `Response.text()` 都消费同一个 body stream。规范要求 `bodyUsed = true` 后不能再消费。

**解决**：
1. `getReader()` 时设置 `bodyUsed = true`（修改 Response 对象的 `bodyUsed` 属性）
2. `text()`/`json()`/`arrayBuffer()` 开始时检查 `bodyUsed`，如果为 `true` 抛 TypeError
3. `getReader()` 也检查 `bodyUsed`，如果已为 `true` 抛 TypeError

**实现**：在 `call_readable_stream_method` 的 `GetReader` 分支中：
```rust
// 检查 bodyUsed
let body_used = read_object_property_by_name(caller, resp_obj, "bodyUsed");
if body_used.map_or(false, |v| value::is_truthy(v)) {
    return type_error("Response body already used");
}
// 设置 bodyUsed = true
set_host_data_property_from_caller(caller, resp_obj, "bodyUsed", value::encode_bool(true));
```

**验证**：Fixture `streams_fetch_body_used.js` — `text()` 后 `getReader()` 抛 TypeError，反之亦然。

---

### R10: 自定义流 controller 回调在 start() 中同步 close

**根因**：`underlyingSource.start(controller)` 可能同步调用 `controller.close()`，此时 stream 立即变为 Closed 状态。后续的 `controller.enqueue()` 应抛 TypeError。

**解决**：
1. `controller_close` 设置 `close_requested = true` + stream state = `Closed`
2. `controller_enqueue` 检查 `close_requested`，如果为 `true` 则返回 TypeError（"Cannot enqueue after close"）
3. `start()` 返回后标记 `started = true`
4. `reader.read()` 在 stream Closed 且 queue 为空时返回 `{done: true}`

**边界情况**：
- `start()` 中先 `enqueue` 再 `close`：chunk 在 queue 中，`read()` 返回 chunk，下次 `read()` 返回 `{done: true}`
- `start()` 中先 `close` 再 `enqueue`：`enqueue` 抛 TypeError（符合规范）
- `start()` 中仅 `close`：`read()` 直接返回 `{done: true}`

**验证**：Fixture `streams_readable_enqueue_close.js` — 测试 `start` 中 close 后 enqueue 的错误行为。

---

## Retirement

- 旧 `fetch_core.rs` 中的 `create_readable_stream_object`（仅 HTTP body）→ 由 `streams_readable.rs` 的 `construct_readable_stream` 替代
- 旧 `fetch_core.rs` 中的 `create_reader_object` → 由 `streams_readable.rs` 的 `create_default_reader_object` 替代
- 旧 `fetch_core.rs` 中的 `call_stream_method_from_caller`（GetReader/Cancel）→ 由 `streams_readable.rs` 的 `call_readable_stream_method` 替代
- 旧 `fetch_core.rs` 中的 `call_reader_method_from_caller`（Read/ReleaseLock）→ 由 `streams_readable.rs` 的 `call_default_reader_method` 替代
- 旧 `Response.body = null`（data: URL）→ 由 ReadableStream 替代
- 旧 `Response.clone()` 静态 body 复制 → 由共享底层流替代
- 旧 `collections_buffers.rs` 中 Map.size / Set.size 作为 `define_host_data_property` 方法 → 改为 `define_host_accessor_property` getter（spec 合规性修复）
- 旧 spec 中 `locked`/`desiredSize` 等"架构限制"标记 → 已移除，底层架构支持 accessor property

---

*Plan 撰写完毕，可进入实现。*
