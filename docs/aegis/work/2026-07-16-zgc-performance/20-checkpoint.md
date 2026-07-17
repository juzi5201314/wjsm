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

## Active slice

Task 1：固定 telemetry、benchmark CLI 与 JDK 25 diagnostic probe；准备 strict RED 测试边界。

## Evidence refs

- `docs/aegis/work/2026-07-16-zgc-performance/90-evidence.md`
- `docs/aegis/specs/2026-07-16-zgc-performance-design.md`
- `docs/aegis/plans/2026-07-16-zgc-performance.md`

## Blocked on

无。

## Next step

- 读取现有 GC telemetry、CLI/benchmark owners 与跨平台资源 provider 基线。
- 写 Task 1 CLI/schema/resource/gate RED 测试并运行计划前两条 RED 命令。

## ResumeStateHint

恢复时先读本文件、`90-evidence.md` Task 0 段、父规格与实施计划；Task 0 提交包含 engine-config 中心化与显式 per-engine snapshot ABI，active heap ABI 未改；当前从 Task 1 RED 开始。

## DriftCheckDraft

- Scope：Task 0 仅 engine config / fingerprint / shared memory64 可行性门。
- Compatibility：32 位 handle identity、ECMAScript、CLI 与 snapshot 开关保持；active heap ABI 未变化。
- Retirement：删除 runtime/support 内重复 `Config::new` owner；现有动态 runtime profile 的 support cwasm 编译 fallback 保持 active 行为，按 Task 15/26 退役。
- Decision：Task 0 GREEN 并关闭；按用户要求将 reviewer gate 延后到 Phase A（Task 0–2）结束，继续 Task 1。
