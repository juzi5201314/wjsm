# 核心 JavaScript Builtins

这一章说明 `wjsm-builtins` 的组织方式，以及它如何覆盖 ECMAScript 与 Web API 的语义算法。

## crate 规模

`wjsm-builtins` 约 60 个文件、17000 行，是 workspace 里第三大 crate。它依赖 `wjsm-host`（trait 契约）和 `wjsm-ir`（常量与值编码），不依赖任何后端。

## 分域组织

| 域 | 文件 | 覆盖 |
| --- | --- | --- |
| 对象与属性 | `object_builtins.rs`、`property.rs`、`private_fields.rs` | 属性读写、定义、枚举、私有字段 |
| 集合 | `collections.rs`、`collections_buffers.rs` | Map/Set/WeakMap/WeakSet |
| 数组 | `array_object.rs` | 数组算法 |
| TypedArray | `typedarray_methods.rs`、`atomics.rs` | TypedArray 方法与 Atomics |
| 字符串 | `string_methods.rs`、`string_iter.rs`、`string_to_number.rs` | 字符串操作与解析 |
| Promise | `promise.rs`、`promise_combinators.rs` | Promise 及组合器 |
| async | `async_fn.rs`、`async_generator.rs`、`generator.rs` | async 函数与生成器 |
| Proxy/Reflect | `proxy_entrypoints.rs`、`proxy_reflect.rs`、`proxy_traps.rs` | Proxy 陷阱与 Reflect |
| JSON | `json.rs` | 解析与序列化 |
| 日期 | `date.rs`、`date_parse.rs` | Date |
| Fetch/Streams | `fetch.rs`、`streams.rs`、`streams_queuing.rs` | Fetch API 与 Streams |
| 弱引用 | `weakref_finalization.rs` | WeakRef/FinalizationRegistry |
| 模块 | `modules.rs` | 动态 import 解析 |
| Inspector | `inspector_host.rs` | CDP 桥接 |
| 渲染 | `render.rs` | console 格式化 |
| 核心 | `core.rs`、`core_async.rs`、`primitive_core.rs`、`misc.rs` | 全局函数与基础操作 |

## 语义拦截的配合

语义层（`wjsm-semantic/src/builtins.rs`）识别已知调用形态并发射 `CallBuiltin`。后端 codegen 查 `NativeCallable` 注册表得到 WASM function index。运行时 host import 函数调用 `wjsm-builtins` 的泛型算法。

三层各司其职：语义层决定「这是什么操作」，后端决定「调哪个函数」，builtins 决定「怎么执行」。

## 深入了解

- [ExecContext 与 Builtins 的解耦](exec-context-and-builtins.md)
- [语义层如何拦截内置方法调用](../frontend/expressions-and-statements.md)
- [Host Import 注册与包装层](host-imports.md)
