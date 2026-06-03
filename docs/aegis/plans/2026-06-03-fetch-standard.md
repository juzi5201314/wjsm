# Fetch Standard 实现计划

**日期**: 2026-06-03
**Spec**: [docs/aegis/specs/2026-06-03-fetch-standard-design.md](../specs/2026-06-03-fetch-standard-design.md)

## Goal

为 wjsm 运行时实现完整 Fetch Standard 支持：HTTP/HTTPS 异步请求、流式 body（ReadableStream pull 模式）、AbortController/AbortSignal、重定向 follow/error/manual、完整 Request/Response/Headers API。

## Architecture

```
JS: fetch(url, init)
  → WASM import "env"."fetch" (func_wrap_async)
  → host function 内 await reqwest 请求
  → wasmtime epoch yield 自动暂停 WASM
  → 响应头到达 → 构造 Response + settle Promise
  → Response.body → ReadableStream (侧表) → reader.read() await chunk
```

**关键决策**（已批准）：
- `func_wrap_async` + reqwest await（非 AsyncHostCompletion channel）
- 流式 body Pull 模式（`reader.read()` 驱动网络 I/O）
- ReadableStream/Reader 方法通过 NativeCallable 分发（与 Response.text() 一致）
- fetch.rs 拆分为 fetch.rs + fetch_core.rs + fetch_http.rs

## Tech Stack

- `reqwest = { version = "0.12", features = ["rustls-tls", "stream"] }` — async HTTP 客户端
- `wasmtime::func_wrap_async` — 已有的 async host function 基础设施
- `tokio` — 已有的 async runtime

## Baseline/Authority Refs

