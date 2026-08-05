# RuntimeState 与 Realm

这一章说明运行时状态的组成、单堆多 realm 的组织，以及 `execution_realm` 如何选择当前 realm。

## RuntimeState

`RuntimeState` 是 wasmtime 后端的核心状态结构，持有：

- `active_realms: Vec<RealmEntry>`：所有已创建的 realm。
- `execution_realm: u32`：当前分配 / 构造 / 字面量 / eval 的 intrinsic 解析目标。
- 对象表、堆元数据、GC 状态。
- 微任务队列、宏任务队列、定时器。
- 模块注册表与缓存。
- Inspector 会话状态。
- `runtime_strings: RuntimeStringTable`：运行时字符串稳定 handle 表（见下节）。

主 realm 是 `active_realms[0]`，惰性登记。`node:vm` 创建的新 realm 追加到 `active_realms`，复用同一个对象表和 GC。

## RealmId 与上限

```rust
pub struct RealmId(pub u32);
pub const DEFAULT_MAX_REALMS: u32 = 1024;
```

`max_realms_limit()` 读 `WJSM_VM_MAX_REALMS`，取正值覆盖默认 1024。每个 realm 拥有独立的 `RealmIntrinsics`——一组原型对象句柄（`object_proto`、`array_proto`、`promise_prototype`、11 种 TypedArray 原型等）。

realm 之间共享对象表和堆，对象可以跨 realm 传递。这与 ADR 0008 的「单堆多 realm」设计一致。

## execution_realm

`execution_realm` 决定内置构造器解析到哪个 realm 的原型。`node:vm.runInContext` 会切换 `execution_realm` 到目标 realm，执行完切回。这保证代码在正确的原型链上运行。

## panic 捕获

`realm.rs` 使用 `catch_unwind` 捕获 WASM 执行中的 panic，避免直接终止进程。捕获后把 panic 转成运行时错误，让上层诊断逻辑有机会输出。

## 运行时字符串表与清扫

字符串值用稳定 handle 引用，handle 是 `RuntimeStringTable` 的表索引（`runtime_strings_gc.rs`）。表项不移动，不属于 `ManagedHeap` 对象图——对象堆的 GC 不直接管理它，由专门的清扫路径回收。

清扫按估算字节量触发，不随每次分配执行：

- `SWEEP_THRESHOLD_BYTES`（16MiB）是初始阈值；`allocated_bytes` 达到 `next_sweep_bytes` 时 `needs_sweep()` 返回真。
- 字符串分配路径（`store_runtime_string` 等）调用 `maybe_sweep_runtime_strings` 检查阈值。
- 清扫时从堆对象图与 host 表根收集存活的 string handle 集合，把不在集合中的槽置 `None` 并回收索引；`next_sweep_bytes` 重设为 `存活字节 × 2`（不低于 16MiB），形成自适应间隔。
- ZGC 周期结束时 `force_sweep_runtime_strings` 无条件执行一轮，保证并发回收后字符串表同步收缩。

快照捕获/恢复用 `snapshot_dense`/`restore_dense` 把表序列化进快照的 `runtime_strings` section。清扫留下的空槽（`None`）会让 `snapshot_dense` 失败——快照只在 bootstrap 后立即捕获，此时表是稠密的。

## 深入了解

- [node:vm 多 Realm 的运行时行为](../runtime-features/node-vm.md)
- [ADR 0008 的单堆多 realm 决策](../reference/adr-index.md)
- [对象表与句柄的 GC 侧细节](../gc/handle-table.md)
