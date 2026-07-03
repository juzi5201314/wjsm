**执行状态**: 未开始。P0-P6 待执行（P3/P4 可并行）。

# 可插拔 GC v2 实施计划（mark-sweep / G1 / ZGC）

**Goal**: 落地 `docs/aegis/specs/2026-07-03-pluggable-gc-v2-design.md`：框架 v2（生命周期完整算法接口 + 增量调度 + 双端 barrier 通道 + host 统一读写层）、INV-C 重写（INV-C1/C2）、三算法（mark-sweep 默认 / g1 / zgc）、三变体 support module、启动时算法选择、定量 pause 验收。

**Architecture**: 见 spec §4。五 crate 协同：`wjsm-runtime`（runtime_gc v2 + heap_access）、`wjsm-backend-wasm`（三变体 emitter + 分配窗口 + barrier 代码生成）、`wjsm-runtime-support`（三份 cwasm）、`wjsm-snapshot-format`（immortal 区）、`wjsm-cli`（`--gc`）。

**Tech Stack**: Rust 2024, wasmtime（epoch interruption，sync `Func::wrap`）, wasm-encoder。测试 `cargo nextest`（per-test ~9s 超时）。

**Baseline/Authority Refs**:
- `docs/aegis/specs/2026-07-03-pluggable-gc-v2-design.md`（本计划唯一设计来源；§22 不变量清单为硬约束）
- `docs/aegis/specs/2026-06-14-pluggable-gc-framework-design.md`（v1，附录 D 承诺已被 v2 spec 附录 B 取代）
- `docs/adr/0003`/`0004`（snapshot/embedded runtime 边界）
- `docs/aegis/specs/2026-07-03-napi-native-addon-design.md`（§15.2 措辞修订对象）
- **已完成 issues 作为 baseline**：#331（side-table GC 可达性）、#332（碎片治理，`heap_governance.rs` 已交付）、#333（growable shadow stack）、#334（reachability-aware side table）、#337（heap limit/OOM）、#339（GC observability）
- **待取代 issues**：#330（增量调度，v1 接口路径）、#336（分代 GC，v1 接口路径）
- **待并入验收**：#335（linear memory footprint 治理，部分覆盖需补全）、#338（GC 回归矩阵，转为 v2 验收资产）

**Compatibility Boundary**（必须保持）:
- 全量 fixture stdout/语义不变（默认 mark-sweep 下全绿）
- NaN-boxing / obj_table 间接 / 活动对象布局（16B header）不变
- `gc()` global 行为保持；`WJSM_STARTUP_SNAPSHOT` 开关语义不变
- safepoint spill 体系（Layer 1/2/3）不变
- spec §22 全部 INV/IMPL 不变量

**Verification**:
- 每阶段末 `cargo nextest run --workspace` 全绿 + 零 warning
- P3 起 GC 密集子集在 `WJSM_TEST_GC=g1|zgc` 下验证
- P5 定量：`WJSM_GC_BENCH=1` churn 基准达 spec §21.2 阈值

**ADR Signal**（保留至完成回填）: INV-C 重写、GcAlgorithm v2 边界、三变体物理边界、增量调度决策 → `docs/adr/0005-pluggable-gc-v2.md`（P6）。baseline-sync：AGENTS.md/CLAUDE.md WASM contract（globals 19→29、host funcs 变更）与 GC 描述。

---

## BaselineUsageDraft

```text
- Required baseline refs: v2 spec（全文）/ v1 spec 附录 D / ADR 0003/0004 / napi spec §4.2+§4.10 / 已完成 issues #331-#334/#337/#339 实现
- Acknowledged before plan refs: 全部（brainstorming 阶段逐文件行号调研）
- Cited in plan refs: v2 spec 各节（任务内 §引用）；v1 plan（任务粒度惯例）
- Missing refs: 无
- Decision: continue
```

## Plan Pressure Test

```text
- Owner / contract / retirement:
    Owner: runtime_gc/{api,registry,scheduler,heap_access,mark_sweep,g1,zgc}（算法单 owner）；
           support emitter 单源参数化（IMPL-19）；roots.rs 共享单 owner。
    Contract: GcAlgorithm v2（spec §6）+ globals/imports 并集（§7.3/7.4）+ INV-C1/C2/E。
    Retirement: v1 trait/gc_maybe_collect 在对应阶段删除/重整（spec §18 清单），
                每阶段 grep 无残留；#330/#336 由本计划取代（v2 接口不兼容 v1 预留桩）
- Architecture integrity / higher-level path: obj_table 间接 = 天然 forwarding 层已被设计利用
    （零引用修正）；heap_access 收敛 host 写点是唯一能杜绝 barrier 漏写的边界。无更高层简化遗漏。
- Verification scope: 阶段独立验证（P0/P1 全量 fixture；P2 断言开启；P3/P4 子集+矩阵；P5 定量）。
- Task executability: 每任务给出文件路径 + 关键签名/指令序列（或 spec 精确 §引用）+ 确切命令。
- Pressure result: proceed
```

## Complexity Budget / Plan-Time Complexity Check

