# ADR 导航

这一章是 wjsm 的架构决策记录（ADR）索引。

## ADR 列表

| ADR | 标题 | 关键决策 |
| --- | --- | --- |
| 0001 | Symbol Property Keys | Symbol 作为属性键的实现 |
| 0002 | RuntimeState Stays Flat | RuntimeState 保持扁平结构 |
| 0003 | Startup Snapshot Boundary | 启动快照的边界定义 |
| 0004 | Build-time Embedded Runtime | 构建期嵌入运行时工件 |
| 0005 | Pluggable GC v2 | 可插拔 GC（已被 0010 取代） |
| 0006 | Runtime Module Loading Boundary | 运行时模块加载边界 |
| 0007 | Inspector Guest Debug | CDP 调试器与 guest debug |
| 0008 | Node VM Multi Realm | `node:vm` 多 realm 实现 |
| 0009 | Async Hooks Host Core | async hooks 在 host core 的实现 |
| 0010 | Generational ZGC Managed Heap | 统一 ManagedHeap，取代 0005 |
| 0011 | Runtime Split by Backend Independence | 运行时按后端独立性拆分 |
| 0012 | Host Builtins Decouple | host 与 builtins 解耦 |
| 0013 | Multi Backend Contract | 多后端契约 |

## 关键 ADR

### ADR 0010：Generational ZGC Managed Heap

取代了 0005 的 pluggable GC v2。确立统一 ManagedHeap：8 字节句柄、shared memory64、三种回收器共用。不引入 memory32、4 字节句柄、dual-heap fallback。

### ADR 0011–0013：后端边界

- **0011**：运行时按后端独立性拆分。wasmtime 依赖只在 host-wasm。
- **0012**：host 与 builtins 解耦。包装层只做类型转换。
- **0013**：多后端契约。新后端的实现指南。

## 参考价值

ADR 是架构决策的事实来源。修改涉及架构边界的代码时，先读相关 ADR，确保改动不违反决策。

## 深入了解

- [核心不变量](invariants.md)
- [Owner 与单一事实来源](owners-and-sources-of-truth.md)
- [用户侧的架构概览](../../user/overview/architecture.md)
