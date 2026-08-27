# RuntimeState 与 Realm

这一章说明 runtime 的状态组织和 realm 隔离机制。

## NativeRuntime 的状态

`NativeRuntime` 拥有以下 mutable state：

| 组件 | 说明 |
| --- | --- |
| `NativeVmContext` | pinned vmctx，generated code 的运行时上下文 |
| `NativeAgentState` | agent 级状态（module、Promise、continuation、worker、scheduler） |
| ManagedHeap | 统一对象堆 + HandleTableV2 |
| Collector | `GenerationalZgc`（唯一生产回收器） |
| Module tables | 已加载模块的 namespace 缓存 |
| Promise tables | pending / fulfilled / rejected Promise |
| Inspector | CDP 调试器会话 |
| Image repository | native image cache |

所有 state 受 owner-thread 约束，不可跨线程共享。

## Realm

Realm 是 JavaScript 执行上下文，有自己的全局对象、intrinsics 和原型链。`NativeAgentState` 维护 realm 表，`RealmId` 标识当前执行 realm。

| 来源 | Realm |
| --- | --- |
| 主模块 | 默认 realm |
| `node:vm.createContext()` | 新 realm，独立 intrinsics |
| `node:vm.Script` / `vm.SourceTextModule` | 在指定 realm 执行 |

每个 realm 有独立的 `RealmIntrinsics`，存储原型句柄和全局构造器。跨 realm 对象可以传递（共享同一 ManagedHeap），但不能直接访问对方的全局对象。

当前没有独立的 `WJSM_VM_MAX_REALMS` 开关；Realm 表由 `NativeAgentState` 持有，受堆预算与进程资源约束。

## SharedRuntimeState

`SharedRuntimeState` 是跨 worker 线程共享的状态，基于 `Arc`。它允许 worker 和主线程共享一些全局信息（如 process 信息）。但 mutable runtime tables 不共享——每个 worker 有自己的。

跨 agent 只通过 structured clone、SAB/Atomics 和显式 IPC 传递。不共享 GC handle 或 raw address。

## GC roots

GC 根集来自三个来源：

1. **栈上活跃句柄**：safepoint 上 `NativeRootFrame` 里 bitmap 置位的槽。
2. **RuntimeState 显式 root**：primordial 句柄（原型对象、全局构造器）。
3. **Realm intrinsics**：每个 realm 的 `RealmIntrinsics` 结构里的原型句柄。

## 深入了解

- [实例化与执行生命周期](instantiation-and-lifecycle.md)
- [根、弱引用与 Finalizer](../gc/roots-weak-and-finalizers.md)
- [`node:vm` 多 Realm](../runtime-features/node-vm.md)
- [Promise、微任务与异步调度器](../runtime-features/async-scheduler.md)
