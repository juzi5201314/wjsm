# TaskIntentDraft

## Requested outcome

深度优化 wjsm ZGC 及对象内存子系统，使算法、数据结构、并发模型和归一化单位成本不弱于 JDK 25 Generational ZGC，并主动利用 Rust、Wasm handle 间接层与 NaN-box 空闲位形成优势；当前阶段先交付可靠设计与可执行实施计划。

## Goal

- 尾延迟优先。
- 全部 collector 统一迁移到 shared memory64 `ManagedHeap`。
- ZGC 实现 concurrent young/old mark、concurrent relocation、精确 remembered set、page/NLAB allocator、预测式 pacer、commit/decommit、NUMA 和 SIMD 特化。
- 预注册矩阵中归一化核心指标不差于 JDK 25 10%，至少两项领先 15%。

## Success evidence

- `docs/aegis/specs/2026-07-16-zgc-performance-design.md` 的能力、正确性、并发和性能门全部有实施任务与验证命令。
- JDK 25 GA、Wasmtime 43.0.2 和当前源码证据被引用。
- 旧 heap ABI、无效 benchmark 与重复 owner 有明确退役任务。

## Stop condition

- `done`：设计获书面复核，实施计划覆盖全部验收与退役面。
- `blocked`：必要的 SharedMemory/memory64 契约无法成立且无批准的新架构。
- `needs-verification`：计划存在但缺少可执行命令、指标或正确性门。
- `scope-exceeded`：方案改变 ECMAScript 语义或保留第二套长期对象堆 owner。

## Non-goals

- 不要求 WJSM 端到端 wall time 在任意负载硬压 JVM JIT。
- 不引入 WASM GC proposal/externref。
- 不保留 memory32 动态对象堆 fallback。
- 不在规划阶段修改生产源码。

## Scope

- 父规格：`docs/aegis/specs/2026-07-16-zgc-performance-design.md`。
- 影响 runtime GC、backend barriers/allocation、support/snapshot ABI、realm/side tables、CLI telemetry、benchmark 与平台内存后端。

## Risk hints

- SharedMemory 全部竞争字段必须满足原子与对齐不变量。
- concurrent relocation 必须证明写竞争、assist、epoch reclaim 与 handle quarantine。
- memory64 影响 backend/support/runtime/snapshot ABI。
- 当前 benchmark 不能支撑性能结论，必须先重建测量真相。

## BaselineReadSetHint

- `docs/aegis/specs/2026-07-03-pluggable-gc-v2-design.md`
- `docs/aegis/plans/2026-07-03-pluggable-gc-v2.md`
- `docs/adr/0003-startup-snapshot-boundary.md`
- `docs/adr/0004-build-time-embedded-runtime.md`
- `docs/adr/0005-pluggable-gc-v2.md`
- 当前 `runtime_gc`、backend、support、snapshot 与 benchmark 源码
- JDK 25 GA `src/hotspot/share/gc/z/`
- JEP 439/474/490
- Wasmtime 43.0.2 SharedMemory/memory64 源码

## BaselineUsageDraft

- required refs：上述本地与外部 refs。
- acknowledged refs：GC v2 spec/plan、ADR 0003/0004/0005、当前源码、JDK 25 GA 与 Wasmtime 43.0.2。
- cited refs：父规格 §4、§24、§25。
- missing refs：`docs/aegis/baseline/` 当前为空；无阻塞，因为产品与架构权威已由批准规格及 ADR 提供。
- decision：continue。

## ImpactStatementDraft

本任务重写 load-bearing object heap ABI、地址宽度、引用颜色、barrier、分配器、GC 并发模型、snapshot wire format 与性能门。ECMAScript 语义和 32 位 JS handle identity保持；全部内部 caller clean cutover，不保留双 owner。
