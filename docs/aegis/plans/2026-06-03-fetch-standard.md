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
| fetch() 签名不变 | `fetch(input, init)` |
| Response.text()/json()/arrayBuffer() 仍返回 Promise | 行为不变，内部改为 async |
| Headers API 不变 | 全部现有方法保持 |
| Request 构造器不变 | 新增 signal 处理 |
| fetch host import type_idx 3 → 可能变为 async 签名 | 需要 backend 同步更新 |

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

## Tasks

### Task 1: 引入 reqwest 依赖

**Files**: `Cargo.toml` (workspace), `crates/wjsm-runtime/Cargo.toml`
**Why**: HTTP fetch 需要 async HTTP 客户端
**Impact**: 新增 workspace 级依赖
**Verification**: `cargo check -p wjsm-runtime`

- [ ] Step 1: 在 workspace `Cargo.toml` 的 `[workspace.dependencies]` 中添加：
```toml
reqwest = { version = "0.12", features = ["rustls-tls", "stream"] }
```

- [ ] Step 2: 在 `crates/wjsm-runtime/Cargo.toml` 的 `[dependencies]` 中添加：
```toml
reqwest = { workspace = true }
```

- [ ] Step 3: 验证编译
```bash
cargo check -p wjsm-runtime
```

- [ ] Step 4: Commit
```bash
git add -A && git commit -m "feat: add reqwest dependency for HTTP fetch"
```

### Task 2: 拆分 fetch.rs 为三个文件

**Files**: `crates/wjsm-runtime/src/host_imports/fetch.rs`, `crates/wjsm-runtime/src/host_imports/fetch_core.rs`, `crates/wjsm-runtime/src/host_imports/fetch_http.rs`, `crates/wjsm-runtime/src/host_imports/mod.rs`
**Why**: fetch.rs 1394 行即将膨胀到 2500+，需提前拆分
**Impact**: 纯重构，行为零变更
**Verification**: `cargo nextest run --workspace`

- [ ] Step 1: 创建 `fetch_core.rs`，从 fetch.rs 移入以下内容：
  - `HeadersEntry`, `HeadersGuard`, `HeadersMethodKind` 类型定义
  - `FetchResponseEntry`, `FetchRequestEntry`, `ResponseType`, `RedirectMode` 等类型定义
  - `RequestMode`, `RequestCredentials`, `RequestCache` 等类型定义
  - 所有 `create_*` 函数：`create_empty_headers`, `create_response_object`, `create_request_object`, `create_headers_object_from_handle`, `create_arraybuffer_with_bytes`
  - 所有 `init_*` 函数：`init_headers_object`
  - 所有 `attach_*` 函数：`attach_headers_methods`, `attach_response_methods`, `attach_request_methods`
  - 所有 `construct_*` 函数：`construct_headers`, `construct_request`, `construct_response`
  - `call_headers_method_from_caller`, `call_response_method_from_caller`, `call_request_method_from_caller`
  - 所有辅助函数：`valid_header_name`, `valid_header_value`, `append_header_pair`, `set_header_pair`, `clone_headers_handle`, `copy_headers_into`, `fill_headers_from_init`, `create_headers_from_init`, `body_bytes_from_value`, `valid_method`, `forbidden_method`, `url_has_credentials`, `parse_redirect_mode`, `valid_request_cache`, `valid_request_credentials`, `define_request_string_property`, `define_request_init_properties`, `null_body_status`, `valid_status_text`
  - 所有 handle getter：`get_headers_handle_from_object`, `get_response_handle_from_object`, `get_request_handle_from_object`
  - `push_native_callable`, `alloc_type_error_from_caller`, `exception_value_from_table`
  - `js_string_from_value`, `object_property`, `string_property`, `number_property`, `bool_property`

- [ ] Step 2: 创建 `fetch_http.rs`，从 fetch.rs 移入：
  - `parse_fetch_input`, `extract_string_from_value`, `extract_string_property`
  - `perform_fetch_and_build_response`（data: URL 路径）
  - `urlencoding_decode`（如果当前在 fetch.rs 中内联）

- [ ] Step 3: 重写 `fetch.rs` 为入口文件，仅包含：
  - `pub(crate) fn define_fetch(...)` — 注册 host functions
  - `pub(crate) mod fetch_core;` 和 `pub(crate) mod fetch_http;` 的 re-exports
  - 或通过 `mod.rs` 模式（检查现有 `host_imports/mod.rs` 的引用方式）

  实际上 host_imports 目录下的模块通过 `mod.rs` 声明。检查 `mod.rs` 中的 `mod fetch;` 声明。拆分后：
  - `mod.rs` 中改为 `mod fetch; mod fetch_core; mod fetch_http;`
  - `fetch.rs` 仅保留 `define_fetch`
  - `fetch_core.rs` 包含所有构造/方法/类型
  - `fetch_http.rs` 包含输入解析 + 执行

