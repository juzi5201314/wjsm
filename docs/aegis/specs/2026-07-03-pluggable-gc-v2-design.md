# wjsm 可插拔 GC 架构 v2 设计规格（mark-sweep / G1 / ZGC）

**状态**: 待审批
**日期**: 2026-07-03
**范围**: `wjsm-runtime`（GC 框架 v2 + 三算法 + host 统一读写层）、`wjsm-backend-wasm`（support module 三变体 + 分配窗口 + barrier 代码生成）、`wjsm-ir`（布局常量）、`wjsm-runtime-support`（三份 cwasm）、`wjsm-snapshot-format`（immortal 区 + ABI hash）、`wjsm-cli`（`--gc` 选择）
**权威来源**: `docs/aegis/specs/2026-06-14-pluggable-gc-framework-design.md`（v1 框架，本 spec 取代其附录 D 稳定性承诺，见附录 B）；`docs/aegis/specs/2026-07-03-napi-native-addon-design.md`（napi root 源 + 措辞修订，§15.2）；issue #332（碎片治理，并入 §9）；HotSpot G1/ZGC 论文与 V8 incremental marking 实践（机制参照）
**ADR 信号**: INV-C 重写（non-moving → handle 恒定 + ptr 纪律）、GcAlgorithm v2 接口边界、三变体 support module 物理边界、增量调度决策 —— 落地后补 `docs/adr/0005-pluggable-gc-v2.md`

---

## 1. 问题陈述

### 1.1 现状（2026-07-03）

v1 框架（2026-06-14 spec）交付了 trait 骨架（`GcAlgorithm: Allocator + Marker + Sweeper` + `RootProvider`）与唯一实现 `MarkSweepCollector`（non-moving + segregated free list）。局限：

1. **两段式 trait 无法表达 evacuation/coloring 模型**：G1 young GC 没有 sweep 阶段（复制即回收），ZGC relocate 不是 sweep。`Marker`/`Sweeper` 切分是 mark-sweep 特化的抽象泄漏。
2. **barrier 是无消费者的 no-op trait**：`WriteBarrier`/`ReadBarrier`/`HeapRegionManager` 无注入通道（WASM 代码生成完全不知道它们存在）。
3. **无算法选择入口**：`RuntimeState.gc_algorithm` 硬编码 `MarkSweepCollector::new()`（`lib.rs:958`）；v1 spec P6 的 `--gc-algorithm` 未落地。
4. **INV-C（对象永不动）过强**：它阻止一切 moving 算法，而调研证明架构真实依赖的是 handle 稳定（§5）。
5. **分配 fast-path 每次分配一次 host call**：`$obj_new`/`$arr_new` 头部无条件 `call gc_maybe_collect`（`helpers_object.rs:29`），counter 在 host 侧递增——v1 spec §7.1 原设计（counter 内联 WASM）未按图实现。
6. **碎片无解**：non-moving 长期 churn 下外部碎片累积（issue #332 WIP 只能做尾部回收缓解，无法根治）。

### 1.2 目标（TaskIntentDraft）

- **Outcome**: wjsm 拥有运行时可选（不重编译用户产物）的三种内置 GC：`mark-sweep`（默认）、`g1`（分代 region evacuation，吞吐/延迟平衡）、`zgc`（染色 handle + load barrier，最低停顿）；g1/zgc 的低停顿有定量验收证明。
- **Goal**: 框架 v2（生命周期完整的算法接口 + 增量调度器 + 双端 barrier 通道 + host 统一读写层）；INV-C 重写解锁 moving。
- **Success evidence**: 分层测试矩阵全绿（§21.1）；churn 基准下 g1/zgc pause 定量达标（§21.2）；INV-C2 审计零残留；旧接口删净。
- **Stop condition**: P0–P6 全部完成且验收标准（§21）逐项通过。
- **Non-goals**（§3）。
- **Risks**（§20）。

### 1.3 BaselineReadSetHint

- `crates/wjsm-runtime/src/runtime_gc/api.rs`（v1 trait 全文，本次重定义）
- `crates/wjsm-runtime/src/runtime_gc/mark_sweep/{mod,marker,sweeper,allocator}.rs`（迁移源）
- `crates/wjsm-runtime/src/runtime_gc/roots.rs:1-117`（RootProvider 实现，共享层保留）
- `crates/wjsm-runtime/src/runtime_gc/heap_governance.rs`（#332 WIP，并入 §9）
- `crates/wjsm-runtime/src/runtime_heap.rs:53-157`（host 分配 + `collect_for_host_alloc`）
- `crates/wjsm-runtime/src/host_imports/core.rs`（`gc_alloc_slow`/`gc_maybe_collect` trampoline）
- `crates/wjsm-backend-wasm/src/compiler_helpers/helpers_object.rs:13-173`（`$obj_new` fast-path）、`:1100-1160`（resize 持 old_ptr 跨 `gc_alloc_slow` —— INV-C2 违反点样本）
- `crates/wjsm-backend-wasm/src/compiler_instructions/instr_main.rs:340-405`（用户模块全部经 helper 调用的证据）
- `crates/wjsm-backend-wasm/src/support_module.rs` + `support_object_helpers.rs`（变体化目标）
- `crates/wjsm-backend-wasm/src/compiler_module/module_setup.rs:12-37`（safepoint 容量检查；shadow stack 固定 64KB + trap）
- `crates/wjsm-ir/src/lib.rs:815-821`（`SHADOW_STACK_SIZE`/guard 常量）
- `crates/wjsm-ir/src/constants.rs`（布局常量、`HANDLE_TABLE_*`）
- `docs/adr/0003`/`0004`（snapshot 边界 + 嵌入式运行时，§15 兼容依据）

### 1.4 ImpactStatementDraft

| 层 | 影响 |
|----|------|
| `wjsm-ir` | 新增布局常量（card/region/satb 参数）；`value.rs` 不变 |
| `wjsm-backend-wasm` | support emitter 参数化（`GcFlavor`）；`$obj_new`/`$arr_new` 统一分配窗口 + counter 内联；g1/zgc 变体 barrier 代码生成；新 globals/imports；safepoint spill 体系**不变** |
| `wjsm-runtime` | `runtime_gc/` v2 重构（api/registry/scheduler/g1/zgc）；新 `heap_access.rs` host 统一读写层；`host_imports` 全部裸写点替换 |
| `wjsm-runtime-support` | build.rs 产出三份 cwasm |
| `wjsm-snapshot-format` | immortal 区边界入格式；ABI hash 输入更新 |
| `wjsm-cli` | `--gc <name>` flag |
| 文档 | AGENTS.md/CLAUDE.md WASM contract 与 GC 描述；N-API spec 措辞修订；ADR 0005 |

**兼容性边界**: 现有 fixture stdout/语义不变；NaN-boxing 不变；obj_table 间接不变；`gc()` global 行为保持；`WJSM_STARTUP_SNAPSHOT` 开关语义不变（ABI hash 升级 → 旧快照冷启动，既有机制）。

---

## 2. 决策矩阵

| # | 维度 | 决策 | 来源 |
|---|------|------|------|
| D1 | 并发模型 | **机制忠实 + 增量调度**：region/分代/RSet/write barrier/染色/load barrier/evacuation 完整移植；"并发阶段"以 pause-budget 切片穿插 safepoint；STW 阶段预留多线程并行接口（首版单线程） | 用户拍板 |
| D2 | 选择粒度 | **启动时选定**（CLI > env > 默认），进程内固定，无热切换 | 用户拍板 |
| D3 | 屏障策略 | **support module 按算法出三份变体 cwasm**；用户模块 import 面三算法一致；eval mode 编译期特化 | 用户拍板 |
| D4 | #332 WIP | **搁置并入重构**：尾部回收/碎片指标在 P0 作为 mark-sweep v2 的一部分重整交付 | 用户拍板 |
| D5 | 默认算法 | **mark-sweep**（基线稳定；g1/zgc 显式启用） | 用户拍板 |
| D6 | 堆布局 | **按算法独立布局**：mark-sweep 保持连续 bump 堆；g1/zgc 各自 region 化 | 用户拍板 |
| D7 | 测试矩阵 | **分层**：默认算法全量 fixture + 三算法 GC 密集子集 + `WJSM_TEST_GC` 全矩阵开关 | 用户拍板 |
| D8 | 验收 | **含定量 pause 项**（§21.2） | 用户拍板 |
| D9 | 框架组织 | **GcAlgorithm v2 单 trait 生命周期接口 + 算法自治堆域**（方案 A；B=硬塞 mark/sweep 语义、C=三套端到端 均否） | 设计呈现批准 |
| D10 | ZGC 分代 | 非分代（经典 ZGC）；分代场景由 G1 承担 | 自决 |
| D11 | G1 RSet | **host-side sparse remembered set + 统一 barrier event buffer**：WASM 不直接读写 region/card metadata；引用槽写入只把事件写入线性 buffer，host flush 后过滤/去重为 sparse card/slot set。理由：wjsm 堆为 MB 级、写点少于读点，省掉 upfront 线性 card table/region table（hello 不背 8MiB 元数据），同时把 metadata owner 收敛到 runtime_gc。 | 自决（HotSpot G1/GenZGC 的 RSet 思路 + wjsm handle 间接层） |
| D12 | ZGC 染色位 | `obj_table` entry 低 2 位（对象 16B 对齐，低 4 位恒 0）；一次 load 同取 ptr+色 | 自决 |
| D13 | 分配触发 | counter 内联 WASM（恢复 v1 spec §7.1 原意），去掉每分配一次 host call | 自决 |

