# 模块加载与执行上下文

这一章说明运行时如何加载和执行用户模块。

## ESM 加载

用户模块在编译期已经由 `wjsm-module` 解析、链接、bundle 成单个 IR Program（见[模块图与解析器](../modules/graph-and-resolution.md)）。运行时收到的 WASM 已经是 self-contained 的主模块，不需要运行时模块解析。

运行时的模块加载主要处理两类入口：

- **主模块**：编译期确定的入口文件，在 `compile_source` 时绑定到 WASM 的入口函数。
- **动态 `import()`**：运行时解析的模块。`modules.rs` 负责解析 specifier、加载源码、编译、实例化。

## 执行上下文

每个模块在创建时绑定到一个 realm。realm 决定模块可见的全局对象、intrinsics 和原型链。跨 realm 的模块共享同一个 WASM store，但有不同的 `RealmIntrinsics`。

`RuntimeState` 维护 realm 表，`RealmId` 标识当前执行 realm。`DEFAULT_MAX_REALMS = 1024` 是 realm 数量上限。

## `node:vm` 与动态代码

`node:vm` 模块提供 `Script`、`SourceTextModule`、`Context` 等构造器，用于在隔离 realm 中执行动态代码。这条路径涉及编译期未知的代码，走 eval 模式编译（见[动态代码、Eval 与解释器](dynamic-code.md)）。

## 深入了解

- [ESM 链接与求值](../modules/esm-linking.md)
- [RuntimeState 与 Realm](../host-runtime/runtime-state-and-realms.md)
- [`node:vm` 多 Realm](node-vm.md)