- [ ] Step 4: 验证编译 + 全测试通过
```bash
cargo nextest run --workspace
```

- [ ] Step 5: Commit
```bash
git add -A && git commit -m "refactor: split fetch.rs into fetch.rs, fetch_core.rs, fetch_http.rs"
```

### Task 3: RuntimeState 新增侧表 + NativeCallable 变体

**Files**: `crates/wjsm-runtime/src/lib.rs`
**Why**: HTTP fetch 需要存储 reqwest Response、ReadableStream、Reader、AbortSignal 状态
**Impact**: RuntimeState 扩展，NativeCallable enum 扩展
**Verification**: `cargo check -p wjsm-runtime`

- [ ] Step 1: 在 RuntimeState struct 中新增字段（在 `fetch_request_table` 之后）：
```rust
/// AbortSignal 侧表：存储 abort 状态
abort_signal_table: Arc<Mutex<Vec<AbortSignalEntry>>>,
/// reqwest Response 侧表：持有未消费的 HTTP response body stream
http_response_table: Arc<Mutex<Vec<HttpResponseEntry>>>,
/// ReadableStream 侧表：存储流状态
readable_stream_table: Arc<Mutex<Vec<ReadableStreamEntry>>>,
/// Reader 侧表：存储 reader → stream 映射
reader_table: Arc<Mutex<Vec<ReaderEntry>>>,
```

- [ ] Step 2: 在 RuntimeState::new() 中初始化新字段：
```rust
abort_signal_table: Arc::new(Mutex::new(Vec::new())),
http_response_table: Arc::new(Mutex::new(Vec::new())),
readable_stream_table: Arc::new(Mutex::new(Vec::new())),
reader_table: Arc::new(Mutex::new(Vec::new())),
```

- [ ] Step 3: 在 lib.rs 中定义新类型（放在 FetchRequestEntry 之后）：
```rust
#[derive(Clone, Debug)]
struct AbortSignalEntry {
    aborted: bool,
    reason: Option<i64>,
}

struct HttpResponseEntry {
    response: Option<reqwest::Response>,
}

#[derive(Clone, Debug)]
enum StreamState {
    Readable,
    Closed,
    Errored,
}

#[derive(Clone, Debug)]
struct ReadableStreamEntry {
    state: StreamState,
    error: Option<String>,
    disturbed: bool,
    locked: bool,
    http_response_handle: Option<u32>,
}

#[derive(Clone, Debug)]
struct ReaderEntry {
    stream_handle: u32,
}
```

- [ ] Step 4: 在 NativeCallable enum 中新增变体（在 `RequestMethod` 之后）：
```rust
StreamMethod {
    handle: u32,
    kind: StreamMethodKind,
},
ReaderMethod {
    handle: u32,
    kind: ReaderMethodKind,
},
AbortControllerConstructor,
AbortControllerAbort {
    signal_handle: u32,
},
```

新增 kind enums：
```rust
#[derive(Clone, Copy)]
enum StreamMethodKind {
    GetReader,
    Cancel,
}

#[derive(Clone, Copy)]
enum ReaderMethodKind {
    Read,
    ReleaseLock,
}
```

- [ ] Step 5: 验证编译
```bash
cargo check -p wjsm-runtime
```

- [ ] Step 6: Commit
```bash
git add -A && git commit -m "feat: add HTTP fetch side tables and NativeCallable variants to RuntimeState"
```

### Task 4: 将 fetch host function 改为 func_wrap_async

**Files**: `crates/wjsm-runtime/src/host_imports/fetch.rs`, `crates/wjsm-runtime/src/host_imports/fetch_http.rs`
**Why**: HTTP 请求是异步的，需要 `func_wrap_async` 实现 epoch yielding
**Impact**: fetch ABI 从同步变为异步——需要 backend host_import_registry 同步更新 type_idx
**Verification**: data: URL fixture 仍通过 + `cargo check -p wjsm-runtime`

- [ ] Step 1: 修改 `define_fetch` 中 fetch 的注册方式，从 `Func::wrap` 改为 `linker.func_wrap_async`：

