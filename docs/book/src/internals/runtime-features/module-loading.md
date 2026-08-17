# 模块加载与执行上下文

运行时如何加载和执行用户模块，取决于模块的类型和来源。整条路径共用同一个 `NativeRuntime` 与 `ManagedHeap`。

## 模块来源

| 来源 | 处理方式 |
| --- | --- |
| 主模块（编译期） | 由 `wjsm-module` 解析、链接、bundle 成单个 IR `Program`，编码进 portable `.wjsm`，`NativeRuntime::execute` 绑定入口 |
| 动态 `import()` | 运行时解析 specifier、加载源码、经 `NativeCompiler` 编成 native image 并执行 |
| `node:vm` 构造器 | 编译期未知的代码，走 eval 模式 lowering + `NativeCompiler` |

## ESM 加载

用户模块在编译期已经由 `wjsm-module` 解析、链接、bundle 成单个 IR Program（见[模块图与解析器](../modules/graph-and-resolution.md)）。运行时收到的是 self-contained 的 portable artifact，不需要再做编译期那种全图解析。

运行时的模块加载主要处理两类入口：

- **主模块**：编译期确定的入口文件，执行 artifact 的入口函数。
- **动态 `import()`**：`modules.rs` 解析 specifier、加载源码、编成 native image。同一 `NativeRuntime`、同一堆。

## 执行上下文

每个模块在创建时绑定到一个 realm。realm 决定模块可见的全局对象、intrinsics 和原型链。跨 realm 的模块共享同一个 `NativeRuntime` / `ManagedHeap`，realm 本身是宿主表（`NativeAgentState` / `node_vm.contexts`），不是第二份运行时。

## `node:vm` 与动态代码

`node:vm` 模块提供 `Script`、`SourceTextModule`、`Context` 等构造器，用于在隔离 realm 中执行动态代码。这条路径涉及编译期未知的代码，走 eval 模式编译（见[动态代码、Eval 与解释器](dynamic-code.md)）。

## 深入了解

- [ESM 链接与求值](../modules/esm-linking.md)
- [RuntimeState 与 Realm](../host-runtime/runtime-state-and-realms.md)
- [`node:vm` 多 Realm](node-vm.md)