---

## 3. 范围声明

**本次实现**：框架 v2 + 三算法完整实现 + 三变体 support module + host 统一读写层 + INV-C2 审计修复 + 选择机制 + 定量 benchmark + 旧接口删除。

**明确排除（Non-goals）**：
- 真线程并发（WASM threads proposal / shared memory / 堆访问原子化）；
- 运行中热切换算法；
- ZGC 分代（D10）；
- 线性内存内的 per-region/card metadata 表（D11 已改为 host-side metadata + 统一 barrier event buffer）；
- WASM GC proposal（stack maps / externref）；
- 改 ECMAScript 语义；
- STW 阶段多线程并行的**实现**（接口按并行留形：mark worklist 与 region 复制的分片接口无 `&mut` 全局耦合；实现 defer，无消费者不写）。

---

## 4. 架构总览

```
┌─ 编译期（wjsm-backend-wasm）──────────────────────────────────────┐
│ emit_support_module(GcFlavor::{MarkSweep,G1,Zgc}) → 三份 wasm      │
│   → wjsm-runtime-support build.rs 预编译三份 cwasm                 │
│ 变体差异（§8）:                                                    │
│   obj_new/arr_new: 统一分配窗口 bump + counter 内联（三变体同构）  │
│   obj_set/elem_set: g1/zgc 写屏障（SATB+card / SATB）              │
│   obj_get/elem_get/一切 handle 解引用: zgc load barrier            │
│ 用户模块: import 面（helpers+globals 并集）三算法完全一致；        │
│   safepoint spill 体系（Layer 1/2/3）不变                          │
│ eval mode: compile_eval(flavor) 按运行时已选算法 inline 特化       │
└────────────────────────────────────────────────────────────────────┘
                     │ host imports: gc_alloc_slow / gc_safepoint_poll
                     │   / gc_barrier_flush / gc_load_barrier_slow
                     │   / gc_take_freed_handle
                     ▼
┌─ 运行期（wjsm-runtime/src/runtime_gc/ v2）────────────────────────┐
│ api.rs        GcAlgorithm v2 + GcContext(继承) + GcStats v2       │
│ registry.rs   GcRegistry: name → factory；启动时装配              │
│ scheduler.rs  StepBudget/pause target/触发字节自适应               │
│ roots.rs      RootProvider（共享单 owner，含 immortal 区扫描）     │
│ heap_access.rs host 统一读写层（host 侧 barrier 唯一通道，§13）    │
│ weak_refs.rs / side_table_refs.rs / native_callable_refs.rs 共享  │
│ mark_sweep/   v2 迁移 + #332 治理（§9）                            │
│ g1/           region/分代/card/SATB/young/mixed（§10）             │
│ zgc/          染色/load barrier/relocate/周期状态机（§11）          │
└────────────────────────────────────────────────────────────────────┘
选择（§16）: CLI --gc > env WJSM_GC > 默认 mark-sweep
            → RuntimeOptions::gc_algorithm → registry 装配
            → install 对应 support cwasm 变体
```

---

## 5. 不变量重写：INV-C → INV-C1 / INV-C2

**废除** v1 INV-C（"对象永不动"）。**依据**（调研证实）：

1. 用户模块 WASM locals / shadow stack 只持 NaN-boxed handle，从不持 raw ptr——所有属性/元素访问都是 `call $obj_get/$obj_set/$elem_get/$elem_set`（`instr_main.rs:355-388`）；
2. resize 路径本身就在做局部 moving：分配新区 → copy → 重写 `obj_table[h]`（`helpers_object.rs:1100-1160`、`runtime_values.rs grow_array/grow_object`）——v1 的 INV-B 与 INV-C 实际矛盾，架构真实依赖的从来是 handle 稳定；
3. raw ptr 跨 GC 点的位置可枚举：helper 内部序列 + host Rust 代码。

**新不变量**：

| ID | 不变量 | 维护点 | 违反后果 |
|----|--------|--------|----------|
| **INV-C1（handle 恒定）** | JS 值层引用 = boxed handle；handle 从分配到对象死亡不变；`obj_table[h] → ptr` 是唯一 truth；moving = 仅 GC 在安全点内更新 obj_table 槽 | 全部算法 + resize 路径 | 引用身份断裂 |
| **INV-C2（raw ptr 纪律）** | 任何解出的 raw ptr 生命周期不得跨越**潜在 moving/collect GC 点**（`gc_alloc_slow`/`gc_safepoint_poll`/`gc_load_barrier_slow`/host 分配/collect）；跨越者必须经 obj_table re-resolve。`gc_barrier_flush` **只 drain event buffer，不 collect/grow/move/recolor，不递增 gc_epoch，因此不是 GC 点**，允许在写前屏障内使用已计算 `slot_addr` | WASM helpers（§8.5）+ host（§13） | moving 后悬垂 ptr → 数据损坏 |

**INV-C2 执法机制**：
- WASM 侧：违反点清单化修复（已知样本：`$obj_set`/`$elem_set`/`$arr_push` 等 resize 序列——`gc_alloc_slow` 返回后重新 `obj_table[h]` 解 old_ptr 再 copy）；三变体 emitter 内以辅助函数 `emit_resolve_handle_ptr` 统一解引用，禁止手写散点。
- host 侧：debug 构建 `HeapPtr { ptr, epoch }` 包装类型 + `RuntimeState.gc_epoch: u64`（每次 collect/step 递增）；`heap_access` 层解引用产出 `HeapPtr`，使用时 `debug_assert_eq!(self.epoch, current_epoch)`。release 零开销（`#[cfg(debug_assertions)]` 字段）。

mark-sweep v2 仍是 non-moving（不搬对象），但同样遵守 INV-C1/C2（纪律统一，无算法特例）。

---

## 6. GcAlgorithm v2 trait（`runtime_gc/api.rs`）