```rust
linker.func_wrap_async(
    "env", "fetch",
    |mut caller: Caller<'_, RuntimeState>, (input,): (i64,)| {
        Box::new(async move {
            let promise = alloc_promise_from_caller(&mut caller, PromiseEntry::pending());

            let (method, url, headers_handle, body_opt, redirect) =
                parse_fetch_input(&mut caller, input, value::encode_undefined());

            if url.is_empty() {
                let err = alloc_type_error_from_caller(&mut caller, "Failed to parse URL from request.");
                settle_promise(caller.data_mut(), promise, PromiseSettlement::Reject(err));
                return promise;
            }

            // data: URL — 同步路径（保持现有行为）
            if url.starts_with("data:") {
                match perform_data_url_fetch(&mut caller, &url) {
                    Ok(response_val) => {
                        settle_promise(caller.data_mut(), promise, PromiseSettlement::Fulfill(response_val));
                    }
                    Err(msg) => {
                        let err = alloc_type_error_from_caller(&mut caller, &msg);
                        settle_promise(caller.data_mut(), promise, PromiseSettlement::Reject(err));
                    }
                }
                return promise;
            }

            // HTTP/HTTPS — 异步路径（将在 Task 5 实现）
            let err = alloc_type_error_from_caller(
                &mut caller,
                &format!("fetch for non-data: URL not implemented yet: {}", url),
            );
            settle_promise(caller.data_mut(), promise, PromiseSettlement::Reject(err));
            promise
        })
    },
)?;
```

注意：`func_wrap_async` 的签名需要确认。查看 `reentrant_async.rs` 中 `func_wrap_async` 的用法确认参数格式。当前 reentrant_async 使用 `linker.func_wrap_async("env", name, |caller, args| Box::new(async move { ... }))` 模式。fetch 的参数是 `(input: i64)`，返回 `i64`。

- [ ] Step 2: 在 fetch_http.rs 中将 `perform_fetch_and_build_response` 的 data: 路径提取为 `perform_data_url_fetch`：

```rust
fn perform_data_url_fetch(
    caller: &mut Caller<'_, RuntimeState>,
    url: &str,
) -> std::result::Result<i64, String> {
    let body = url.split(',').nth(1).unwrap_or("").to_string();
    let decoded = urlencoding_decode(&body);
    let bytes = decoded.into_bytes();
    let resp_headers = create_empty_headers(caller);
    Ok(create_response_object(
        caller, 200, "OK".to_string(), resp_headers,
        url.to_string(), bytes, ResponseType::Basic, false, None,
    ))
}
```

- [ ] Step 3: 更新 `host_import_registry.rs` 中 fetch 的 type_idx。fetch 从同步 `(i64) → i64` (type_idx 3) 变为 async 签名。需要查看当前 async host function 使用的 type_idx，或创建新的。先查看已有的 async import 的 type_idx：
  - 当前 `func_wrap_async` 的 import 类型需要匹配 wasmtime 的 async ABI。在 wasmtime 中，async host function 的 WASM 侧签名与同步签名相同——差异在于 host function 的执行模型。因此 type_idx **不需要改变**。

- [ ] Step 4: 验证 data: URL fixture 通过
```bash
cargo nextest run -E 'test(happy__fetch_data_url)'
```

- [ ] Step 5: Commit
```bash
git add -A && git commit -m "feat: convert fetch host function to func_wrap_async"
```

### Task 5: 实现 HTTP fetch 核心路径

**Files**: `crates/wjsm-runtime/src/host_imports/fetch_http.rs`
**Why**: 核心价值——让 fetch 支持 HTTP/HTTPS
**Impact**: 新增 async 网络能力
**Verification**: 新增 fixture `fetch_http_get.js`

- [ ] Step 1: 在 fetch_http.rs 中实现 `perform_http_fetch`：

