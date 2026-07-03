# GC v2 执行状态追踪

**计划**: `docs/aegis/plans/2026-07-03-pluggable-gc-v2.md`  
**设计规格**: `docs/aegis/specs/2026-07-03-pluggable-gc-v2-design.md`  
**开始日期**: 2026-07-03  
**当前状态**: 就绪待执行

---

## Issue 状态总览

### 已关闭（统一到 v2 计划）

| Issue | 标题 | 关闭原因 | v2 对应 |
|---|---|---|---|
| #328 | 内存管理与 GC 模块审查报告 | completed | 父级追踪，全部子 issue 已解决或并入 v2 |
| #330 | 实现增量 GC 调度与有界暂停 | superseded | P0-P2 框架 + P3/P4 算法增量步进 |
| #335 | 建立 linear memory footprint 治理 | completed | P5 T5.4 补齐 committed_pages/reusable 指标 |
| #336 | 实现分代 GC、写屏障与 remembered set | superseded | P3 G1 完整实现 |
| #338 | 建立生产级 GC 内存正确性回归矩阵 | completed | P5 T5.5 审计清单 + 补充遗漏项 |

### 已完成（作为 v2 baseline）

| Issue | 标题 | 完成日期 | v2 集成点 |
|---|---|---|---|
| #331 | 修复 side-table-backed JS 引用的 GC 可达性缺口 | 2026-07-02 | P0 baseline（已交付） |
| #332 | 实现碎片治理：压缩、搬迁或 region evacuation | 2026-07-02 | P0 T0.6 集成 heap_governance.rs |
| #333 | 将 shadow stack 改为可增长并提供可恢复栈溢出路径 | 2026-07-02 | P0 baseline（已交付） |
| #334 | 将 host side table 扫描改为 reachability-aware | 2026-07-02 | P0 baseline（已交付） |
| #337 | 引入受控堆上限与 OOM 语义 | 2026-07-02 | P0 baseline（已交付） |
| #339 | 暴露生产级 GC 可观测性与诊断工具 | 2026-07-02 | P5 GcStats v2 扩展基础 |

---

## 阶段执行清单

### P0: 框架 v2 + mark-sweep 迁移（6 任务）

- [ ] T0.1 api.rs v2 类型（新增，不删旧）
- [ ] T0.2 MarkSweepCollector impl v2
- [ ] T0.3 调用方切换 + 删 v1 trait
- [ ] T0.4 registry.rs
- [ ] T0.5 scheduler.rs 骨架
- [ ] T0.6 集成现有 #332 实现到 mark-sweep v2

**验收**: 全量 fixture 绿 + `gc_fragmentation_churn` 绿 + 零 warning

---

### P1: 布局层（6 任务）

- [ ] T1.1 immortal 边界 + snapshot format 升级
- [ ] T1.2 新 globals ×10（backend + WasmEnv）
- [ ] T1.3 分配 fast-path 重构（窗口 bump + counter 内联）
- [ ] T1.4 host imports 换代
- [ ] T1.5 emitter 参数化（GcFlavor，仅 MarkSweep 变体）
- [ ] T1.6 阶段验证

**验收**: 全量 fixture 绿 + dump-wat 检查分配序列 + snapshot 冷/热双路绿

---

### P2: host 统一读写层 + INV-C2 审计（7 任务）

- [ ] T2.1 heap_access.rs + epoch 断言
- [ ] T2.2 裸写点清单
- [ ] T2.3 裸写点替换（批次 1）
- [ ] T2.4 裸写点替换（批次 2）
- [ ] T2.5 裸写点替换（批次 3）
- [ ] T2.6 WASM 侧 INV-C2（resize re-resolve + emit_resolve_handle_ptr）
- [ ] T2.7 阶段验证

**验收**: debug 构建全量 fixture 绿（断言开启）+ 裸写点清单勾销完成

---

### P3: G1（9 任务，可与 P4 并行）

- [ ] T3.0 `WJSM_TEST_GC` 矩阵机制
- [ ] T3.1 region.rs（域组织 + attach_heap）
- [ ] T3.2 card.rs + host 侧 barrier
- [ ] T3.3 g1 变体 barrier 代码生成 + 第二份 cwasm
- [ ] T3.4 young.rs（young GC）
- [ ] T3.5 concurrent_mark.rs（增量标记）
- [ ] T3.6 mixed.rs（CSet evacuation）
- [ ] T3.7 mod.rs 组装 + registry 接入
- [ ] T3.8 阶段验证