```rust
pub type Handle = u32;
pub type Value = i64;

/// 分配请求（fast-path 窗口耗尽后进入 slow-path 的完整上下文）。
pub struct AllocRequest {
    pub size: usize,
    pub heap_type: u8,
    pub capacity: u32,
}

/// 增量步进预算（调度器折算，§12）。
pub struct StepBudget {
    /// 本步最多处理字节数（mark 遍历/复制搬迁计量）。
    pub work_bytes: usize,
    /// 硬时间上限（超过立即让出）。
    pub deadline: std::time::Instant,
}

pub enum StepOutcome {
    /// 当前无 GC 周期在进行。
    Idle,
    /// 步进了部分工作，剩余量估算（调度器据此调 trigger）。
    Progress { remaining_estimate: usize },
    /// 一个完整周期在本步收尾（stats 已写入 ctx.stats）。
    CycleComplete,
}

/// v2 算法接口：生命周期完整，取代 v1 的 Allocator+Marker+Sweeper 组合。
pub trait GcAlgorithm: Send + Sync {
    fn name(&self) -> &'static str;

    /// 实例化后一次性接管动态堆域 [dynamic_start, heap_limit)。
    /// live immortal objects 为 obj_table 中 ptr 落在 [object_heap_start, immortal_objects_end)
    /// 的对象；[immortal_objects_end, dynamic_start) 是 padding，不扫描、不分配。
    /// 算法在 dynamic_start 后接管自己的动态域；metadata 默认驻留 host side，WASM 只共享分配窗口与 barrier buffer globals。

    fn attach_heap(&mut self, ctx: &mut GcContext, dynamic_start: usize);

    /// 分配 slow-path：fast-path bump 窗口耗尽后进入。
    /// 算法自决：换窗口（新 region）/ 触发回收 / grow / None（真 OOM →
    /// trampoline trap，IMPL-4 继承）。返回线性内存 ptr（handle 注册由调用方）。
    fn alloc_slow(
        &mut self,
        ctx: &mut GcContext,
        roots: &mut dyn RootProvider,
        req: AllocRequest,
    ) -> Option<usize>;

    /// safepoint 轮询：增量步进入口（WASM `gc_safepoint_poll` → 调度器 → 此处）。
    fn safepoint_step(
        &mut self,
        ctx: &mut GcContext,
        roots: &mut dyn RootProvider,
        budget: StepBudget,
    ) -> StepOutcome;

    /// 完整回收：`gc()` 显式调用 / OOM 兜底。同步跑完当前周期（或发起并跑完新周期）。
    fn collect_full(&mut self, ctx: &mut GcContext, roots: &mut dyn RootProvider) -> GcStats;

    /// zgc load barrier slow-path：修复坏色 handle，返回修复后 obj_table entry
    /// （new_ptr | good_color）。仅 zgc 变体的 WASM 会调用；其余算法默认
    /// debug_assert 不可达（release 直读 entry 返回）。
    fn load_barrier_slow(&mut self, ctx: &mut GcContext, h: Handle) -> u32 {
        let _ = h;
        debug_assert!(false, "load_barrier_slow called on non-zgc algorithm");
        0
    }

    /// 统一 barrier event buffer flush（WASM 侧缓冲满时 `gc_barrier_flush` → 此处）。
    /// event 自带 old_value/new_value，flush 不依赖读取当前 slot，避免写前 flush 漏 RSet。
    fn barrier_flush(&mut self, ctx: &mut GcContext) { let _ = ctx; }

    /// host 侧写 hook（heap_access 统一层唯一调用方，§13）。
    /// target 用于 owner/generation 判定；slot_addr 用于 card/precise-slot RSet；old_val/new_val 分别服务 SATB/RSet。
    fn on_host_write(&mut self, ctx: &mut GcContext, target: Handle, slot_addr: usize, old_val: Value, new_val: Value) {
        let _ = (ctx, target, slot_addr, old_val, new_val);
    }

    /// host 侧解引用 hook（heap_access::resolve 调用；zgc relocate 期强制 heal）。
    /// 返回 Some(ptr) 表示算法已介入（heal 后的 ptr）；None = 直读 obj_table。
    fn on_host_resolve(&mut self, ctx: &mut GcContext, h: Handle) -> Option<usize> {
        let _ = (ctx, h);
        None
    }

    /// 本轮回收释放的 handle 列表（供 handle_free_list 复用，IMPL-10 继承）。
    /// 算法在周期收尾时经 ctx.with_state 推入 handle_free_list（协议同 v1）。
    /// —— 不是 trait 方法，写在此处作为契约说明。

    fn last_stats(&self) -> &GcStats;
}
```

**保留不变**：`GcContext`（StoreContextMut + WasmEnv，不持 slice，IMPL-8/#9 全部继承）；`RootProvider` trait 签名（含 owner-aware `is_marked` 参数，#334）；`HeapObjectQuery` 能力注入。
**删除**：v1 `Allocator`/`Marker`/`Sweeper`/`WriteBarrier`/`ReadBarrier`/`HeapRegionManager`/`MarkProgress`（§18 清单）。
**GcContext 扩充**（只增）：`gc_epoch()`、host-side region/page/RSet metadata 读写辅助、obj_table entry 色位读写辅助、barrier buffer base/flush 辅助、`alloc_window_set(ptr, end)`。

---

## 7. 堆布局与统一分配窗口

### 7.1 线性内存布局（三算法共同骨架）

```
[data segment（字符串，0..data_end）]
[handle table（obj_table_ptr..，容量止于 shadow stack 基址）]
[shadow stack 64KB 固定 + 64B guard canary]
[barrier event buffer 24KB 固定 + padding]
[object_heap_start（v2 起 64KB 对齐） → immortal objects（snapshot 对象） → immortal_objects_end]
[padding → dynamic_start（64KB 对齐，算法 attach_heap 入口）]
[dynamic heap 域（算法自治）→ heap_limit]
```

- mark-sweep：dynamic 域 = 连续 bump + free list（现状延续，D6）。`object_heap_start` 的精确数值不是兼容边界；v2 将其 64KB 对齐以消除 region/card/page 索引歧义，snapshot format 版本 + ABI hash 覆盖此布局变化。
- g1/zgc：dynamic 域 = region/page 数组（64KB/region|page，与 wasm 页边界对齐）；**算法 metadata 驻留 host side**（Rust `Vec`/bitset/sparse set），WASM 只写 barrier event buffer，不直接访问 card table/region table/page meta。`immortal_objects_end..dynamic_start` 是 padding，不参与对象扫描。

barrier event entry 固定 24B：`flags: u32`（bit0=SATB old-value，bit1=RSet slot），`slot_addr: u32`，`old_value: i64`，`new_value: i64`。G1 使用 SATB+RSet；ZGC 只使用 SATB；mark-sweep 不生成 barrier。event 自带 `new_value`，因此满缓冲 flush 即使发生在实际 slot store 前，也不会因读取旧槽值而漏记 old→young RSet。host `gc_barrier_flush` drain 后把 `__barrier_buf_ptr` 重置到 runtime 记录的 buffer base。

### 7.2 统一分配窗口（三变体同构 fast-path）

新增 globals `__alloc_ptr`/`__alloc_end`，`$obj_new`/`$arr_new` fast-path 统一为：

```wasm
;; size 计算（同现状）
;; counter 内联（D13）：
global.get __gc_alloc_bytes ; local.get size ; i32.add ; global.set __gc_alloc_bytes
global.get __gc_alloc_bytes ; global.get __gc_trigger_bytes ; i32.ge_u
if: call $gc_safepoint_poll        ;; host：调度器（重置 __gc_alloc_bytes）
;; handle 复用（同现状 take_or_alloc_handle）
;; 窗口 bump：
global.get __alloc_ptr ; local.get size ; i32.add
global.get __alloc_end ; i32.le_u
if (result i32):
  global.get __alloc_ptr ; local.tee ptr ; local.get size ; i32.add ; global.set __alloc_ptr
  local.get ptr
else:
  ... call $gc_alloc_slow(size, heap_type, cap)  ;; -1 → unreachable（同现状）
end
;; header 初始化 + obj_table[handle]=ptr（同现状，INV-A 继承）
;; g1/zgc 变体追加：allocate-black（G1 = region-level implicit-black；ZGC = entry 置当前 good 色，§10.4/§11.3）
```

窗口语义：mark-sweep 窗口 = `[heap_ptr, min(mem_size, heap_limit))`（host 在 grow/sweep 后同步 `__alloc_end`；`heap_ptr` global 保留，与 `__alloc_ptr` 同步维护——对 mark-sweep 二者恒等，g1/zgc 下 `heap_ptr` 表示堆域高水位仅供诊断）；g1 窗口 = 当前 eden region 剩余段；zgc 窗口 = 当前分配 page 剩余段。**移除现状"每次分配无条件 call gc_maybe_collect"的 host call**（1.1 #5 根治）。

### 7.3 globals 清单（v2 全集 = 现有 20 - 退休 `__alloc_counter` + 新增 8 = 27）

| global | 类型 | 写者 | 读者 | 用途 |
|--------|------|------|------|------|
| `__alloc_ptr` | i32 mut | WASM bump / host 换窗口 | 双方 | 分配窗口指针 |
| `__alloc_end` | i32 mut | host | WASM | 分配窗口末端 |
| `__gc_alloc_bytes` | i32 mut | WASM 累加 / host 重置 | 双方 | 步进触发计量 |
| `__gc_trigger_bytes` | i32 mut | host（调度器/pacer 自适应） | WASM | 步进触发阈值 |
| `__gc_phase` | i32 mut | host | WASM barrier | 0=Idle 1=Mark 2=Relocate（zgc）/1=ConcMark（g1） |
| `__good_color` | i32 mut | host | WASM load barrier | zgc 当前 good 色（低 2 位掩码值） |
| `__barrier_buf_ptr` / `__barrier_buf_end` | i32 mut | WASM 追加 / host flush 重置 | 双方 | 统一 barrier event buffer（24B/entry） |