```rust
async fn perform_http_fetch(
    caller: &mut Caller<'_, RuntimeState>,
    method: String,
    url: String,
    headers_handle: u32,
    body: Option<Vec<u8>>,
    redirect: RedirectMode,
    signal_handle: Option<u32>,
) -> std::result::Result<i64, String> {
    // 1. 检查 abort
    if let Some(handle) = signal_handle {
        if is_signal_aborted(caller, handle) {
            return Err("The operation was aborted".to_string());
        }
    }

    // 2. 构建 reqwest 请求
    let redirect_policy = match redirect {
        RedirectMode::Follow => reqwest::redirect::Policy::limited(20),
        RedirectMode::Error => reqwest::redirect::Policy::none(),
        RedirectMode::Manual => reqwest::redirect::Policy::limited(0),
    };

    let client = reqwest::Client::builder()
        .redirect(redirect_policy)
        .build()
        .map_err(|e| format!("fetch client error: {}", e))?;

    let http_method: reqwest::Method = method.parse()
        .map_err(|e| format!("invalid method: {}", e))?;

    let mut builder = client.request(http_method, &url);

    // 3. 添加 headers
    let headers = caller.data().headers_table.lock().expect("headers mutex");
    if let Some(entry) = headers.get(headers_handle as usize) {
        for (name, value) in &entry.pairs {
            builder = builder.header(name.as_str(), value.as_str());
        }
    }
    drop(headers);

    // 4. 添加 body
    if let Some(body_bytes) = body {
        builder = builder.body(body_bytes);
    }

    // 5. 发送请求（await — wasmtime 自动 yield）
    let response = builder.send().await
        .map_err(|e| format!("fetch failed: {}", e))?;

    // 6. 提取响应信息
    let status = response.status().as_u16();
    let status_text = response.status().canonical_reason().unwrap_or("").to_string();
    let resp_url = response.url().to_string();
    let redirected = response.url().as_str() != url;

    // 7. 提取响应 headers
    let resp_headers = create_empty_headers(caller);
    {
        let mut htable = caller.data().headers_table.lock().expect("headers mutex");
        if let Some(entry) = htable.get_mut(resp_headers as usize) {
            for (key, value) in response.headers() {
                entry.pairs.push((key.as_str().to_ascii_lowercase(), value.to_str().unwrap_or("").to_string()));
            }
        }
    }

    // 8. 存储 reqwest Response（用于后续流式读取）
    let http_handle = {
        let mut table = caller.data().http_response_table.lock().expect("http_response mutex");
        let handle = table.len() as u32;
        table.push(HttpResponseEntry { response: Some(response) });
        handle
    };

    // 9. 构造 Response 对象（body 暂为 null，通过 ReadableStream 懒加载）
    let resp_obj = create_response_object_with_http_handle(
        caller, status, status_text, resp_headers, resp_url,
        ResponseType::Basic, redirected, http_handle,
    );

    Ok(resp_obj)
}
```

- [ ] Step 2: 实现 `create_response_object_with_http_handle`（在 fetch_core.rs 中）：
  - 与 `create_response_object` 类似，但增加 `__http_response_handle__` 隐藏属性
  - `body` 属性设为 null（后续 Task 7 实现懒创建 ReadableStream）
  - 增加 `bodyUsed` 属性

- [ ] Step 3: 实现 `is_signal_aborted` 辅助函数：
```rust
fn is_signal_aborted(caller: &mut Caller<'_, RuntimeState>, handle: u32) -> bool {
    let table = caller.data().abort_signal_table.lock().expect("abort_signal mutex");
    table.get(handle as usize).map(|e| e.aborted).unwrap_or(false)
}
```

- [ ] Step 4: 在 Task 4 的 `define_fetch` 中替换 "not implemented" 错误为实际 HTTP 路径：
```rust
// HTTP/HTTPS — 异步路径
match perform_http_fetch(&mut caller, method, url, headers_handle, body_opt, redirect, None).await {
    Ok(response_val) => {
        settle_promise(caller.data_mut(), promise, PromiseSettlement::Fulfill(response_val));
    }
    Err(msg) => {
        let err = alloc_type_error_from_caller(&mut caller, &msg);
        settle_promise(caller.data_mut(), promise, PromiseSettlement::Reject(err));
    }
}
```

- [ ] Step 5: 更新 `parse_fetch_input` 以正确从 Request 对象提取 method/headers/body/redirect：
  - 当前实现只提取 url，method 硬编码为 GET
  - 需要从 Request-like 对象读取 `.method`, `.headers`, `.body`, `.redirect` 属性
  - 如果 init 对象非 undefined，从中读取覆盖值

- [ ] Step 6: 新增 fixture `fixtures/happy/fetch_http_get.js`：
```javascript
// KNOWN-NETWORK: requires HTTP access
fetch("https://httpbin.org/get")
  .then(r => {
    console.log(r.status);
    console.log(r.ok);
    return r.text();
  })
  .then(t => console.log(t.length > 0))
  .catch(e => console.log("error: " + e.message));
```

新增 `fixtures/happy/fetch_http_get.expected`：
```
exit_code: 0
--- stdout ---
200
true
true
--- stderr ---
```

- [ ] Step 7: 验证
```bash
cargo nextest run -E 'test(happy__fetch_http_get)'
```

- [ ] Step 8: Commit
```bash
git add -A && git commit -m "feat: implement HTTP fetch core path (GET, response headers, Promise settle)"
```

### Task 6: Response body 消费方法改为 async

**Files**: `crates/wjsm-runtime/src/host_imports/fetch_core.rs`, `crates/wjsm-runtime/src/host_imports/fetch_http.rs`
**Why**: HTTP 响应 body 需要 await 完整读取；data: URL 路径保持同步
**Impact**: `Response.text()`/`json()`/`arrayBuffer()` 内部逻辑分支
**Verification**: `fetch_data_url.js` fixture + 新 HTTP fixture 通过

