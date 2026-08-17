# `node:vm` 多 Realm

这一章说明 `node:vm` 模块如何创建隔离的执行上下文（realm）。

## Realm 概念

Realm 是 JavaScript 的执行上下文，有自己的全局对象、intrinsics 和原型链。`node:vm.createContext()` 创建一个新 realm，`vm.Script` 和 `vm.SourceTextModule` 在指定 realm 中执行。

wjsm 的 realm 是宿主表，不是第二份 `NativeRuntime`。`NativeAgentState::node_vm` 用 `HashMap<u32, VmContextOptions>` 登记 sandbox 句柄，并维护 `active_contexts` 栈。每个 context 可以有独立的 `Array.prototype` / `Array` 构造器（`RealmArrayConstructor`）。跨 realm 对象仍活在同一 `ManagedHeap` 上。

## createContext

`vm.createContext(sandbox)` 把 sandbox 对象登记为新 realm 的全局对象，并按需分配该 realm 的 array intrinsics。`vm.Script` 在该 context 中执行代码。`vm.runInContext` 是快捷方式。

## Script 与 SourceTextModule

| 构造器 | 用途 |
| --- | --- |
| `vm.Script` | 编译脚本，可在多个 context 中多次运行 |
| `vm.SourceTextModule` | 编译 ES 模块，按 ESM 语义链接 |
| `vm.compileFunction` | 编译函数体，可指定作用域 |

三者都走 eval 模式 lowering，再经 `NativeCompiler` 编成 native image，在当前 `NativeRuntime` 上执行。

## Isolate 与边界

realm 之间通过句柄引用对象，但不能直接访问对方的全局对象。这与 Node.js 的 vm 行为一致。跨 realm 调用需要经过 proxy 或 `vm.runInContext`。

## 深入了解

- [RuntimeState 与 Realm](../host-runtime/runtime-state-and-realms.md)
- [动态代码、Eval 与解释器](dynamic-code.md)
- [用户侧的动态代码与隔离上下文](../../user/runtime/dynamic-code-and-vm.md)
