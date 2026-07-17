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
- Task 4 implementation：feature-gated `ManagedHeap<M>`、Shared/Native memory 后端与 4/4 integration tests 通过；Miri 在 180 秒依赖编译阶段超时，保留 `needs-verification`。
- Task 5 GREEN：feature-gated 32 GiB Wasmtime shared memory64 V2 handle region、8-byte high48/low16 atomic entry、64 KiB logical commit、epoch quarantine/reuse 完成；handle table 3、concurrency 1、Loom 1、default isolation 1 均通过。

## Active slice

Task 6：实现 page/NLAB/object map/bitmap allocator，并在既有 benchmark CLI 注册 `micro allocator` component；仍在私有 feature gate 下。

## Evidence refs

- `docs/aegis/work/2026-07-16-zgc-performance/90-evidence.md`
- `docs/aegis/specs/2026-07-16-zgc-performance-design.md`
- `docs/aegis/plans/2026-07-16-zgc-performance.md`

## Blocked on

canonical workload 的 30 样本 baseline 重跑：用户要求不运行耗时程序且所有命令硬超时 180 秒；已有 1 样本 smoke，最终 distribution evidence 延后到计划终局专用 runner。

## Next step

- 读取现有分配器、GC object metadata 与 Task 5 layout，定义 64 KiB heap-relative page/NLAB/object map 边界。
- 写 allocator 与 micro component RED tests；不修改 active runtime allocator 或默认 benchmark contract。

## ResumeStateHint

恢复时先读本文件、`90-evidence.md` 的 Task 0–5 段、父规格与实施计划；Task 5 commit 已落地，Miri protocol verification 受 180 秒约束保留，当前从 Task 6 RED 开始。

## DriftCheckDraft

- Scope：Task 5 在 `managed-heap-v2` 下新增独立 memory64 32 GiB V2 table；active 4-byte `obj_table`、Linker ABI 与 collector 不变。
- Compatibility：entry ABI 固定为 high48 address / low16 state；`resolve` 为 region base + `handle * 8` 的直接 SeqCst entry load，不经过 block map 或锁。
- Retirement：Task 5 原 sparse `BTreeMap` staging owner 已移除；V2 region 仍仅由 feature test 触达，Task 15 单点切换时才会退休 active old table。
- Decision：Task 5 GREEN 并提交；Task 4 Miri evidence 仍为 `needs-verification`，继续 Task 6。