```text
- Artifact class: 跨 crate 架构重构（运行时子系统替换）
- Target files: 见下方 File Map
- Current pressure: runtime_gc 3.7k 行；host_imports 34 文件 21k 行（裸写点分散）；
    support_object_helpers.rs 1318 行（变体化改造对象）
- Projected post-change pressure: runtime_gc ~9k 行（g1/zgc 各 5 子模块，每文件 ≤500 行）；
    host_imports 写点收敛到 heap_access 后净复杂度下降
- Budget result: at-risk → 治理：g1/zgc 强制子模块拆分（region/card/young/concurrent_mark/mixed；
    color/page/mark/relocate）；P2 分 4 批替换防单任务过大；emitter 变体差异只以 match flavor
    局部分支表达（禁复制）
- Recommendation: add owner file（g1//zgc//heap_access.rs/registry.rs/scheduler.rs）+ split task
```

## File Map

**新建**：
```
crates/wjsm-runtime/src/runtime_gc/registry.rs        # GcRegistry + GcAlgorithmKind
crates/wjsm-runtime/src/runtime_gc/scheduler.rs       # StepBudget/trigger 自适应/pause target
crates/wjsm-runtime/src/runtime_gc/heap_access.rs     # host 统一读写层（§13）
crates/wjsm-runtime/src/runtime_gc/g1/{mod,region,card,young,concurrent_mark,mixed}.rs
crates/wjsm-runtime/src/runtime_gc/zgc/{mod,color,page,mark,relocate}.rs
crates/wjsm-runtime/tests/gc_pause_bench.rs           # 定量基准（WJSM_GC_BENCH 门控）
fixtures/happy/gc_g1_young_churn.{js,expected} 等 GC 密集新 fixture
docs/adr/0005-pluggable-gc-v2.md                      # P6 回填
docs/aegis/work/2026-07-03-gc-v2/bare-write-audit.md  # P2 裸写点勾销清单
```

**重写/大改**：
```
crates/wjsm-runtime/src/runtime_gc/api.rs             # v2 trait（spec §6 全量替换）
crates/wjsm-runtime/src/runtime_gc/mod.rs             # 模块组织 + 文档更新
crates/wjsm-runtime/src/runtime_gc/mark_sweep/*.rs    # 迁移至 v2 + 集成现有 #332 heap_governance
crates/wjsm-runtime/src/runtime_gc/heap_governance.rs # 保留并重整（已完成，非 WIP；归入 mark_sweep 内部使用）
crates/wjsm-backend-wasm/src/support_module.rs        # emit_support_module(GcFlavor)
crates/wjsm-backend-wasm/src/support_object_helpers.rs# 变体 barrier + resize re-resolve
crates/wjsm-backend-wasm/src/compiler_helpers/helpers_object.rs   # 窗口 bump + counter 内联（eval inline 路径）
crates/wjsm-backend-wasm/src/compiler_array_helpers.rs            # 同上
crates/wjsm-runtime-support/build.rs                  # 三份 cwasm
crates/wjsm-runtime/src/lib.rs                        # RuntimeOptions::gc_algorithm + registry 装配 + gc_epoch
crates/wjsm-runtime/src/runtime_heap.rs               # host 分配接 v2 + heap_access
crates/wjsm-runtime/src/host_imports/core.rs          # gc_safepoint_poll/gc_barrier_flush/gc_load_barrier_slow
crates/wjsm-snapshot-format/src/lib.rs                # immortal 边界字段 + abi_hash 输入
crates/wjsm-cli/src/*                                 # --gc flag
tests/fixture_runner.rs                               # WJSM_TEST_GC 矩阵注入（T3.0）
```

**批量修改**（P2 审计）：`host_imports/*.rs` 全部裸写点 → heap_access。

---

## Tasks 总览

| 阶段 | 任务 | 独立验证 | 提交粒度 |
|------|------|----------|----------|
| P0 | T0.1-T0.6 | 框架 v2 + mark-sweep 迁移 + #332 收尾；全量 fixture 绿 | 每任务一提交 |
| P1 | T1.1-T1.6 | immortal 区 + 分配窗口 + emitter 参数化（仅 MS 变体）；快照冷/热双路绿 | 同上 |
| P2 | T2.1-T2.7 | heap_access + 裸写点四批替换 + WASM resize re-resolve；断言开启全绿 | 同上 |
| P3 | T3.0-T3.8 | G1 全量；子集 @ g1 绿 + `WJSM_TEST_GC=g1` 全量绿 | 同上 |
| P4 | T4.1-T4.6 | ZGC 全量；同上 @ zgc + relocate 期 host 专项 | 同上 |
| P5 | T5.1-T5.4 | 选择机制 + GcStats v2 + benchmark 定量达标 | 同上 |
| P6 | T6.1-T6.4 | 删除清单 + 文档/ADR + 全矩阵终验 | 同上 |

每任务通用步骤（不再逐任务重复）：**(1) 写测试/fixture → (2) 确认 RED（新行为）或基线绿（重构）→ (3) 实现 → (4) `cargo nextest run --workspace` 绿 + `cargo build` 零 warning → (5) 提交（消息含阶段任务号）**。

---

## P0：框架 v2 + mark-sweep 迁移 + #332 收尾

### T0.1 api.rs v2 类型（新增，不删旧）

**Files**: `runtime_gc/api.rs`
**内容**: 按 spec §6 原文新增 `AllocRequest`/`StepBudget`/`StepOutcome`/`GcAlgorithm`(v2 trait，先命名 `GcAlgorithmV2` 与旧 trait 共存至 T0.3)。`GcContext` 增 `gc_epoch()`（读 `RuntimeState.gc_epoch: AtomicU64`，lib.rs 同任务加字段）与 `alloc_window_set(ptr, end)`（写 `__alloc_ptr`/`__alloc_end`——global 本阶段尚不存在，方法先按 `Option<Global>` 容错，P1 接通）。
**Why**: v2 接口先行，后续任务全部编译期锚定。
**Verification**: `cargo nextest run -p wjsm-runtime` 绿（纯新增无行为变化）。