- [Fetch Standard §5](https://fetch.spec.whatwg.org/#fetch-api) — 权威规范
- `docs/async-scheduler.md` — 已有 async 基础设施设计
- `crates/wjsm-runtime/src/scheduler.rs` — AsyncHostCompletion + Materialize 模式
- `crates/wjsm-runtime/src/host_imports/reentrant_async.rs` — func_wrap_async 使用范例

## Compatibility Boundary

| 保证 | 说明 |
|---|---|
| data: URL 行为不变 | 同步处理，Promise 同步 settle |
| 现有 fixture .expected 零变更 | 无回归 |
| fetch() 签名不变 | `fetch(input, init)` — WASM ABI 从 `(i64)→i64` 改为 `(i64,i64)→i64` |
| Response.text()/json()/arrayBuffer() 仍返回 Promise | 行为不变，内部改为 Materialize 闭包异步 settle |
| Headers API 不变 | 全部现有方法保持 |
| Request 构造器不变 | 新增 signal 解析 + FetchRequestEntry.signal_handle 字段 |
| fetch host import type_idx 变更 | 从 3（单参数）改为新的双参数 type_idx |

## Plan Pressure Test

```
Plan Pressure Test:
- Owner / contract / retirement: fetch.rs → fetch_http.rs (新 owner: HTTP 执行), fetch_core.rs (新 owner: 构造/方法)
- Verification scope: HTTP fetch E2E + 流式 body + AbortController + 回归
- Task executability: 所有任务有精确文件路径和代码
- Pressure result: proceed
```

## Plan-Time Complexity Check

```
Plan-Time Complexity Check:
- Target files: fetch.rs (1394 行), lib.rs (RuntimeState + NativeCallable), host_import_registry.rs
- Existing size signals: fetch.rs 1394 行即将膨胀
- Owner fit: 拆分后各 ~700 行，owner 清晰
- Add-in-place risk: 不拆分会到 ~2500 行
- Better file boundary: fetch_core.rs (构造/方法) + fetch_http.rs (HTTP/流式/Abort)
- Recommendation: extract helper — 拆分为 3 个文件
```

---

## Completed Tasks

Tasks 1–11 have been implemented and verified (785 tests passed). Remaining work is captured below.

---

## Remaining Tasks

### Remaining 1: ReadableStream + Reader full streaming body

**Files**: `crates/wjsm-runtime/src/host_imports/fetch_core.rs`
**Why**: HTTP Response body 当前为 null（Task 6 实现了 text/json/arrayBuffer 但 Response.body 仍是 null）
**Impact**: 新增 NativeCallable 分发 + Reader 对象
**Verification**: 新增 `fetch_stream_body.js` fixture

- Step 1: 在 `create_response_object_with_http_handle` 中把 `body` 属性从 `null` 改为 `create_readable_stream_object`
- Step 2: 实现 `create_readable_stream_object` 和 `create_reader_object`
- Step 3: 实现 `call_stream_method_from_caller` (GetReader, Cancel)
- Step 4: 实现 `call_reader_method_from_caller` (Read, ReleaseLock)
- Step 5: Reader.read() 使用 Materialize 模式：`tokio::spawn` 中 `await resp.chunk()`，完成后发送 `Materialize` 闭包构造 `{done, value}`
- Step 6: 新增 fixture `fixtures/happy/fetch_stream_body.js`

### Remaining 2: AbortController / AbortSignal Semantic + Backend 对接

**Files**: `crates/wjsm-ir/src/builtin.rs`, `crates/wjsm-semantic/src/builtins.rs`, `crates/wjsm-backend-wasm/src/host_import_registry.rs`
**Why**: `new AbortController()` 需要 semantic 识别和 backend 注册
**Impact**: 跨层变更
**Verification**: 新增 `abort_controller.js` fixture

- Step 1: 在 `wjsm-ir/src/builtin.rs` 中新增 `AbortControllerConstructor`
- Step 2: 在 `wjsm-semantic/src/builtins.rs` 中添加 `"AbortController" => Some(Builtin::AbortControllerConstructor)` 解析
- Step 3: 在 `wjsm-backend-wasm/src/host_import_registry.rs` 中添加 `abort_controller_constructor` spec
- Step 4: 新增 fixture `fixtures/happy/abort_controller.js`

### Remaining 3: 重定向模式 + HTTP 方法完整支持

**Files**: `crates/wjsm-runtime/src/host_imports/fetch_http.rs`
**Why**: Fetch Standard 要求 follow/error/manual 重定向和 POST/PUT 等 body 传输
**Impact**: 扩展 fetch 功能
**Verification**: 新增 fixture

- Step 1: `perform_http_fetch` 中处理 `RedirectMode::Manual`（返回 OpaqueRedirect Response）
- Step 2: `parse_fetch_input` 中完善 body 提取（ArrayBuffer → bytes, string → bytes）
- Step 3: 新增 fixture `fixtures/happy/fetch_http_post.js`

## Risks

| 风险 | 缓解 |
|---|---|
| reqwest 依赖体积 | `rustls-tls` 比 `native-tls` 更小，无系统依赖 |
| 流式 body 的 reqwest Response 生命周期 | take/put_back 模式，await 期间不持锁 |
| fixture 依赖外部 HTTP 服务 | `KNOWN-NETWORK` 标记 + `WJSM_SKIP_NETWORK` 环境变量跳过 |
| epoch yielding 与长时间请求 | wasmtime 自动 yield |
| NativeCallable 同步分发 vs async body 消费 | `tokio::spawn` + `Materialize` 闭包（`Store` + `WasmEnv` 路径创建 JS 对象） |

## Retirement

- 旧 `perform_fetch_and_build_response`（仅 data: URL + HTTP 报错）→ 由 `perform_data_url_fetch` + `perform_http_fetch` 替代
- 旧 `Func::wrap` fetch host function → 由 `func_wrap_async` 替代
- 旧 `Response.body = null` → 由 ReadableStream 对象替代（仅 HTTP Response）
- 旧 `FetchResponseEntry`（无 `http_response_handle`）→ 由带 `http_response_handle: Option<u32>` 的新版本替代
- 旧 `FetchRequestEntry`（无 `signal_handle`）→ 由带 `signal_handle: Option<u32>` 的新版本替代
- 旧 `fetch` WASM 签名 `(i64)→i64` → 由 `(i64,i64)→i64` 替代