**验收**: GC 子集 @ g1 绿 + `WJSM_TEST_GC=g1` 全量绿 + pause 初测

---

### P4: ZGC（6 任务，可与 P3 并行）

- [ ] T4.1 color.rs + page.rs
- [ ] T4.2 zgc 变体 load barrier + 第三份 cwasm
- [ ] T4.3 mark.rs（增量标记）
- [ ] T4.4 relocate.rs（增量搬迁 + 强制 heal）
- [ ] T4.5 mod.rs 组装 + registry
- [ ] T4.6 阶段验证

**验收**: `WJSM_TEST_GC=zgc` 全量绿 + relocate 期 host 读写专项测试绿

---

### P5: 选择机制 + 可观测性 + 定量基准（6 任务）

- [ ] T5.1 三入口选择
- [ ] T5.2 GcStats v2 + pause 直方图
- [ ] T5.3 gc_pause_bench.rs（定量基准）
- [ ] T5.4 linear memory footprint 治理指标（并入 #335）
- [ ] T5.5 GC 回归矩阵完整性审计（并入 #338）
- [ ] T5.6 阶段验证

**验收**: spec §21.2 定量项达标 + footprint 指标齐全 + 回归清单勾销

---

### P6: 清理 + 文档 + 终验（4 任务）

- [ ] T6.1 删除清单执行
- [ ] T6.2 文档同步
- [ ] T6.3 ADR 0005
- [ ] T6.4 全矩阵终验

**验收**: grep 无残留 + 三算法全量绿 + 零 warning + 执行状态更新为完成

---

## 验收总清单（最终）

### 功能验收（spec §21.1）

- [ ] 全量 fixture（470+）默认 mark-sweep 全绿
- [ ] GC 密集子集在 g1/zgc 各自全绿
- [ ] `WJSM_TEST_GC={g1,zgc}` 全量 fixture 手动矩阵全绿
- [ ] 长循环不 OOM（三算法）
- [ ] 三算法 handle 复用正常
- [ ] snapshot 三算法均正常恢复 + ABI hash 更新后冷启动
- [ ] INV-C2 debug epoch 断言零触发
- [ ] 删除清单执行完毕 + grep 无残留 + 零 warning
- [ ] `--gc`/`WJSM_GC`/`RuntimeOptions` 三入口优先级正确

### 定量验收（spec §21.2）

- [ ] g1 young GC 单次 pause max ≤8ms 且 ≤ mark-sweep full 的 1/5
- [ ] zgc 任意 pause max ≤8ms 且 ≤ mark-sweep full 的 1/5
- [ ] 三算法 churn 总耗时均 ≤ mark-sweep 基线 × 1.25
- [ ] g1/zgc `external_fragmentation` < mark-sweep 同负载值
- [ ] 所有指标来自 GcStats v2 实测，benchmark 断言阈值

### Footprint 验收（#335 并入）

- [ ] GcStats 含 `committed_pages`、`free_bytes_reusable` 字段
- [ ] 长运行测试验证对象存活量下降后优先复用空闲区域
- [ ] memory.size 稳态（GC 后不持续增长）

### 回归矩阵验收（#338 并入）

- [ ] 回归覆盖清单文档（`regression-matrix-coverage.md`）生成并勾选
- [ ] 每类历史缺口有可运行测试
- [ ] 补充遗漏项（Proxy + GC、BoundFunction 多轮 churn）
- [ ] 新增 fixture 三算法矩阵绿

---

## 工作日志

### 2026-07-03

- ✅ 更新计划文件，标记 #331-#334/#337/#339 为已完成 baseline
- ✅ 标记 #330/#336 为被 v2 取代（v1 trait 不兼容）
- ✅ 并入 #335（footprint）和 #338（回归矩阵）验收到 P5
- ✅ 修正 T0.6 任务描述：#332 已完成，改为集成现有实现
- ✅ 关闭全部 GC 相关 open issues（#328/#330/#335/#336/#338）
- ✅ 确认 `memory-safety` label 下无遗漏 open issues
- ✅ 创建本执行状态追踪文档

**下一步**: 开始执行 P0 T0.1

---

## 风险与阻塞

_执行期间发现的新风险或阻塞在此记录_

---

## 完成标志

P0-P6 全部任务勾选完成 + 全部验收清单通过 → 回 #328 评论确认 → 本文档归档。