### T0.2 MarkSweepCollector impl v2

**Files**: `runtime_gc/mark_sweep/mod.rs`
**内容**:
```rust
impl GcAlgorithmV2 for MarkSweepCollector {
    fn name(&self) -> &'static str { "mark-sweep" }
    fn attach_heap(&mut self, ctx: &mut GcContext, dynamic_start: usize) {
        // mark-sweep: 动态域 = 连续 bump；记录 dynamic_start 供尾部回收下界（TRAIL-4 对齐）
        self.dynamic_start = dynamic_start;
    }
    fn alloc_slow(&mut self, ctx, roots, req) -> Option<usize> {
        // v1 语义重排：free list → bump → collect_full → free list/bump → grow → None
        // （迁移自现 alloc_slow + host_imports/core.rs gc_alloc_slow 的 collect/grow 序列，
        //   collect 逻辑收进算法内，trampoline 只做参数解包与 trap）
    }
    fn safepoint_step(&mut self, ctx, roots, _budget) -> StepOutcome {
        // 阈值判断由调度器完成；进入即整轮 collect_full → CycleComplete
    }
    fn collect_full(&mut self, ctx, roots) -> GcStats { /* = v1 collect_with_provider 全逻辑 */ }
    fn last_stats(&self) -> &GcStats { &self.stats_cache }
}
```
freed handles 入 `handle_free_list`、weak refs、owner-backed 侧表回收协议原样保留在 `collect_full` 尾部。
**Verification**: 单元测试（现有 mark_sweep 测试改走 v2 入口副本）+ workspace 绿。

### T0.3 调用方切换 + 删 v1 trait

**Files**: `lib.rs`（`gc_algorithm: Arc<Mutex<Box<dyn GcAlgorithmV2 + Send + Sync>>>`）、`host_imports/core.rs`（`gc_alloc_slow`/`gc_maybe_collect`/`gc()` trampoline 改调 v2）、`runtime_heap.rs`（`collect_for_host_alloc`→`collect_full`；`try_gc_for_host_alloc` 同）、`runtime_gc/api.rs`（删 `Allocator`/`Marker`/`Sweeper`/`WriteBarrier`/`ReadBarrier`/`HeapRegionManager`/`MarkProgress`；`GcAlgorithmV2` 更名 `GcAlgorithm`）、`runtime_gc/mark_sweep/{mod,marker,sweeper,allocator}.rs`（删除 v1 trait impl 块；marker/sweeper 逻辑已在 T0.2 迁入 `collect_full` 内部，保留为私有函数或内联；allocator 逻辑并入 `alloc_slow`）。
**Verification**: `grep -rn 'trait Allocator\|trait Marker\|trait Sweeper\|WriteBarrier\|ReadBarrier\|HeapRegionManager' crates/wjsm-runtime/src/` 零命中；`grep -rn 'impl Marker\|impl Sweeper\|impl Allocator' crates/wjsm-runtime/src/runtime_gc/` 零命中；全量 fixture 绿。

### T0.4 registry.rs

**Files**: 新建 `runtime_gc/registry.rs`；`lib.rs` 装配点替换。
```rust
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GcAlgorithmKind { MarkSweep, G1, Zgc }
impl std::str::FromStr for GcAlgorithmKind { /* "mark-sweep"|"g1"|"zgc"，错误列出合法值 */ }
/// P3/P4 接入前对 G1/Zgc 返回 Err（装配期显式拒绝，非 stub 算法）。
pub fn create(kind: GcAlgorithmKind) -> Result<Box<dyn GcAlgorithm + Send + Sync>, String> {
    match kind {
        GcAlgorithmKind::MarkSweep => Ok(Box::new(MarkSweepCollector::new())),
        GcAlgorithmKind::G1 => Err("g1 尚未接入（P3）".into()),   // T3.7 替换为 Ok
        GcAlgorithmKind::Zgc => Err("zgc 尚未接入（P4）".into()), // T4.5 替换为 Ok
    }
}
```
`RuntimeState::new_*` 经 `RuntimeOptions.gc_algorithm`（本阶段默认 `MarkSweep`，选项面 P5 开放）。
**Verification**: 单测 FromStr/装配；workspace 绿。

### T0.5 scheduler.rs 骨架

**Files**: 新建 `runtime_gc/scheduler.rs`；`host_imports/core.rs` 的 `gc_maybe_collect` 内部改经调度器（对 mark-sweep 行为等价：阈值达→collect_full，继承 `update_gc_threshold_after_collection` 自适应；此处仅换居所，WASM 侧仍每分配调用——P1 才内联 counter）。
```rust
pub struct GcScheduler {
    pub pause_target: Duration,        // WJSM_GC_PAUSE_TARGET_MS 默认 4ms
    pub trigger_bytes: usize,          // 初始 256KB；idle/cycle 双档自适应（spec §12）
    step_work_bytes: usize,            // [64KB, 8MB] 自适应
}
impl GcScheduler {
    pub fn budget(&self) -> StepBudget { StepBudget { work_bytes: self.step_work_bytes,
        deadline: Instant::now() + self.pause_target } }
    pub fn after_step(&mut self, outcome: &StepOutcome, elapsed: Duration) { /* 超时减半/富余倍增 */ }
}
```
**Verification**: 调度器单测（自适应收敛）；workspace 绿。

