# WHATWG Streams Standard 完整实现设计规格

**日期**: 2026-06-03
**状态**: Draft
**范围**: wjsm-runtime 完整 WHATWG Streams Standard（ReadableStream + WritableStream + TransformStream + pipeTo + pipeThrough + BYOB reader + QueuingStrategy + Symbol.asyncIterator）

---

## 1. 背景与动机

当前 wjsm 的 ReadableStream 是**仅用于 fetch HTTP body 消费的内部适配层**：
- 已有：`getReader()` / `reader.read()` / `reader.releaseLock()` / `stream.cancel()`
- 已有：侧表 `readable_stream_table` + `reader_table` + `http_response_table` + `AbortSignal`
- 已有：异步 host function 基础设施（`func_wrap_async` + `tokio::spawn` + `Materialize` 闭包）
- 缺失：`new ReadableStream(underlyingSource)` 构造函数、DefaultController、QueuingStrategy、tee、pipeTo、WritableStream、TransformStream、BYOB reader、Symbol.asyncIterator

**问题**：
1. `Response.body` 对 HTTP 响应仅返回有 `getReader()`/`cancel()` 的适配对象，但 `locked` 是静态属性（不随 `getReader()` 变化），`bodyUsed` 在 `getReader()` 时不更新
2. `data:` URL 的 `Response.body` 为 `null`（应为 ReadableStream）
3. `Response.clone()` body 是静态复制（应共享底层流）
4. JS 代码无法创建自定义 ReadableStream（无构造函数）
5. 无 WritableStream、TransformStream、pipeTo

---

## 2. 目标

