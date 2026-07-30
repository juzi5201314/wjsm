# Memory64 与共享内存模型

对象堆使用 shared memory64，因为 32 位地址空间（4 GiB）对 JavaScript 程序不够用。

## 内存常量

| 常量 | 值 | 定义位置 |
| --- | --- | --- |
| `HEAP_MEMORY_MIN_PAGES` | 524288 | `wjsm-ir` |
| `HEAP_MEMORY_MAX_PAGES` | 4294967296 | `wjsm-ir` |
| 最小地址空间 | 32 GiB | min_pages × 64 KiB |
| 最大地址空间 | 256 TiB | max_pages × 64 KiB |

这些常量是 user wasm 与 host 的唯一对齐来源，修改后需要同步更新两者。

## 为什么 shared

shared memory 允许多线程访问同一块内存。ZGC 和 G1 的并发回收阶段需要工作线程读写对象堆，shared memory 是前提条件。

WASM 的 `shared` memory 要求 `threads` feature 启用，wjsm 在 `Cargo.toml` 的 wasmtime features 里显式启用。

## 约束

shared memory64 是 WASM 提案，不是所有 WebAssembly 运行时都支持。这是 wjsm 产物无法在通用 WASI 运行时上运行的约束之一。

`atomics` 和 `bulk-memory` 也是 shared memory 的依赖项。`Atomics` 操作通过 WASM atomics 指令实现，用于 GC 屏障和并发标记。

## 深入了解

- [ManagedHeap 架构](managed-heap.md)
- [Import、Export 与主模块 ABI 中的三块内存](../backend/imports-exports-and-abi.md)
- [用户侧的内存配置](../../user/configuration/memory.md)