### T0.6 集成现有 #332 实现到 mark-sweep v2

**Files**: `runtime_gc/heap_governance.rs`（已完成模块，保留并按需调整接口以配合 v2）、`runtime_gc/mark_sweep/sweeper.rs`（集成 heap_governance 调用：尾部空间回收 + 碎片指标计算）、`api.rs`（GcStats 已含 #332 碎片字段，确认保留）、`lib.rs`（`last_gc_stats` 存储点确认就位）。
**内容**: #332 已于 commit `1fc1e8a` 完成交付（`heap_governance.rs` + 测试 + fixture 全绿）；本任务将其集成到 mark-sweep v2 的 `collect_full` 流程中——sweep 后调用 `reclaim_trailing_free_space` + `compute_metrics`，碎片指标写入 `GcStats`。确认现有 `tests/heap_governance.rs` 与 `fixtures/happy/gc_fragmentation_churn.*` 在 v2 迁移后仍绿。
**Verification**: `cargo nextest run -E 'test(heap_governance)'` + `-E 'test(happy__gc_fragmentation_churn)'` 绿；全量绿。

---

## P1：布局层（immortal 区 + 分配窗口 + emitter 参数化）

### T1.1 immortal 边界 + snapshot format 升级

**Files**: `wjsm-snapshot-format/src/lib.rs`（format 版本 +1；新字段 `immortal_end_rel: u32`（相对 object_heap_start）；`abi_hash()` 输入追加 v2 布局常量：REGION_SIZE=65536/CARD_SIZE=512/globals 并集名单）、`wjsm-runtime/src/runtime_startup.rs`（restore 后 `RuntimeState.immortal_end` = 快照堆末端 64KB 上取；冷启动 = bootstrap 完成后当场划定）、`lib.rs`（`immortal_end: usize` 字段 + `attach_heap` 调用点：实例化完成后对当前算法调 `attach_heap(ctx, immortal_end)`）。
**Why**: 三算法统一的永久区边界（INV-E），g1/zgc 域起点。
**Verification**: 快照冷启动（`WJSM_STARTUP_SNAPSHOT=0`）与热恢复双路径全量 fixture 绿；`abi_hash` 单测更新。

### T1.2 新 globals ×10（backend + WasmEnv）

**Files**: `compiler_module/module_setup.rs`（globals 定义/导入/re-export：`__alloc_ptr __alloc_end __gc_alloc_bytes __gc_trigger_bytes __gc_phase __good_color __card_table_base __region_meta_base __satb_ptr __satb_end`，语义表 = spec §7.3）、`wjsm-runtime/src/wasm_env.rs`（对应 `Option<Global>` 字段 + `from_caller` 解析）、`support_module.rs`（同名 import 对齐）。
**三变体一致性约束**（IMPL-19 单源保证）：所有 10 个 globals 在三变体的 `module_setup.rs` + `support_module.rs` 中**全部声明**（import/re-export），保证 `WasmEnv::from_caller` 解析时字段集一致。mark-sweep 变体的 WASM 代码不读写 `__good_color`/`__card_table_base`/`__region_meta_base`/`__satb_*`（保持初始值），仅在 P3/P4 emitter 变体臂中生成对应指令。
**Verification**: dump-wat 检查 globals 段（三变体各含全部 10 个 globals 声明）；全量绿（globals 未被读写，纯扩容约定）。

### T1.3 分配 fast-path 重构（窗口 bump + counter 内联）

**Files**: `compiler_helpers/helpers_object.rs`（`$obj_new`）、`compiler_array_helpers.rs`（`$arr_new`）、`support_object_helpers.rs`（support 版同步）。
**内容**: 按 spec §7.2 指令序列重写：删除头部无条件 `call gc_maybe_collect`；插入 `__gc_alloc_bytes` 累加 + 阈值判断 + 条件 `call $gc_safepoint_poll`；bump 检查改 `__alloc_ptr/__alloc_end` 窗口（`heap_ptr` 同步维护——mark-sweep 下 host 在 sweep/grow 后设 `__alloc_end = min(mem_size, heap_limit)`，`mark_sweep::attach_heap` 与 `collect_full` 尾部同步窗口）。
**Verification**: dump-wat 比对新序列；全量 fixture 绿；`fixtures/happy/gc_*` 长循环仍不 OOM。

### T1.4 host imports 换代

**Files**: `host_imports/core.rs`（新增 `gc_safepoint_poll`：重置 `__gc_alloc_bytes`、经调度器 budget 调 `safepoint_step`、`after_step` 反馈、按需更新 `__gc_trigger_bytes`；删除 `gc_maybe_collect`）、`host_import_registry/specs_part*.rs`（签名表同步）、`compiler_*` 调用点（T1.3 已改）。
**Verification**: `grep -rn 'gc_maybe_collect' crates/` 零命中；全量绿。

### T1.5 emitter 参数化（GcFlavor，仅 MarkSweep 变体）