（`__barrier_buf_ptr`/`__barrier_buf_end` 各计 1 个；新增共 **8** 个。）mark-sweep 变体只用前 4 个，其余 import 存在但不读。G1/ZGC metadata 不再暴露为 env globals；用户模块 import/re-export 并集（WASM contract 版本升级，文档同步 §18）。

### 7.4 host imports 变更

| import | 处置 |
|--------|------|
| `gc_alloc_slow(size, ht, cap) -> i32` | 保留；内部改经 `GcRegistry` 当前算法 `alloc_slow` |
| `gc_safepoint_poll()` | 新增；先 flush barrier event buffer（若非空），再重置 `__gc_alloc_bytes`、按 phase 派发 `safepoint_step` |
| `gc_barrier_flush()` | 新增；drain 统一 barrier event buffer：SATB event → 当前算法 `barrier_flush_satb(old_value)`；RSet event → G1 用 event 自带的 `slot_addr/new_value` + host-side metadata 过滤/去重；最后把 `__barrier_buf_ptr` 重置为 buffer base。**硬约束：不得 collect/grow/move/recolor，不递增 `gc_epoch`，不是 INV-C2 GC 点** |
| `gc_load_barrier_slow(h: i32) -> i32` | 新增；zgc 坏色修复，返回新 entry |
| `gc_take_freed_handle() -> i32` | 保留 |

---

## 8. 三变体 support module（`wjsm-backend-wasm`）

### 8.1 emitter 参数化（单源，禁止复制）

```rust
pub enum GcFlavor { MarkSweep, G1, Zgc }
pub fn emit_support_module(flavor: GcFlavor) -> Result<Vec<u8>>
```

所有 helper emit 函数接 `flavor`，差异点以 `match flavor` 局部分支表达；**不允许**为变体复制整份 emitter（drift 风险 R4）。`wjsm-runtime-support` build.rs 产出 `wjsm_support_{mark_sweep,g1,zgc}.cwasm` 三份并全部嵌入；`install_embedded_support_cwasm` 按选定算法装载。启动快照 ABI 使用 **flavor-independent support ABI union hash**（env globals/import/export/helper 签名并集），不哈希具体 flavor 指令字节；每份 cwasm 可另有产物校验 hash，但不得参与 startup snapshot ABI 匹配。

### 8.2 g1 变体 write barrier（`obj_set`/`elem_set`/一切引用槽写入点，写前插入）

```wasm
;; 统一 barrier event buffer，entry = { flags:u32, slot_addr:u32, old_value:i64, new_value:i64 }
flags = 0
old = i64.load slot

;; (a) SATB：并发标记期捕获旧值（snapshot-at-the-beginning 不变式）
global.get __gc_phase ; i32.const PHASE_MARK ; i32.eq
if:
  (if tag_needs_root(old)): flags |= EVENT_SATB

;; (b) RSet：引用槽写入位置 + 新值进入 host-side remembered set（host flush 后过滤 owner/value generation）
(if tag_needs_root(new_val)):
  flags |= EVENT_RSET

if flags != 0:
  p = global.get __barrier_buf_ptr
  i32.store(p, flags)
  i32.store(p + 4, slot_addr)
  i64.store(p + 8, old)
  i64.store(p + 16, new_val)
  p = p + 24
  global.set __barrier_buf_ptr(p)
  if p == global.get __barrier_buf_end: call $gc_barrier_flush
```

保守性：WASM 不查 owner region，也不查 new_val 是否 young；host flush 侧用 event 自带 `new_value` + `obj_table` + host-side `RegionMeta` 精化，只有 old/immortal/humongous→young 的槽进入 RSet。这样牺牲少量 buffer 带宽，换掉线性内存 card table/region table 和 barrier 内 metadata 访问，同时避免“写前 flush 读到旧槽值”漏记 RSet。RSet 在 host 侧采用 sparse dirty-card set；热点 card 可升级为 precise-slot bitmap（实现可选，但接口预留）。

`gc_barrier_flush` 是写屏障内部可调用的 buffer drain，不得触发 collection、relocation、memory.grow 或 obj_table recolor；真正会推进 GC 的入口只有 `gc_safepoint_poll` / `gc_alloc_slow` / ZGC `gc_load_barrier_slow`。因此满缓冲 flush 后继续执行同一次 slot store 不违反 INV-C2。

### 8.3 zgc 变体 load barrier（一切 handle→ptr 解引用点）

```wasm
;; emit_resolve_handle_ptr(flavor=Zgc)：
entry = i32.load(obj_table_ptr + h*4)
entry ; i32.const 3 ; i32.and ; global.get __good_color ; i32.ne
if: entry = call $gc_load_barrier_slow(h)
ptr = entry & 0xFFFF_FFFC
```

fast 路径开销 ≈ 3 条指令（and/比较/br 未命中直落）。zgc 变体的 `obj_set`/`elem_set` 另含 SATB event（mark 期，复用 §8.2 的统一 buffer，但不产生 RSet event）。

### 8.4 mark-sweep 变体

零 barrier（与现状指令序列等价 + §7.2 分配窗口重构）。

### 8.5 INV-C2 修复点（三变体统一）

resize 序列（`$obj_set` 扩容、`$arr_push` 扩容、`$arr_unshift` 头部插入扩容、`$arr_splice` 中间插入扩容、arguments 物化、`$obj_define_property` property slots 扩容，以及所有"分配后引用旧数据"的 helper）改为：`gc_alloc_slow`/分配返回后**重新 `obj_table[h]` 解 old_ptr** 再 `memory.copy`。解引用一律经 `emit_resolve_handle_ptr(flavor)`（zgc 下自动含 load barrier）。P2 产出完整违反点清单文档核对（R1 缓解）。

### 8.6 eval mode

`compile_eval(program, flavor)`：运行时已知算法后编译，inline 对应变体逻辑（含 barrier）。`runtime_eval.rs` 传入当前算法 flavor。

---

## 9. mark-sweep v2（`runtime_gc/mark_sweep/`）

- 逻辑迁移到 v2 接口：`alloc_slow` = free list best-fit → allocation-assist lazy sweep → bump → start/finish collection → grow → None；`collect_full` = v1 `collect_with_provider` 语义（fixed-point 侧表、weak refs、handle 复用协议全继承），但内部切成 mark + sweep-block 两相，供 `safepoint_step` 复用。
- **lazy sweep**（借鉴 Go sweep pacing/Immix line 思路，但不改成 moving）：普通阈值 safepoint 不再总是一口气扫完整堆；先 STW mark，随后按 64KB sweep block 增量 sweep，`alloc_slow` 若 free list 不足则主动 sweep 若干 block 偿还 allocation debt。`gc()`/显式 `collect_full` 仍同步扫完，保持可观测语义简单。这样默认算法保留最少 barrier/最强稳定性，同时把长 sweep pause 从“堆大小相关”降为“mark pause + 若干 sweep slices”。
- **#332 并入（D4）**：`heap_governance.rs` 重整为 mark-sweep 内部模块——尾部空间回收（TRAIL-1..4 不变量保留）+ 碎片指标计算；tail reclaim 只在 sweep cursor 完成全堆后执行（`collect_full` 当场执行，lazy sweep 在最后一个 sweep block 完成时执行），禁止在仍有 unswept block 时回退 `heap_ptr`；修复 WIP 测试与实现签名不一致（`compute_metrics` 参数、`TailReclaimResult` 字段对齐实现）；`tests/heap_governance.rs` 与 `fixtures/happy/gc_fragmentation_churn.*` 修至全绿。`GcStats` 碎片字段（已在 WIP 中扩展）纳入 GcStats v2。
- marker worklist（IMPL-6）/sweeper ptr-sort（IMPL-7）/SegregatedFreeList 原样迁移；sweeper 额外维护 `sweep_cursor` 与 block epoch，保证未扫 block 不进入 free list，已扫 free runs 仍按地址序入表。

---

## 10. wjsm-G1（`runtime_gc/g1/`，子模块 ≤500 行纪律拆分：`region.rs`/`rset.rs`/`young.rs`/`concurrent_mark.rs`/`mixed.rs`/`mod.rs`）

### 10.1 堆域组织（attach_heap）

```
[dynamic_start → N × 64KB region，域起点 64KB 对齐]
host-side metadata:
  region_meta: Vec<RegionMeta>（1B/region + age/implicit-black epoch）
  remembered set: sparse dirty-card set（必要时热点 card 升级 precise-slot bitmap）
  barrier event buffer: 线性内存固定 24KB，由 §7.1/§8.2 共享
```

