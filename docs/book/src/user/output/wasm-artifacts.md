# WASM 产物与宿主要求

`wjsm build` 产出的 `.wasm` 不是独立可执行程序。它假定宿主提供 wjsm 的完整 import 表、三块内存和 support 模块，因此只能由 wjsm 自己的宿主加载。

## 产物依赖什么

用 `disasm --skeleton` 看一个最小程序的接口面：

```bash
wjsm build -e 'console.log(1)' -o /tmp/hello.wasm
wjsm disasm /tmp/hello.wasm --skeleton | head -40
```

`console.log(1)` 这一行源码的产物就有 507 个 import，分属两个模块：

| Import 模块 | 内容 |
| --- | --- |
| `env` | 宿主函数（`console_log`、`fetch`、`obj_get`…）、三块内存、函数表、可变全局 |
| `wjsm_support` | 预编译 support 模块提供的高频 helper |

内存导入是硬约束：

```text
(import "env" "memory" (memory 8))
(import "env" "__shadow_memory" (memory 1))
(import "env" "__heap_memory" (memory i64 524288 4294967296 shared))
```

`__heap_memory` 是 64 位共享内存，宿主 Engine 必须开启 `memory64`、`threads`、`multi-memory` 和 `bulk-memory`。通用 WASI 运行时不满足这组条件，也无法提供那 507 个 import。

> <details><summary>为什么「500+ 个 import」不能简化？</summary>
>
> 这些 import 是「JS 引擎的所有原子操作」——`obj_get`（属性读）、`obj_set`（属性写）、`arr_new`（数组分配）、`string_eq`（字符串比较）、`fetch`（HTTP 请求）、`console_log`……加起来是 JS 运行时的全部能力。
>
> wjsm 不能把它们直接编进 WASM 里——那会让产物膨胀到几 MB，而且违反 WASM 的「逻辑分离」原则（计算与宿主 IO 分开）。
>
> 替代方案是把这些操作都内联进每个产物——技术上可以，但代价是每个产物都要包含整套 JS 运行时实现，相当于「每个 .wasm 是个独立 JS 引擎」。这显然不实际。
>
> 当前的设计：所有产物共享同一套「宿主 import」，由 wjsm 二进制（或基于 `wjsm-runtime` 的 Rust 宿主）提供。换 host 的成本是实现 500+ 个 import 函数。
>
> </details>

## 怎么运行产物

日常执行直接用 `wjsm run <file>`，它在同一进程内完成编译和执行，不需要落盘 `.wasm`。

需要在 Rust 里嵌入执行，依赖 `wjsm-host-wasm`（或兼容 facade `wjsm-runtime`）调用 `execute_with_options`，参见[作为 Rust 库嵌入](../workflows/embedding.md)。

## 产物用途

`.wasm` 文件的实际价值在于分析和回归比较，而不是分发：

- `wjsm validate` 确认字节码通过 Wasmtime 校验。
- `wjsm size` 比较改动前后的 section 体积。
- `wjsm disasm` 检查具体函数的生成代码。

## 深入了解

- [Import、Export 与主模块 ABI](../../internals/backend/imports-exports-and-abi.md)
- [Support 模块与辅助函数](../../internals/backend/support-module.md)
- [Memory64 与共享内存模型](../../internals/gc/memory64.md)
- [Engine 配置](../../internals/host-runtime/engine-configuration.md)
