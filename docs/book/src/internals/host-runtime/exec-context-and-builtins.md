# ExecContext 与 Builtins 解耦

这一章说明 `wjsm-builtins` 如何在不知道后端的前提下实现全部 ECMAScript 语义算法。

## 问题

Builtin 语义算法（如 `PromiseResolve`、`Object.keys`、`Proxy` trap 分派）需要操作对象、触发 GC、调用用户 JS 函数、注册微任务。这些操作的具体实现依赖后端运行时：wasmtime 后端用 `Caller<RuntimeState>`，native 后端用自己的上下文。

如果 builtins 直接持有 `Caller<RuntimeState>`，它们就永久绑定 wasmtime——这是 ADR 0012 之前的状态。

## 解法

`ExecContext` trait 定义 builtins 需要的全部操作（约 330 个方法），builtins 以 `<E: ExecContext>` 泛型实例化。wasmtime 后端用 `WasmExecContext` 实现，native 后端用 `NativeExecContext`。编译期单态化，零 vtable 开销。

```rust
pub fn promise_resolve<E: ExecContext>(
    ctx: &mut E,
    constructor: Value,
    value: Value,
) -> Result<Value> { ... }
```

## 再入与 reentrant_async

builtins 可能回调用户 JS（如 `Promise.then` 的 reaction、`Proxy` trap、`toString`）。`ExecContext` 的再入方法封装这条路径，保证调用栈深度与异步上下文正确传播。

ADR 0013 列出四类不迁入 `wjsm-builtins` 的豁免，因为它们本质是后端职责：分配与 GC glue、I/O 桥（`fetch_http`）、再入基础设施（`reentrant_async`）以及 bootstrap 全局装配。这些留在 `wjsm-host-wasm`。

## 语义算法的规模

`wjsm-builtins` 约 60 个文件、17000 行，覆盖：

| 域 | 文件 |
| --- | --- |
| 对象与属性 | `object_builtins.rs`、`property.rs`、`private_fields.rs` |
| 集合 | `collections.rs`、`collections_buffers.rs` |
| 数组 | `array_object.rs`、`atomics.rs`、`typedarray_methods.rs` |
| 字符串 | `string_methods.rs`、`string_iter.rs`、`string_to_number.rs`、`number_format.rs` |
| Promise / async | `promise.rs`、`promise_combinators.rs`、`async_fn.rs`、`async_generator.rs`、`generator.rs` |
| Proxy / Reflect | `proxy_entrypoints.rs`、`proxy_reflect.rs`、`proxy_traps.rs` |
| JSON | `json.rs` |
| 日期 | `date.rs`、`date_parse.rs` |
| Fetch / Streams | `fetch.rs`、`streams.rs`、`streams_queuing.rs` |
| 模块 | `modules.rs` |
| Inspector | `inspector_host.rs` |
| 弱引用 | `weakref_finalization.rs` |
| 核心 | `core.rs`、`core_async.rs`、`primitive_core.rs`、`misc.rs` |

## 深入了解

- [Host 能力 Trait 的层次设计](host-traits.md)
- [核心 JavaScript Builtins 的分域组织](javascript-builtins.md)
- [多后端完全支撑契约](../backend/multi-backend-boundary.md)