- [ ] Step 1: 修改 `call_response_method_from_caller` 中的 Text/Json/ArrayBuffer 分支：
  - 对于有 `__http_response_handle__` 属性的 Response：通过 `perform_http_body_consume` 异步消费
  - 对于没有该属性的 Response（data: URL / 用户构造）：保持现有同步逻辑

  由于 NativeCallable 的调用是同步分发，但 body 消费需要异步，需要调整架构：
  - **方案**：Response body 消费方法（text/json/arrayBuffer）改为在 `call_native_callable` 中检测到 ResponseMethod::{Text,Json,ArrayBuffer} 时，通过 `func_wrap_async` 路径执行
  - 实际更简单的方案：**在 `call_response_method_from_caller` 中，如果有 http_response_handle，立即返回一个 pending Promise，然后通过 `host_completion_tx` 发送 Materialize 闭包来 settle**
  - **最简方案**：在 `call_response_method_from_caller` 中，如果有 http_response_handle，直接 await reqwest 的 `bytes()`。但这需要 `call_response_method_from_caller` 本身是 async。
  - **最终方案**：参考现有 reentrant_async.rs 模式——`call_native_callable` 在 runtime_builtins.rs 中是同步的。对于需要 async 的 Response 方法，需要注册一个单独的 `func_wrap_async` host import（类似 `stream_read`），在 NativeCallable 分发时跳转到该异步路径。

  实际实现：在 `define_fetch` 中注册一个 `response_body_consume` 的 `func_wrap_async` host import。当 `call_response_method_from_caller` 检测到 HTTP Response 时，不是直接执行，而是将 (response_handle, kind) 信息存储，然后调用 WASM 侧的 `response_body_consume` import 来触发异步消费。

  **更简单的替代方案**：在 ResponseMethod 的 NativeCallable 中存储一个标记。在 `call_native_callable` 中，遇到 ResponseMethod::{Text,Json,ArrayBuffer} 且 Response 有 `__http_response_handle__` 时，创建 pending Promise 并通过 `host_completion_tx` 发送一个 Materialize 闭包，闭包内 tokio::spawn 一个 task 执行 reqwest bytes() 读取，完成后通过 channel 回传结果。

  **最终选择**：使用 Materialize 闭包模式（利用已有的 `host_completion_tx` channel）。这是最干净的方案，不需要改变 NativeCallable 的分发模型。

- [ ] Step 2: 实现异步 body 消费。在 `call_response_method_from_caller` 中：
  - 检查 Response 对象是否有 `__http_response_handle__` 属性
  - 如果有：
    1. 标记 body_used = true
    2. alloc pending Promise
    3. 从 http_response_table take 出 reqwest::Response
    4. 获取 host_completion_tx
    5. tokio::spawn 一个 task：await resp.bytes()，完成后通过 tx 发送 Materialize 闭包
    6. Materialize 闭包在 main loop 中执行：构造 JS 值 + settle Promise
    7. 返回 Promise handle
  - 如果没有（data: URL / 用户构造）：保持现有同步逻辑

- [ ] Step 3: 验证 data: URL fixture 不受影响
```bash
cargo nextest run -E 'test(happy__fetch_data_url)'
```

- [ ] Step 4: 验证 HTTP fetch body 消费
```bash
cargo nextest run -E 'test(happy__fetch_http_get)'
```

- [ ] Step 5: Commit
```bash
git add -A && git commit -m "feat: async Response body consumption for HTTP responses"
```

### Task 7: 实现 ReadableStream + 流式 body

**Files**: `crates/wjsm-runtime/src/host_imports/fetch_core.rs`, `crates/wjsm-runtime/src/host_imports/fetch_http.rs`, `crates/wjsm-runtime/src/lib.rs` (NativeCallable dispatch)
**Why**: 大文件场景需要流式读取，Fetch Standard 规定 Response.body 是 ReadableStream
**Impact**: 新增 ReadableStream/Reader 对象类型和侧表
**Verification**: 新增 `fetch_stream_body.js` fixture

- [ ] Step 1: 实现 ReadableStream 对象创建（在 fetch_core.rs 中）：
```rust
fn create_readable_stream_object(
    caller: &mut Caller<'_, RuntimeState>,
    http_response_handle: u32,
) -> i64 {
    let stream_handle = {
        let mut table = caller.data().readable_stream_table.lock().expect("stream mutex");
        let handle = table.len() as u32;
        table.push(ReadableStreamEntry {
            state: StreamState::Readable,
            error: None,
            disturbed: false,
            locked: false,
            http_response_handle: Some(http_response_handle),
        });
        handle
    };

    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let obj = alloc_host_object(caller, &env, 4);

    let handle_val = value::encode_f64(stream_handle as f64);
    let _ = define_host_data_property_from_caller(caller, obj, "__stream_handle__", handle_val);

    // locked getter（暂用 data property，后续可改为 getter）
    let _ = define_host_data_property_from_caller(caller, obj, "locked", value::encode_bool(false));

    // getReader() 方法
    let callable = NativeCallable::StreamMethod { handle: stream_handle, kind: StreamMethodKind::GetReader };
    let idx = push_native_callable(caller, callable);
    let val = value::encode_native_callable_idx(idx);
    let _ = define_host_data_property_from_caller(caller, obj, "getReader", val);

    obj
}
```