**Files**: `support_module.rs`（`pub enum GcFlavor; pub fn emit_support_module(flavor: GcFlavor)`；内部全部 helper emit 函数带 flavor 参数，本阶段所有 `match flavor` 分支仅 MarkSweep 实现、G1/Zgc 臂 `unreachable!("emitted in P3/P4")`——**注意**：这不违反无 stub 规则，因 build.rs 本阶段只请求 MarkSweep 变体，G1/Zgc 臂在 P3/P4 任务内完成前不可达）、`wjsm-runtime-support/build.rs`（产物命名 `wjsm_support_mark_sweep.cwasm`，`support_module_layout_hash` 按 flavor 计）、`wjsm-runtime/src/lib.rs`（`install_embedded_support_cwasm` 接口改按 kind 选择，本阶段仅一份）。
**Verification**: 全量绿；layout hash 单测。

### T1.6 阶段验证

`cargo nextest run --workspace` + `WJSM_STARTUP_SNAPSHOT=0 cargo nextest run -E 'test(happy__)'` 双路绿；提交。

---

## P2：host 统一读写层 + INV-C2 审计

### T2.1 heap_access.rs + epoch 断言

**Files**: 新建 `runtime_gc/heap_access.rs`；`lib.rs`（`gc_epoch: Arc<AtomicU64>`，每次 `collect_full`/`safepoint_step` 有实质工作时 +1）。
**内容**（签名 = spec §13，补 `SlotPart`）:
```rust
pub struct HeapPtr { pub ptr: usize, #[cfg(debug_assertions)] epoch: u64 }
impl HeapPtr { pub fn get(&self, ctx: &mut GcContext) -> usize {
    #[cfg(debug_assertions)] debug_assert_eq!(self.epoch, ctx.gc_epoch(), "INV-C2: ptr crossed GC point");
    self.ptr } }
pub enum SlotPart { Value, Getter, Setter }
pub fn resolve<C: AsContextMut<Data=RuntimeState>>(ctx: &mut C, env: &WasmEnv, h: Handle) -> Option<HeapPtr>;
pub fn write_property_slot(ctx, env, h, slot_idx, part: SlotPart, val: i64);  // 读旧值 → on_host_write → 写
pub fn write_element(ctx, env, h, idx, val: i64);
pub fn write_proto(ctx, env, h, proto: u32);
```
`resolve` 内嵌 `on_host_resolve` 分派（zgc 前恒 None 直读）。
**Verification**: 单测（mock 写 + epoch 断言触发用例）；workspace 绿。

### T2.2 裸写点清单

**Files**: 新建 `docs/aegis/work/2026-07-03-gc-v2/bare-write-audit.md`。
**内容**: `grep -rn 'HEAP_OBJECT_PROPERTY\|HEAP_ARRAY_ELEMENT\|HEAP_OBJECT_PROTO_OFFSET' crates/wjsm-runtime/src/ --include='*.rs'` 全量输出整理为核对表（文件/行/写或读/替换任务号），读点标注"短生命周期确认"。
**Verification**: 清单覆盖 grep 全部命中（数量核对）。

### T2.3-T2.5 裸写点替换（三批）

- **T2.3**: `runtime_values.rs` / `runtime_heap.rs` / `runtime_builtins.rs` / `host_imports/core.rs`
- **T2.4**: `host_imports/{collections,map_set 相关,typedarray*,streams*}.rs` 族
- **T2.5**: 其余 host_imports + `runtime_{promises,generator,async_fn,collections,...}.rs`

每批：清单勾销 + 全量 fixture 绿后提交。写点全部改 `heap_access::write_*`；跨 GC 点的 ptr 使用改 `resolve` 短窗口模式。
**Verification**: 各批后清单勾销数递增；P2 末 `grep` 复跑，剩余命中全部是 heap_access 内部或只读短窗口（清单注明）。

### T2.6 WASM 侧 INV-C2（resize re-resolve + emit_resolve_handle_ptr）

**Files**: `support_object_helpers.rs` / `compiler_helpers/helpers_object.rs` / `compiler_array_helpers.rs`。
**内容**: 新增 `emit_resolve_handle_ptr(func, flavor, handle_local, ptr_local)` 统一解引用 emitter（MarkSweep/G1 = 直载 obj_table；Zgc 分支 P4 补）；**排查并修复所有 resize 点**：`$obj_set` 扩容 / `$arr_push` 扩容 / `$arr_unshift` 头部插入扩容 / `$arr_splice` 中间插入扩容 / arguments 物化 / `$obj_define_property` property slots 扩容 / 其余 `gc_alloc_slow` 后引用旧数据的 helper——改为分配返回后重新经 `emit_resolve_handle_ptr` 解 old_ptr 再 `memory.copy`。逐 helper 排查并在任务提交消息列出修复点清单。
**Verification**: dump-wat 抽查 resize 序列；`grep -rn 'gc_alloc_slow' crates/wjsm-backend-wasm/src/ --include='*.rs'` 命中点与清单交叉核对（确认每个 resize 后持 old_ptr 的点均已 re-resolve）；全量绿。

### T2.7 阶段验证

debug 构建（断言开启）全量 fixture + `WJSM_STARTUP_SNAPSHOT=0` 双路绿；提交。

---

## P3：G1（子模块 ≤500 行；spec §10 全节为实现蓝图）

### T3.0 `WJSM_TEST_GC` 矩阵机制（P3/P4 验证前置）

