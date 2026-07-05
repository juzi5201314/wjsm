# ADR 0005: Pluggable GC v2 Boundary

**Status**: Accepted after implementation verification. P0-P5 已落地：mark-sweep 默认、G1、ZGC 三算法可通过 `RuntimeOptions` / CLI `--gc` / `WJSM_GC` 选择；`WJSM_TEST_GC` 保留为测试矩阵 override。P6 收口中。

**Date**: 2026-07-05

## Context

ADR/计划 `2026-06-14-pluggable-gc-framework` 先建立了 mark-sweep 时代的 GC 抽象，但它仍隐含一个过窄的 INV-C：JS 对象堆实际 non-moving，host 侧裸写与 WASM fast-path 都可以长期持有 raw pointer。该边界不足以承载 G1/ZGC：

1. **moving GC 需要 handle 恒定而非 ptr 恒定**。host side table、N-API scope、ArrayBuffer/TypedArray/DataView、WeakRef/FinalizationRegistry 都必须以 handle 为跨 GC 稳定身份。
2. **引用槽写入必须统一进入 barrier owner**。host 写和 support module 写入过去分散在 object/array/proxy/collection helper 中，无法给 G1 RSet/SATB 或 ZGC SATB/load barrier 提供完整事件流。
3. **单一 support cwasm 不再成立**。mark-sweep 不需要 write/load barrier；G1 需要 24B SATB/RSet event；ZGC 还需要 colored obj_table entry 和 load barrier helper。
4. **可观测性必须成为契约**。pause、relocation、fragmentation、barrier、RSet、load barrier hit 等指标是三算法验收的一部分，不能只靠 fixture 是否通过。

## Decision

wjsm 采用 **Pluggable GC v2** 边界：

1. **INV-C 拆分为 INV-C1 / INV-C2**：
   - **INV-C1**：跨 GC 稳定身份是 NaN-boxed handle，不是 wasm linear memory pointer。G1/ZGC 可以移动对象；`obj_table[handle]` 是唯一 ptr owner。
   - **INV-C2**：allocation slow path、explicit `gc()`、safepoint 与 barrier flush 是明确 GC 点。复制/搬迁内部不得再次走 mutator allocation slow path。
2. **GC lifecycle 接口 v2**：算法实现 `GcAlgorithm` 的 attach/alloc_slow/safepoint_step/collect_full/barrier/on_host_write/load_barrier/stats hooks；`runtime_gc::registry` 是算法创建 owner。
3. **host 读写统一入口**：host 对 JS 对象引用槽的 proto/property/element 写入走 `runtime_gc::heap_access`，该模块负责 old/new value 捕获、write barrier 调用、ZGC colored ptr resolve。
4. **三 support module 物理变体**：`wjsm-runtime-support` build.rs 生成 mark-sweep/G1/ZGC 三份 cwasm；runtime startup 根据 active GC algorithm 选择对应 support flavor。
5. **G1 owner 拆分**：
   - `g1::region`：host-side region metadata。
   - `g1::rset`：dirty card、precise slot、SATB event buffer owner。
   - `g1::young`：STW young evacuation。
   - `g1::concurrent_mark`：incremental mark、old/humongous cleanup。
   - `g1::mixed`：old region CSet compaction。
6. **ZGC owner 拆分**：
   - `zgc::color`：low 2-bit color protocol、good color phase。
   - `zgc::page`：host-side zPage metadata。
   - `zgc::mark`：incremental mark、SATB、dead handle cleanup。
   - `zgc::relocate`：relocation set、copy/heal、source page reclaim。
7. **可观测性 source of truth**：`GcStats` v2 与 RuntimeState pause/footprint history 是 GC 验收与日志的 owner；`WJSM_GC_LOG=1` 输出每周期摘要。

## Consequences

### Positive

- 三算法共享同一 runtime contract；默认 mark-sweep 行为保持，G1/ZGC 通过 registry/support flavor 切换。
- moving GC 不再要求更新所有 JS 引用槽；槽内保持 handle，移动只更新 `obj_table`。
- host 与 WASM 双端 barrier 统一为同一事件语义，G1 与 ZGC 不需要各自重新扫描所有写路径。
- 三 support cwasm 明确反映算法物理差异，避免 mark-sweep 为 G1/ZGC barrier 付常态成本。
- Pause、footprint、fragmentation、barrier/load-barrier 指标可被测试与日志直接消费。