metadata 不占用 wasm linear dynamic heap，随 committed/grow 后 region_count 扩展；`attach_heap` 只划定 region 域、初始化 host-side `region_meta`/RSet、设置首个 Eden 窗口，不再为 card table/region table 预留 8MiB 线性空间。region_meta 值：`FREE/EDEN/SURVIVOR/OLD/HUMONGOUS_START/HUMONGOUS_CONT/IMMORTAL/META`。**索引基准固定为 `object_heap_start`**：`region_idx = (addr - object_heap_start) >> 16`，`card_idx = (addr - object_heap_start) >> 9`；metadata 覆盖 committed region 域 + immortal 段，grow 时追加，immortal 段标注为 `IMMORTAL`。大对象（size > 32KB = region/2）→ 连续 humongous region，直入 old 逻辑代。

### 10.2 分配

eden region 内 bump（§7.2 窗口）；窗口耗尽 → `alloc_slow`：取 Free region 设为 Eden 换窗口；Eden 配额（初始动态堆 25%，自适应）用尽 → young GC。

### 10.3 young GC（STW，短停顿）

roots = shadow stack + host 侧表（全扫，条目数量级小）+ immortal 对象引用槽扫描 + host-side **dirty cards/precise slots** 指出的 old/humongous/immortal 对象引用槽。dirty card 扫描后不能无条件清空：若精化扫描确认该 card 不再含 Eden/Survivor 引用，则清 dirty；若仍含 young 引用，则保持 dirty / 重新置 dirty，保证 old→survivor 引用在下一轮 young GC 仍可作为 root。
遍历只 follow young 对象（old 活性由并发标记负责）；活对象复制到 Survivor region（age+1；age ≥ 2 或 Survivor 不足 → 晋升 Old）；**只更新 obj_table**（INV-C1 红利：零引用修正）；死亡 young 对象 handle 回收进 free list；Eden/被清空的 From-Survivor region 归还 Free。复制/晋升到 Old/Humongous 后必须扫描目的对象槽位：若仍含 Eden/Survivor 引用，则标脏目的 card，防复制后 old→young 边丢失。weak refs 按本轮 freed handles 处理（共享层）。

### 10.4 并发标记（增量切片）

触发：old（含 humongous）占 heap_limit 比例 ≥ IHOP（45%）。
- **初始标记**（STW，附着在一次 young GC 上）：执行完整 `RuntimeRoots` fixed-point snapshot（含 host 侧表）并在 mutator 恢复前入队/标记，捕获 SATB 起点时刻的侧表旧 root；随后 `__gc_phase = MARK`。
- **增量 drain**：`safepoint_step` 按 budget 处理 worklist（全堆 mark，覆盖 young+old；barrier event buffer 中 SATB event 定期并入 worklist）；mutator 侧 SATB barrier（§8.2a）维持快照不变式；**G1 allocate-black 采用 region-level implicit-black**：mark 期新发放或作为复制/晋升/evacuation 目的地的 Eden/Survivor/Old/Humongous region 本周期视为全活，cleanup 不回收这些 region；下一轮 mark 再按对象粒度精确统计。G1 不新增 `__mark_bitmap_base`，WASM 分配序列零额外 mark bitmap 指令；host-side `RegionMeta` 的 implicit-black epoch 是本语义唯一 owner。
- **final remark**（STW）：drain barrier event buffer 中 SATB 残留 + **host 侧表 fixed-point 重扫**（侧表结构变化不经 WASM barrier，此处兜底，R2）；
- **cleanup**：统计各 region 活字节；全死 region 直接归还 Free。

### 10.5 mixed GC

标记完成后：按 region 活字节升序选 CSet（pause budget 折算复制字节上限），STW evacuate（old→old 压缩复制 + obj_table 更新）；**不做 per-reference 修正，也不需要 incoming-reference remembered set**：引用槽保存 handle，`obj_table[h]` 更新后全堆自然指向新位置。复制到 Old/Humongous 目的 region 后同 young 晋升规则扫描目的对象槽位，若仍含 young 引用则标脏目的 card。mixed 分多次执行直到候选耗尽或收益低于阈值（活字节 > 85% 的 region 不搬）。碎片由此根治（#332 目标的 g1 解）。

### 10.6 host 侧配合

`on_host_write(target, slot_addr, old_val, new_val)` = Rust 版 §8.2 同语义：SATB event + RSet event 直接入 host-side set，不绕线性 buffer；`slot_addr` 是 precise-slot/card owner，不允许只传 target handle。host 分配大对象/host `alloc_host_object_impl` 走算法 `alloc_slow` 同一入口（`runtime_heap.rs` 分配路径统一接到 v2）。

---

## 11. wjsm-ZGC（`runtime_gc/zgc/`，拆分：`color.rs`/`page.rs`/`mark.rs`/`relocate.rs`/`mod.rs`）

### 11.1 堆域组织

`[dynamic_start → N × 64KB zPage]`。非分代（D10）。page meta / live bytes / relocation-set bits 全部为 host-side metadata，随 committed page_count 扩展；ZGC 只需要线性内存中的统一 barrier event buffer（SATB）和 obj_table entry 色位，不在 wasm dynamic heap 头部保留 page meta。

### 11.2 染色协议（obj_table entry 低 2 位，D12；标准 ZGC 双 good 切换）

```
00 = 空槽/未初始化（handle 空闲态，与 ptr==0 一致）
01 = Marked0   10 = Marked1   11 = Remapped
```

`__good_color` 在一个周期内切换两次（host 维护）：

- **mark 期**：good = 本周期 mark 色（Marked0/Marked1 逐周期交替）。entry 色 ≠ good（携带上周期 Remapped 或旧 mark 色）→ load barrier slow：标记对象入 worklist + entry 色置本周期 mark 色。
- **relocate 期**：good = Remapped(11)。entry 色 = mark 色（≠ 11）→ load barrier slow：对象在 RS → 搬迁 + `entry = new_ptr | 11`；不在 RS → 仅置 `entry |= 11` 色位。
- **下周期 mark start**：good 翻转为另一 mark 色 → 上周期所有 Remapped/旧 mark 色引用再次变"坏"→ 驱动重新标记。

barrier 恒为单比较 `(entry & 3) != __good_color`（§8.3 序列不变）。mark 色双色交替的意义：免去周期间全表清色扫描（上周期 mark 色在下周期自然是坏色）。

### 11.3 周期状态机

```
Idle → MarkStart(STW: __good_color=本周期 mark 色(双色交替), root snapshot,
                 __gc_phase=MARK)
     → MarkStep*(增量: worklist drain, budget 切片; SATB 吸收覆盖写;
                 load barrier 命中坏色对象 → 协助标记+置 mark 色; allocate-black
                 = 新对象 entry 直接置当前 good 色)
     → MarkEnd(STW: SATB 残留 + host 侧表 fixed-point 重扫 + weak refs)
     → SelectRelocSet(活字节/碎片率排序；优先低密度/高碎片 page，dense page aging-in-place；碎片率>25% 的 page 候选，按预算截断)
     → RelocateStep*(增量: __gc_phase=RELOCATE; __good_color=Remapped;
                     主动搬 RS 内活对象 + load barrier 命中 mark 色对象 →
                     RS 内协助搬/RS 外置 Remapped 色; 搬完的 page 归还)
     → Idle(收尾: freed handles 入 free list, stats 写入)
```

搬迁 = 目标 page 分配 + memcpy + `obj_table[h] = new_ptr | good`。**无 per-reference self-healing**：obj_table 单点 truth，一处更新全堆生效（wjsm 相对真 ZGC 的结构性简化，见 §2 D12 依据）。

### 11.4 host 侧配合

`on_host_resolve`：RELOCATE 期解引用 RS 内对象 → 先协助搬迁再返回新 ptr（**强制**，否则 host 写旧位置丢数据，R3）；`heap_access::resolve` 是唯一通道。`on_host_write`：MARK 期 SATB。

---

## 12. 增量调度器（`runtime_gc/scheduler.rs`）