**Files**: `tests/fixture_runner.rs`（E2E harness 读 env `WJSM_TEST_GC` → 映射 `GcAlgorithmKind` 注入 `RuntimeOptions`；无效值 panic 提示合法值）、`crates/wjsm-cli/src/`（`run_file_in_process` 透传 options——若签名不含 options 则加带 options 变体，原签名保留默认转发）。
**Why**: T3.8/T4.6 的全量矩阵与子集验证依赖同一 fixture 集按算法重跑（D7 分层矩阵）。
**Verification**: `WJSM_TEST_GC=mark-sweep cargo nextest run -E 'test(happy__hello)'` 绿；`WJSM_TEST_GC=bogus` 报错列合法值。

### T3.1 region.rs（域组织 + attach_heap）

**Files**: 新建 `runtime_gc/g1/{mod,region}.rs`。
**内容**: `RegionMeta` 枚举（`Free/Eden/Survivor/Old/HumongousStart/HumongousCont/Immortal/Meta`，u8 编码与 WASM 侧一致）；`attach_heap`：元数据区（region_meta 表 + card table + SATB 4KB）划定 → 写 `__region_meta_base/__card_table_base/__satb_ptr/__satb_end` globals → region 域 64KB 对齐起点 → immortal 段标注 → 首个 Eden region 设窗口。region 分配/归还 API（`take_free_as(kind)` / `release(idx)`）。host 分配路径（`alloc_host_object_impl`）接 `alloc_slow`。
**Verification**: 单测（域划分边界/humongous 连续段/immortal 标注）；`WJSM_GC=g1` 冒烟（hello fixture 手跑——registry 本任务起对 G1 返回 Ok）。

### T3.2 card.rs + host 侧 barrier

**Files**: 新建 `g1/card.rs`；`heap_access.rs`（`on_host_write` 分派已就位，G1 impl 补 Rust 版 SATB+card，逻辑 = spec §8.2 双 (a)(b)）。
**Verification**: 单测（card 索引计算/dirty 迭代器/SATB 队列 flush）。

### T3.3 g1 变体 barrier 代码生成 + 第二份 cwasm

**Files**: `support_module.rs`/`support_object_helpers.rs`（`GcFlavor::G1` 臂：`obj_set`/`elem_set` 及一切引用槽写入点插入 spec §8.2 指令序列；分配序列 allocate-black——mark 期置标记位，G1 的标记位 = 算法内 mark bitmap，WASM 侧以 `__gc_phase==MARK` 时经 `gc_safepoint_poll` 前置标记？**定案**：G1 allocate-black 由 host 在换窗口时对新 Eden region 整体记录"mark 期新生 region 全活"，WASM 分配序列零额外指令——region 粒度 allocate-black，实现于 concurrent_mark 存活判定）、`wjsm-runtime-support/build.rs`（+`wjsm_support_g1.cwasm`）、`lib.rs`（install 按 kind 选变体）、`runtime_eval.rs`（eval flavor 传递）。
**Verification**: dump-wat（g1 变体 barrier 序列）；`WJSM_TEST_GC=g1` 跑 `happy__` 子集冒烟。

### T3.4 young.rs（young GC）

**Files**: 新建 `g1/young.rs`；`runtime_gc/roots.rs`（新增共享函数 `for_each_immortal_region_root(ctx, visit)`：扫描 `[object_heap_start, immortal_end)` 全部对象引用槽——P4 zgc mark 复用，spec §14）。
**内容**: spec §10.3 全流程：root 集（复用 `RuntimeRoots` + immortal 扫描 + dirty card 精化扫描）→ young 对象图复制（Survivor bump / age≥2 晋升 Old / Survivor 不足直晋升）→ obj_table 更新 → freed handles → weak refs 共享处理 → region 归还 → GcStats（cycle_kind=Young, pause 记录）。**复制期间 INV-C2**：young GC 在 host call 内 STW，复制序列自身不触发分配路径以外的 GC 点（复制目标 region 由算法直接管理，不走 `alloc_slow`）。
**Verification**: 单测（age 晋升/survivor 溢出/humongous 不动）；新 fixture `gc_g1_young_churn.js`（分配-丢弃循环 + 存活集校验 stdout）。

### T3.5 concurrent_mark.rs（增量标记）

**Files**: 新建 `g1/concurrent_mark.rs`。
**内容**: spec §10.4：IHOP 触发（old 占 45%）→ 初始标记附着 young GC → `safepoint_step` 增量 drain（budget 切片；SATB 缓冲并入）→ final remark（STW：SATB 残留 + `RuntimeRoots` fixed-point 重扫 = IMPL-18）→ cleanup（region 活字节统计 + 全死 region 归还）。mark bitmap 复用 `MarkBitmap`（handle 索引）。
**Verification**: 单测（SATB 一致性场景：标记中覆盖写 old 引用，对象存活）；`WJSM_TEST_GC=g1` 长循环 fixture。

### T3.6 mixed.rs（CSet evacuation）

**Files**: 新建 `g1/mixed.rs`。
**内容**: spec §10.5：活字节升序选 CSet（pause budget 折算字节上限；>85% 活跳过）→ STW evacuate（old→old 复制 + obj_table 更新 + freed handles）→ 多轮直至候选耗尽。
**Verification**: 单测（CSet 预算截断/85% 阈值）；碎片 fixture（`gc_fragmentation_churn` @ g1 断言 `external_fragmentation` 下降——经 GcStats 可观测输出）。

### T3.7 mod.rs 组装 + registry 接入