- [ ] Step 2: 实现 Reader 对象创建：
```rust
fn create_reader_object(
    caller: &mut Caller<'_, RuntimeState>,
    stream_handle: u32,
) -> i64 {
    let reader_handle = {
        let mut table = caller.data().reader_table.lock().expect("reader mutex");
        let handle = table.len() as u32;
        table.push(ReaderEntry { stream_handle });
        handle
    };

    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let obj = alloc_host_object(caller, &env, 4);

    let handle_val = value::encode_f64(reader_handle as f64);
    let _ = define_host_data_property_from_caller(caller, obj, "__reader_handle__", handle_val);

    // read() 方法
    let callable = NativeCallable::ReaderMethod { handle: reader_handle, kind: ReaderMethodKind::Read };
    let idx = push_native_callable(caller, callable);
    let val = value::encode_native_callable_idx(idx);
    let _ = define_host_data_property_from_caller(caller, obj, "read", val);

    // releaseLock() 方法
    let callable = NativeCallable::ReaderMethod { handle: reader_handle, kind: ReaderMethodKind::ReleaseLock };
    let idx = push_native_callable(caller, callable);
    let val = value::encode_native_callable_idx(idx);
    let _ = define_host_data_property_from_caller(caller, obj, "releaseLock", val);

    obj
}
```

- [ ] Step 3: 实现 NativeCallable 分发（在 runtime_builtins.rs 中）：
```rust
NativeCallable::StreamMethod { handle, kind } => {
    call_stream_method_from_caller(caller, this_val, handle, kind, &args)
}
NativeCallable::ReaderMethod { handle, kind } => {
    call_reader_method_from_caller(caller, this_val, handle, kind, &args)
}
NativeCallable::AbortControllerConstructor => {
    construct_abort_controller(caller, this_val, &args)
}
NativeCallable::AbortControllerAbort { signal_handle } => {
    abort_controller_abort(caller, signal_handle, &args)
}
```

- [ ] Step 4: 实现 `call_stream_method_from_caller`：
  - GetReader: 标记 stream locked，创建 Reader 对象
  - Cancel: 标记 stream closed

- [ ] Step 5: 实现 `call_reader_method_from_caller`：
  - Read: 创建 pending Promise，通过 host_completion_tx 发送 Materialize 闭包执行 `resp.chunk().await`
  - ReleaseLock: 解锁 stream

- [ ] Step 6: 修改 `create_response_object_with_http_handle` 中 `body` 属性：从 `null` 改为懒创建的 ReadableStream 对象。

- [ ] Step 7: 新增 fixture `fixtures/happy/fetch_stream_body.js`：
```javascript
// KNOWN-NETWORK: requires HTTP access
fetch("https://httpbin.org/bytes/1024")
  .then(r => {
    console.log(r.status);
    let reader = r.body.getReader();
    return reader.read();
  })
  .then(result => {
    console.log(result.done);
    console.log(result.value.byteLength > 0);
  })
  .catch(e => console.log("error: " + e.message));
```

- [ ] Step 8: 验证
```bash
cargo nextest run -E 'test(happy__fetch_stream_body)'
```

- [ ] Step 9: Commit
```bash
git add -A && git commit -m "feat: implement ReadableStream + streaming body for HTTP fetch"
```

### Task 8: 实现 AbortController / AbortSignal

**Files**: `crates/wjsm-runtime/src/host_imports/fetch_core.rs`, `crates/wjsm-runtime/src/lib.rs` (NativeCallable + Builtin dispatch), `crates/wjsm-ir/src/builtin.rs`, `crates/wjsm-backend-wasm/src/host_import_registry.rs`
**Why**: Fetch Standard 规定 Request.signal 支持，AbortController 是中止请求的标准 API
**Impact**: 新增全局构造器 + Builtin
**Verification**: 新增 `abort_controller.js` + `fetch_abort.js` fixture

- [ ] Step 1: 在 `wjsm-ir/src/builtin.rs` 中新增：
```rust
AbortControllerConstructor,
```
在 Display impl 中添加：
```rust
Self::AbortControllerConstructor => "AbortController",
```