实现符合 [WHATWG Streams Standard](https://streams.spec.whatwg.org/) 的完整实现，包括：

### 2.1 ReadableStream

| 接口/方法 | 规范章节 | 状态 |
|---|---|---|
| `new ReadableStream(underlyingSource, strategy)` | §4.2.1 | 新增 |
| `ReadableStream.locked` getter | §4.2.2 | 修复（当前静态属性） |
| `ReadableStream.cancel(reason)` | §4.2.3 | 已有（强化：调用 underlyingSource.cancel） |
| `ReadableStream.getReader()` | §4.2.4 | 已有（强化：locked 状态、bodyUsed 更新） |
| `ReadableStream.tee()` | §4.2.5 | 新增 |
| `ReadableStream[Symbol.asyncIterator]()` | §4.3.1 | 新增 |
| `ReadableStreamDefaultReader.read()` | §4.5.4 | 已有（强化：controller.desiredSize 检查） |
| `ReadableStreamDefaultReader.releaseLock()` | §4.5.5 | 已有 |
| `ReadableStreamDefaultReader.closed` | §4.5.6 | 新增 |
| `ReadableStreamDefaultController.enqueue(chunk)` | §4.6.1 | 新增 |
| `ReadableStreamDefaultController.close()` | §4.6.2 | 新增 |
| `ReadableStreamDefaultController.error(e)` | §4.6.3 | 新增 |
| `ReadableStreamDefaultController.desiredSize` | §4.6.4 | 新增 |
| `ReadableStreamDefaultController.cancel(reason)` | §4.6.5 | 已有（强化） |

### 2.2 WritableStream

| 接口/方法 | 规范章节 | 状态 |
|---|---|---|
| `new WritableStream(underlyingSink, strategy)` | §5.2.1 | 新增 |
| `WritableStream.locked` getter | §5.2.2 | 新增 |
| `WritableStream.abort(reason)` | §5.2.3 | 新增 |
| `WritableStream.getWriter()` | §5.2.4 | 新增 |
| `WritableStreamDefaultWriter.write(chunk)` | §5.5.4 | 新增 |
| `WritableStreamDefaultWriter.close()` | §5.5.5 | 新增 |
| `WritableStreamDefaultWriter.abort(reason)` | §5.5.6 | 新增 |
| `WritableStreamDefaultWriter.releaseLock()` | §5.5.7 | 新增 |
| `WritableStreamDefaultWriter.closed` | §5.5.8 | 新增 |
| `WritableStreamDefaultWriter.desiredSize` | §5.5.9 | 新增 |
| `WritableStreamDefaultWriter.ready` | §5.5.10 | 新增 |
| `WritableStreamDefaultController.error(e)` | §5.6.1 | 新增 |
| `WritableStreamDefaultController.signal` | §5.6.2 | 新增 |

### 2.3 TransformStream

| 接口/方法 | 规范章节 | 状态 |
|---|---|---|
| `new TransformStream(transformer, writableStrategy, readableStrategy)` | §6.2.1 | 新增 |
| `TransformStream.readable` | §6.2.2 | 新增 |
| `TransformStream.writable` | §6.2.3 | 新增 |
| `TransformStreamDefaultController.enqueue(chunk)` | §6.4.1 | 新增 |
| `TransformStreamDefaultController.error(e)` | §6.4.2 | 新增 |
| `TransformStreamDefaultController.terminate()` | §6.4.3 | 新增 |

### 2.4 pipeTo / pipeThrough

| 接口/方法 | 规范章节 | 状态 |
|---|---|---|
| `ReadableStream.prototype.pipeTo(destination, options)` | §7.1 | 新增 |
| `ReadableStream.prototype.pipeThrough(transform, options)` | §7.2 | 新增 |

### 2.5 Queuing Strategy

| 接口/方法 | 规范章节 | 状态 |
|---|---|---|
| `CountQueuingStrategy(highWaterMark)` | §8.1 | 新增 |
| `CountQueuingStrategy.size` | §8.1 | 新增 |
| `ByteLengthQueuingStrategy(highWaterMark)` | §8.2 | 新增 |
| `ByteLengthQueuingStrategy.size` | §8.2 | 新增 |

### 2.6 BYOB Reader

| 接口/方法 | 规范章节 | 状态 |
|---|---|---|
| `ReadableStream.getReader({ mode: 'byob' })` | §4.2.4 | 新增 |
| `ReadableStreamBYOBReader.read(view)` | §4.7.4 | 新增 |
| `ReadableStreamBYOBReader.releaseLock()` | §4.7.5 | 新增 |
| `ReadableStreamBYOBReader.closed` | §4.7.6 | 新增 |
| `ReadableStreamByteStreamController.byobRequest` | §4.8 | 新增 |
| `ReadableStreamByteStreamController.desiredSize` | §4.8 | 新增 |
| `ReadableStreamByteStreamController.close()` | §4.8 | 新增 |
| `ReadableStreamByteStreamController.enqueue(chunk)` | §4.8 | 新增 |
| `ReadableStreamByteStreamController.error(e)` | §4.8 | 新增 |

### 2.7 fetch 集成修复

| 功能 | 状态 |
|---|---|
| `data:` URL `Response.body` 返回 ReadableStream | 新增 |
| `Response.clone()` 共享底层流 | 修复 |
| `bodyUsed` 在 `getReader()` 时更新 | 修复 |
| `locked` 在 `getReader()` 时变为 `true` | 修复 |
| `Response.body.cancel()` 正确取消 HTTP 请求 | 修复 |

---

## 3. 非目标

- **ReadableStream 的 `forEach` 方法** — 不是 Streams Standard 的一部分
- **Service Worker 拦截** — 非浏览器环境
- **Transferable Streams** — 无 Worker 环境
- **ReadableStream.from(iterable)** — 非标准（WHATWG 提案阶段）
- **WritableStream 的 `close()` 在 `write()` 未完成时自动排队** — 简化实现，先排 queue

---

## 4. 架构

### 4.1 核心决策

#### 4.1.1 侧表 + NativeCallable + async host function

与现有 fetch 一致：

```
JS: new ReadableStream({ start(controller) { ... } })
  → WASM call native_callable (constructor dispatch)
  → host function 中执行 JS 回调 start(controller)
  → controller.enqueue(chunk) 通过 NativeCallable 分发
  → reader.read() 通过 NativeCallable 分发到 async host function
  → await Promise settle
```

**关键问题**：`controller.enqueue()` 和 `controller.close()` 需要**在 JS 回调中调用** — 这意味着 `controller` 对象必须是一个有效的 JS 可调用对象，且 `enqueue` 方法是 `NativeCallable` 类型。这与现有 `fetch` 的 `getReader()` 路径一致。

#### 4.1.2 Accessor Property 支持（解决 getter 限制）

**问题**：现有 `define_host_data_property_from_caller` 只写 data property，导致 `locked`/`desiredSize`/`ready`/`closed` 等规范要求 getter 的属性只能作为方法暴露（与 Map.size 相同的妥协）。

**发现**：底层架构**已支持** accessor property：
- 属性 slot 布局（32 字节）已预留 getter（offset 16）和 setter（offset 24）空间
- `$obj_get` WASM helper 已有 accessor 分支 — 检测 `FLAG_IS_ACCESSOR`，加载 getter，通过 `call_indirect` type 12 调用
- `$obj_set` WASM helper 已有 accessor setter 分支
- `obj_define_property` host function 已支持完整的 accessor descriptor
- NativeCallable 可通过 `resolve_callable_for_helper` + `call_indirect` 被调用

**方案**：新增 `define_host_accessor_property_from_caller(caller, obj, name, getter, setter)` 函数：
```rust
pub(crate) fn define_host_accessor_property_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
    name: &str,
    getter: i64,  // NativeCallable idx（NaN-boxed）
    setter: i64,  // NativeCallable idx 或 undefined
) -> Option<()> {
    // 与 define_host_data_property_with_env 相同的 slot 分配逻辑
    // 但写入：
    // flags = FLAG_CONFIGURABLE | FLAG_ENUMERABLE | FLAG_IS_ACCESSOR
    // offset 8  = undefined（value unused for accessor）
    // offset 16 = getter
    // offset 24 = setter
}
```

**效果**：
- `locked`/`desiredSize`/`ready`/`closed`/`signal` 作为真正的 getter — 读取 `stream.locked` 时自动调用 NativeCallable
- 连带修复 Map.size/Set.size 的 spec 合规性（当前为方法，改为 getter）


### 4.2 侧表扩展

当前 RuntimeState 已有：
- `readable_stream_table: Vec<ReadableStreamEntry>` — 新增 `controller_handle` 和 `is_byte_stream` 字段
- `reader_table: Vec<ReaderEntry>` — 新增 `pending_read_promise: Option<i64>` 字段（用于存储等待 controller.enqueue 的 read Promise）
- `http_response_table: Vec<HttpResponseEntry>`

新增：

```rust
/// WritableStream 侧表
writable_stream_table: Arc<Mutex<Vec<WritableStreamEntry>>>,
/// Writer 侧表
writer_table: Arc<Mutex<Vec<WriterEntry>>>,
/// TransformStream 侧表
transform_stream_table: Arc<Mutex<Vec<TransformStreamEntry>>>,
/// Controller 侧表（DefaultController / ByteStreamController / WritableController / TransformController）
/// chunk_queue 直接存储在 StreamControllerEntry 中，无需单独的 chunk 侧表
stream_controller_table: Arc<Mutex<Vec<StreamControllerEntry>>>,
```

```rust
struct ReadableStreamEntry {
    state: StreamState,          // Readable / Closed / Errored
    error: Option<String>,
    disturbed: bool,
    locked: bool,
    http_response_handle: Option<u32>,
    // 新增：自定义流字段
    controller_handle: Option<u32>,   // 关联的 controller
    is_byte_stream: bool,              // BYOB reader 支持
}

struct WritableStreamEntry {
    state: WritableStreamState,  // Writable / WritableClosed / WritableErrored / WritableClosing
    error: Option<String>,
    locked: bool,
    controller_handle: Option<u32>,
    abort_signal: Option<i64>,   // NaN-boxed AbortSignal JS 对象（用于 controller.signal getter）
}

struct WriterEntry {
    stream_handle: u32,
    closed_promise: i64,         // JS Promise 对象
    ready_promise: i64,          // JS Promise 对象
    pending_writes: Vec<PendingWrite>, // 队列中的 write
}

struct TransformStreamEntry {
    readable_stream_handle: u32,
    writable_stream_handle: u32,
    controller_handle: Option<u32>,
}

struct StreamControllerEntry {
    kind: ControllerKind,        // Default / ByteStream / Writable / Transform
    stream_handle: u32,
    // Default controller
    chunk_queue: VecDeque<i64>,     // NaN-boxed JS values（VecDeque 适合前端 pop）
    strategy_size: Option<i64>,  // NaN-boxed JS function
    started: bool,
    close_requested: bool,
    // Byte stream controller
    byob_reader_handle: Option<u32>,
    pull_requested: bool,
    // Writable controller
    abort_requested: bool,
    abort_reason: Option<i64>,
    // Transform controller
    flush_requested: bool,
}

enum ControllerKind {
    ReadableDefault,
    ReadableByteStream,
    Writable,
    Transform,
}
```

### 4.3 NativeCallable 扩展

新增：

```rust
enum NativeCallable {
    // 已有...
    // ReadableStream
    ReadableStreamConstructor,
    ReadableStreamMethod { handle: u32, kind: ReadableStreamMethodKind },
    // Reader
    ReadableStreamDefaultReaderConstructor,
    ReadableStreamDefaultReaderMethod { handle: u32, kind: ReadableStreamDefaultReaderMethodKind },
    ReadableStreamBYOBReaderConstructor,
    ReadableStreamBYOBReaderMethod { handle: u32, kind: ReadableStreamBYOBReaderMethodKind },
    // Controller
    ReadableStreamDefaultControllerMethod { handle: u32, kind: ReadableStreamDefaultControllerMethodKind },
    ReadableStreamByteStreamControllerMethod { handle: u32, kind: ReadableStreamByteStreamControllerMethodKind },
    // WritableStream
    WritableStreamConstructor,
    WritableStreamMethod { handle: u32, kind: WritableStreamMethodKind },
    WritableStreamDefaultWriterConstructor,
    WritableStreamDefaultWriterMethod { handle: u32, kind: WritableStreamDefaultWriterMethodKind },
    WritableStreamDefaultControllerMethod { handle: u32, kind: WritableStreamDefaultControllerMethodKind },
    // TransformStream
    TransformStreamConstructor,
    TransformStreamMethod { handle: u32, kind: TransformStreamMethodKind },
    TransformStreamDefaultControllerMethod { handle: u32, kind: TransformStreamDefaultControllerMethodKind },
    // Queuing Strategy
    CountQueuingStrategyConstructor,
    ByteLengthQueuingStrategyConstructor,
    // pipeTo
    ReadableStreamPipeTo,
}
```

### 4.4 文件拆分

当前：`fetch_core.rs` (1645 行) + `fetch_http.rs` (121 行)

拆分后：

| 文件 | 职责 | 预估行数 |
|---|---|---|
| `fetch_core.rs` | Headers/Request/Response 构造、方法、属性、headers 验证 | ~800 |
| `fetch_http.rs` | HTTP 请求执行、重定向、AbortSignal | ~200 |
| `streams_readable.rs` | ReadableStream + DefaultReader + DefaultController + ByteStreamController + BYOBReader + QueuingStrategy | ~1200 |
| `streams_writable.rs` | WritableStream + DefaultWriter + DefaultController | ~600 |
| `streams_transform.rs` | TransformStream + DefaultController + pipeTo + pipeThrough | ~600 |
| `streams_queuing.rs` | CountQueuingStrategy + ByteLengthQueuingStrategy | ~100 |
| `streams_fetch.rs` | fetch 与 streams 集成（Response.body 修复、clone body 共享） | ~200 |

### 4.5 Semantic + Backend 变更

**wjsm-ir/src/builtin.rs** 新增：

```rust
ReadableStreamConstructor,
WritableStreamConstructor,
TransformStreamConstructor,
CountQueuingStrategyConstructor,
ByteLengthQueuingStrategyConstructor,
```

**wjsm-semantic/src/builtins.rs** 新增 builtin 解析：

```rust
"ReadableStream" => Some(Builtin::ReadableStreamConstructor),
"WritableStream" => Some(Builtin::WritableStreamConstructor),
"TransformStream" => Some(Builtin::TransformStreamConstructor),
"CountQueuingStrategy" => Some(Builtin::CountQueuingStrategyConstructor),
"ByteLengthQueuingStrategy" => Some(Builtin::ByteLengthQueuingStrategyConstructor),
```

**wjsm-backend-wasm/src/host_import_registry.rs** 新增 host import specs：

```rust
HostImportSpec {
    name: "readable_stream_constructor",
    // 签名: (env, this, args_base, args_count) -> i64
},
HostImportSpec {
    name: "writable_stream_constructor",
},
HostImportSpec {
    name: "transform_stream_constructor",
},
HostImportSpec {
    name: "count_queuing_strategy_constructor",
},
HostImportSpec {
    name: "byte_length_queuing_strategy_constructor",
},
```

---

## 5. 详细设计

### 5.1 ReadableStream 构造函数

```rust
fn construct_readable_stream(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    args: &[i64],
) -> Option<i64> {
    // 1. 解析 underlyingSource
    let source_obj = args.first().filter(|v| value::is_object(**v)).copied();
    let strategy_obj = args.get(1).filter(|v| value::is_object(**v)).copied();
    
    // 2. 创建 controller
    let controller_handle = {
        let mut table = caller.data().stream_controller_table.lock().expect("mutex");
        let handle = table.len() as u32;
        table.push(StreamControllerEntry {
            kind: ControllerKind::ReadableDefault,
            stream_handle: 0, // 占位
            chunk_queue: Vec::new(),
            high_water_mark: 1.0,
            strategy_size: None,
            started: false,
            close_requested: false,
            byob_reader_handle: None,
            pull_requested: false,
            abort_requested: false,
            abort_reason: None,
            flush_requested: false,
        });
        handle
    };
    
    // 3. 创建 stream
    let stream_handle = {
        let mut table = caller.data().readable_stream_table.lock().expect("stream mutex");
        let handle = table.len() as u32;
        table.push(ReadableStreamEntry {
            state: StreamState::Readable,
            error: None,
            disturbed: false,
            locked: false,
            http_response_handle: None,
            controller_handle: Some(controller_handle),
            is_byte_stream: false,
        });
        handle
    };
    
    // 4. 回写 stream_handle 到 controller
    {
        let mut table = caller.data().stream_controller_table.lock().expect("mutex");
        if let Some(entry) = table.get_mut(controller_handle as usize) {
            entry.stream_handle = stream_handle;
        }
    }
    
    // 5. 解析 strategy
    if let Some(strategy) = strategy_obj {
        let hwm_raw = read_object_property_by_name(caller, strategy, "highWaterMark");
        if let Some(raw) = hwm_raw {
            let hwm = value::decode_f64(raw);
            if !hwm.is_nan() && hwm.is_finite() {
                let mut table = caller.data().stream_controller_table.lock().expect("mutex");
                if let Some(entry) = table.get_mut(controller_handle as usize) {
                    entry.high_water_mark = hwm;
                }
            }
        }
        let size_fn = read_object_property_by_name(caller, strategy, "size");
        if let Some(size) = size_fn {
            if value::is_object(size) || value::is_native_callable_idx(size) {
                let mut table = caller.data().stream_controller_table.lock().expect("mutex");
                if let Some(entry) = table.get_mut(controller_handle as usize) {
                    entry.strategy_size = Some(size);
                }
            }
        }
    }
    
    // 6. 调用 underlyingSource.start(controller)
    if let Some(source) = source_obj {
        let start_fn = read_object_property_by_name(caller, source, "start");
        if let Some(start) = start_fn {
            // 创建 controller JS 对象（带 enqueue/close/error 方法的 NativeCallable 对象）
            let controller_obj = create_controller_object(caller, controller_handle);
            // 调用 start(controller) — 使用 call_wasm_callback 同步调用 JS 函数
            let _ = call_wasm_callback(caller, start, value::encode_undefined(), &[controller_obj]);
        }
    }
    
    // 7. 标记 started
    {
        let mut table = caller.data().stream_controller_table.lock().expect("mutex");
        if let Some(entry) = table.get_mut(controller_handle as usize) {
            entry.started = true;
        }
    }
    
    // 8. 构造 ReadableStream JS 对象
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let obj = alloc_host_object(caller, &env, 8);
    let handle_val = value::encode_f64(stream_handle as f64);
    let _ = define_host_data_property_from_caller(caller, obj, "__stream_handle__", handle_val);
    
    // 9. 设置 locked getter（真正的 accessor property，而非 data property 方法）
    let locked_getter = NativeCallable::ReadableStreamMethod {
        handle: stream_handle,
        kind: ReadableStreamMethodKind::GetLocked,
    };
    let idx = push_native_callable(caller, locked_getter);
    let getter_val = value::encode_native_callable_idx(idx);
    let _ = define_host_accessor_property_from_caller(caller, obj, "locked", getter_val, value::encode_undefined());
    
    // 10. 设置方法
    let methods = &[
        ("getReader", ReadableStreamMethodKind::GetReader),
        ("cancel", ReadableStreamMethodKind::Cancel),
        ("tee", ReadableStreamMethodKind::Tee),
    ];
    for (name, kind) in methods {
        let callable = NativeCallable::ReadableStreamMethod {
            handle: stream_handle,
            kind: *kind,
        };
        let idx = push_native_callable(caller, callable);
        let val = value::encode_native_callable_idx(idx);
        let _ = define_host_data_property_from_caller(caller, obj, name, val);
    }
    
    // 11. Symbol.asyncIterator — 使用 "Symbol.asyncIterator" 字符串属性名（与现有 iterable 实现一致）
    let async_iter_callable = NativeCallable::ReadableStreamMethod {
        handle: stream_handle,
        kind: ReadableStreamMethodKind::AsyncIterator,
    };
    let idx = push_native_callable(caller, async_iter_callable);
    let val = value::encode_native_callable_idx(idx);
    let _ = define_host_data_property_from_caller(caller, obj, "Symbol.asyncIterator", val);
    Some(obj)
}
```

**`create_controller_object`**：创建 controller 的 JS 对象，包含 `enqueue`/`close`/`error` 方法（通过 `NativeCallable::ReadableStreamDefaultControllerMethod` 分发），以及 `desiredSize` 属性（getter 通过 `NativeCallable` 分发）。与 `create_headers_object_from_handle` 模式一致。

### 5.2 Controller.enqueue(chunk)

```rust
fn controller_enqueue(
    caller: &mut Caller<'_, RuntimeState>,
    controller_handle: u32,
    chunk: i64,
) -> Option<i64> {
    let mut table = caller.data().stream_controller_table.lock().expect("mutex");
    let entry = table.get_mut(controller_handle as usize)?;
    
    if entry.close_requested {
        // TypeError: stream is already closed
        return Some(type_error_exception(caller, "Cannot enqueue after close"));
    }
    
    // 推入 chunk 队列
    entry.chunk_queue.push(chunk);
    
    // 检查是否有等待的 reader
    drop(table);
    
    // 尝试 resolve reader.read() 的 pending promise
    let stream_handle = {
        let table = caller.data().stream_controller_table.lock().expect("mutex");
        table.get(controller_handle as usize)?.stream_handle
    };
    
    // 检查是否有 reader 在等待 read — 遍历 reader_table 找到 stream_handle 匹配的 reader
    let pending_promise = {
        let reader_table = caller.data().reader_table.lock().expect("reader mutex");
        reader_table.iter()
            .find(|r| r.stream_handle == stream_handle)
            .and_then(|r| r.pending_read_promise)
    };
    
    if let Some(promise) = pending_promise {
        // 从 chunk_queue 取出刚推入的 chunk（或队首 chunk）
        let chunk = {
            let mut table = caller.data().stream_controller_table.lock().expect("mutex");
            table.get_mut(controller_handle as usize)
                .and_then(|e| e.chunk_queue.pop_front())
        };
        let result = build_reader_result(caller, false, chunk);
        settle_promise(caller.data_mut(), promise, PromiseSettlement::Fulfill(result));
        // 清除 pending_read_promise
        {
            let mut reader_table = caller.data().reader_table.lock().expect("reader mutex");
            if let Some(entry) = reader_table.iter_mut().find(|r| r.stream_handle == stream_handle) {
                entry.pending_read_promise = None;
            }
        }
    }
    
    Some(value::encode_undefined())
}
```

### 5.3 reader.read() 改进

现有 `reader.read()` 仅支持 HTTP body chunk（从 `http_response_table` 读取）。需要扩展为：

```rust
async fn reader_read_async(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
) -> i64 {
    let promise = alloc_promise_from_caller(caller, PromiseEntry::pending());
    
    let reader_handle = get_reader_handle_from_object(caller, this_val);
    let stream_handle = get_stream_handle_from_reader(caller, reader_handle);
    
    // 1. 检查是否有自定义流的 chunk
    let maybe_chunk = {
        let mut table = caller.data().stream_controller_table.lock().expect("mutex");
        let stream_entry = caller.data().readable_stream_table.lock().expect("stream mutex")
            .get(stream_handle as usize).cloned();
        if let Some(stream) = stream_entry {
            if let Some(ctrl_handle) = stream.controller_handle {
                if let Some(ctrl) = table.get_mut(ctrl_handle as usize) {
                    if !ctrl.chunk_queue.is_empty() {
                        ctrl.chunk_queue.pop_front()
                    } else {
                        None
                    }
                } else { None }
            } else { None }
        } else { None }
    };
    
    if let Some(chunk) = maybe_chunk {
        let result = build_reader_result(caller, false, Some(chunk));
        settle_promise(caller.data_mut(), promise, PromiseSettlement::Fulfill(result));
        return promise;
    }
    
    // 2. 检查是否 HTTP 流
    let http_handle = get_http_response_handle(caller, stream_handle);
    if let Some(http) = http_handle {
        // 现有 HTTP 路径（Materialize 模式）
        return reader_read_http_async(caller, http, promise);
    }
    
    // 3. 检查是否已关闭
    let is_closed = {
        let table = caller.data().readable_stream_table.lock().expect("stream mutex");
        let entry = table.get(stream_handle as usize);
        matches!(entry.map(|e| e.state), Some(StreamState::Closed))
    };
    
    if is_closed {
        let result = build_reader_result(caller, true, None);
        settle_promise(caller.data_mut(), promise, PromiseSettlement::Fulfill(result));
        return promise;
    }
    
    // 4. 否则 pending，等待 controller.enqueue
    // 存储 pending promise 到 reader entry
    {
        let mut table = caller.data().reader_table.lock().expect("reader mutex");
        if let Some(entry) = table.get_mut(reader_handle as usize) {
            entry.pending_read_promise = Some(promise);
        }
    }
    
    promise
}
```

### 5.4 tee()

```rust
fn readable_stream_tee(
    caller: &mut Caller<'_, RuntimeState>,
    stream_handle: u32,
) -> Option<i64> {
    let stream = {
        let table = caller.data().readable_stream_table.lock().expect("stream mutex");
        table.get(stream_handle as usize)?.clone()
    };
    
    // 创建两个新 controller
    let ctrl1 = create_controller(caller, stream_handle);
    let ctrl2 = create_controller(caller, stream_handle);
    
    // 创建两个新 stream
    let stream1 = create_readable_stream(caller, Some(ctrl1), false);
    let stream2 = create_readable_stream(caller, Some(ctrl2), false);
    
    // 如果原始流有 controller，clone chunk queue
    if let Some(ctrl_handle) = stream.controller_handle {
        let chunks = {
            let table = caller.data().stream_controller_table.lock().expect("mutex");
            table.get(ctrl_handle as usize)?.chunk_queue.clone()
        };
        {
            let mut table = caller.data().stream_controller_table.lock().expect("mutex");
            if let Some(ctrl1) = table.get_mut(ctrl1 as usize) {
                ctrl1.chunk_queue = chunks.clone();
            }
            if let Some(ctrl2) = table.get_mut(ctrl2 as usize) {
                ctrl2.chunk_queue = chunks.clone();
            }
        }
    }
    
    // 返回 [stream1, stream2]
    let arr = alloc_array(caller, 2);
    let _ = set_array_element(caller, arr, 0, stream1);
    let _ = set_array_element(caller, arr, 1, stream2);
    Some(arr)
}
```

### 5.5 WritableStream

```rust
fn construct_writable_stream(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    args: &[i64],
) -> Option<i64> {
    let sink_obj = args.first().filter(|v| value::is_object(**v)).copied();
    let strategy_obj = args.get(1).filter(|v| value::is_object(**v)).copied();
    
    // 创建 controller
    let controller_handle = {
        let mut table = caller.data().stream_controller_table.lock().expect("mutex");
        let handle = table.len() as u32;
        table.push(StreamControllerEntry {
            kind: ControllerKind::Writable,
            stream_handle: 0,
            chunk_queue: Vec::new(),
            high_water_mark: 1.0,
            strategy_size: None,
            started: false,
            close_requested: false,
            byob_reader_handle: None,
            pull_requested: false,
            abort_requested: false,
            abort_reason: None,
            flush_requested: false,
        });
        handle
    };
    
    // 创建 stream
    let stream_handle = {
        let mut table = caller.data().writable_stream_table.lock().expect("mutex");
        let handle = table.len() as u32;
        table.push(WritableStreamEntry {
            state: WritableStreamState::Writable,
            error: None,
            locked: false,
            controller_handle: Some(controller_handle),
        });
        handle
    };
    
    // 回写
    {
        let mut table = caller.data().stream_controller_table.lock().expect("mutex");
        if let Some(entry) = table.get_mut(controller_handle as usize) {
            entry.stream_handle = stream_handle;
        }
    }
    
    if let Some(sink) = sink_obj {
        let start_fn = read_object_property_by_name(caller, sink, "start");
        if let Some(start) = start_fn {
            // 创建 controller JS 对象（带 error/signal 方法的 NativeCallable 对象）
            let controller_obj = create_controller_object(caller, controller_handle);
            let _ = call_wasm_callback(caller, start, value::encode_undefined(), &[controller_obj]);
        }
    }
    
    // 构造 JS 对象
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let obj = alloc_host_object(caller, &env, 8);
    let handle_val = value::encode_f64(stream_handle as f64);
    let _ = define_host_data_property_from_caller(caller, obj, "__writable_stream_handle__", handle_val);
    
    // 方法
    let methods = &[
        ("getWriter", WritableStreamMethodKind::GetWriter),
        ("abort", WritableStreamMethodKind::Abort),
    ];
    for (name, kind) in methods {
        let callable = NativeCallable::WritableStreamMethod { handle: stream_handle, kind: *kind };
        let idx = push_native_callable(caller, callable);
        let val = value::encode_native_callable_idx(idx);
        let _ = define_host_data_property_from_caller(caller, obj, name, val);
    }
    
    Some(obj)
}
```

### 5.6 pipeTo

```rust
async fn readable_stream_pipe_to(
    caller: &mut Caller<'_, RuntimeState>,
    source_handle: u32,
    dest_obj: i64,
    _options: i64,
) -> i64 {
    let promise = alloc_promise_from_caller(caller, PromiseEntry::pending());
    
    // 获取 dest 的 writer
    let dest_handle = get_writable_stream_handle_from_object(caller, dest_obj);
    let writer = get_writer_or_create(caller, dest_handle);
    
    // 获取 source 的 reader
    let reader = get_reader_or_create(caller, source_handle);
    
    // 循环：reader.read() -> writer.write(chunk)
    loop {
        let read_result = reader_read_async(caller, reader).await;
        let done = get_done_from_reader_result(caller, read_result);
        let value = get_value_from_reader_result(caller, read_result);
        
        if done {
            let _ = writer_close_async(caller, writer).await;
            settle_promise(caller.data_mut(), promise, PromiseSettlement::Fulfill(value::encode_undefined()));
            break;
        }
        
        let _ = writer_write_async(caller, writer, value).await;
    }
    
    promise
}
```

### 5.7 fetch body 修复

**data: URL Response.body**：

```rust
fn create_data_url_response_with_body(caller, status, body, url) {
    // 1. 创建自定义 controller（关闭状态，所有 bytes 已在 queue 中）
    let ctrl = create_controller(caller, 0);
    let chunk = create_uint8array_from_bytes(caller, &body);
    controller_enqueue(caller, ctrl, chunk);
    controller_close(caller, ctrl);
    // 2. 创建 stream
    let stream_obj = create_readable_stream_with_controller(caller, Some(ctrl), false);
    // 3. 设置 Response.body = stream_obj（替换原来的 null）
    let _ = set_host_data_property_from_caller(caller, resp_obj, "body", stream_obj);
}
```

**Response.clone() 共享 body**：

```rust
fn response_clone_with_shared_body(caller, handle) {
    let stream_handle = get_stream_handle_from_response(caller, handle);
    let new_stream = clone_stream_with_shared_controller(caller, stream_handle);
    // 两个 Response 共享同一个底层 stream
}
```

---

## 6. 兼容性边界

| 边界 | 保证 |
|---|---|
| 现有 fetch fixture `.expected` 零变更 | 无回归 |
| `data:` URL 行为不变 | 同步处理，Promise 同步 settle；仅 body 从 null 变为 ReadableStream |
| 现有 Response.text()/json()/arrayBuffer() | 行为不变，路径不变 |
| 现有 NativeCallable dispatch 路径 | 不变，新增变体 |
| 现有 `AbortController` | 不变 |

---

## 7. 测试

### 7.1 新增 Fixture

| Fixture | 测试内容 |
|---|---|
| `streams_readable_constructor.js` | `new ReadableStream({ start, pull, cancel })` |
| `streams_readable_enqueue_close.js` | `controller.enqueue()` / `controller.close()` |
| `streams_readable_locked.js` | `locked` getter 正确性 |
| `streams_readable_tee.js` | `tee()` 两个分支 |
| `streams_readable_async_iter.js` | `for await...of` |
| `streams_readable_byob.js` | `getReader({ mode: 'byob' })` |
| `streams_writable_constructor.js` | `new WritableStream({ start, write, close, abort })` |
| `streams_writable_writer.js` | `getWriter().write() / close() / abort()` |
| `streams_writable_locked.js` | `locked` 正确性 |
| `streams_transform.js` | `new TransformStream({ transform, flush })` |
| `streams_pipe_to.js` | `stream.pipeTo(writable)` |
| `streams_pipe_through.js` | `stream.pipeThrough(transform)` |
| `streams_queuing_strategy.js` | `CountQueuingStrategy` / `ByteLengthQueuingStrategy` |
| `streams_fetch_body_data_url.js` | `data:` URL Response.body 为 ReadableStream |
| `streams_fetch_clone_shared.js` | `Response.clone()` 共享 body stream |
| `streams_fetch_body_used.js` | `bodyUsed` 在 `getReader()` 时更新 |

### 7.2 集成测试

- `crates/wjsm-runtime/tests/streams.rs` — 单元测试 controller queue 管理、tee 逻辑、pipe 逻辑

### 7.3 回归测试

`cargo nextest run --workspace` 必须通过。

---

## 8. 实现阶段

| 阶段 | 内容 | 依赖 | 验证 |
|---|---|---|---|
| Phase 1 | ReadableStream 构造函数 + DefaultController + DefaultReader + locked + cancel + fetch body 修复 | 无 | 6 fixtures |
| Phase 2 | tee() + Symbol.asyncIterator + QueuingStrategy | Phase 1 | 4 fixtures |
| Phase 3 | BYOB reader + ByteStreamController | Phase 1 | 2 fixtures |
| Phase 4 | WritableStream + DefaultWriter + DefaultController | Phase 1 | 4 fixtures |
| Phase 5 | TransformStream + DefaultController | Phase 4 | 2 fixtures |
| Phase 6 | pipeTo + pipeThrough | Phase 4 + 5 | 2 fixtures |
| Phase 7 | Semantic + Backend 对接（新 builtin + host import） | Phase 1-6 | 编译通过 |
| Phase 8 | 测试 + 回归验证 | Phase 7 | 全部通过 |

## 9. 风险

### R1: `define_host_accessor_property_from_caller` 与 WASM helper 兼容性

**根因**：新增的 accessor property 写入函数必须与 `$obj_get`/`$obj_set` 的读取逻辑严格匹配。

**代码证据**（已验证）：
- `$obj_get`（compiler_helpers.rs:398-428）：`flags & FLAG_IS_ACCESSOR != 0` → 加载 offset 16 getter → `call_indirect type 12`
- `$obj_set`（compiler_helpers.rs:765-812）：`flags & FLAG_IS_ACCESSOR != 0` → 加载 offset 24 setter → `call_indirect type 12`
- Slot 布局：offset 0-3=name_id, 4-7=flags, 8-15=value, 16-23=getter, 24-31=setter

**解决**：`define_host_accessor_property_from_caller` 写入 `flags = FLAG_CONFIGURABLE | FLAG_ENUMERABLE | FLAG_IS_ACCESSOR`，offset 8 = undefined，offset 16 = getter，offset 24 = setter。与 `define_host_data_property_with_env` 唯一区别是 flags 含 `FLAG_IS_ACCESSOR` 且 getter/setter 替代 value。

**验证**：Phase 0 先将 Map.size 改为 getter — 如果不兼容，Map fixture 立即失败。

### R2: NativeCallable getter 的 `this` 绑定

**根因**：`$obj_get` 调用 getter 时传 `this_val = local 0`（boxed object）。

**解决**：handle 在构造时已绑定到 NativeCallable 变体（`ReadableStreamMethod { handle, kind: GetLocked }`），getter 实现直接使用 `handle` 字段，无需从 `this_val` 解析。`this_val` 传入但不使用。与现有 NativeCallable 模式一致。

**验证**：`stream.locked` 返回正确值。如果绑定有问题，fixture 立即捕获。

### R3: controller.enqueue() 同步 resolve reader.read()

**根因**：`enqueue` 在 JS 回调中同步调用，需要立即 resolve 等待中的 `read()` promise。

**解决**：
1. `ReaderEntry.pending_read_promise` 存储 `read()` 的未 resolve promise
2. `controller_enqueue`：推入 chunk → 遍历 reader_table 找到 pending_read_promise → pop chunk → settle promise → 清除 pending
3. `controller_close`：settle `{done: true}`
4. `controller_error`：reject promise

**验证**：Fixture `streams_readable_enqueue_close.js`

### R4: pipeTo 循环与 wasmtime epoch yielding

**根因**：无限循环可能触发 epoch interruption。

**解决**：`func_wrap_async` + `tokio::spawn`，每次 `read().await`/`write().await` yield 回 tokio runtime。已有 HTTP reader.read() 的 async 路径证明此模式可行。

**验证**：Fixture `streams_pipe_to.js` — pipe 10+ chunks 无超时。

### R5: tee() 两个 reader 同时消费

**根因**：两个子 stream 共享底层 source 的 chunk。

**解决**：
1. `tee()` 创建两个独立 controller + chunk_queue（clone 原始 queue 当前内容）
2. 原始 stream 的 `controller.enqueue` 被拦截，同时推入两个子 controller
3. `ReadableStreamEntry.is_tee_branch` + `tee_source_handle` 标记
4. 两个子 stream 的 `pull` 委托给原始 stream 的 `pull`

**验证**：Fixture `streams_readable_tee.js`

### R6: BYOB reader 需要 ArrayBuffer 直接写入

**根因**：`read(view)` 要求数据直接写入用户提供的 TypedArray。

**解决**：从 TypedArray 读取 `__typedarray_handle__` → buffer_ptr + byte_offset，memcpy chunk 数据到该地址，返回同一个 view。

**边界**：view 太小 → 写 min(chunk.len(), view.byteLength)；view detached → TypeError；stream closed → `{done: true, value: view}`

**验证**：Fixture `streams_readable_byob.js`

### R7: 侧表数量增加

**解决**：`StreamControllerEntry` 合并 4 种 controller（`ControllerKind` 区分），1 个侧表替代 4 个独立侧表。

### R8: 流式 body 与 Response.text() 同时消费

**解决**：`getReader()` 设置 `bodyUsed = true`；`text()`/`json()`/`arrayBuffer()` 检查 `bodyUsed`，已消费则抛 TypeError。

**验证**：Fixture `streams_fetch_body_used.js`

### R9: start() 中同步 close 后 enqueue

**解决**：`controller_close` 设置 `close_requested = true`；`controller_enqueue` 检查 `close_requested`，为 true 则抛 TypeError。

**边界**：先 enqueue 再 close → chunk 在 queue，下次 read 返回 `{done: true}`；先 close 再 enqueue → TypeError。

**验证**：Fixture `streams_readable_enqueue_close.js`

---

## 10. ADR 信号

- **`define_host_accessor_property_from_caller` 新增工具函数**：底层 WASM 架构（`$obj_get`/`$obj_set`）已完整支持 accessor property，但 runtime 层只暴露了 `define_host_data_property_from_caller`。新增 accessor 版本后，`locked`/`desiredSize`/`ready`/`closed`/`signal` 可作为真正的 getter 实现。连带修复 Map.size/Set.size 的 spec 合规性。替代方案：继续用 data property 方法。选择 accessor 因为规范明确要求 getter 语义，且底层已有完整支持。
- **新侧表模式**：`stream_controller_table` 合并多种 controller（Default/ByteStream/Writable/Transform）到单一侧表，用 `ControllerKind` 区分。替代方案：每种 controller 独立侧表。选择合并侧表以减少 RuntimeState 字段数量。
- **NativeCallable 变体激增**：从当前 3 个 Stream/Reader/AbortController 变体增加到 20+ 变体。替代方案：用单一 `StreamMethod { handle, kind }` 变体 + 大的 `kind` enum。已采用此模式（`kind` enum 内部区分）。其中 `GetLocked`/`GetDesiredSize`/`GetReady`/`GetClosed` 等变体作为 accessor property 的 getter 调用，不再作为 data property 方法暴露。
- **controller.enqueue() 同步 resolve reader.read()**：Streams Standard 要求 enqueue 立即 resolve pending read。实现使用 `pending_read_promise: Option<i64>` 字段在 `ReaderEntry` 中存储等待的 promise。
- **fetch 的 data: URL Response.body 从 null 变为 ReadableStream**：API 变更，但符合规范。
- **WritableStreamDefaultController.signal**：使用现有 `AbortController` 的 `.signal` 属性。如果 WritableStream 构造时未传入 signal，创建内部 AbortController。`WritableStreamEntry.abort_signal` 字段存储 NaN-boxed JS AbortSignal 对象，`signal` getter 通过 `define_host_accessor_property_from_caller` 暴露。

---

## 11. TaskIntentDraft

- **Outcome**: wjsm runtime 完整支持 WHATWG Streams Standard
- **Goal**: 所有 ReadableStream + WritableStream + TransformStream + pipeTo + pipeThrough + BYOB + QueuingStrategy 方法通过 fixture 验证
- **Success evidence**: 16+ 个新增 fixture 全部通过，现有 470+ 个 fixture 零回归
- **Stop condition**: `cargo nextest run --workspace` 全部通过
- **Non-goals**: Service Worker 拦截、Transferable Streams、CORS、FormData body
- **Scope**: wjsm-runtime 侧表 + NativeCallable + host import + fixture + fetch 集成修复
- **Risks**: controller 回调同步 resolve 逻辑复杂；BYOB reader 需要 ArrayBuffer 直接写入

## 12. BaselineReadSetHint

- **Authority**: `crates/wjsm-runtime/src/host_imports/fetch_core.rs` — 现有 stream/reader 实现
- **Authority**: `crates/wjsm-runtime/src/host_imports/reentrant_async.rs` — async host function 模式
- **Authority**: `crates/wjsm-runtime/src/lib.rs` — RuntimeState + NativeCallable + Promise
- **Authority**: `crates/wjsm-ir/src/builtin.rs` — Builtin enum
- **Authority**: `crates/wjsm-semantic/src/builtins.rs` — builtin 解析
- **Authority**: `crates/wjsm-backend-wasm/src/host_import_registry.rs` — host import 注册
- **Gap**: `WHATWG Streams Standard` 规范文本 — 需要参考 §4-§8
- **Gap**: 现有 `fetch_core.rs` 中 `call_response_method` 的 bodyUsed 逻辑 — 需要确认 fetch body 消费路径

## 13. ImpactStatementDraft

| 层 | 文件 | 影响 |
|---|---|---|
| wjsm-runtime | `lib.rs` | 大 — 新增 5+ 侧表字段、NativeCallable 变体 20+ |
| wjsm-runtime | `fetch_core.rs` | 中 — Response.clone() 和 body 修复 |
| wjsm-runtime | `fetch_http.rs` | 小 — data: URL body 路径 |
| wjsm-runtime | `streams_readable.rs` | 大 — 新增 1200+ 行 |
| wjsm-runtime | `streams_writable.rs` | 中 — 新增 600+ 行 |
| wjsm-runtime | `streams_transform.rs` | 中 — 新增 600+ 行 |
| wjsm-ir | `builtin.rs` | 小 — 新增 5 个变体 |
| wjsm-semantic | `builtins.rs` | 小 — 新增 5 行 |
| wjsm-backend-wasm | `host_import_registry.rs` | 中 — 新增 5 个 host import |
| fixtures | `happy/` | 大 — 新增 16+ 个 |

---

*Spec 撰写完毕，待用户审阅。*