**Files**: `g1/mod.rs`（`G1Collector` impl `GcAlgorithm` 全钩子：`alloc_slow` = 换 Eden/触发 young/mixed 级联/grow/None；`safepoint_step` = 按 phase 派发 young/mark 切片/mixed；`collect_full` = young + 完整标记 + mixed 到干净；`on_host_write`/`barrier_flush` 接 card.rs）；`registry.rs`（G1 → Ok）。
**Verification**: g1 单元测试全绿。

### T3.8 阶段验证

`cargo nextest run -E 'test(gc_)' `（默认）+ `WJSM_TEST_GC=g1 cargo nextest run --workspace`（全量矩阵手动）绿；`WJSM_GC_LOG=1` 抽查 young pause 数量级；提交。

---

## P4：ZGC（spec §11 为实现蓝图；与 P3 独立可并行）

### T4.1 color.rs + page.rs

**Files**: 新建 `runtime_gc/zgc/{mod,color,page}.rs`。
**内容**: 色协议常量与 entry 读写（`entry = ptr | color`，掩码 `0x3`/`!0x3`）；双 good 切换状态（spec §11.2）；page 域组织（attach_heap 同 T3.1 模式，无代别）。
**Verification**: 色协议单测（双 good 切换全状态转移表）。

### T4.2 zgc 变体 load barrier + 第三份 cwasm

**Files**: `support_module.rs`/`support_object_helpers.rs`（`GcFlavor::Zgc` 臂：`emit_resolve_handle_ptr` Zgc 分支 = spec §8.3 序列，覆盖**全部** helper 解引用点；`obj_set`/`elem_set` SATB（mark 期）；分配序列 allocate-black = 新 entry 直接 `| __good_color`）、`build.rs`（+`wjsm_support_zgc.cwasm`）。
**Verification**: dump-wat（load barrier 序列逐 helper 抽查）；`WJSM_TEST_GC=zgc` 冒烟（registry 开 Zgc）。

### T4.3 mark.rs（增量标记）

**Files**: 新建 `zgc/mark.rs`。
**内容**: MarkStart（STW：good=本周期 mark 色、root snapshot）→ 增量 drain（load barrier 协助标记经 `load_barrier_slow` 入 worklist）→ MarkEnd（STW：SATB 残留 + 侧表 fixed-point + weak refs）。
**Verification**: 单测（坏色命中标记/SATB 覆盖写场景）。

### T4.4 relocate.rs（增量搬迁 + 强制 heal）

**Files**: 新建 `zgc/relocate.rs`；`heap_access.rs`（zgc `on_host_resolve`：RELOCATE 期 RS 内对象 → 同步搬迁 + 返回新 ptr = IMPL-17）。
**内容**: SelectRelocSet（碎片率>25%，预算截断）→ RelocateStep（good=Remapped；主动搬 + `load_barrier_slow` 协助搬：目标 page 分配 → memcpy → `obj_table[h]=new|11` → 源 page 计数归零即归还）。
**Verification**: **专项测试**（R3）：RELOCATE 阶段中 host 读/写 RS 内对象（构造 fixture：relocate 步进间用 host builtin 改写对象属性再读回）数据一致；debug 断言"RS 对象解引用必已 heal"零触发。

### T4.5 mod.rs 组装 + registry

**Files**: `zgc/mod.rs`（`ZgcCollector` impl 全钩子；`alloc_slow` = 换 page/加速步进（mutator 配速，spec §12 落后补步进）/grow/None；`collect_full` = 同步整周期）；`registry.rs`（Zgc → Ok）。
**Verification**: zgc 单测全绿。

### T4.6 阶段验证

`WJSM_TEST_GC=zgc cargo nextest run --workspace` 全量绿 + GC 子集绿；提交。

---

## P5：选择机制 + 可观测性 + 定量基准

### T5.1 三入口选择

**Files**: `lib.rs`（`RuntimeOptions::gc_algorithm: GcAlgorithmKind` + `with_gc_algorithm` 构造器；env `WJSM_GC` 解析入默认值链）、`wjsm-cli`（`run`/`eval` 子命令 `--gc <mark-sweep|g1|zgc>`，优先级 CLI > env > 默认；非法值错误信息列合法值）。
**Verification**: CLI 集成测试三入口优先级；`wjsm run --gc g1 fixtures/happy/hello.js` 手验。

### T5.2 GcStats v2 + pause 直方图

**Files**: `api.rs`（spec §17 字段全量）、`lib.rs`（`gc_pause_hist` 环形缓冲 256 条）、各算法填充点、`WJSM_GC_LOG=1` 周期摘要 eprintln。
**Verification**: 单测（直方图环形语义）；`WJSM_GC_LOG=1` 三算法各跑 churn fixture 人工核对字段合理性。

### T5.3 gc_pause_bench.rs（定量基准）

**Files**: 新建 `crates/wjsm-runtime/tests/gc_pause_bench.rs`（`WJSM_GC_BENCH=1` 门控，否则 skip）。
**内容**: churn 负载 JS（1e7 分配 + 5% 存活入 Map + 周期大数组，固定 heap_limit）内嵌 `-e` 源；三算法各执行采 GcStats：断言 spec §21.2 四项（g1 young max ≤8ms 且 ≤ ms-full/5；zgc 同；吞吐 ≤1.25×；碎片对比）。
**Verification**: `WJSM_GC_BENCH=1 cargo nextest run -E 'test(gc_pause_bench)'` 达标（不达标 → 调 §12 自适应参数/步进粒度，调参过程记录入 work notes）。