- **触发协议**：WASM 累加 `__gc_alloc_bytes` ≥ `__gc_trigger_bytes` → `gc_safepoint_poll`（先 flush barrier buffer）→ 调度器构造 `StepBudget { work_bytes, deadline = now + pause_target }` → `safepoint_step`。
- **pause target**：默认 4ms；`WJSM_GC_PAUSE_TARGET_MS` / `RuntimeOptions` 可调。budget.work_bytes 由近期步进吞吐自适应（超时则减半，富余则倍增，clamp [64KB, 8MB]）。
- **Go-style heap-goal pacer**：每轮 collection 结束后计算 `heap_goal = live_bytes + max(live_bytes * gc_percent / 100, root_scan_bytes_estimate)`，再受 `heap_limit` 与 pause target 截断；`__gc_trigger_bytes` = `(heap_goal - allocated_bytes) / remaining_steps` 的平滑值。若 mutator 分配速率超过 GC 步进吞吐，`alloc_slow` 进入 allocation assist，同步执行步进直到 debt 清零，避免“分配跑赢 GC”。
- **trigger 自适应**：Idle 期 trigger 根据 heap_goal 放大；周期进行中 trigger 收紧（64KB 起），保证 barrier flush + mark/relocate 步进频率跟上分配速率。mark-sweep 默认只增量/惰性 sweep，不并发 mark；g1/zgc 使用 pacer 控制 incremental/concurrent work。
- 所有 GC 工作发生在 sync host call 内（sync `Func::wrap`，不 `.await`、不回进 WASM——IMPL-3 全继承；epoch async yield 与 GC 无交叠）。
- STW 内并行预留：mark worklist 分片接口 + region 复制无共享 `&mut`；实现 defer（§3）。

---

## 13. host 统一读写层（`runtime_gc/heap_access.rs`）

**本重构主工作量之一**。host Rust 侧对对象堆的读写收敛到唯一入口：

```rust
/// 解 handle → ptr。zgc RELOCATE 期强制 heal（on_host_resolve）。
/// debug 构建返回 HeapPtr{ptr, epoch}，使用时校验 epoch（INV-C2 执法）。
pub fn resolve(ctx, env, h: Handle) -> Option<HeapPtr>;
/// 写属性槽 value/getter/setter：先计算精确 slot_addr，读旧值，调用 on_host_write(target, slot_addr, old, new)，再写。
pub fn write_property_slot(ctx, env, h: Handle, slot_idx: usize, part: SlotPart, val: i64);
/// 写数组元素：slot_addr = elements_base + idx * 8，同样进入 on_host_write。
pub fn write_element(ctx, env, h: Handle, idx: usize, val: i64);
/// 写 proto 字段：proto 是 u32 handle/null 哨兵；实现内转换为 NaN-boxed old/new value，slot_addr = ptr + HEAP_OBJECT_PROTO_OFFSET 后再 barrier。
pub fn write_proto(ctx, env, h: Handle, proto: u32);
```

- **审计范围**：`host_imports/`（34 文件）+ `runtime_values.rs`/`runtime_heap.rs`/`runtime_builtins.rs` 等所有直接 `data_mut` 写对象属性槽/元素/proto 的点，全部替换；**不留旧裸写路径**（barrier 漏写 = 増量标记漏对象 = 误回收）。裸读（不跨 GC 点）替换为 `resolve` + 短生命周期使用。
- P2 产出机械核对清单：`grep` 全部 `HEAP_OBJECT_PROPERTY`/`HEAP_ARRAY_ELEMENT`/`HEAP_OBJECT_PROTO_OFFSET` 偏移写点逐条勾销；再用 `copy_from_slice(&proto`、`ptr..ptr + 4`、`PROTO_OFFSET`、`setPrototypeOf`/`Object.create`/`Reflect.setPrototypeOf` 等模式交叉核对裸 proto header 写，全部替换为 `heap_access::write_proto`（R1）。
- host 侧表写入（promise/continuation 等 Rust 结构）**不经此层**（不是堆写）——一致性由 initial mark 的 `RuntimeRoots` fixed-point snapshot 捕获起点旧 root，并由 final remark 重扫兜底（§10.4/§11.3）。

---

## 14. Root 发现与共享层

- `RootProvider`/`RuntimeRoots` 签名与 fixed-point 协议不变（#334 owner-aware 语义保留）；三算法共用（单 owner）。
- 新增共享扫描项：**immortal 对象引用槽扫描**（mark 与 young GC 的 root 源，§10.3/§11.3）。不要线性扫描 `[object_heap_start, dynamic_start)`：该区间可能含对齐 padding 或 abandoned 旧块。实现必须遍历 live `obj_table` handles，筛选 ptr 落在 `[object_heap_start, immortal_objects_end)` 的对象并用统一对象 walker 扫槽位；live handle 指向非法 header 属于 runtime invariant 破坏，debug 构建触发断言，release 路径返回错误/Trap。
- 新增共享 `ObjectWalker` + `ScanTask`：普通对象扫描整对象；大数组/大 property storage 拆成 **oblets**（例如每 256/512 slots 一个 task），避免单个巨型数组让 mark/young/mixed/zgc step 超过 pause target。walker 只产生 handle/value 级 root，不泄漏裸 ptr；算法只消费 `ScanTask`，不复制各自扫描器。
- weak refs / stream 清理 / owner-backed 侧表回收：v1 协议保留，入口改为“算法周期收尾统一调用共享函数”（`weak_refs::process_after_collection(ctx, freed)`）。
- napi root 源（napi spec §4.2）：`RootProvider` 扩展点已就位，napi 落地时按 handle 注入即可（moving 兼容性见 §15.2）。
---

## 15. snapshot / N-API 兼容

### 15.1 startup snapshot：immortal 区

快照堆映像 = **bootstrap immortal objects**：restore 后 live `obj_table` 中 ptr 落在 `[object_heap_start, immortal_objects_end)` 的对象为三算法一致的永久对象集合（primordial 对象本就永久可达）——不回收、不搬迁、参与 root 扫描与 remembered-set 覆盖。`immortal_objects_end` 是有效 bootstrap 对象字节末端；`dynamic_start = align_up(immortal_objects_end, 64KB)` 是算法 `attach_heap` 接管点；两者之间的 padding 不扫描、不分配。snapshot format 新增/重命名 immortal 对象边界字段（`immortal_objects_end_rel`）+ format 版本递增；snapshot 存储的 obj_table rel offset 始终是不带 ZGC 色位的裸 ptr 偏移；`abi_hash()` 输入追加 GcFlavor 无关的 v2 布局常量（barrier event layout、region/page 参数、globals 并集签名、flavor-independent support ABI union hash）。冷启动路径（无快照）：bootstrap 完成后当场划定 `immortal_objects_end` 与 `dynamic_start`，行为一致。ZGC 选择时，restore/cold freeze 后必须把所有 live immortal `obj_table[h]` 重写为 `ptr | initial_good_color`；mark-sweep/g1 保持纯 ptr。

### 15.2 N-API spec 修订

`2026-07-03-napi-native-addon-design.md` 两处措辞将"GC 为 non-moving mark-sweep"修订为"GC 保证 **handle 恒定**（INV-C1）"：napi_value slot 存 boxed handle，moving 不改写 slot 内容，语义不变；`ArrayBufferEntry.data` 裸指针指向 host 侧 `Vec<u8>`（不在对象堆），不受 moving 影响。napi_ref pinned root 集接入 §14 扩展点。

---

## 16. 算法选择机制

优先级：CLI `--gc <mark-sweep|g1|zgc>` > env `WJSM_GC` > 默认 `mark-sweep`（D5）。
- `wjsm-cli`：`run`/`eval` 子命令加 `--gc`；非法名 → 错误退出并列出可选值。
- `wjsm-runtime`：`RuntimeOptions::gc_algorithm: GcAlgorithmKind`（enum，`FromStr`）；`GcRegistry::create(kind)` 装配算法实例 + 装载对应 support cwasm 变体 + eval flavor 记录。
- 进程内固定（D2）；`gc()` JS global → 当前算法 `collect_full`。

---

## 17. 可观测性（GcStats v2）

继承现有全部字段（含 #332 碎片指标）+ 新增：

```rust
pub struct GcStats {
    /* v1 + #332 字段全保留 */
    pub cycle_kind: CycleKind,        // Full / Young / Mixed / ZgcCycle / Step
    pub pause_ns_max: u64,            // 本周期内单次 STW/step 最大时长
    pub pause_ns_total: u64,
    pub pause_count: usize,
    pub relocated_bytes: usize,       // g1 evacuate / zgc relocate
    pub relocated_objects: usize,
    pub committed_pages: usize,
    pub free_bytes_reusable: usize,
    pub regions_total: usize, pub regions_free: usize,
    pub regions_eden: usize, pub regions_survivor: usize,
    pub regions_old: usize, pub regions_humongous: usize,
    pub satb_flushes: usize,
    pub barrier_events: usize,
    pub rset_cards: usize,
    pub rset_precise_slots: usize,
    pub scan_oblets: usize,
    pub load_barrier_mark_hits: usize,
    pub load_barrier_relocate_hits: usize,
}
```

