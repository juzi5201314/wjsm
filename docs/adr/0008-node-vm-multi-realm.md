# ADR 0008：node:vm 单堆多 Realm

- 状态：Accepted
- 日期：2026-07-11
- 关联：issue #313（原将通用 `vm.runInNewContext` 列为非目标；本 ADR 记录对该非目标的**范围覆盖**）

## 背景

需要 Node 兼容的 `require('vm')` / `node:vm`：独立 intrinsics（`[] instanceof Array === false`）、
跨 realm 对象引用自由流动、sandbox contextify、可选 `timeout`。现有 runtime 是单
wasmtime Store + 单 `obj_table` + 单 GC 堆；不得为每个 realm 复制 Store。

## 决策

1. **单堆多 realm**  
   所有 realm 共享同一 Store / 线性内存 / `obj_table` / GC。`Realm` =
   `RealmId` + `global_object` + `RealmIntrinsics` + `CodeGenFlags`，登记在
   `RuntimeState.active_realms`（扁平字段，遵 ADR 0002）。

2. **pristine 可达图克隆**  
   `createContext` 从主 realm 当前 primordial 可达闭包 BFS 克隆对象到 **dynamic
   heap** 新 handle，再用 `ObjectHandleMapPolicy` 重映射。禁止二次 snapshot
   restore / immortal 整段 memcpy。

3. **walker + 双 RemapPolicy**  
   共享 `walk_and_remap_heap`：`FuncTableIndexRangePolicy`（snapshot 恢复改
   WASM 函数表索引）与 `ObjectHandleMapPolicy`（realm 克隆改 object/array handle
   与 proto）。禁止混为一种 HandleMap 语义。

4. **`execution_realm` + proto global swap（非 TLS）**  
   分配 / 构造 / 字面量 / eval 统一读 `RuntimeState.execution_realm`。进入 realm
   时 swap 父模块 mutable `__array_proto_handle` / `__object_proto_handle`（compiled
   eval import 同一批 global）；嵌套栈式 restore。禁用 TLS。

5. **timeout 与 async-yield epoch 隔离**  
   现网 `epoch_deadline_async_yield_and_update` 已占用 epoch。vm `timeout` 在
   帧内临时 `epoch_deadline_trap` + 后台 `engine.increment_epoch()`，退出**必定**
   恢复 async_yield；解释器路径用 `vm_deadline: Instant` 在循环回边检查。

6. **条件 GC root / sandbox 生命周期**  
   realm ≥1 仅当 sandbox global 已被其它 root 标记 live 时才 root 其
   `intrinsics`；GC 后 `reclaim_dead_realms` 去掉不可达 realm 与 `contextified`
   条目。realm 0 不因登记额外强持有 global。

7. **`VmMethod` / 构造器与 snapshot ABI**  
   无状态 `NativeCallable::VmMethod { kind }` 可进 `SnapshotNativeCallable`
   （discriminant 86）。带 `RealmId` 的动态构造器**禁止**进 snapshot 子集。

## 后果

- 单 realm 程序：`execution_realm=0`，行为与开销不变。
- 新增 `node:vm` builtin、`runtime_node_vm` host bridge、相关 fixtures。
- 非目标显式抛错：`SourceTextModule` / `SyntheticModule` / `measureMemory`。
- snapshot ABI 因 `VmMethod` 扩展而 rebake（构建期 `abi_hash`）。

## 替代方案（否决）

| 方案 | 否决原因 |
|------|----------|
| 每 realm 独立 Store | 跨 realm 对象引用需 structuredClone；与 Node 语义不符 |
| TLS 注入 current realm | 与 async host / 重入冲突；难审计 |
| 直接改全局 epoch 为 trap | 破坏 async-yield 协作调度 |

## 参考

- `docs/aegis/plans/2026-07-10-node-vm-multi-realm.md`
- `docs/aegis/specs/2026-07-10-node-vm-multi-realm-design.md`
- ADR 0002 / 0003 / 0004 / 0005
