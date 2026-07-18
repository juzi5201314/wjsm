# TodoCheckpointDraft

## Current todo

Task 12 已完成并通过 feature/default gate；当前进入 Task 13（准备 V2 snapshot/support artifact ABI）。

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
- Task 1：专用 `wjsm-gc-bench` CLI、preflight 与历史 30 样本 baseline 已验证；Phase A 审查后 canonical workload 改为跨 WJSM/JDK contract，当前只完成 1 样本 smoke，30 样本新合同需在用户允许的专用 runner 窗口重跑。
- Task 2 GREEN：GC color bit 38–43 纯 helper 已加入；wjsm-ir 22 项与 snapshot-format 6 项通过，JS identity smoke 输出 `true`；Phase A 审查后 raw f64 保留 payload bit 的边界测试已通过。
- Task 3 GREEN：私有 `managed-heap-v2` feature 下 control plane 3/3 通过；`--no-default-features` default-options 1/1 通过，active runtime 未切换。
- Task 4 implementation：feature-gated `ManagedHeap<M>`、Shared/Native memory 后端与 4/4 integration tests 通过；Miri 在 180 秒依赖编译阶段超时，保留 `needs-verification`。
- Task 5 GREEN：feature-gated 32 GiB Wasmtime shared memory64 V2 handle region、8-byte high48/low16 atomic entry、64 KiB logical commit、epoch quarantine/reuse 完成；handle table 3、concurrency 1、Loom 1、default isolation 1 均通过。
- Task 6 GREEN：feature-gated 64 KiB page/NLAB allocator、heap-relative `ObjectRef`、atomic object map、双 mark bitmap、contiguous large/humongous 与 relocation reserve 完成；13 个 staged heap tests、default isolation 通过，`micro allocator --heap 256m --samples 30` admitted 且 passed。
- Task 7 implementation：feature-gated Store-free fixed `GcWorkerPool`、fixed-capacity packet slab、crossbeam local/injector/peer-steal、inflight drain/park/ordered join 完成；228 staged tests、deterministic queue steal、Loom termination、concurrent producer 均通过。TSan 与 Miri 仍为 `needs-verification`：TSan 已越过 host/target ABI 配置错误但 180 秒内未执行测试体；Miri 延续 Task 4 的依赖编译上限。
- Task 8 GREEN：`managed-heap-v2` user/support modules共同导入 min 32 GiB handle reserve、max 256 TiB high48 address ABI 的 shared memory64，给 control/object heap 留出独立 grow 区；i64 NLAB cursor globals、8-byte atomic entry 与 dynamic host ABI 已建立。默认 backend 64/64 与 V2 backend 59/59 通过；runtime feature support precompile 和 default isolation 通过。
- Task 9 GREEN：feature-gated 动态 host object/array/proxy/descriptor 路径以 `HeapAccessV2` 和 atomic handle table 判定对象 owner，默认 runtime 隔离保持通过。
- Task 10 GREEN：`MarkSweepV2` 在 `ManagedHeap<SharedHeapMemory>`、`HandleTableV2` 和 immutable `RootSnapshot` 上完成 mark/retire/sweep，默认 collector 未切换。
- Task 11 GREEN：feature-gated `G1V2` 仅拥有一张 `HandleTableV2`，复用 managed pages、双 mark bitmap、`GcWorkerPool`、`G1RSet` 与 `GcTelemetry`；young/mixed/full 回收、promotion failure、跨 cycle old-to-young 边和 promoted destination 重新入卡均已验证。
- Task 12 GREEN：feature-gated `ZgcV2` 在 `ManagedHeap<SharedHeapMemory>`、唯一 `HandleTableV2`、page/object map/current bitmap 上实现增量 `Mark → Relocate → Idle` safepoint cycle；无 concurrent worker 或 colored store，relocation 期间显式拒绝 reference mutation，默认 active ZGC 未切换。

## Active slice

Task 13：准备 feature-gated V2 snapshot/support artifact ABI，保持 active V1 format 不变。

## Evidence refs

- `docs/aegis/work/2026-07-16-zgc-performance/90-evidence.md`
- `docs/aegis/specs/2026-07-16-zgc-performance-design.md`
- `docs/aegis/plans/2026-07-16-zgc-performance.md`

## Blocked on

canonical workload 的 30 样本 baseline 重跑：用户要求不运行耗时程序且所有命令硬超时 180 秒；已有 1 样本 smoke，最终 distribution evidence 延后到计划终局专用 runner。

## Next step

- 从 `wjsm-snapshot-format`、startup snapshot build chain、runtime support cwasm ABI 与 Task 8 memory64 export 合同开始 Task 13。
- 保持 Task 10 `mark_sweep_v2`、Task 11 `g1_v2`、Task 12 `zgc_v2` 与默认 runtime gates 同时通过。

## ResumeStateHint

恢复时先读本文件、`90-evidence.md` 的 Task 0–12 段、父规格与实施计划；Task 12 已实现并完成验证，下一项为 Task 13。

## DriftCheckDraft

- Scope：Task 12 新增 feature-gated `ZgcV2`；每轮由 immutable `RootSnapshot` 启动，current bitmap 标记后在 safepoint 内同步复制，atomic handle relocation 保持对象 identity，dead object 在 relocate 前 retire。
- Compatibility：默认 active ZGC 和其 legacy `ZPageSpace`/color 状态机未切换；V2 不启动 worker、不引入 colored store，relocation 时拒绝 reference mutation。
- Retirement：V2 ZGC 不读取 legacy bump/page entry；legacy ZGC 仍为 default active path，仅在 Task 15/26 退役。
- Decision：Task 12 GREEN，进入 Task 13。
