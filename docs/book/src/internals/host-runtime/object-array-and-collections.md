# 对象、数组与集合算法

这一章说明 `wjsm-builtins` 如何实现对象模型的核心算法，以及它们如何与 GC 交互。

## 对象模型

对象通过句柄引用（`TAG_OBJECT_HANDLE`），句柄是对象表的下标。对象布局由 `wjsm-gc` 决定（见[对象布局与分配](../gc/object-layout-and-allocation.md)），builtins 只通过 `ExecContext` 的方法操作对象。

`object_builtins.rs` 覆盖：属性定义（`DefineOwnProperty`）、属性读取（`Get`/`GetOwnProperty`）、属性写入（`Set`）、属性删除（`Delete`）、属性枚举（`Enumerate`/`OwnPropertyKeys`）、原型链遍历（`GetPrototypeOf`/`SetPrototypeOf`）。

## 集合

`collections.rs` 实现 Map/Set/WeakMap/WeakSet：

- Map/Set 用对象表存储键值对，GC 跟踪键的可达性。
- WeakMap/WeakSet 的键是弱引用，GC 回收键时自动清除条目。
- `WeakRef` 和 `FinalizationRegistry` 在 `weakref_finalization.rs` 实现。

## 数组

`array_object.rs` 处理数组特定算法：稀疏数组、length 属性、`push`/`pop`/`splice`/`slice` 等。数组元素通过 `elem_get`/`elem_set` helper 访问，与对象属性路径分离。

## Proxy

`proxy_entrypoints.rs` 是 Proxy 的入口点。Proxy 对象有 `TAG_PROXY` 标签，`obj_get`/`obj_set`/`obj_has` 等操作检测到 Proxy 后走 trap 路径。陷阱实现在 `proxy_traps.rs`，Reflect API 在 `proxy_reflect.rs`。

trap 调用用户提供的 handler 函数，需要再入机制。这条路径通过 `ExecContext` 的再入方法完成。

## GC 交互

所有分配路径（`obj_new`、`arr_new`、`closure_create`、`promise_resolve` 等）可能触发 GC。builtins 通过 `ExecContext` 的 `gc_safepoint` 方法进入 safepoint，GC 在此刻扫描栈上的活跃句柄。

builtins 不直接管理 GC 状态，只通过 `ExecContext` 的方法触发或查询。这是「后端无关」的关键：GC 算法的选择和实现留给 `wjsm-gc`。

## 深入了解

- [对象布局与分配的 GC 侧细节](../gc/object-layout-and-allocation.md)
- [GC Spill 与 safepoint 的后端实现](../backend/liveness-slots-and-spills.md)
- [Proxy trap 的用户侧限制](../../user/runtime/limitations.md)
