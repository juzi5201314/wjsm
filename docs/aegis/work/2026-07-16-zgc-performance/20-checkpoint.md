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

## Active slice

Task 2：添加非激活 NaN-box GC color helpers；不改变 active entry、backend store、snapshot ABI 或 runtime heap。

## Evidence refs

- `docs/aegis/work/2026-07-16-zgc-performance/90-evidence.md`
- `docs/aegis/specs/2026-07-16-zgc-performance-design.md`
- `docs/aegis/plans/2026-07-16-zgc-performance.md`

## Blocked on

无。

## Next step

- 读取 `wjsm-ir/src/value.rs` 的 NaN-box layout 与现有 value unit/property tests。
- 为所有 handle-backed tags/runtime string 与非引用值写 RED 边界测试。

## ResumeStateHint

恢复时先读本文件、`90-evidence.md` 的 Task 0/1 段、父规格与实施计划；Task 1 提交包含专用 benchmark CLI、fail-closed resource admission、JDK 25 probe 与 runtime telemetry；当前从 Task 2 RED 开始。

## DriftCheckDraft

- Scope：Task 1 限定 measurement/telemetry/benchmark 驱动；不改变 active collector 算法或性能阈值。
- Compatibility：全量 nextest 跳过 `wjsm-gc-bench` 的 11 个 ignore tests；benchmark 只通过专用 CLI 执行；错误 JDK 版本输出 `needs-verification` JSON，不把环境缺失变成通用测试失败。
- Retirement：旧 `gc_stress`/`zgc_autoresearch`/`zgc_barrier_pressure` 暂不删除，Task 26 统一迁移和退役。
- Decision：Task 1 GREEN 并关闭；本机无 JDK 25 patched probe 的指标保持 `needs-verification`，不伪造 counter，继续 Task 2。