- [ ] Step 2: 在 `host_import_registry.rs` 中新增：
```rust
HostImportSpec {
    name: "abort_controller_constructor",
    type_idx: 12, // (env, this, args_base, args_count) → i64
    key: Some(HostImportKey::Builtin(Builtin::AbortControllerConstructor)),
    group: None,
},
```

- [ ] Step 3: 在 `define_fetch` 中注册 `abort_controller_constructor` host import（同步 Func::wrap）：
```rust
let abort_controller_ctor = Func::wrap(
    &mut store,
    |mut caller: Caller<'_, RuntimeState>, _env: i64, this_val: i64, args_base: i32, args_count: i32| -> i64 {
        let args: Vec<i64> = (0..args_count.max(0))
            .map(|index| read_shadow_arg(&mut caller, args_base, index as u32))
            .collect();
        construct_abort_controller(&mut caller, this_val, &args).unwrap_or_else(value::encode_undefined)
    },
);
linker.define(&mut store, "env", "abort_controller_constructor", abort_controller_ctor)?;
```

- [ ] Step 4: 在 fetch_core.rs 中实现 `construct_abort_controller`：
```rust
pub(crate) fn construct_abort_controller(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    _args: &[i64],
) -> Option<i64> {
    let signal_handle = {
        let mut table = caller.data().abort_signal_table.lock().expect("abort_signal mutex");
        let handle = table.len() as u32;
        table.push(AbortSignalEntry { aborted: false, reason: None });
        handle
    };

    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let obj = if value::is_object(this_val) {
        this_val
    } else {
        alloc_host_object(caller, &env, 4)
    };

    // signal 属性
    let signal_obj = create_signal_object(caller, signal_handle);
    let _ = define_host_data_property_from_caller(caller, obj, "signal", signal_obj);

    // abort() 方法
    let callable = NativeCallable::AbortControllerAbort { signal_handle };
    let idx = push_native_callable(caller, callable);
    let val = value::encode_native_callable_idx(idx);
    let _ = define_host_data_property_from_caller(caller, obj, "abort", val);

    Some(obj)
}
```

- [ ] Step 5: 实现 `create_signal_object` 和 `abort_controller_abort`：

```rust
fn create_signal_object(caller: &mut Caller<'_, RuntimeState>, handle: u32) -> i64 {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let obj = alloc_host_object(caller, &env, 4);

    let handle_val = value::encode_f64(handle as f64);
    let _ = define_host_data_property_from_caller(caller, obj, "__signal_handle__", handle_val);
    let _ = define_host_data_property_from_caller(caller, obj, "aborted", value::encode_bool(false));

    // reason 属性
    let _ = define_host_data_property_from_caller(caller, obj, "reason", value::encode_undefined());

    obj
}

pub(crate) fn abort_controller_abort(
    caller: &mut Caller<'_, RuntimeState>,
    signal_handle: u32,
    args: &[i64],
) -> Option<i64> {
    let reason = args.first().copied();
    let mut table = caller.data().abort_signal_table.lock().expect("abort_signal mutex");
    if let Some(entry) = table.get_mut(signal_handle as usize) {
        entry.aborted = true;
        entry.reason = reason;
    }
    Some(value::encode_undefined())
}
```

- [ ] Step 6: 在 Semantic 层处理 AbortController 构造器。在 `wjsm-semantic` 中，`AbortController` 被识别为 host builtin 构造器（与 Headers/Request/Response 模式一致）。需要：
  - 在 `builtins.rs` 中添加 `AbortController` 的解析
  - 在 lowerer 中生成 `Builtin::AbortControllerConstructor` 调用

- [ ] Step 7: 在 `perform_http_fetch` 中添加 signal 检查（已在 Task 5 中预留 `signal_handle` 参数，现激活）：
  - 从 Request init 中提取 signal
  - fetch 前检查 abort
  - 请求完成后检查 abort

- [ ] Step 8: 新增 fixture `fixtures/happy/abort_controller.js`：
```javascript
let controller = new AbortController();
console.log(controller.signal.aborted);
controller.abort();
console.log(controller.signal.aborted);
```

新增 `fixtures/happy/abort_controller.expected`：
```
exit_code: 0
--- stdout ---
false
true
--- stderr ---
```

- [ ] Step 9: 验证
```bash
cargo nextest run -E 'test(happy__abort_controller)'
```

- [ ] Step 10: Commit
```bash
git add -A && git commit -m "feat: implement AbortController/AbortSignal"
```

### Task 9: 完善重定向 + HTTP 方法支持

**Files**: `crates/wjsm-runtime/src/host_imports/fetch_http.rs`, `crates/wjsm-runtime/src/host_imports/fetch_core.rs`
**Why**: 重定向 follow/error/manual 和 POST/PUT 等方法是 Fetch Standard 基本要求
**Impact**: 扩展 fetch 功能
**Verification**: 新增 fixture