`last_gc_stats`（#332 WIP 已加）保留为可观测出口 + 新增累计 pause 直方图（`RuntimeState.gc_pause_hist`，环形缓冲最近 256 次），`WJSM_GC_LOG=1` 时每周期 eprintln 摘要。benchmark（§21.2）直接消费这些数据。

---

## 18. 删除/迁移清单（根除 duplicate owner）

| 现有物 | 处置 |
|--------|------|
| v1 trait：`Allocator`/`Marker`/`Sweeper`/`WriteBarrier`/`ReadBarrier`/`HeapRegionManager`/`MarkProgress` | **删除**（v2 接口取代） |
| `MarkSweepCollector` v1 组合实现 | **迁移**至 v2 接口（§9），内部逻辑保留 |
| `gc_maybe_collect` host import + `$obj_new`/`$arr_new` 头部无条件调用 + 旧 `__alloc_counter` global | **删除**（§7.2 counter 内联为 `__gc_alloc_bytes` + `gc_safepoint_poll`） |
| `heap_governance.rs` WIP（含签名不一致测试） | **重整**入 mark-sweep v2（§9） |
| `runtime_heap.rs` host 分配 + `collect_for_host_alloc` | **改接** v2 registry 入口；裸写点入 §13 替换清单 |
| v1 spec 附录 D 稳定性承诺 | **取代声明**（附录 B） |
| N-API spec "non-moving" 措辞 | **修订**（§15.2） |
| AGENTS.md / CLAUDE.md WASM contract（globals/imports 数、GC 描述） | **更新** |
| `docs/adr/0005-pluggable-gc-v2.md` | **新增**（落地后回填） |

删除纪律：每阶段完成后 grep 确认无残留引用（v1 同款流程）。

---

## 19. 实施阶段

| 阶段 | 内容 | 验证 |
|------|------|------|
| **P0** | 框架 v2：api/registry/scheduler 骨架；mark-sweep 迁移至 v2（行为等价）；#332 重整收尾（heap_governance 测试修复、fixture 绿） | 全量 fixture 绿（默认算法）；`gc_fragmentation_churn` 绿；单元测试 trait 契约 |
| **P1** | 布局层：immortal 边界 + snapshot format 升级；统一分配窗口 + counter 内联 + 新 globals；support emitter 参数化（仅出 mark-sweep 变体验证重构无回归）；`gc_safepoint_poll` 替换 | 全量 fixture 绿；dump-wat 检查分配序列；snapshot 冷/热启动均绿 |
| **P2** | `heap_access.rs` + host 裸写点全量替换 + INV-C2 审计（helper resize re-resolve + WASM 违反点清单 + debug epoch 断言） | 全量 fixture 绿（断言开启）；裸写点清单勾销文档 |
| **P3** | g1 变体 + G1 算法（region/rset/barrier-buffer/young/并发标记/mixed） | GC 密集子集 @ g1 绿；`WJSM_TEST_GC=g1` 全量绿；young/mixed 单元测试；pause 初测 |
| **P4** | zgc 变体 + ZGC 算法（染色/load barrier/relocate 状态机） | 同上 @ zgc；relocate 期 host 读写专项测试（R3） |
| **P5** | 选择机制（CLI/env/API）+ GcStats v2 完善 + benchmark 套件 + 定量验收调参 | §21.2 定量项达标 |
| **P6** | 删除清单执行 + 文档（AGENTS.md/CLAUDE.md/ADR 0005/N-API 修订）+ 全矩阵验证 | grep 无残留；三算法 `WJSM_TEST_GC` 全量绿；零 warning |

阶段独立可提交；P3/P4 互相独立（可并行推进）。

---

## 20. 风险与缓解

| 风险 | 缓解 |
|------|------|
| **R1** host 裸写点遗漏（21k 行审计面）→ barrier 漏写 → 增量标记误回收 | heap_access 唯一入口 + P2 机械 grep 清单逐条勾销 + debug epoch 断言 + GC 密集子集在 g1/zgc 下高频回归 |
| **R2** host 侧表结构变化不经 barrier → SATB 不变式破洞 | initial mark STW 执行完整 `RuntimeRoots` fixed-point snapshot 捕获起点旧 root；final remark 再做 fixed-point 重扫兜底（§10.4/§11.3）；侧表条目量级小，remark 成本可控 |
| **R3** zgc RELOCATE 期 host 写旧位置 | `heap_access::resolve` 强制 `on_host_resolve` heal；debug 断言"RS 内对象解引用必已 heal"；P4 专项测试 |
| **R4** 三变体 emitter drift | 单源参数化（§8.1 禁复制）；startup snapshot ABI 使用 flavor-independent support ABI union hash，具体 flavor cwasm 字节 hash 不参与 snapshot ABI；共用指令序列测试 |
| **R5** 增量周期被分配速率跑赢 → OOM | trigger 自适应 + `alloc_slow` 内同步补步进（§12）；长循环 fixture 三算法验证 |
| **R6** pause 不达标 | budget 自适应；benchmark 前移至 P3/P4 内部初测，不留到 P5 才发现 |
| **R7** snapshot 兼容回归 | immortal objects 设计不改快照对象内容；新增 `immortal_objects_end`/`dynamic_start` 边界与 object_heap_start 64KB 对齐由 format 版本 + ABI hash 既有 fallback 机制覆盖；P1 冷/热双路径测试 |
| **R8** counter 内联/窗口重构破坏现分配语义 | P1 仅重构不加算法，全量 fixture 独立验证后再进 P3/P4 |
| **R9** `Arc<Mutex<dyn GcAlgorithm>>` 跨 clone 的 RuntimeState 共享语义（agent_cluster 等） | 保持 v1 同款共享结构（算法实例进程内单例）；registry 装配仅在启动时 |

---

## 21. 验收标准

### 21.1 功能

1. 全量 fixture（470+）在默认 mark-sweep 下全绿；GC 密集子集（`gc_*`、长循环 churn、async/streams/weak/BYOB 系列）在 g1、zgc 下各自全绿；`WJSM_TEST_GC={g1,zgc}` 全量 fixture 手动矩阵全绿（D7）。
2. 长循环不 OOM（`for(…1e8…) arr.push({x:i})`）在三算法下均成立。
3. 三算法 handle 复用正常（churn 后 `obj_table_count` 有界）。
4. snapshot：三算法均从 embedded snapshot 正常恢复；ABI hash 更新后旧产物冷启动。
5. INV-C2：debug epoch 断言在全量 fixture + 子集矩阵下零触发。
6. 删除清单（§18）执行完毕，grep 无残留；构建零 warning。
7. `--gc`/`WJSM_GC`/`RuntimeOptions` 三入口生效且优先级正确；非法值报错清晰。

### 21.2 定量（churn 基准：1e7 次短命对象分配 + 5% 存活入持久 Map + 周期性大数组，固定 heap_limit；`tests/gc_pause_bench.rs`，`WJSM_GC_BENCH=1` 门控）

1. **g1**：young GC 单次 pause max ≤ **8ms**（2×pause target），且 ≤ 同负载 mark-sweep 单次 full collect pause 的 **1/5**。
2. **zgc**：任意单次 STW/step pause max ≤ **8ms**，同样 ≤ mark-sweep full collect 的 **1/5**。
3. **吞吐**：三算法 churn 总耗时均 ≤ mark-sweep 基线 × **1.25**。
4. **碎片**（#332 承接）：churn 稳态下 g1 `external_fragmentation` < mark-sweep 同负载值（mixed 压缩生效证据）；zgc 同。
5. 指标全部来自 GcStats v2 实测输出（§17），benchmark 断言阈值，不达标即测试失败。

---

## 22. 不变量与实现约束清单

### 22.1 堆/对象层

| ID | 不变量 | 维护点 |
|----|--------|--------|
| INV-A | obj_table 是堆块完整索引（分配返回前注册） | 三变体 fast-path、alloc_slow、host 分配 |
| INV-B | resize 重写 obj_table 槽（handle 不变）——即局部 moving | resize 路径（v1 继承，语义并入 INV-C1） |
| **INV-C1** | handle 恒定；obj_table 唯一 truth；moving 仅在安全点内更新槽 | 全部算法 |
| **INV-C2** | raw ptr 不跨潜在 GC 点；跨越必 re-resolve | §8.5 WASM + §13 host |
| INV-D | 活动对象布局（16B header）不变 | 本次不改 |
| **INV-E** | immortal 区对象不回收不搬迁，但其引用槽是 root 源且写入过 barrier | attach_heap + roots + barrier |