### T5.4 linear memory footprint 治理指标（并入 #335）

**Files**: `api.rs`（GcStats 新增 `committed_pages: usize`、`free_bytes_reusable: usize`）、各算法填充点、`lib.rs`（RuntimeState 新增 `memory_footprint_hist`）、新增 `tests/gc_footprint_long_run.rs`（长运行增长-回落-再增长压力测试，`WJSM_GC_BENCH=1` 门控）。
**内容**: #335 部分已由 #332（free bytes）+ #337（heap budget）覆盖，但 committed pages（wasm linear memory 实际占用）、页复用策略、footprint 可观测性仍缺失。本任务补齐：GcStats 记录当前 `memory.size` 页数（committed）与可复用空间；长运行测试验证"对象存活量下降后，后续分配优先复用空闲区域而非持续 grow"（mark-sweep free list / g1 region 归还 / zgc page 复用均计入）。wasm linear memory 本身不可 shrink（wasmtime 限制），但通过复用控制 footprint 稳态。
**Verification**: `WJSM_GC_BENCH=1 cargo nextest run -E 'test(gc_footprint_long_run)'` 绿；GcStats 输出含 committed/reusable 字段；长运行 fixture 手工观察 memory.size 稳态（GC 后不持续增长）。

### T5.5 GC 回归矩阵完整性审计（并入 #338）

**Files**: 审计现有 `fixtures/happy/gc_*`、`fixtures/happy/*{weak,async,streams,typedarray,fetch}*` 等覆盖面；按需补充缺失 fixture。
**内容**: #338 要求的侧表、长期 churn、OOM 前一致性、WeakRef/FinalizationRegistry、BoundFunction/Proxy/Closure 等边界已由 #331-#334/#339 修复覆盖，现有 fixture 已含 `gc_safepoint_spill_stress`、`gc_weak_ref`、`gc_finalization_registry`、`gc_side_table_roots` 等。本任务审计清单：确认每类历史缺口都有可运行测试，补充遗漏项（如 Proxy + GC、多轮 churn 后 BoundFunction 存活）。所有新 fixture 在三算法矩阵下验证（T3.0 `WJSM_TEST_GC` 机制）。
**Verification**: 生成回归覆盖清单文档（`docs/aegis/work/2026-07-03-gc-v2/regression-matrix-coverage.md`）逐项勾选；新增 fixture 三算法绿。
### T5.6 阶段验证 + 提交

---

## P6：清理 + 文档 + 终验

### T6.1 删除清单执行

spec §18 逐条 grep 复核（v1 trait 名/`gc_maybe_collect`/WIP 残留）；`#[allow(dead_code)]` 清扫（api.rs 现有标注复查）。
**Verification**: grep 记录附提交消息；零 warning。

### T6.2 文档同步

**Files**: `AGENTS.md`/`CLAUDE.md`（WASM contract：globals 数、host funcs 表、GC 描述改三算法可选）、`docs/aegis/specs/2026-07-03-napi-native-addon-design.md`（两处 "non-moving" → "handle 恒定（INV-C1）"，spec §15.2）。
**Verification**: 文档描述与 `module_setup.rs` globals 实数核对。

### T6.3 ADR 0005

**Files**: 新建 `docs/adr/0005-pluggable-gc-v2.md`（决策：INV-C1/C2 取代 INV-C、v2 接口边界、三变体物理边界、增量调度、v1 附录 D 取代声明；alternatives：真线程并发/纯 STW/运行时开关单变体——均记否决理由）；INDEX.md Baselines 段登记。
**Verification**: recording-architecture-decisions 惯例格式。

### T6.4 全矩阵终验

默认全量 + `WJSM_TEST_GC=g1` 全量 + `WJSM_TEST_GC=zgc` 全量 + 快照双路 + `WJSM_GC_BENCH=1` 定量 + 零 warning。执行状态头更新为完成；提交。

---

## Risks（执行期跟踪，缓解 = spec §20）

| 风险 | 阶段哨兵 |
|------|----------|
| R1 裸写遗漏 | P2 清单勾销数 = grep 命中数；P3/P4 子集高频回归 |
| R2 侧表 SATB 破洞 | T3.5/T4.3 final remark 单测 + async/streams fixture @ g1/zgc |
| R3 relocate 期 host 写旧位置 | T4.4 专项测试 + debug 断言 |
| R4 变体 drift | IMPL-19：emitter diff review 检查 `match flavor` 之外无变体分叉 |
| R5 分配跑赢 GC | 长循环 fixture 三算法 + T4.5 配速补步进 |
| R6 pause 不达标 | T3.8/T4.6 先行 `WJSM_GC_LOG` 数量级检查，不留到 T5.3 |
| R8 窗口重构回归 | T1.3 独立提交 + 全量绿后才进 P2 |

## Retirement Track

| 旧物 | 退休点 | 验证 |
|------|--------|------|
| v1 trait 组（Allocator/Marker/Sweeper/Write/Read/RegionMgr） | T0.3 | grep 零命中 |
| `gc_maybe_collect`（import + 每分配 host call） | T1.4 | grep 零命中 |
| #332 WIP 半成品态 | T0.6 | 测试绿 + close #332 |
| v1 spec 附录 D 承诺 | T6.3 ADR 记录 | 文档链接 |
| N-API spec non-moving 措辞 | T6.2 | 文档 diff |
