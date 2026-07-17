# TodoCheckpointDraft

## Current todo

Task 0 实现与三条 GREEN 已完成，等待主代理复审后标记最终完成；未进入 Task 1。

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
- 实现 `wjsm-engine-config` 唯一 Wasmtime config owner，并迁移 runtime/support/snapshot 调用链。
- Wasmtime family 精确锁定 `=43.0.2`。
- Task 0 GREEN：engine-config 2、shared_memory64 1、runtime-support 9、snapshot-format 6、startup snapshot 9、engine pool 6；runtime-snapshot 构建零 warning。
- Task 1 GREEN：专用 `wjsm-gc-bench` CLI preflight 与 30 样本 ZGC baseline 在 180 秒上限内完成；runtime telemetry 1 项通过，benchmark crate tests 11 项全部 ignore，不参与 workspace 常规 gate。
- Task 2 GREEN：GC color bit 38–43 纯 helper 已加入；wjsm-ir 22 项与 snapshot-format 6 项通过，JS identity smoke 输出 `true`。

## Active slice

Phase A 统一审查：复核 Tasks 0–2 的平台可行性、measurement contract、NaN-box boundary 与 retire track；批准后才进入 Task 3。

## Evidence refs

- `docs/aegis/work/2026-07-16-zgc-performance/90-evidence.md`
- `docs/aegis/specs/2026-07-16-zgc-performance-design.md`
- `docs/aegis/plans/2026-07-16-zgc-performance.md`

## Blocked on

无。

## Next step

- 请求 Phase A 的规格符合性审查与代码质量审查（用户指定每个大阶段一次）。
- 无 Critical/Important finding 后开始 Task 3 的私有 `managed-heap-v2` control plane。

## ResumeStateHint

恢复时先读本文件、`90-evidence.md` 的 Task 0–2 段、父规格与实施计划；Phase A 的三个任务均已提交，当前等待统一 review，未启动 Task 3。

## DriftCheckDraft

- Scope：Task 2 仅在 `wjsm-ir::value` 定义 inactive color constants/helpers 与合同测试；不接入任何 active runtime owner。
- Compatibility：color 只占 payload bit 38–43；`strip_gc_color` 保留低 32 位 handle identity 与 tag；snapshot-format ABI hash owner不依赖 `wjsm-ir`。
- Retirement：无旧 owner 或 fallback 新增；ZGC 当前 entry color 仍是既有 runtime 私有实现，Task 16 才统一接入。
- Decision：Task 2 GREEN 并关闭；按用户指令先完成 Phase A 统一 reviewer gate，再继续 Task 3。