### 22.2 分配/触发层（v1 IMPL-1..5 继承，变更项）

| ID | 约束 |
|----|------|
| IMPL-2' | 触发计量内联 WASM（`__gc_alloc_bytes`），`gc_safepoint_poll` 仅在达阈值时调用 |
| IMPL-3 | 全部 GC host imports = sync `Func::wrap`，闭包内不 `.await`/不回进 WASM（继承） |
| IMPL-5' | 步进/回收只发生在 safepoint（分配点/poll/load-barrier-slow），spill 已就位（继承）；`gc_barrier_flush` 只 drain buffer，不推进 GC |
| **IMPL-14** | mark 期 allocate-black：G1 = region-level implicit-black（mark 期新发放或作为复制/晋升/evacuation 目的地的 region 本周期全活，不新增 bitmap global）；ZGC = 新 obj_table entry 直接置当前 good 色 |
| **IMPL-15** | 窗口换页/换 region 只由 host `alloc_slow` 执行；WASM 只 bump 不换窗口 |

### 22.3 算法层

| ID | 约束 |
|----|------|
| IMPL-6/7/8/9/10 | v1 全继承（worklist 不递归 / sweep ptr-sort / ctx 不持 slice / continuation root / handle 槽复用） |
| **IMPL-16** | SATB 不变式：并发标记期覆盖写的旧引用必入 SATB（WASM barrier + host on_host_write 双端） |
| **IMPL-17** | zgc RELOCATE 期任何 host 解引用必经 resolve（heal），禁止直读 obj_table |
| **IMPL-18** | final remark 必含 host 侧表 fixed-point 重扫（R2 兜底） |
| **IMPL-19** | 三变体由单源 emitter 参数化生成，禁止复制 emitter |

---

## 附录 A：治理 artifacts

```text
BaselineUsageDraft:
- Required baseline refs: 2026-06-14 GC spec / ADR 0003 / ADR 0004 / napi spec / #332 WIP
- Cited in design refs: 全部（§1.3 BaselineReadSetHint 逐文件行号）
- Missing refs: 无
- Decision: continue

Baseline Role Alignment:
- Product baseline: 用户任务指令（三算法内置 + 运行时可选 + 完全重构）
- Architecture baseline: INV-C 重写属 Design Defect 修正（v1 不变量过强，与 INV-B 矛盾，
  resize 路径证明真实依赖是 handle 稳定）；scope: architecture
- Result: aligned（v1 附录 D 承诺由本 spec 显式取代，见附录 B）

Architecture Integrity Lens:
- Invariant: INV-C1/C2/E（§22）
- Canonical owner: 算法=runtime_gc/{mark_sweep,g1,zgc}；调度=scheduler.rs；
  host 堆写=heap_access.rs（唯一）；root=roots.rs（唯一）；变体生成=support emitter（单源）
- Responsibility overlap: 无（v1 trait 删除后 barrier/region 只有 v2 一套）
- Higher-level simplification: obj_table 间接使三算法免去引用修正——已利用（D12）
- Retirement: §18 清单 + 阶段 grep
- Verdict: pass

Product Risk Lens:
- Value: 低停顿可证明（定量验收）；碎片根治（mixed/relocate）；算法选择自由
- Non-goals: §3
- Trade-offs: g1/zgc 变体 barrier 常驻开销（选用才付）；三变体维护约束（IMPL-19）
- Decision needed: 无（D1-D13 已闭合）

Complexity Budget:
- Artifact class: 跨 crate 架构重构（runtime/backend/support/snapshot/cli）
- 现压力: runtime_gc 3.7k 行 → 预估 v2 全量 ~8-9k 行（g1/zgc 各 ~1.5-2k，子模块 ≤500 行拆分）
- Budget result: at-risk → 治理：§10/§11 首段规定子模块拆分；P2 heap_access 收敛写点降低 host_imports 复杂度
```

## 附录 B：对 v1 spec 附录 D 的取代声明

2026-06-14 spec 附录 D 承诺"trait 签名 / GcContext 字段集 / fast-path 物理边界稳定"，其适用前提是"后续算法只 impl 新 struct 不改框架"。本次任务（用户指令：完全重构 + 内置 G1/ZGC）改变了前提：两段式 trait 无法表达 evacuation/coloring（§1.1 #1）。本 spec 取代该承诺：v2 接口（§6）成为新稳定边界；`GcContext` 字段集只增不减的承诺**继续有效**；`Handle`/`Value` 别名、NaN-boxing、obj_table 间接、safepoint spill 体系不变的承诺**继续有效**。此取代作为 ADR 0005 的核心内容之一记录。

## 附录 C：参数与命名清单（默认值，均可 env 覆盖）

| 参数 | 默认 | 覆盖 |
|------|------|------|
| region/page 大小 | 64KB | `WJSM_GC_REGION_SIZE`（64KB 倍数） |
| card 大小 | 512B | 编译期常量 |
| pause target | 4ms | `WJSM_GC_PAUSE_TARGET_MS` |
| eden 初始配额 | 动态堆 25% | 自适应 |
| 晋升 age | 2 | 编译期常量 |
| IHOP | old 占 45% | `WJSM_GC_IHOP_PERCENT` |
| barrier event buffer | 24KB，24B/entry（1024 entries） | 编译期常量 |
| zgc RS 碎片阈值 | 25% | 编译期常量 |
| 步进 trigger 初值 | 256KB | 自适应（§12） |
| benchmark 门控 | — | `WJSM_GC_BENCH=1` |
| 测试矩阵 | — | `WJSM_TEST_GC={mark-sweep,g1,zgc}` |
| GC 日志 | 关 | `WJSM_GC_LOG=1` |

## 附录 D：成熟实现借鉴点（已筛选后落地）

| 来源 | 可迁移点 | v2 落地 |
|------|----------|---------|
| V8 Orinoco（incremental/concurrent marking、write barrier、concurrent sweeping） | mark/sweep 不必都在一个长停顿里完成；barrier 只维护三色/SATB 不变量 | mark-sweep lazy sweep；G1/ZGC 统一 barrier event buffer；`ObjectWalker`/oblet 避免大对象长步进 |
| Go GC guide/runtime（heap goal pacer、allocation assist、lazy sweeping） | 用 live heap + root 成本推导下一轮目标；mutator 跑赢 GC 时用 allocation assist 补偿 | §12 heap-goal pacer；`alloc_slow` 偿还 GC debt；mark-sweep sweep-block 惰性推进 |
| HotSpot G1 / Generational ZGC | remembered set/card 思路与 store barrier；分代低停顿依赖 old→young 精确根 | G1 host-side sparse dirty-card/precise-slot RSet；dirty card 精化后 re-dirty；barrier event buffer 由 host 过滤 generation |
| OpenJDK ZGC | colored pointers + load barrier + relocation set，pause 与 live set/heap size 解耦 | ZGC `obj_table` 低位染色；bad color slow path heal；relocation set 按低密度/高碎片 page 选取 |
| Shenandoah self-fixing barrier | barrier 命中后修复引用，后续 fast path 受益 | ZGC slow path/host resolve 统一 heal `obj_table[h]`，并用 `gc_epoch` 让旧 HeapPtr 失效 |
| Immix | line/block 粒度回收能降低碎片并改善 locality，但 full moving 不必复制到默认算法 | mark-sweep 只吸收 block/line 级 lazy sweep 思想，不引入默认 moving，moving 由 G1/ZGC 承担 |

参考链接：V8 concurrent marking <https://v8.dev/blog/concurrent-marking>；V8 Orinoco <https://v8.dev/blog/trash-talk>；Go GC guide <https://go.dev/doc/gc-guide>；Go runtime GC <https://go.dev/src/runtime/mgc.go>；OpenJDK ZGC JEP 333 <https://openjdk.org/jeps/333>；Generational ZGC JEP 439 <https://openjdk.org/jeps/439>；Shenandoah wiki <https://wiki.openjdk.org/spaces/shenandoah/pages/25002018/Main>；Immix paper <https://www.steveblackburn.org/pubs/papers/immix-pldi-2008.pdf>。