### Negative / Risks

- `obj_table` entry 低位在 ZGC 中带 color，所有 raw ptr consumer 必须经 `heap_access::resolve` 或显式去色；Eval inline helper 也必须遵守。
- 三 support artifact 增加 ABI 组合面；support layout/hash 变更需要同时验证 mark-sweep/G1/ZGC。
- G1/ZGC 的 moving semantics 使“临时 raw ptr 跨 allocation/GC 点”成为硬错误；resize/copy path 必须在 slow allocation 后重新解析 handle。
- Incremental GC 的正确性依赖 barrier flush 与 fixed-point roots；新增 side table owner 时必须加入 roots/cleanup 矩阵。

## Compatibility Boundary

- `wjsm-runtime` public execution API 保持兼容；新增 `RuntimeOptions::with_gc_algorithm` 与 CLI/env 选择是扩展。
- `WJSM_TEST_GC` 保留给测试矩阵，用户入口使用 `--gc` 或 `WJSM_GC`。
- Snapshot 边界仍由 ADR 0003/0004 约束：用户对象不进入 startup snapshot；support cwasm 作为 build-time artifact 由 ADR 0004 的 ABI hash 纪律覆盖。
- N-API/host side table 对外仍暴露 stable handle/backing store 有效窗口；不承诺 JS object raw ptr 稳定。

## Alternatives Considered

### 纯 STW mark-sweep 扩展

放弃。实现成本低，但无法满足 G1/ZGC pause/fragmentation 目标，也无法验证 moving GC root/barrier 边界。

### 真线程并发 GC

放弃。当前 wasmtime Store/Caller 与 RuntimeState 访问模型是同步、单线程 owner；引入真线程会立即扩大 scheduler/host import/side table 同步面。v2 采用 safepoint budgeted incremental step，先获得可验证 pause 行为。

### 单 support module runtime switch

放弃。把 G1/ZGC barrier/load-barrier 全塞入一个 support cwasm 会让 mark-sweep fast path 永久承担条件分支/host call 成本，也让 ABI hash 无法表达算法物理边界。

### Moving GC 直接重写所有引用槽

放弃。所有对象/闭包/side table/N-API scope 的引用槽重写成本和遗漏风险过高。handle 恒定 + `obj_table` 更新是更小、更可审计的移动边界。

## Status of Implementation

| Slice | Status | Evidence |
|---|---|---|
| P0 GC lifecycle v2 + mark-sweep registry | ✅ | `cargo nextest run --workspace` 持续通过 |
| P1 layout/globals/support parameterization | ✅ | 27 globals + alloc window + support flavor ABI |
| P2 host heap access owner | ✅ | `heap_access` 接管 proto/property/element writes；resize re-resolve |
| P3 G1 | ✅ | region/RSet/young/concurrent mark/mixed + G1 workspace/happy 验证 |
| P4 ZGC | ✅ | color/page/support/mark/relocate + ZGC workspace/happy 验证 |
| P5 observability | ✅ | `GcStats` v2、pause/footprint bench、regression matrix |
| P6 cleanup/docs/ADR | ⏳ | 本 ADR + final matrix 待 T6.4 收口 |

## Baseline Sync

- `AGENTS.md` 已同步 WASM contract、27 globals、三 support cwasm 变体、GC 选择入口。
- `docs/aegis/specs/2026-07-03-napi-native-addon-design.md` 已将 non-moving 假设改为 handle 恒定（INV-C1）。
- `docs/aegis/INDEX.md` Baselines 登记本 ADR。

## References

- [ADR 0002: RuntimeState 保持扁平的侧表集合](0002-runtimestate-stays-flat.md)
- [ADR 0003: Startup Snapshot Boundary](0003-startup-snapshot-boundary.md)
- [ADR 0004: Build-Time Embedded Runtime](0004-build-time-embedded-runtime.md)
- Spec: `docs/aegis/specs/2026-07-03-pluggable-gc-v2-design.md`
- Plan/work evidence: `docs/aegis/work/2026-07-03-gc-v2/90-evidence.md`