- [ ] Step 1: 完善 `parse_fetch_input` 以从 init 对象读取 method/headers/body/redirect/signal：
  - 如果 input 是 Request 对象，从其属性读取默认值
  - 如果 init 对象存在，从中覆盖 method/headers/body/redirect/signal

- [ ] Step 2: 处理 manual redirect 的 `ResponseType::OpaqueRedirect`：
  - 当 `RedirectMode::Manual` 且响应状态为 3xx 时，response_type 设为 `opaqueredirect`
  - `type` 属性返回 `"opaqueredirect"`
  - body 为 null，status 为 0，headers 为空

- [ ] Step 3: 支持 POST/PUT/PATCH/DELETE 方法的 body 传输：
  - 已在 Task 5 中预留 `body` 参数
  - 需要确保 `parse_fetch_input` 正确提取 body（string → bytes, ArrayBuffer → bytes）

- [ ] Step 4: 新增 fixture `fixtures/happy/fetch_http_post.js`：
```javascript
// KNOWN-NETWORK: requires HTTP access
fetch("https://httpbin.org/post", { method: "POST", body: "hello" })
  .then(r => r.json())
  .then(j => console.log(j.data))
  .catch(e => console.log("error: " + e.message));
```

- [ ] Step 5: 验证
```bash
cargo nextest run -E 'test(happy__fetch_http_post)'
```

- [ ] Step 6: Commit
```bash
git add -A && git commit -m "feat: implement redirect modes and HTTP methods for fetch"
```

### Task 10: Semantic + Backend 对接

**Files**: `crates/wjsm-semantic/src/builtins.rs`, `crates/wjsm-semantic/src/lowerer_core.rs`, `crates/wjsm-backend-wasm/src/host_import_registry.rs`, `crates/wjsm-backend-wasm/src/compiler_builtins.rs`
**Why**: AbortController 构造器需要 semantic 识别和 backend 注册
**Impact**: 跨层变更
**Verification**: `cargo nextest run --workspace`

- [ ] Step 1: 在 `wjsm-semantic/src/builtins.rs` 中添加 `AbortController` 作为 host builtin 构造器

- [ ] Step 2: 在 `wjsm-backend-wasm/src/host_import_registry.rs` 中添加 `abort_controller_constructor` spec（已在 Task 8 Step 2 中完成）

- [ ] Step 3: 确保 backend compiler 在遇到 `Builtin::AbortControllerConstructor` 时 emit 正确的 import 调用

- [ ] Step 4: 全 workspace 测试
```bash
cargo nextest run --workspace
```

- [ ] Step 5: Commit
```bash
git add -A && git commit -m "feat: wire AbortController through semantic and backend layers"
```

### Task 11: 全量回归测试 + fixture 收尾

**Files**: fixtures, `crates/wjsm-runtime/tests/`
**Why**: 确保所有新增功能正常、现有功能无回归
**Impact**: 验证阶段
**Verification**: `cargo nextest run --workspace`

- [ ] Step 1: 运行全 workspace 测试
```bash
cargo nextest run --workspace
```

- [ ] Step 2: 确认 data: URL fixture 不受影响
```bash
cargo nextest run -E 'test(happy__fetch_data_url)'
```

- [ ] Step 3: 运行所有 fetch 相关 fixture
```bash
cargo nextest run -E 'test(fetch_)'
```

- [ ] Step 4: 如果网络依赖 fixture 在无网络环境下失败，添加 `KNOWN-NETWORK` 标记并确保 CI 可配置跳过

- [ ] Step 5: 更新 AGENTS.md 中的 fetch 描述（从 "data: URL only" 改为完整描述）

- [ ] Step 6: Commit
```bash
git add -A && git commit -m "test: verify full fetch implementation + update docs"
```

## Risks

| 风险 | 缓解 |
|---|---|
| reqwest 依赖体积 | `rustls-tls` 比 `native-tls` 更小 |
| 流式 body 的 reqwest Response 生命周期 | take/put_back 模式，await 期间不持锁 |
| fixture 依赖外部 HTTP 服务 | 标记 KNOWN-NETWORK，CI 可跳过 |
| epoch yielding 与长时间请求 | wasmtime 自动 yield |
| NativeCallable 同步分发 vs async body 消费 | 使用 host_completion_tx + Materialize 闭包 |

## Retirement

- 旧 `perform_fetch_and_build_response`（仅 data: URL + HTTP 报错）→ 由 `perform_data_url_fetch` + `perform_http_fetch` 替代
- 旧 `Func::wrap` fetch host function → 由 `func_wrap_async` 替代
- 旧 `Response.body = null` → 由 ReadableStream 对象替代（仅 HTTP Response）
