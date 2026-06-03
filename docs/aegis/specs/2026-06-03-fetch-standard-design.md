# Fetch Standard 完整实现设计规格

**日期**: 2026-06-03
**状态**: Draft
**范围**: wjsm-runtime HTTP/HTTPS fetch + 流式 body + AbortSignal + 完整 Request/Response/Headers API

## 1. 背景与动机

当前 fetch 实现仅支持 `data:` URL，对 HTTP/HTTPS 直接返回错误。报告声称"使用 ureq 同步阻塞"，实际无任何 HTTP 客户端依赖。但核心问题真实存在：

1. **无 HTTP 支持** — `perform_fetch_and_build_response` 对非 data: URL 直接 `Err`
2. **同步 host function** — `define_fetch` 用 `Func::wrap` 注册，对 data: URL 同步 settle Promise（语义正确但无法扩展到异步场景）
3. **无 AbortSignal** — Request 构造器解析了 signal 字段但未实际使用
4. **无流式 body** — Response.body 始终为 null，body 消费是一次性读取全量 bytes

## 2. 目标

实现符合 [Fetch Standard](https://fetch.spec.whatwg.org/) §5 (Fetch API) 的完整 fetch：

- `fetch()` 对 HTTP/HTTPS 返回异步 settle 的 Promise
- 不阻塞 WASM 主线程（利用 wasmtime epoch yielding）
- Request/Response/Headers 完整 API
- 流式 body（ReadableStream pull 模式）
- AbortController/AbortSignal
- 重定向 follow/error/manual
- HTTP 方法 GET/HEAD/POST/PUT/PATCH/DELETE 等
- 请求/响应 header 传递
- 请求 body（string / ArrayBuffer / TypedArray）

## 3. 非目标

- Service Worker 拦截（非浏览器环境）
- CSP / Mixed Content（非浏览器环境）
- CORS 预检请求（非浏览器环境，所有请求视为 same-origin）
- Opaque / filtered response 类型
- FormData / Blob / URLSearchParams body 编码
- `Response.formData()` 方法
- `Response.blob()` 方法（无 Blob API）
- Cache API
- Cookie jar / credential 自动管理（credentials 选项解析但不影响实际请求）
- Content-Security-Policy、Referrer Policy、Integrity metadata

## 4. 调查验证

### 4.1 报告验证

| 报告声称 | 验证结果 |
|---|---|
| "使用 ureq 同步阻塞 HTTP 客户端" | **不实**。无任何 HTTP 客户端依赖 |
| "同步执行，阻塞 WASM 主线程" | **部分属实**。对 data: URL 同步执行（正确），HTTP 未实现 |
| "制造异步假象" | **不实**。data: URL 的 Promise 同步 settle 是 spec 允许的 |

### 4.2 现有基础设施

- **Async host function**: `func_wrap_async` 已广泛使用（reentrant_async.rs 中 arr_proto_sort、func_call 等）
- **Epoch yielding**: `store.epoch_deadline_async_yield_and_update(1)` 已配置
- **AsyncHostCompletion channel**: 已建立但当前无 host import 使用（仅测试中使用）
- **Scheduler**: post-main 阶段 drain microtasks + 处理 host completion + timer
- **reqwest**: 未引入，需新增

## 5. 架构

### 5.1 核心决策：`func_wrap_async` + reqwest await

```
fetch(input, init)
  → func_wrap_async host function
  → alloc Promise, return handle
  → if data: URL → 同步处理 + settle (保持现有行为)
  → if http/https → await reqwest 请求
  → wasmtime epoch yield 自动暂停 WASM 执行
  → 响应头到达 → 构造 Response 对象 + settle Promise
  → return promise handle
```

- Materialize 闭包模式适合一次性简单值，不适合多步流式场景
- `func_wrap_async` 直接持有 `&mut Caller`，可分配 JS 对象，无需间接闭包
- 直接 await 避免了 channel 传递和 Materialize 闭包的额外复杂度
- 与现有 async host function 模式一致

### 5.2 流式 Body：Pull 模式

Fetch Standard 规定 `process response` 在响应头到达时触发（fetch Promise settle），`process response end-of-body` 在 body 完成后触发。实现遵循此语义：

1. `fetch()` settle 时 Response 对象已构造（有 status/headers/url）
2. `Response.body` getter 返回 ReadableStream 对象（懒创建）
3. `getReader()` 返回 Reader 对象
4. `reader.read()` 是 `func_wrap_async` host function — await reqwest 的 `chunk()`
5. 每个 chunk 到达时 fulfill Promise<{done, value}>
6. body 结束时 fulfill {done: true, value: undefined}
7. 错误时 reject Promise

**简化路径**：`Response.text()`/`json()`/`arrayBuffer()` 直接 await 完整 body，不经过 ReadableStream。这些方法变为 `func_wrap_async` host function。

### 5.3 侧表设计

新增 RuntimeState 字段：

```rust
/// ReadableStream 侧表
readable_stream_table: Arc<Mutex<Vec<ReadableStreamEntry>>>,
/// Reader 侧表
reader_table: Arc<Mutex<Vec<ReaderEntry>>>,
/// AbortSignal 侧表
abort_signal_table: Arc<Mutex<Vec<AbortSignalEntry>>>,
/// reqwest Response 侧表（持有未消费的 response body stream）
http_response_table: Arc<Mutex<Vec<HttpResponseEntry>>>,
```

```rust
struct ReadableStreamEntry {
    state: StreamState,       // Readable / Closed / Errored
    error: Option<String>,
    disturbed: bool,
    locked: bool,
    /// 关联的 http_response_table handle（用于 pull 模式读取）
    http_response_handle: Option<u32>,
}

struct ReaderEntry {
    stream_handle: u32,
}

enum StreamState {
    Readable,
    Closed,
    Errored,
}

struct AbortSignalEntry {
    aborted: bool,
    reason: Option<i64>,
}

struct HttpResponseEntry {
    /// reqwest Response 对象（用于流式读取 chunk）
    /// Option 因为消费后取出
    response: Option<reqwest::Response>,
}
```

### 5.4 文件拆分

当前 `fetch.rs` (1394 行) 拆分为：

| 文件 | 职责 | 预估行数 |
|---|---|---|
| `fetch_core.rs` | Headers/Request/Response 构造、方法、属性、headers 验证 | ~800 |
| `fetch_http.rs` | HTTP 请求执行、流式 body、AbortSignal、ReadableStream | ~700 |
| `fetch.rs` | `define_fetch` 入口 + re-export | ~100 |

### 5.5 Host Import Registry 变更
### 5.5 Host Import Registry + NativeCallable 变更

ReadableStream/Reader 方法遵循与 Response 方法相同的 NativeCallable 分发模式（不注册为独立 host import）：

新增 `NativeCallable` 变体：
- `StreamMethod { handle, kind: StreamMethodKind }` — ReadableStream 方法
- `ReaderMethod { handle, kind: ReaderMethodKind }` — Reader 方法

`StreamMethodKind`: GetReader, Cancel
`ReaderMethodKind`: Read, ReleaseLock

新增 host import specs（仅用于构造器）：

| Import 名 | 签名 | 用途 |
|---|---|---|
| `abort_controller_constructor` | (env, this, args_base, args_count) → i64 | AbortController 构造 |

ReadableStream/Reader 的方法调用通过已有的 `call_native_callable` 路径分发到 `func_wrap_async` 注册的异步实现。
### 5.6 依赖变更

`Cargo.toml` (workspace):
```toml
reqwest = { version = "0.12", features = ["rustls-tls", "stream"] }
```

`crates/wjsm-runtime/Cargo.toml`:
```toml
reqwest = { workspace = true }
```

## 6. 详细设计

### 6.1 `define_fetch` 改为 async

```rust
pub(crate) fn define_fetch(
    linker: &mut Linker<RuntimeState>,
    mut store: &mut Store<RuntimeState>,
) -> Result<()> {
    // fetch(i64, i64) → i64  [input, init]
    linker.func_wrap_async(
        "env", "fetch",
        |mut caller: Caller<'_, RuntimeState>, (input, init): (i64, i64)| {
            Box::new(async move {
                let promise = alloc_promise_from_caller(&mut caller, PromiseEntry::pending());
                
                let (method, url, headers_handle, body, redirect, signal_handle) =
                    parse_fetch_input(&mut caller, input, init);
                
                if url.is_empty() {
                    reject_with_type_error(&mut caller, promise, "Failed to parse URL");
                    return promise;
                }
                
                // data: URL — 同步路径（保持现有行为）
                if url.starts_with("data:") {
                    let result = perform_data_url_fetch(&mut caller, &url);
                    match result {
                        Ok(response_val) => settle_promise(caller.data_mut(), promise, PromiseSettlement::Fulfill(response_val)),
                        Err(msg) => reject_with_type_error(&mut caller, promise, &msg),
                    }
                    return promise;
                }
                
                // HTTP/HTTPS — 异步路径
                let result = perform_http_fetch(&mut caller, method, url, headers_handle, body, redirect, signal_handle).await;
                match result {
                    Ok(response_val) => settle_promise(caller.data_mut(), promise, PromiseSettlement::Fulfill(response_val)),
                    Err(msg) => reject_with_type_error(&mut caller, promise, &msg),
                }
                
                promise
            })
        },
    )?;
    
    // ... 其余 constructor 注册保持同步 ...
    Ok(())
}
```

### 6.2 `perform_http_fetch`

```rust
async fn perform_http_fetch(
    caller: &mut Caller<'_, RuntimeState>,
    method: String,
    url: String,
    headers_handle: u32,
    body: Option<Vec<u8>>,
    redirect: RedirectMode,
    signal_handle: Option<u32>,
) -> Result<i64, String> {
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
        // Manual: limited(0) 让 reqwest 不跟随任何重定向，返回 3xx 响应
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
    
    // 6. 检查 abort（请求完成后再检查）
    if let Some(handle) = signal_handle {
        if is_signal_aborted(caller, handle) {
            return Err("The operation was aborted".to_string());
        }
    }
    
    // 7. 提取响应信息
    let status = response.status().as_u16();
    let status_text = response.status().canonical_reason().unwrap_or("").to_string();
    let resp_url = response.url().to_string();
    let redirected = response.url().as_str() != url;
    
    // 8. 提取响应 headers
    let resp_headers = create_empty_headers(caller);
    let mut htable = caller.data().headers_table.lock().expect("headers mutex");
    if let Some(entry) = htable.get_mut(resp_headers as usize) {
        for (key, value) in response.headers() {
            entry.pairs.push((key.as_str().to_ascii_lowercase(), value.to_str().unwrap_or("").to_string()));
        }
    }
    drop(htable);
    
    // 9. 存储 reqwest Response（用于后续流式读取）
    let http_handle = {
        let mut table = caller.data().http_response_table.lock().expect("http_response mutex");
        let handle = table.len() as u32;
        table.push(HttpResponseEntry { response: Some(response) });
        handle
    };
    
    // 10. 构造 Response 对象（body 暂为 null，通过 ReadableStream 懒加载）
    let resp_obj = create_response_object_with_stream(
        caller, status, status_text, resp_headers, resp_url,
        ResponseType::Basic, redirected, http_handle,
    );
    
    Ok(resp_obj)
}
```

### 6.3 ReadableStream 实现

**ReadableStream 对象**：
- 隐藏属性 `__stream_handle__`
- 方法 `getReader()`
- 属性 `locked`（getter）
**`reader.read()` 实现** (通过 NativeCallable::ReaderMethod { kind: Read } 分发，最终在 `func_wrap_async` 中执行)：

关键设计要点：
- **锁顺序**：永远不嵌套持有 reader_table + readable_stream_table 锁。先从 reader_table 读取 stream_handle，drop 锁，再从 readable_stream_table 读取 http_response_handle。
- **take/put_back**：从 http_response_table take 出 reqwest::Response，await chunk 后立即 put_back。await 期间不持有任何 Mutex 锁。
- **Promise 构造**：result 对象为 `{ done: boolean, value: Uint8Array | undefined }` 的 JS 对象。

```rust
async fn reader_read_async(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
) -> i64 {
    let promise = alloc_promise_from_caller(caller, PromiseEntry::pending());
    
    let reader_handle = match get_reader_handle_from_object(caller, this_val) {
        Some(h) => h,
        None => {
            let err = alloc_type_error_from_caller(caller, "Reader not usable");
            settle_promise(caller.data_mut(), promise, PromiseSettlement::Reject(err));
            return promise;
        }
    };
    
    // Step 1: 从 reader_table 获取 stream_handle（立即 drop 锁）
    let stream_handle = {
        let rtable = caller.data().reader_table.lock().expect("reader mutex");
        rtable.get(reader_handle as usize).map(|r| r.stream_handle)
    };
    let Some(stream_handle) = stream_handle else {
        fulfill_reader_done(caller, promise);
        return promise;
    };
    
    // Step 2: 从 readable_stream_table 获取 http_response_handle（立即 drop 锁）
    let http_handle = {
        let stable = caller.data().readable_stream_table.lock().expect("stream mutex");
        stable.get(stream_handle as usize).and_then(|s| s.http_response_handle)
    };
    let Some(http_handle) = http_handle else {
        fulfill_reader_done(caller, promise);
        return promise;
    };
    
    // Step 3: take reqwest Response（立即 drop 锁）
    let mut response = {
        let mut table = caller.data().http_response_table.lock().expect("http_response mutex");
        table.get_mut(http_handle as usize).and_then(|e| e.response.take())
    };
    
    let Some(ref mut resp) = response else {
        fulfill_reader_done(caller, promise);
        return promise;
    };
    
    // Step 4: await chunk（无锁期间，wasmtime 可 yield）
    match resp.chunk().await {
        Ok(Some(chunk)) => {
            // put back response
            let mut table = caller.data().http_response_table.lock().expect("http_response mutex");
            if let Some(entry) = table.get_mut(http_handle as usize) {
                entry.response = response;
            }
            drop(table);
            
            let value = create_uint8array_from_bytes(caller, &chunk);
            fulfill_reader_value(caller, promise, false, value);
        }
        Ok(None) => {
            // body 结束，不放回 response
            fulfill_reader_done(caller, promise);
        }
        Err(e) => {
            let err = alloc_type_error_from_caller(caller, &e.to_string());
            settle_promise(caller.data_mut(), promise, PromiseSettlement::Reject(err));
        }
    }
    
    promise
}
```

### 6.4 Response body 消费方法

`Response.text()`/`json()`/`arrayBuffer()` 改为 async host function：

```rust
// 通过 NativeCallable::ResponseMethod 分发
// ResponseMethodKind::Text → func_wrap_async 中 await 完整 body
async fn response_consume_body(
    caller: &mut Caller<'_, RuntimeState>,
    response_handle: u32,
    kind: ResponseMethodKind,
) -> i64 {
    let promise = alloc_promise_from_caller(caller, PromiseEntry::pending());
    
    // 检查 body_used
    // ...
    
    // 检查是否有 http_response_handle
    let http_handle = get_http_response_handle(caller, response_handle);
    
    if let Some(http_handle) = http_handle {
        // HTTP 响应 — await 完整 body
        let mut response = take_http_response(caller, http_handle);
        if let Some(ref mut resp) = response {
            match resp.bytes().await {
                Ok(bytes) => {
                    // 存储 bytes 到 response entry 的 body 字段
                    store_response_body(caller, response_handle, &bytes);
                    // 根据 kind 构造结果
                    let result = match kind {
                        ResponseMethodKind::Text => store_runtime_string(caller, String::from_utf8_lossy(&bytes).to_string()),
                        ResponseMethodKind::Json => json_parse_to_wasm(caller, ..., value::encode_undefined()),
                        ResponseMethodKind::ArrayBuffer => create_arraybuffer_with_bytes(caller, &bytes),
                        _ => value::encode_undefined(),
                    };
                    settle_promise(caller.data_mut(), promise, PromiseSettlement::Fulfill(result));
                }
                Err(e) => {
                    let err = alloc_type_error_from_caller(caller, &e.to_string());
                    settle_promise(caller.data_mut(), promise, PromiseSettlement::Reject(err));
                }
            }
            // 放回 response（已无 body）
            put_back_http_response(caller, http_handle, response);
        }
    } else {
        // data: URL 或用户构造的 Response — 使用已存储的 body bytes
        // 现有同步逻辑
    }
    
    promise
}
```

### 6.5 AbortController / AbortSignal

**AbortSignal 侧表**：
```rust
struct AbortSignalEntry {
    aborted: bool,
    reason: Option<i64>,  // NaN-boxed JS value
}
```

**AbortController 构造**：
```rust
fn construct_abort_controller(caller: &mut Caller<'_, RuntimeState>, this_val: i64, args: &[i64]) -> Option<i64> {
    let signal_handle = {
        let mut table = caller.data().abort_signal_table.lock().expect("abort_signal mutex");
        let handle = table.len() as u32;
        table.push(AbortSignalEntry { aborted: false, reason: None });
        handle
    };
    let obj = /* construct object */;
    // 创建 signal 子对象
    let signal_obj = create_signal_object(caller, signal_handle);
    define_host_data_property_from_caller(caller, obj, "signal", signal_obj);
    // abort() 方法 — NativeCallable::AbortControllerAbort
    Some(obj)
}
```

**abort() 方法**：
```rust
fn abort_controller_abort(caller: &mut Caller<'_, RuntimeState>, this_val: i64, args: &[i64]) -> Option<i64> {
    let signal_obj = object_property(caller, this_val, "signal")?;
    let handle = get_signal_handle_from_object(caller, signal_obj)?;
    let mut table = caller.data().abort_signal_table.lock().expect("abort_signal mutex");
    if let Some(entry) = table.get_mut(handle as usize) {
        entry.aborted = true;
        entry.reason = args.first().copied();
    }
    // 触发 abort 事件（如果需要）
    Some(value::encode_undefined())
}
```

**在 fetch 中检查 abort**：
- `perform_http_fetch` 开始时检查
- 请求完成后检查
`wjsm-ir/src/builtin.rs` 新增：

```rust
// 在 Builtin enum 中新增（仅构造器需要独立 builtin）
AbortControllerConstructor,
```

ReadableStream/Reader 方法通过 `NativeCallable::StreamMethod` / `NativeCallable::ReaderMethod` 分发，不需要独立 Builtin 枚举。abort() 方法通过 `NativeCallable::AbortControllerAbort` 分发。

### 6.7 Semantic 层变更

`wjsm-semantic` 需要处理新的 host builtin 调用：

- `AbortController` 作为 host builtin 构造器（`Builtin::AbortControllerConstructor`），与 `Headers`/`Request`/`Response` 模式一致
- `ReadableStream.getReader()` / `Reader.read()` / `Reader.releaseLock()` 作为 NativeCallable 方法，在 runtime 分发。这些方法不需要 lowering 层知道——它们是宿主对象上的方法属性，通过 `call_native_callable` 路径执行

### 6.8 Backend 变更

`host_import_registry.rs` 新增对应的 `HostImportSpec` 条目，并在编译时 emit 正确的 import 声明。

## 7. 兼容性边界

| 边界 | 保证 |
|---|---|
| data: URL 行为 | 不变 — 同步处理，Promise 同步 settle |
| 现有 fixture | `.expected` 零变更 |
| API 签名 | `fetch(input, init)` 签名不变，init 对象扩展 |
| Response.text()/json()/arrayBuffer() | 返回 Promise，行为不变 |
| Headers API | 不变 |
| Request 构造器 | 不变（新增 signal 处理） |

## 8. 测试

### 8.1 新增 Fixture

| Fixture | 测试内容 |
|---|---|
| `fetch_http_get.js` | `fetch("https://httpbin.org/get").then(r => r.json())` |
| `fetch_http_post.js` | POST with string body |
| `fetch_redirect_follow.js` | 重定向跟随 |
| `fetch_redirect_manual.js` | manual 重定向返回 opaqueredirect |
| `fetch_abort.js` | AbortController abort |
| `fetch_stream_body.js` | ReadableStream + reader.read() 分块读取 |
| `abort_controller.js` | AbortController/AbortSignal API |
| `fetch_response_headers.js` | 响应头读取 |
| `fetch_request_init.js` | Request init (method, headers, body, redirect) |

### 8.2 集成测试

`crates/wjsm-runtime/tests/fetch_http.rs` — 使用 `httpbin.org` 或本地 mock server 测试。

### 8.3 回归测试

全 workspace `cargo nextest run --workspace` 必须通过。

## 9. 实现阶段

| 阶段 | 内容 | 依赖 |
|---|---|---|
| Phase 1 | 依赖引入 + 文件拆分 + data: URL 路径保持 | 无 |
| Phase 2 | HTTP fetch 核心路径 (GET/POST, 响应头构造, Promise settle) | Phase 1 |
| Phase 3 | Response body 消费 (text/json/arrayBuffer async) | Phase 2 |
| Phase 4 | ReadableStream + 流式 body (getReader/read) | Phase 2 |
| Phase 5 | AbortController/AbortSignal | Phase 2 |
| Phase 6 | 重定向 follow/error/manual | Phase 2 |
| Phase 7 | Semantic + Backend 对接 (新 builtin + host import) | Phase 2-6 |
| Phase 8 | 测试 + Fixture + 回归验证 | Phase 7 |

| 风险 | 缓解 |
|---|---|
| reqwest 依赖体积 | `rustls-tls` 比 `native-tls` 更小，且无系统依赖 |
| 流式 body 的 reqwest Response 生命周期 | `http_response_table` 持有 `Option<reqwest::Response>`，take/put_back 模式，await 期间不持锁 |
| fixture 依赖外部 HTTP 服务 | 可使用 `httpbin.org` 或本地 tiny HTTP server；标记为 network-dependent |
| epoch yielding 与长时间请求 | wasmtime 自动 yield，不影响其他逻辑 |
| Manual redirect 响应类型 | `limited(0)` 返回 3xx 原始响应，需标记 `response_type: OpaqueRedirect` |

## 11. ADR 信号

- **新依赖引入**：reqwest 作为 workspace 级依赖，需确认 license (MIT/Apache-2.0，兼容)
- **fetch host function 改为 async**：影响 ABI 签名（从 `Func::wrap` 到 `func_wrap_async`），需同步更新 backend 的 import 类型
- **ReadableStream 侧表模式**：如果未来实现完整 Streams Standard，侧表可能需要重构
