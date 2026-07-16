# TodoCheckpointDraft

## Current todo

实施计划已完成三轮共识审查和主机资源/OOM安全修订；等待选择执行方式。

## Completed

- 审计当前 ZGC、分配/扫描/barrier 热路和现有 benchmark。
- 运行当前 Criterion、自研 ZGC benchmark、barrier-pressure benchmark 与 `perf stat/record`。
- 研究 JDK 25 GA Generational ZGC、JEP 439/474/490 与 Wasmtime 43.0.2 SharedMemory/memory64。
- 与用户确认尾延迟优先、算法归一化目标、真并发重构、全 collector 统一 heap、shared memory64、portable+SIMD 和堆规模矩阵。
- 分四节确认完整架构设计。
- 写入 `docs/aegis/specs/2026-07-16-zgc-performance-design.md`。
- 书面规格完成自审并获用户批准。
- 写入并修订 `docs/aegis/plans/2026-07-16-zgc-performance.md`，覆盖 Task 0–27、RED/GREEN、性能门与退役。
- Round 1：4 Blocking、8 Important、3 Minor；全部核验并修订。
- Round 2：首轮15项全部closed；主代理裁决单点active切换、证据收敛与完整mutable-header分类。
- Round 3：`Consensus status: agree`，无open Blocking/Important。
- 核验当前主机/cgroup内存；在规格§18.5和计划Task 1/23/24/25加入fail-closed preflight、硬隔离、独占顺序执行与运行中熔断。

## Active slice

Design Spec 与 implementation plan 均已完成。

## Evidence refs

- `docs/aegis/work/2026-07-16-zgc-performance/90-evidence.md`
- `docs/aegis/specs/2026-07-16-zgc-performance-design.md`
- `docs/aegis/plans/2026-07-16-zgc-performance.md`

## Blocked on

无。

## Next step

- 选择 subagent-driven 或 inline execution。
- 执行从 Task 0 的 Wasmtime shared memory64/config parity 可行性门开始，不得跳过。

## ResumeStateHint

恢复时先读本文件、`10-intent.md`、父规格与实施计划；Round 3共识及主机资源安全修订均已落文，生产源码尚未修改。

## DriftCheckDraft

- Scope：仍是先设计和计划，不执行实现。
- Compatibility：32 位 handle identity、ECMAScript、CLI 与 snapshot 开关保持。
- Retirement：旧 heap ABI、假并发 ZGC、无效 benchmark 明确 delete-first。
- Decision：plan-consensus-agree + resource-safety-amended；continue-to-execution。
