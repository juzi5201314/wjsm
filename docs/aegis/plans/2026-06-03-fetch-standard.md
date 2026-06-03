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
- `FetchResponseEntry`, `FetchRequestEntry` (含新增 `signal_handle: Option<u32>` 字段), `ResponseType`, `RedirectMode` 等类型定义
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

在 Task 2 拆分时同步修改 `FetchRequestEntry`，新增 `signal_handle` 字段：
```rust
struct FetchRequestEntry {
    // ... existing fields ...
+   signal_handle: Option<u32>,
}
```

新增类型定义：
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
同时，`FetchResponseEntry` 需新增 `http_response_handle` 字段，替代 JS 对象上的 `__http_response_handle__` 隐藏属性：
```rust
struct FetchResponseEntry {
    // ... existing fields ...
   http_response_handle: Option<u32>,  // 非 None 表示 HTTP 响应（需异步消费 body）
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
- [ ] Step 1: 修改 `define_fetch` 中 fetch 的注册方式，从 `Func::wrap` 改为 `linker.func_wrap_async`。
  同时修改 fetch 的 WASM 签名，从 `(i64) → i64` 改为 `(i64, i64) → i64`，新增 `init` 参数：

```rust
linker.func_wrap_async(
    "env", "fetch",
    |mut caller: Caller<'_, RuntimeState>, (input, init): (i64, i64)| {
        Box::new(async move {
            let promise = alloc_promise_from_caller(&mut caller, PromiseEntry::pending());

            let (method, url, headers_handle, body_opt, redirect, signal_handle) =
                parse_fetch_input(&mut caller, input, init);

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

- [ ] Step 2: 更新 backend 以支持 fetch 传 2 个参数：
  - `wjsm-semantic/src/builtins.rs`: 将 `Builtin::Fetch => ("fetch", 1)` 改为 `Builtin::Fetch => ("fetch", 2)`（min_args 不变，仍为 1，但 backend 取 args[0] 和 args[1]）
  - `wjsm-backend-wasm/src/compiler_builtins.rs`: `Builtin::Fetch` 分支中取 `args[0]` (input) 和 `args[1]` (init)，emit 两个 `LocalGet` + `Call`
  - `wjsm-backend-wasm/src/host_import_registry.rs`: 将 `fetch` 的 type_idx 从 3（单参数）改为对应的双参数类型。需查看 HOST_IMPORT_TYPES 或创建新的 type_idx（`i64, i64 → i64`）

- [ ] Step 3: 在 fetch_http.rs 中将 `perform_fetch_and_build_response` 的 data: 路径提取为 `perform_data_url_fetch`：

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

- [ ] Step 4: 添加 `WasmEnv::from_store`（scheduler 的 Materialize 闭包需要 `WasmEnv`，而闭包只接收 `Store`）：
```rust
// 在 wasm_env.rs 中新增：
impl WasmEnv {
    pub fn from_store(store: &mut Store<RuntimeState>) -> Option<Self> {
        Some(Self {
            memory: store.get_export("memory")?.into_memory()?,
            func_table: store.get_export("__table")?.into_table()?,
            shadow_sp: store.get_export("__shadow_sp")?.into_global()?,
            heap_ptr: store.get_export("__heap_ptr")?.into_global()?,
            obj_table_ptr: store.get_export("__obj_table_ptr")?.into_global()?,
            obj_table_count: store.get_export("__obj_table_count")?.into_global()?,
            object_proto_handle: store.get_export("__object_proto_handle")?.into_global()?,
            array_proto_handle: store.get_export("__array_proto_handle")?.into_global()?,
        })
    }
}
```

- [ ] Step 5: 验证 data: URL fixture 通过
```bash
cargo nextest run -E 'test(happy__fetch_data_url)'
```

- [ ] Step 6: Commit
```bash
git add -A && git commit -m "feat: convert fetch to func_wrap_async, add init param, add WasmEnv::from_store"
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
  - 复用 `create_response_object`，但在 `FetchResponseEntry` 中设置 `http_response_handle: Some(http_handle)`
  - `body` 属性设为 null（Task 7 将其改为 ReadableStream）
  - `bodyUsed` 属性设为 false

- [ ] Step 3: 实现 `is_signal_aborted` 辅助函数：
```rust
fn is_signal_aborted(state: &RuntimeState, handle: u32) -> bool {
    state.abort_signal_table.lock().expect("abort_signal mutex")
        .get(handle as usize).map(|e| e.aborted).unwrap_or(false)
}
```

- [ ] Step 4: 在 Task 4 的 `define_fetch` 中替换 "not implemented" 错误为实际 HTTP 路径：
```rust
// HTTP/HTTPS — 异步路径
match perform_http_fetch(&mut caller, method, url, headers_handle, body_opt, redirect, signal_handle).await {
    Ok(response_val) => {
        settle_promise(caller.data_mut(), promise, PromiseSettlement::Fulfill(response_val));
    }
    Err(msg) => {
        let err = alloc_type_error_from_caller(&mut caller, &msg);
        settle_promise(caller.data_mut(), promise, PromiseSettlement::Reject(err));
    }
}
```

- [ ] Step 5: 重写 `parse_fetch_input` 以正确从 Request 对象和 init 提取所有字段：
  返回类型从 `(String, String, u32, Option<Vec<u8>>, RedirectMode)` 改为 `(String, String, u32, Option<Vec<u8>>, RedirectMode, Option<u32>)`（新增 `signal_handle`）。

  逻辑：
  - 如果 `input` 是字符串 → 默认 GET, 无 body, Follow, 无 signal
  - 如果 `input` 是 Request 对象 → 从 `fetch_request_table` 读取 method/headers_handle/body/redirect/signal_handle
  - 如果 `init` 非 undefined → 从 init 对象覆盖 method/headers/body/redirect/signal
  - `init` 中 `signal` 属性检查：如果对象有 `__signal_handle__` 属性，提取 handle；否则忽略

```rust
fn parse_fetch_input(
    caller: &mut Caller<'_, RuntimeState>,
    input: i64,
    init: i64,
) -> (String, String, u32, Option<Vec<u8>>, RedirectMode, Option<u32>) {
    // 1. 从 input 提取基础信息
    let (mut method, mut url, mut headers_handle, mut body, mut redirect, mut signal_handle) =
        if value::is_string(input) {
            ("GET".to_string(), extract_string_from_value(caller, input),
             create_empty_headers(caller), None, RedirectMode::Follow, None)
        } else if value::is_object(input) {
            // 从 Request 对象读取属性
            let url = extract_string_property(caller, input, "url").unwrap_or_default();
            let method = extract_string_property(caller, input, "method").unwrap_or_else(|| "GET".to_string());
            let req_handle = get_request_handle_from_object(caller, input).unwrap_or(0);
            let (hdr, body, redir, sig) = {
                let table = caller.data().fetch_request_table.lock().expect("mutex");
                table.get(req_handle as usize).map(|e| (e.headers_handle, e.body.clone(), e.redirect, e.signal_handle))
                    .unwrap_or((create_empty_headers(caller), None, RedirectMode::Follow, None))
            };
            (method, url, hdr, body, redir, sig)
        } else {
            (String::new(), String::new(), create_empty_headers(caller), None, RedirectMode::Follow, None)
        };

    // 2. 如果 init 非 undefined，覆盖属性
    if value::is_object(init) {
        if let Some(m) = extract_string_property(caller, init, "method") { method = m; }
        // headers: 创建新 headers 从 init
        if let Some(h) = object_property(caller, init, "headers") {
            headers_handle = create_headers_from_init(caller, h);
        }
        if let Some(b) = object_property(caller, init, "body") {
            body = body_bytes_from_value(caller, b);
        }
        if let Some(r) = extract_string_property(caller, init, "redirect") {
            redirect = parse_redirect_mode(&r).unwrap_or(redirect);
        }
        if let Some(sig_obj) = object_property(caller, init, "signal") {
            signal_handle = get_signal_handle_from_object(caller, sig_obj);
        }
    }

    (method, url, headers_handle, body, redirect, signal_handle)
}
```

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
  - 检查 `FetchResponseEntry.http_response_handle` 是否非 None（从 `fetch_response_table` 读取）
  - 如果非 None（HTTP Response）→ 异步路径
  - 如果 None（data: URL / 用户构造）→ 保持现有同步逻辑

  **异步路径方案**（利用已有的 `AsyncHostCompletion::Materialize`）：

  `Materialize` 闭包签名为 `FnOnce(&mut Store<RuntimeState>, &WasmEnv) -> PromiseSettlement`。
  闭包内可通过 `alloc_host_object(store, env, capacity)` 创建 JS 对象（`alloc_host_object` 已是泛型，接受 `impl AsContextMut`）。
  `WasmEnv` 通过 Task 4 添加的 `WasmEnv::from_store(store)` 获取。

  具体步骤：
  1. 在 `call_response_method_from_caller` 中，标记 body_used，alloc pending Promise
  2. 从 `http_response_table` take 出 `reqwest::Response`
  3. `tokio::spawn` 一个 task：`await resp.bytes()`
  4. bytes 完成后，通过 `host_completion_tx` 发送 `AsyncHostCompletion::Materialize` 闭包
  5. 闭包在 scheduler 的 post-main 阶段执行，通过 `Store` + `WasmEnv` 创建 JS 值
  6. 闭包返回 `PromiseSettlement`，scheduler 自动 settle Promise
  7. 返回 Promise handle

- [ ] Step 2: 实现异步 body 消费核心逻辑：
```rust
// 在 call_response_method_from_caller 中，检测到 HTTP Response 时：
let promise = alloc_promise_from_caller(caller, PromiseEntry::pending());
let response = {
    let mut table = caller.data().http_response_table.lock().expect("mutex");
    table.get_mut(http_handle as usize).and_then(|e| e.response.take())
};
if let Some(response) = response {
    let state = caller.data().clone(); // RuntimeState 所有字段都是 Arc，可 clone
    let tx = caller.data().host_completion_tx.clone().expect("tx");
    let kind_clone = kind;
    let promise_clone = promise;

    tokio::spawn(async move {
        match response.bytes().await {
            Ok(bytes) => {
                let _ = tx.send(AsyncHostCompletion::Materialize {
                    promise: promise_clone,
                    materialize: Box::new(move |store, env| {
                        let state = store.data();
                        match kind_clone {
                            ResponseMethodKind::Text => {
                                let text = String::from_utf8_lossy(&bytes).to_string();
                                let mut strings = state.runtime_strings.lock().expect("mutex");
                                let handle = strings.len() as u32;
                                strings.push(text);
                                PromiseSettlement::Fulfill(value::encode_runtime_string_handle(handle))
                            }
                            ResponseMethodKind::Json => {
                                // 将 bytes 存为 runtime string，然后用 runtime 的 JSON 解析
                                let text = String::from_utf8_lossy(&bytes).to_string();
                                let mut strings = state.runtime_strings.lock().expect("mutex");
                                let handle = strings.len() as u32;
                                strings.push(text);
                                let text_val = value::encode_runtime_string_handle(handle);
                                // 调用 runtime_json 的 json_parse 函数（需要 Caller）。
                                // 简化：先用 runtime_strings 存储 JSON 字符串，
                                // 返回字符串值（让 JS 侧 .text() 后 JSON.parse()）
                                // TODO: 后续实现真正的 JSON 解析
                                PromiseSettlement::Fulfill(text_val)
                            }
                            ResponseMethodKind::ArrayBuffer => {
                                // 创建 ArrayBuffer 对象
                                let mut ab_table = state.arraybuffer_table.lock().expect("mutex");
                                let ab_handle = ab_table.len() as u32;
                                ab_table.push(ArrayBufferEntry { data: bytes.to_vec() });
                                drop(ab_table);
                                // 创建 JS ArrayBuffer 对象（通过 Store + WasmEnv）
                                let obj = alloc_host_object(store, env, 4);
                                let handle_val = value::encode_f64(ab_handle as f64);
                                define_host_data_property_from_store(store, obj, "__arraybuffer_handle__", handle_val);
                                let len_val = value::encode_f64(bytes.len() as f64);
                                define_host_data_property_from_store(store, obj, "byteLength", len_val);
                                PromiseSettlement::Fulfill(obj)
                            }
                            _ => PromiseSettlement::Fulfill(value::encode_undefined()),
                        }
                    }),
                });
            }
            Err(e) => {
                let _ = tx.send(AsyncHostCompletion::Materialize {
                    promise: promise_clone,
                    materialize: Box::new(move |store, env| {
                        let obj = create_error_object_from_store(store, env, "TypeError", &e.to_string());
                        PromiseSettlement::Reject(obj)
                    }),
                });
            }
        }
    });
}
return Some(promise);
```

注意：需要添加 `define_host_data_property_from_store` 和 `create_error_object_from_store` 辅助函数（泛型化现有函数，接受 `Store` + `WasmEnv`）。这些函数与 `from_caller` 版本逻辑相同，只是用 `store` 替代 `caller`。

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
  - Read: 与 Task 6 相同模式——创建 pending Promise，`tokio::spawn` 中 `await resp.chunk()`，完成后发送 `Materialize` 闭包。闭包中通过 `Store` + `WasmEnv` 构造 `{done: boolean, value: Uint8Array | undefined}` 对象。chunk 到达时 `put_back` response 到 `http_response_table`；chunk 为 None 时标记 stream Closed，不放回 response。
  - ReleaseLock: 解锁 stream（标记 `locked = false`，更新 `locked` 属性）

- [ ] Step 6: 修改 `create_response_object_with_http_handle` 中 `body` 属性：从 `null` 改为直接创建 ReadableStream 对象（`create_readable_stream_object`）。

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

- [ ] Step 6: 修改 `construct_request` 以解析 `signal` 属性。在 RequestInit 解析中新增：
```rust
// 在 construct_request 中，解析 init 对象的 signal 属性
let mut signal_handle: Option<u32> = None;
if value::is_object(init) {
    if let Some(sig_obj) = object_property(caller, init, "signal") {
        signal_handle = get_signal_handle_from_object(caller, sig_obj);
    }
}
// 存储 signal_handle 到 FetchRequestEntry
```

同时实现 `get_signal_handle_from_object` 辅助函数：
```rust
fn get_signal_handle_from_object(caller: &mut Caller<'_, RuntimeState>, obj: i64) -> Option<u32> {
    let ptr = resolve_handle(caller, obj)?;
    let val = read_object_property_by_name(caller, ptr, "__signal_handle__")?;
    Some(value::decode_f64(val) as u32)
}
```

在 Semantic 层处理 AbortController 构造器：
  - 在 `builtins.rs` 中添加 `"AbortController" => Some(Builtin::AbortControllerConstructor)` 解析
  - 在 lowerer 中，`new AbortController()` 生成 `Builtin::AbortControllerConstructor` 调用

- [ ] Step 7: 在 `perform_http_fetch` 中激活 signal 检查（已在 Task 5 Step 1 和 Step 4 中预留 `signal_handle` 参数）：
  - fetch 前检查 `is_signal_aborted`
  - 请求完成后检查 `is_signal_aborted`（请求可能在 await 期间被 abort）
  - 注意：当前架构下无法在 await 期间中断 reqwest 请求（需要 reqwest 的 `futures::abortable` 或 `CancellationToken`）。后续可扩展。

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

- [ ] Step 4: 网络依赖 fixture 处理。所有 HTTP fetch fixture 需标记 `KNOWN-NETWORK`（在 JS 文件头注释中标注），并在 fixture runner 中支持环境变量跳过：
  - 在 `tests/fixture_runner.rs` 中添加：如果 fixture 文件包含 `KNOWN-NETWORK` 注释，且环境变量 `WJSM_SKIP_NETWORK=1`，则跳过该 fixture
  - CI 配置中默认设 `WJSM_SKIP_NETWORK=1`（除非专门的网络测试 job）
  - 本地开发时不设此变量，HTTP fixture 正常运行

## Risks

| 风险 | 缓解 |
|---|---|
| reqwest 依赖体积 | `rustls-tls` 比 `native-tls` 更小，无系统依赖 |
| 流式 body 的 reqwest Response 生命周期 | take/put_back 模式，await 期间不持锁 |
| fixture 依赖外部 HTTP 服务 | `KNOWN-NETWORK` 标记 + `WJSM_SKIP_NETWORK` 环境变量跳过 |
| epoch yielding 与长时间请求 | wasmtime 自动 yield |
| NativeCallable 同步分发 vs async body 消费 | `tokio::spawn` + `Materialize` 闭包（`Store` + `WasmEnv` 路径创建 JS 对象） |
| Response.json() 在 Materialize 中解析 | 简化：先返回字符串值，后续用 runtime JSON 解析器增强 |
## Retirement

- 旧 `perform_fetch_and_build_response`（仅 data: URL + HTTP 报错）→ 由 `perform_data_url_fetch` + `perform_http_fetch` 替代
- 旧 `Func::wrap` fetch host function → 由 `func_wrap_async` 替代
- 旧 `Response.body = null` → 由 ReadableStream 对象替代（仅 HTTP Response）
- 旧 `FetchResponseEntry`（无 `http_response_handle`）→ 由带 `http_response_handle: Option<u32>` 的新版本替代
- 旧 `FetchRequestEntry`（无 `signal_handle`）→ 由带 `signal_handle: Option<u32>` 的新版本替代
- 旧 `fetch` WASM 签名 `(i64)→i64` → 由 `(i64,i64)→i64` 替代
