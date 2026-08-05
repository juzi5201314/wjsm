# 对象、数组与集合算法

这一章说明 `wjsm-builtins` 如何实现对象模型的核心算法，以及它们如何与 GC 交互。

## 对象模型

对象通过句柄引用（`TAG_OBJECT_HANDLE`），句柄是对象表的下标。对象布局由 `wjsm-gc` 决定（见[对象布局与分配](../gc/object-layout-and-allocation.md)），builtins 只通过 `ExecContext` 的方法操作对象。

`object_builtins.rs` 覆盖：属性定义（`DefineOwnProperty`）、属性读取（`Get`/`GetOwnProperty`）、属性写入（`Set`）、属性删除（`Delete`）、属性枚举（`Enumerate`/`OwnPropertyKeys`）、原型链遍历（`GetPrototypeOf`/`SetPrototypeOf`）。

## 集合

`wjsm-builtins` 的 `collections.rs` 定义 Map/Set/WeakMap/WeakSet 的语义算法入口，而 Map/Set 的存储核心在 `wjsm-host-wasm/src/runtime_collections.rs`：

- **Map/Set 哈希索引**：keys/values（Set 仅 values）用保持插入顺序的平行 `Vec` 存储；`index` 是 SameValueZero 稳定哈希 → 槽位索引的映射（仅存活键）。删除打 tombstone（`deleted` 平行标记）以保持迭代顺序，tombstone 过半时压缩重建（剔除 tombstone 槽位并重建索引）。哈希冲突（不同键同哈希）回退存活槽位线性扫描保证正确。
- **WeakMap/WeakSet** 的键是弱引用，GC 回收键时自动清除条目。
- **`WeakRef` 和 `FinalizationRegistry`** 在 `weakref_finalization.rs` 实现。

这套结构把「查找」从 O(n) 线性扫描降到哈希定位，同时保序——`Map` 的迭代顺序仍严格等于插入顺序（tombstone 不物理移动元素）。

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
