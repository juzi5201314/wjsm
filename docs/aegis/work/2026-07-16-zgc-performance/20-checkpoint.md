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
- Task 1：专用 `wjsm-gc-bench` CLI、preflight 与历史 30 样本 baseline 已验证；Phase A 审查后 canonical workload 改为跨 WJSM/JDK contract，当前只完成 1 样本 smoke，30 样本新合同需在用户允许的专用 runner 窗口重跑。
- Task 2 GREEN：GC color bit 38–43 纯 helper 已加入；wjsm-ir 22 项与 snapshot-format 6 项通过，JS identity smoke 输出 `true`；Phase A 审查后 raw f64 保留 payload bit 的边界测试已通过。
- Task 3 GREEN：私有 `managed-heap-v2` feature 下 control plane 3/3 通过；`--no-default-features` default-options 1/1 通过，active runtime 未切换。

## Active slice

Task 4：实现泛型 `HeapMemory`、`SharedHeapMemory` 与 native/Miri 测试后端；仍在私有 feature gate 下。

## Evidence refs

- `docs/aegis/work/2026-07-16-zgc-performance/90-evidence.md`
- `docs/aegis/specs/2026-07-16-zgc-performance-design.md`
- `docs/aegis/plans/2026-07-16-zgc-performance.md`

## Blocked on

canonical workload 的 30 样本 baseline 重跑：用户要求不运行耗时程序且所有命令硬超时 180 秒；已有 1 样本 smoke，最终 distribution evidence 延后到计划终局专用 runner。

## Next step

- 读取现有 Wasmtime shared memory API 与 Task 3 control plane，设计 sealed `HeapMemory` trait。
- 写 managed heap memory 与 Miri protocol RED 测试；不修改 Linker active memory。

## ResumeStateHint

恢复时先读本文件、`90-evidence.md` 的 Task 0–3 段、父规格与实施计划；Task 3 commit 已落地，当前从 Task 4 RED 开始；最终 reviewer 仅在全计划完成后执行。

## DriftCheckDraft

- Scope：Task 3 仅新增默认关闭 feature 下的 Store-free control plane；旧 RuntimeState/GcContext/collector 实现不变。
- Compatibility：`gc_control_plane` integration target 在 feature 关闭时自身 cfg-out，`--no-default-features` 不会编译 gated imports。
- Retirement：private feature 和 V2 types 的删除仍由 Task 15 单点切换负责；无 active fallback 或双 runtime path 新增。
- Decision：Task 3 GREEN 并关闭；继续 Task 4，最终 canonical 30-sample distribution 仍为 `needs-verification`。
