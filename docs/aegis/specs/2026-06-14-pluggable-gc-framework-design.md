# wjsm 可插拔 GC 框架设计规格

**状态**: 待审批（R1 修订，已采纳外部审查意见）
**日期**: 2026-06-14
**修订**: R1 — 采纳外部代码审查（经验证，含正确性修正 #1/#2/#3/#7/#8/#9/#11，收敛 #5/#6/#10，补充 #4；驳回被验证为错误的"当前 compact 不需 sort"论断）。改动：§6 GcContext 改为 Caller 注入（#9 grow 借用安全）；§7.1/#2 proactive counter 移至 WASM fast-path；§8.1 mark 改 worklist（#11 栈溢出）；§8.2 确认 sort 必需（#3 resize 破坏单调性）；§9.1 冻结初始 size class（#5）；§11.1 liveness 覆盖 Phi 合并（#10）；新增 §18 不变量与实现约束清单。
**范围**: `wjsm-runtime`（GC 框架 + MarkSweep 实现）、`wjsm-backend-wasm`（分配路径改造 + safepoint spill）、`wjsm-ir`（liveness + 类型推断 pass）
**权威来源**: `bug.md` O2；`docs/aegis/specs/2026-06-02-unified-async-execution-model-design.md`（async Store re-entry）；`docs/aegis/specs/2026-06-07-runtime-side-table-lifecycle-design.md`（侧表回收，与本 spec 互补）
**ADR 信号**: GC 算法 trait 抽象边界、分配路径物理边界（编译期 fast-path / 运行期 slow-path）、活动对象布局不变性 —— owner=`runtime_gc/`（新）、`compiler_helpers.rs`（分配）、`compiler_instructions.rs`（safepoint）；不变量=**对象永不动（non-moving），handle→ptr 映射稳定**

---

## 1. 问题陈述

### 1.1 现状（2026-06-14）

代码库**已存在一个真正可用的 mark-sweep-compact GC**（`runtime_builtins.rs:trigger_gc` L2939-3223 + `runtime_heap.rs:mark_object_recursive` L577-761），做完整 mark → fixed-point 侧表追踪 → 压缩（compact）→ 重写 `obj_table` slot。架构上有两个关键优势：

1. 所有对象引用走 **handle index**（`obj_table` 间接），不是原始指针；
2. 完整的 fixed-point 侧表 root 追踪（promise reactions / microtask / continuation / streams / collections）。

但该 GC **当前对所有自动分配路径不可达**。提交 `0849b37`（2026-06-14）将 `$obj_new` / `$arr_new` 的 OOM 回退从 `gc_collect` 改为 `memory.grow`，并删除了 proactive GC（每 1000 次分配触发）。**当前堆单调增长，直到 JS 显式调用 `gc()` 或进程退出。**

### 1.2 根因：O2

`bug.md` O2（标记 FIXED，实为 workaround）：在 `$obj_new` / `$arr_new` 同步路径触发 GC 时，**调用者 WASM 局部变量（locals）里的对象引用对 host 不可见**。当前 collector 是 moving（compact 式），GC 后对象被 `ptr::copy` 移动并重写 `obj_table`，但 WASM locals 里缓存的原始指针/handle 不会更新 → 悬垂引用。

具体机制：`gc_collect` 是 host import；在 async Store（`Config::async_support(true)` + epoch yield）下，host call 是潜在 suspend 点，synchronous JS 操作中途触发会破坏线性语义。

### 1.3 目标（TaskIntentDraft）

- **Outcome**: wjsm 拥有像 V8/JVM 那样的、自动触发、安全、**可插拔**的 GC；堆对象在不可达时被回收，同步长循环不 OOM。
- **Goal**: 用 non-moving mark-sweep + free list 替代 `memory.grow` 无限扩容；建立 `GcAlgorithm` trait 框架，预留 generational/incremental/parallel 接入点。
- **Success evidence**:
  - `for (let i = 0; i < 1e8; i++) arr.push({x: i})` 不再 OOM，GC 自动触发回收死对象；
  - 现有 470+ fixture 全绿（尤其 async / stream / continuation / BYOB 路径）；
  - （safepoint 安全性验收见 §17 #3）
  - 任意 `impl GcAlgorithm` 的实现都能通过统一入口运行且保持 fixture 全绿。
- **Stop condition**: MarkSweep 算法实现 + 框架落地 + 长循环回归测试证明非平凡场景安全。
- **Non-goals**:
  - 不实现 generational / incremental / parallel GC（仅留 trait 扩展点）；
  - 不引入 WASM GC proposal（stack maps / externref）—— 保持 host 扫描模型；
  - 不做分代 write barrier 的真实实现（defer 到 generational，见 §12）。
- **Scope boundary**: trait 框架 + MarkSweep 实现 + 编译器 safepoint spill + 根除现有 GC 双实现。
- **Risks**: 见 §15。

### 1.4 BaselineReadSetHint

- `crates/wjsm-runtime/src/runtime_heap.rs:4-39`（host allocator + obj_table 注册）、`:577-761`（mark phase，**递归实现，待改 worklist**）
- `crates/wjsm-runtime/src/runtime_values.rs:190-282`（`grow_array`/`grow_object`：resize 把已有 handle 的 ptr 重写到更高位置，破坏 handle→ptr 单调性 —— sweep 排序必需的根因）
- `crates/wjsm-runtime/src/runtime_builtins.rs:2939-3223`（`trigger_gc` 完整实现，含 `live_objects.sort_by_key` L3178）、`:2590-2918`（fixed-point 侧表 tracer）、`:3187-3212`（compact 每轮独立 `data_mut` 重借，borrow 模式参考）
- `crates/wjsm-runtime/src/host_imports/core.rs:1218-1642`（旧 `gc_collect` host import，不完整，待删除）
- `crates/wjsm-runtime/src/runtime_eval.rs:193-202`（**全 runtime 唯一** `memory.grow` 调用点；用 `data_size` 不持 slice，borrow 安全模式参考）
- `crates/wjsm-runtime/src/lib.rs:833-836,851-854`（epoch interruption，**非** `async_support`；sync `Func::wrap` 不 yield）
- `crates/wjsm-backend-wasm/src/compiler_helpers.rs:56-195`（`$obj_new`，`memory.grow` OOM 在 73-109）
- `crates/wjsm-backend-wasm/src/compiler_array_helpers.rs:11-146`（`$arr_new`，`memory.grow` OOM 在 28-58）
- `crates/wjsm-backend-wasm/src/compiler_instructions.rs:770-918`（call-site arg push，shadow_sp save/restore）
- `crates/wjsm-backend-wasm/src/compiler_module.rs:320-518`（memory layout、globals）
- `crates/wjsm-backend-wasm/src/compiler_core.rs:6-8`（`local_idx`，类型盲）
- `crates/wjsm-semantic/src/lowerer_async_eval.rs:70-144`（liveness 骨架，可移植）
- `crates/wjsm-ir/src/value.rs:367-375`（`is_js_object`，需扩展为完整 handle 谓词）

### 1.5 ImpactStatementDraft

| 层 | 影响 |
|----|------|
| `wjsm-ir` | 新增 `ValueTy`（Handle/Scalar）+ per-ValueId liveness pass（从零建）；扩展 `tag_needs_root` 谓词 |
| `wjsm-backend-wasm` | `$obj_new`/`$arr_new` 改 bump + host slow；新增 safepoint spill 代码生成；shadow stack 容量策略 |
| `wjsm-runtime` | 新增 `runtime_gc/` 模块组（trait + MarkSweep + free list + roots）；废弃 compact 路径；合并两套 GC 为单一框架入口；host `gc_alloc_slow` |
| `wjsm-cli` | `--gc-algorithm` 调试开关 |
| `wjsm-semantic` | 无变化 |
| `wjsm-ir`（对象布局） | **无变化**（活动对象布局不变，见 §5） |
| 文档 | AGENTS.md GC 描述更新；`bug.md` O2 → RESOLVED |

**兼容性边界**: 现有 fixture stdout/语义不变；NaN-boxing 不变；`obj_table` 间接不变；`gc()` global 行为保持（背后换成框架）。

---

## 2. 决策矩阵

| 维度 | 决策 | 理由 |
|------|------|------|
| 回收语义 | **Non-moving** | 对象永不动；handle→ptr 不变；从根本上消除 O2 的悬垂引用 |
| 触发策略 | **alloc 时精确 spill** | 可任意时刻触发；同步长循环不 OOM |
| Root 发现 | shadow stack + host tables + spilled locals | host 只能扫 memory，所以 locals 必须 spill 到 shadow stack |
| Spill 策略 | **liveness + 保守类型** | polymorphic ops（GetProp/Call/Phi）无法静态定型 → 保守当 Handle；liveness 干掉死值 |
| 抽象层级 | **全面 trait + 预留未来 hook** | JVM 式；trait 完整，hook 默认 no-op |
| 分配物理边界 | **WASM bump fast-path + host slow-path** | fast-path 烧进 WASM（快），slow-path 走 trait（可插拔）；对象布局固定 |
| Free list | **Segregated fit**（size class table） | 分配 O(1)；P0 实测定 class |
| 方案 | **A：激进 JVM 式** | trait + 预留 WriteBarrier/HeapRegion/mark_step 等 |

---

## 3. 范围声明

**本次实现**：MarkSweep（non-moving + free list）+ 完整 GC trait 框架 + safepoint spill + 旧两套 GC 删除。

**后续独立工作（明确排除）**：generational / incremental / parallel GC。每个后续算法只需 `impl GcAlgorithm for XxxGc`，不改框架、不改其他算法（接入契约见附录 D）。本次只为这些算法预留 trait 接口与 hook，**不写真实 barrier 实现**（见 §12）。

---

## 4. 架构总览

```
┌─ wjsm-backend-wasm (编译期, 烧进 WASM) ──────────────────────────┐
│  $obj_new / $arr_new:                                            │
│    if bump_ok { bump; return handle }        ← fast-path, 固定   │
│    else call host gc_alloc_slow(size, ...)   ← slow-path, 可插拔 │
│                                                                  │
│  Safepoint spill (编译器在每个 alloc 点前生成):                  │
│    for each live object-typed local:                            │
│      i64.store [shadow_sp]; shadow_sp += 8                      │
│    call $obj_new                                                 │
│    shadow_sp = saved             ← 复位即可, non-moving 无需 reload │
│                                                                  │
│  新增 IR pass: ValueTy 类型推断 + per-ValueId liveness           │
└──────────────────────────────────────────────────────────────────┘
                            │ (host call)
                            ▼
┌─ wjsm-runtime/src/runtime_gc/ (新模块组, 运行期) ────────────────┐
│  api.rs       GcAlgorithm/Allocator/Marker/Sweeper/RootProvider/ │
│               WriteBarrier/ReadBarrier/HeapRegionManager/        │
│               GcContext/GcStats/Handle/Value                     │
│  registry.rs  GcRegistry::create(name) -> Box<dyn GcAlgorithm>   │
│  mark_bitmap  MarkBitmap (从 RuntimeState 提取)                  │
│  roots.rs     ShadowStackScanner + HostTableScanner + fixed-point │
│  mark_sweep/                                                     │
│    mod.rs       MarkSweepCollector (impl GcAlgorithm)            │
│    allocator.rs SegregatedFreeList (size-class table + coalesce) │
│    marker.rs    mark phase (**worklist**, 移植自 mark_object_recursive, #11) │
│    sweeper.rs   sweep → 按 ptr sort + 线性重建 free list (#3)    │
│  context.rs   GcContext: 持 Caller（不持 slice，#9），with_memory/grow │
└──────────────────────────────────────────────────────────────────┘
                            │
                            ▼
┌─ wjsm-runtime/src/lib.rs (RuntimeState) ─────────────────────────┐
│  gc_algorithm: Box<dyn GcAlgorithm>   ← 运行期可换               │
│  gc_registry:  GcRegistry                                        │
│  删除: trigger_gc (compact 路径) / core.rs gc_collect             │
│  保留: gc_mark_bits / alloc_counter / gc_threshold                │
└──────────────────────────────────────────────────────────────────┘
```

---

## 5. 内存与对象布局

### 5.1 活动对象布局：不变

```
[16 字节 header][payload]
 header:
   offset 0..4   proto_handle (u32, 0xFFFF_FFFF = null proto)
   offset 4      heap_type tag (u8)
   offset 5..8   padding (3B)
   offset 8..12  capacity (u32)
   offset 12..16 num_props / length (u32)
 payload:
   object: capacity × 32B property slot [name_id(4),flags(4),value(8),getter(8),setter(8)]
   array:  capacity × 8B  NaN-boxed element
```

**本次不改活动对象布局**（不需要 inline boundary tag）。理由：sweep 用 `obj_table` + marked bits 按 ptr 顺序线性重建 free list，天然合并相邻空闲块，无需 inline metadata（见 §8.2）。所有现有 `obj_get`/`obj_set`/mark 遍历代码零改动。

### 5.2 NaN-boxing：不变

所有 JS 值仍为 `i64`，box 编码与 `value.rs` 完全一致。

### 5.3 obj_table 间接：不变

所有对象引用仍走 handle index → `obj_table[handle] → ptr`。non-moving 下 `ptr` 一次分配后永不变，handle 抽象仍有价值（稳定引用身份 + 为未来 moving 算法保留）。

### 5.4 空闲块：off-heap free list 管理

空闲块**不侵入活动对象内存**。free list 是 off-heap 的 `SegregatedFreeList`（§9），sweep 时重建。空闲区在 WASM linear memory 内（覆盖已死对象的原位置），但其元数据（ptr/size/next）由 free list 结构持有，不依赖 inline tag。

> **保留余地**：未来 moving 算法需要 forwarding pointer 时，可在此处引入布局变更（加 footer 或 forwarding slot）。本次不兑现。

---

## 6. GC Trait 框架（`runtime_gc/api.rs`）

```rust
// ── 基础别名 ──
pub type Handle = u32;          // obj_table 下标
pub type Value = i64;           // NaN-boxed

// ── 对象元信息查询（所有算法共享，只读） ──
pub trait HeapObjectQuery {
    fn object_size(&self, h: Handle) -> usize;                 // 从 header 算
    fn object_ptr(&self, h: Handle) -> usize;                  // obj_table[h]
    fn heap_type(&self, h: Handle) -> u8;
    fn iterate_slots(&self, h: Handle, f: &mut dyn FnMut(Value)); // mark 用
}

// ── 分配器：fast-path 固定烧进 WASM，slow-path 走 trait ──
pub trait Allocator {
    /// fast-path bump 失败后调用。策略决定：free list / GC / grow。
    fn alloc_slow(&mut self, ctx: &mut GcContext, size: usize, heap_type: u8, capacity: u32) -> Option<Handle>;
    /// 接收被 sweep 释放的空闲区（MarkSweep 用；future compact 可空实现）
    fn add_free_region(&mut self, ptr: usize, size: usize);
    /// 预留：未来 TLAB / region-local 分配
    fn alloc_thread_local(&mut self, _ctx: &mut GcContext, _size: usize) -> Option<Handle> { None }
}

// ── Root 发现 ──
// 【#6】不返回 Vec（避免每次 GC clone 整个 shadow stack）。改为回调式：
// marker 在 mark 阶段已持 ctx 借用，RootProvider 直接 ctx.with_memory 扫描并把
// 每个 root 喂给 marker 的 visit 回调。shadow stack 扫描量大（数千槽），回调式
// 零额外分配；host 表条目少，实现内部可用小 buffer。
pub trait RootProvider {
    /// 扫描 shadow stack，对每个 root handle 调 visit。
    fn for_each_shadow_stack_root(&mut self, ctx: &mut GcContext, visit: &mut dyn FnMut(Handle));
    /// 扫描 host 侧表（promise/microtask/continuation/streams/...），含 fixed-point。
    fn for_each_host_table_root(&mut self, ctx: &mut GcContext, visit: &mut dyn FnMut(Handle));
    /// 预留：未来精确栈扫描（WASM GC proposal / stack maps）。默认空。
    fn for_each_wasm_local_root(&mut self, _ctx: &mut GcContext, _visit: &mut dyn FnMut(Handle)) {}
}

// ── Mark 策略 ──
pub enum MarkProgress { Complete, Pending(usize) } // 剩余 work 估算

pub trait Marker {
    fn mark(&mut self, ctx: &mut GcContext, roots: &mut dyn Iterator<Item = Handle>);
    fn is_marked(&self, h: Handle) -> bool;
    /// 预留：incremental mark 步进接口
    fn mark_step(&mut self, _ctx: &mut GcContext, _budget: usize) -> MarkProgress {
        // 默认：一次性 mark 完（non-incremental 算法）
        MarkProgress::Complete
    }
}

// ── Sweep 策略 ──
pub trait Sweeper {
    fn sweep(&mut self, ctx: &mut GcContext);
    /// 预留：concurrent sweep 步进
    fn sweep_step(&mut self, ctx: &mut GcContext, _budget: usize) -> usize { self.sweep(ctx); 0 }
}

// ── 预留 hook：region / card-table / barrier（generational/G1 用） ──
pub trait HeapRegionManager {
    type Region; type Card;
    fn regions(&self) -> std::slice::Iter<'_, Self::Region>;
    fn card_for(&self, ptr: usize) -> Self::Card;
}

pub trait WriteBarrier {
    /// 对象字段写时调用。non-moving MarkSweep 默认 no-op（无消费者）。
    fn on_write(&mut self, _ctx: &mut GcContext, _target: Handle, _field: usize, _val: Value) {}
}

pub trait ReadBarrier {
    fn on_read(&mut self, _ctx: &mut GcContext, _target: Handle, _field: usize) {}
}

// ── 顶层算法：组装 Allocator + Marker + Sweeper ──
pub trait GcAlgorithm: Allocator + Marker + Sweeper {
    fn collect(&mut self, ctx: &mut GcContext) -> GcStats;
    fn algorithm_name(&self) -> &'static str;
}

// ── 算法运行时上下文（注入给 trait 方法） ──
//
// 【#9 关键约束】不持有 `&mut [u8]`。原因：gc_alloc_slow 在 mark/sweep 后可能
// 仍不够空间，需 memory.grow()。Wasmtime 下 `memory.grow(&mut store, _)` 与
// `memory.data_mut(&store)` 都可变借用 store —— 持有 slice 时无法 grow，强行
// unsafe 是 UB（grow 会 remap 后端 buffer，slice 悬垂）。
// 现有代码（runtime_eval.rs:193-202 唯一 grow 点）从不跨 grow 持 slice；
// trigger_gc / gc_collect 都不 grow（core.rs:1512-1515 freed 不够直接返回失败）。
// 故 GcContext 持 `&mut Caller`，每阶段重新 data()/data_mut()（现有 compact
// 每轮独立 data_mut 的模式，runtime_builtins.rs:3187-3212）。
pub struct GcContext<'a, 'b> {
    pub caller: &'a mut Caller<'b, RuntimeState>,
    pub memory: Memory,                    // wasmtime Memory 句柄（轻量，不含借用）
    pub gc_algorithm_name: &'static str,   // 仅用于日志
    pub stats: GcStats,
}

impl<'a, 'b> GcContext<'a, 'b> {
    /// 读 memory。借用caller，离开作用域后可再 grow / data_mut。
    /// 算法在每个无 grow 的计算阶段调用。
    pub fn with_memory<R>(&mut self, f: impl FnOnce(&Caller<'_, RuntimeState>, &[u8]) -> R) -> R {
        let data = self.memory.data(&*self.caller);
        f(&*self.caller, data)
    }
    /// 写 memory。同上，单独可变借用。
    pub fn with_memory_mut<R>(&mut self, f: impl FnOnce(&mut Caller<'_, RuntimeState>, &mut [u8]) -> R) -> R {
        let data = self.memory.data_mut(&mut *self.caller);
        f(&mut *self.caller, data)
    }
    /// 扩页。必须在外层调用，不持 slice。失败返回 Err（由 gc_alloc_slow 决定 trap 或重试）。
    pub fn grow(&mut self, pages: u64) -> Result<u64, ()> {
        self.memory.grow(&mut *self.caller, pages).map_err(|_| ())
    }
    /// 读/写 RuntimeState（caller.data()）。
    pub fn with_state<R>(&mut self, f: impl FnOnce(&mut RuntimeState) -> R) -> R {
        f(self.caller.data_mut())
    }
}

// 算法需要的堆元信息从 Caller + memory 在每个阶段现场算，不缓存 slice：
//   object_size(handle): with_memory(|_, data| parse header at obj_table[handle])
//   obj_table_count / heap_ptr / heap_base: 读导出 global（caller.get_export）

pub struct GcStats {
    pub marked: usize,
    pub swept: usize,
    pub freed_bytes: usize,
    pub elapsed: std::time::Duration,
}
```

### 6.1 设计要点

1. **trait 切片原则**：`GcAlgorithm: Allocator + Marker + Sweeper`。Allocator 的 fast-path 烧进 WASM 不抽象，只有 `alloc_slow` 走 trait（匹配 §2 物理边界）。HeapObjectQuery / RootProvider 是能力注入，不随算法变。
2. **hook 默认实现 = 零开销**：`WriteBarrier::on_write` / `mark_step` / `sweep_step` 默认 no-op 或一次性完成。MarkSweep 不调任何 barrier，运行时零成本。
3. **`GcContext` 注入 Caller，不持 slice（#9）**：算法通过 `with_memory` / `with_memory_mut` 在每个无 grow 的计算阶段重新借用 memory；需 grow 时走 `ctx.grow()`，前后不持 slice。这保证 grow 不产生悬垂引用（UB）。算法不直接访问 `RuntimeState` 全部字段，经 `with_state` 暴露 —— 仍锁定"算法能做什么"的边界，但 grow 安全。
4. **预留 incremental**：`mark_step`/`sweep_step` 已留。future incremental GC 只 impl 这些，不改框架。（`mark_worklist` 不作为 ctx 字段暴露，由算法内部持有 —— 见 §8.1。）
5. **预留 generational**：`HeapRegionManager`/`WriteBarrier`/`ReadBarrier` 已留。future generational impl 这些 + real barrier。

---

## 7. 分配路径（WASM + host 协作）

### 7.1 WASM fast-path（编译期生成，固定）

```wasm
;; $obj_new (param $capacity i32) (result i32)
local $size = 16 + capacity * 32
local $ptr  = __heap_ptr
if (ptr + size) <= (memory.size * 65536):
    __heap_ptr = ptr + size
    init_header(ptr, OBJECT, capacity)      ;; 写 16B header
    ;; 【#7】优先复用已回收的 handle 槽，避免 obj_table 无限膨胀
    handle = take_or_alloc_handle()         ;; free_list 非空取一个，否则 obj_table_count++
    obj_table[handle] = ptr
    ;; 【#2】proactive counter 在 fast-path 出口也递增并检查（否则只命中 fast-path 时永不触发）
    __alloc_counter += 1
    if __alloc_counter >= __gc_threshold:
        __alloc_counter = 0
        call $gc_maybe_collect()            ;; host: collect。spill 已就位（§11）
    return handle
else:
    ;; slow-path：调 host。safepoint spill 已在调用前由编译器生成（§11）
    return call $gc_alloc_slow(size, OBJECT, capacity)
```

> **`take_or_alloc_handle`（#7）**：WASM 端维护一个 `handle_free_list` 栈（global + memory 头部）。fast-path 先 pop；空则 `obj_table_count++`。这避免 handle 单调增长导致 obj_table 无限膨胀（sweep 回收的槽必须能被 fast-path 复用）。

> **`gc_maybe_collect`（#2）**：host import，sync `Func::wrap`（不 yield，见 §12.3 / R4）。内部调 `gc_algorithm.collect`。之所以拆出独立 import 而非塞进 `gc_alloc_slow`：fast-path 命中时也要触发 proactive GC，但此时已 bump 成功、handle 已分配，只差 collect。collect 是 non-moving，不动 handle，安全。

> **与现状差异**：现状 OOM 直接 `memory.grow`（无限扩容）。新版：fast-path 命中时也走 proactive 检查；OOM 走 `gc_alloc_slow`，`memory.grow` 仅作其内部最后兜底。

### 7.2 host `gc_alloc_slow`（走当前 GcAlgorithm）

```rust
// 【#8】返回 Option<Handle>；None 表示真 OOM，由调用方（host import trampoline）决定 trap。
// 不在函数体内 trap，保持纯 Rust 控制流，便于测试。
fn gc_alloc_slow(ctx: &mut GcContext, size: usize, heap_type: u8, capacity: u32) -> Option<Handle> {
    let algo = current_algorithm(ctx);  // &mut dyn GcAlgorithm
    // 1. 先试当前策略的 slow 分配（MarkSweep: free list best-fit）
    if let Some(h) = algo.alloc_slow(ctx, size, heap_type, capacity) { return Some(h); }
    // 2. 触发一次 GC，再试
    algo.collect(ctx);
    if let Some(h) = algo.alloc_slow(ctx, size, heap_type, capacity) { return Some(h); }
    // 3. 最后兜底：扩页（ctx.grow 不持 slice，安全，见 §6 GcContext）后再试
    let pages = (size.div_ceil(65536) + 1) as u64;
    let _ = ctx.grow(pages);   // 忽略失败，下面 alloc 仍失败则返回 None
    algo.alloc_slow(ctx, size, heap_type, capacity)
}

// host import trampoline：WASM 调用入口。trap 在此层处理。
fn gc_alloc_slow_import(mut caller: Caller<'_, RuntimeState>, size: i32, ht: i32, cap: i32) -> i32 {
    let mut ctx = GcContext::new(&mut caller);
    match gc_alloc_slow(&mut ctx, size as usize, ht as u8, cap as u32) {
        Some(h) => h as i32,
        None => { caller.data_mut().trap(OutOfMemory); -1 }  // trap：WASM unreachable
    }
}
```

### 7.3 proactive GC（恢复自动触发）

恢复被 `0849b37` 删除的 `alloc_counter` / `gc_threshold` 机制。

**触发位置（#2 修正）**：counter 在**每次分配出口**递增并检查，包括 fast-path。实现方式见 §7.1：fast-path 出口内联 `__alloc_counter += 1; if >= threshold { collect }`；slow-path 在 `gc_alloc_slow` 成功返回前由 trampoline 同样递增。两处都满足"spill 已就位、collect 是 non-moving"的安全前提。

`gc_threshold` 初值 1000，可调。**关键安全前提**：
- collect 发生在 spill 之后（fast-path：safepoint spill 在 bump 前；slow-path：spill 在 `call gc_alloc_slow` 前）；
- collect 是 non-moving，不动 handle/ptr，已分配的 handle 不失效；
- `gc_maybe_collect` 是 sync `Func::wrap`，不 yield（§12.3）。

---

## 8. MarkSweep 算法

### 8.0 前置不变量（实现必须维护，见 §18）

- **INV-A（obj_table 是堆块完整索引）**：任何 bump/alloc_slow 成功的对象，其 `(handle, ptr)` 必须在返回调用方**之前**写入 `obj_table`。bump 与 obj_table 注册原子化（fast-path 同一基本块内完成，不跨 host call）。任何不注册 handle 的"裸 bump"禁止存在。sweep 依赖此不变量覆盖所有块 —— 否则漏块 → free list 幽灵块 → 后续分配覆盖活对象。
- **INV-B（resize 重写 obj_table 槽）**：`grow_array`/`grow_object`/`$obj_set` reallocate 把已有 handle 的 ptr 重写到更高位置（runtime_values.rs:222-226/273-277，compiler_helpers.rs:1037-1047）。**这是 handle→ptr 单调性破坏的根因**，sweep 必须按 ptr 重排（§8.2）。resize 后旧块成为死块，由 sweep 回收。

### 8.1 Mark phase（**worklist，非递归** —— #11 修正）

现有 `mark_object_recursive`（runtime_heap.rs:577-761）是真 Rust 栈递归（L751-760 逐 child 递归）。深对象图（链表 10000 层）会栈溢出 —— 循环检测（mark bitmap）只防环路，不防深链。**移植时改为显式 worklist**：

```
1. reset mark_bits; worklist.clear()   // Vec<Handle>，算法内部持有（不入 GcContext，见 §6.1 #4）
2. // seed roots
   ctx.roots.for_each_shadow_stack_root(ctx, &mut |h| mark_push(h, &mut worklist))
   ctx.roots.for_each_host_table_root(ctx,   &mut |h| mark_push(h, &mut worklist))
3. // drain worklist（迭代，不递归）
   while let Some(h) = worklist.pop():
       mark_object_children(ctx, h, &mut worklist)   // 把 h 的 proto/props/elements/env_obj 推入 worklist
4. // fixed-point host 侧表（移植自 trace_runtime_side_table_roots_fixed_point）
   loop:
       before = mark_bits.popcount()
       ctx.roots.for_each_host_table_root(ctx, &mut |h| mark_push(h, &mut worklist))
       drain worklist (同 3)
       if mark_bits.popcount() == before: break
   覆盖：microtask_queue / promise reactions+resolved/rejected /
         continuation_table.captured_vars / stream readers/controllers /
         BYOB views / async_generator queue / combinator contexts
```

`mark_object_children` 读单个对象的引用（proto / property values+getters+setters / array elements / closure env_obj / native callable 内部引用），全部推入 worklist，不递归。每批 `with_memory` 借用处理，借用周期短（单对象），无 grow。

> **深对象图安全**：worklist 是堆上 `Vec`，深度仅受堆容量限制，不消耗 Rust 栈。

### 8.2 Sweep phase（线性重建 free list；**sort 必需** —— #3 验证）

核心：**不改活动对象布局**。用 `obj_table` + marked bits 按 ptr 顺序重建 free list。

```
// 1. 收集所有已分配块信息（含已死）。依赖 INV-A（obj_table 完整）。
blocks = []
for handle in 0..obj_table_count:
    ptr = obj_table[handle]
    if ptr == 0: continue          // 空槽（被 sweep 回收的 handle）
    size = heap_query.object_size(handle)   // 从 header(heap_type+capacity) 算
    blocks.push((ptr, size, is_marked(handle)))

// 【#3】sort 按 ptr 必需：resize（INV-B）会把低 handle 重写到高 ptr，
// 破坏 handle→ptr 单调性。现有 compact 也 sort（runtime_builtins.rs:3178）。
// 不 sort 会导致相邻合并错误（把不相邻块误判为相邻）。
blocks.sort_by_key(|b| b.ptr)

// 2. 线性扫描，合并相邻 unmarked 块
free_list.clear()                  // SegregatedFreeList
i = 0
while i < blocks.len():
    if blocks[i].marked:
        i += 1; continue
    run_ptr = blocks[i].ptr
    run_end = run_ptr + blocks[i].size
    i += 1
    while i < blocks.len() && !blocks[i].marked:
        run_end = blocks[i].ptr + blocks[i].size   // 天然合并相邻
        i += 1
    free_list.add_free_region(run_ptr, run_end - run_ptr)

// 3. 处理 weak refs（清除 unmarked 对应的 WeakRef/FinalizationRegistry）
process_weak_refs()

// 4. reset alloc_counter
```

**sort 开销优化（#3，P3 后视情况启用）**：每次 GC 全量 sort `obj_table_count` 条目（可能数万）是热点。优化路径：维护一个**按 ptr 排序的辅助索引**（`Vec<handle>` 按 obj_table[handle] 排序），在 resize 重写槽时增量维护（插入排序或平衡树），GC 时直接用，免全量 sort。**初版先全量 sort 保证正确性**（与现有 compact 同开销），profile 后再上索引。这是性能优化，非正确性阻塞项。

**为什么不需要 boundary tag**：sweep 线性遍历所有已知块（来自 obj_table），天然知道每个块的起止；相邻 unmarked 块在循环中合并。活动对象的 size 从 header 算（现有逻辑），空闲块的 size 在合并时累积。无需 inline metadata。

### 8.3 handle 槽复用（#7 配合）

sweep 后，unmarked handle 的 `obj_table` 槽置 0，handle 推入 host 维护的 `handle_free_list`。**关键**：fast-path（§7.1）通过 `take_or_alloc_handle` 优先复用这些槽，否则 handle 单调增长、obj_table 无限膨胀。`obj_table_count` 不缩减（下标稳定，呼应 §1.4 兼容性边界）；空闲槽供新分配复用。

> handle_free_list 由 host 维护（push 在 sweep，pop 暴露给 WASM 经一个 host import `gc_take_freed_handle() -> i32`，返回 -1 表示空）。这避免 WASM 直接操作 host 数据结构。

---

## 9. Segregated Free List（`mark_sweep/allocator.rs`）

### 9.1 size class table（**冻结初始值** —— #5）

```rust
// 冻结的初始 class table（P0 验证 + 微调，不推翻重设计）。
// 依据：对象 = 16 + cap*32（cap 常见 4..16 → 144..528B）；数组 = 16 + len*8（len 常见 0..128 → 16..1040B）。
// class 间距设计为覆盖 p50/p90，大块稀疏。P0 实测直方图后只做局部增删/移动，不重构结构。
const SIZE_CLASSES: &[usize] = &[
    16, 48, 80, 112, 144, 176, 208, 272, 336, 432,
    528, 640, 768, 1024, 1536, 2048, 4096, 8192, 16384,
];
const BIG_CLASS: usize = SIZE_CLASSES.len();  // > 16384 直接进 big list
const MIN_BLOCK: usize = 16;                  // 可分割的最小块（< header 无意义）

fn size_class(size: usize) -> usize {
    // 二分查找第一个 >= size 的 class；无则 BIG_CLASS
}
```

> **冻结理由（#5）**：class table 是 allocator 核心数据结构。若 P0 结论推翻 table，P3 的 `SegregatedFreeList` 需重写。故 P0 产出为"**验证 + 微调**"（局部增删 class、移动边界），而非"重新设计"。class 数量只影响 `lists: Vec` 长度，P3 用 `VecDeque` 实现不依赖具体 class 数 —— 即使 P0 微调，P3 代码结构不变。这消除 P0↔P3 的耦合返工风险。

### 9.2 数据结构

```rust
pub struct SegregatedFreeList {
    /// 每 class 一个空闲块列表（off-heap，不侵入活动对象内存）
    /// 每个空闲块: (ptr, size)
    lists: Vec<VecDeque<(usize, usize)>>,  // index = size_class
    big_list: VecDeque<(usize, usize)>,    // 大块
}
```

### 9.3 alloc_slow（best-fit in class）

```
fn alloc_slow(size):
    cls = size_class(size)
    // 精确 class 或更大的 class
    for c in cls..BIG_CLASS:
        if let Some((ptr, block_size)) = lists[c].pop_front():
            remaining = block_size - size
            if remaining >= MIN_BLOCK:   // 可分割
                add_free_region(ptr + size, remaining)
            return ptr
    // big list
    // ... 类似 best-fit
    None  // 都不命中 → 调用方触发 GC / grow
```

### 9.4 add_free_region（接收空闲区，按 class 入表）

```
fn add_free_region(ptr, size):
    cls = size_class(size)
    lists[cls].push_back((ptr, size))
    // 注：不在此处做邻接合并（sweep 已线性合并过）
    // 仅当 dealloc 单块（未来 moving/dealloc 路径）才需合并检查
```

MarkSweep 的 sweep 是唯一释放点，已在 §8.2 线性合并，故 `add_free_region` 无需重复合并。

---

## 10. Root 发现（`runtime_gc/roots.rs`）

移植自 `runtime_builtins.rs:2974-3091` + `trace_runtime_side_table_roots_fixed_point`，重构为 `RootProvider` 的回调实现（§6）：marker 在 mark 阶段持 `&mut GcContext`，调 `for_each_*_root(ctx, visit)`；provider 内部 `ctx.with_memory` 扫描，直接 `visit(handle)`，零中间 Vec（#6）。

```
ShadowStackScanner (for_each_shadow_stack_root):
  ctx.with_memory(|caller, data| {
    let [base, sp) = [object_heap_start - 65536, __shadow_sp];
    for addr in (base..sp).step_by(8):
        val = read i64 at addr
        if tag_needs_root(val):              // §11.3 扩展谓词
            visit(decode_handle(val))
        else if is_closure(val):
            // closure → 解析 env_obj handle 作为 root（查 closures 表）
            visit(decode_env_obj_handle(val))
  })

HostTableScanner (for_each_host_table_root, 含 fixed-point):
  ctx.with_state(|st| {
    // 直接根：IR function property objects (0..num_ir_functions)
    //         timer callbacks / closure env_obj / module namespace cache
    // fixed-point：microtask_queue / promise reactions+values /
    //              continuation_table.captured_vars / streams / BYOB /
    //              async_generator queue / combinator contexts
    // （fixed-point 由 marker 驱动，§8.1 step 4：循环 until mark_bits.popcount 不变）
  })
```

**关键修正**（相对旧 `core.rs` 不完整版）：`continuation_table` 每个非 completed 条目的 `captured_vars` 必须作为**顶层 root**（当前仅间接通过 microtask reaction 追踪，存在漏扫风险——一个 outer promise pending 但无 reaction 引用的 continuation 会被误回收）。直接在 `for_each_host_table_root` 遍历所有非 completed continuation 的 `captured_vars`。

---

## 11. Safepoint Spill 与编译器分析

### 11.1 新增 IR pass：per-ValueId liveness（`wjsm-ir`）

从零建。移植骨架自 `lowerer_async_eval.rs:70-144`（`compute_use_def` + `compute_liveness`），泛化为 `HashSet<ValueId>`：

```
compute_use_def(fn) -> (Vec<HashSet<ValueId>> defs, Vec<HashSet<ValueId>> uses)
  // 每条 producing instruction 的 dest = def
  // 每个指令的 ValueId 操作数 = use
compute_liveness(fn) -> Vec<HashSet<ValueId>>  // 每条指令后的 live 集合
  // 标准 backward dataflow 迭代到不动点
```

落在 `wjsm-ir`（零外部依赖，归属正确）。`wjsm-semantic` 的 suspend 逻辑后续可复用（去重）。

**控制流合并（#10）**：标准 backward liveness 在 CFG join（多个前驱汇合的 BasicBlock 入口）取 **union**（`live_in = ∪ succ.live_out \ defs ∪ uses`）。wjsm IR 有显式 `BasicBlock` + `Terminator`（lib.rs:264-269），CFG 结构完整。移植时必须：
- 按 BasicBlock 而非按指令线性迭代（块级 live_in/live_out，再块内指令级细化）；
- join 点 union（不是 intersect —— live-in 是"任一后继需要则需要"）；
- Phi 节点特殊处理：Phi 的每个入参仅对其对应前驱块的 live-out 有贡献（Phi 不在 join 点统一 use，而是按边分发）。移植骨架 `lowerer_async_eval.rs:99-144` 是按指令线性迭代、无显式 CFG join 概念 —— **必须扩展为块级 + Phi 边分发**，否则在 if/else/loop 汇合点 liveness 错误，导致 safepoint 误判（漏 spill 活值 → GC 误回收，或多 spill 死值 → 性能损失）。

P1 单元测试必须覆盖：if/else 汇合、loop 回边、嵌套控制流，断言 join 点 live 集合正确。

### 11.2 新增 IR pass：ValueTy 类型推断

```rust
pub enum ValueTy { Handle, Scalar, Unknown }  // Unknown 保守当 Handle

// 按 Instruction variant 推断:
//   NewObject/NewArray/CreateClosure/GetSuperBase → Handle
//   Const(Number|Bool|Null|Undefined) → Scalar
//   Binary/Compare/Unary(算术) → Scalar
//   GetProp/GetElem/Call/CallBuiltin/Phi/LoadVar → Unknown(→Handle 保守)
//   其他产生 handle tag 的 → Handle
fn infer_value_ty(fn) -> HashMap<ValueId, ValueTy>
```

### 11.3 扩展 `tag_needs_root`（`value.rs`）

现有 `is_js_object`（value.rs:367-375）遗漏。扩展为完整谓词，覆盖所有"低 32 位是 handle"的 tag：

```
needs_root = is_object || is_array || is_function || is_closure || is_bound
          || is_proxy || is_native_callable || is_bigint || is_symbol
          || is_regexp || is_scope_record
          || is_runtime_string_handle || is_exception(payload is handle)
          || is_iterator || is_enumerator
```

供 shadow stack 扫描与 spill 类型推断共用。

### 11.4 Safepoint spill 代码生成（`compiler_instructions.rs`）

每个 **safepoint** = 任意可能分配的指令点（`NewObject` / `NewArray` / 可能分配的 `Call`/`CallBuiltin`）。在每个 safepoint 的分配 call **之前**插入：

```wasm
;; 假设当前函数已知 liveness[value_at_safepoint] 和 value_ty
local $saved_sp = __shadow_sp
for each local where liveness.contains(local) AND value_ty[local] != Scalar:
    local.get $local_val
    i64.store [__shadow_sp]
    __shadow_sp = __shadow_sp + 8
call $obj_new / $arr_new / ...
__shadow_sp = $saved_sp        ;; 复位即可,无需逐个 reload
```

**non-moving 的关键优势**：GC 不移动对象、不改 local 值，故 spill 后**无需 reload**（local 里的 handle 仍有效）。只需在 GC 运行期间把值暴露给 shadow stack 扫描。这比 moving 方案省一半 spill 开销。

**函数级不变量（#4）**：`__shadow_sp` 在**函数入口与出口必须相等**（函数不泄漏 spill 区给调用方）。每个 safepoint 是独立 save/restore 对（save 到局部、spill、call、restore）—— **循环内每个 safepoint 独立保存/恢复**，循环迭代间 `__shadow_sp` 不累积漂移。嵌套 safepoint（alloc 在可能 alloc 的 call 内）由被调函数自己负责其 spill，调用方的 save/restore 保证返回后 sp 复位，嵌套安全。

**容量检查（R2）**：spill 区上限 = `max_live_object_locals × 8B`（函数级可静态估）。函数 prologue 检查 `__shadow_sp + spill_upper_bound + frame_size <= __shadow_stack_end`，否则 trap（防溢出覆盖对象堆）。spill_upper_bound 在编译期算出，作为函数元数据。worst case：50 live object locals × 8B = 400B/帧 × 100 层深度 = 40KB，64KB shadow stack 充裕；超出由容量检查 trap 而非静默损坏。

### 11.5 纯算术函数优化

若函数内无 safepoint（无 alloc 点），则零 spill。若所有 live 值都是 Scalar，spill_set 为空。这覆盖大量纯计算函数。

---

## 12. Barrier / Region 接口（预留，真实实现 defer）

### 12.1 接口保留（§6 已定义）

`WriteBarrier` / `ReadBarrier` / `HeapRegionManager` / `Marker::mark_step` / `Sweeper::sweep_step` 全部保留为 trait，默认实现 no-op / 一次性完成。

### 12.2 真实实现 defer 到 generational（理由）

MarkSweep 是 non-moving + 全堆 mark，**没有 write barrier 的消费者**。remembered set 不存在，barrier 记录无处可去。现在写真实 barrier 实现 = 无消费者死代码，违反 AGENTS.md "无 stub / 无部分实现" 硬规则。

**结论**：
- 本次：trait 接口完整保留，默认 no-op，运行时零成本；
- 后续：generational GC 实现时，一并实现真实 write barrier（dirty card 记录）+ 需要的布局变更（forwarding pointer / mark bit in header）。那时才有消费者。

附录 D 写明接入契约。

### 12.3 async 安全论证（sync host import 不 yield）

`gc_alloc_slow` / `gc_maybe_collect` 必须注册为 **sync `Func::wrap`**（匹配现有 `gc_collect`，core.rs:1221）。

**经验证**：runtime 用 epoch interruption（`config.epoch_interruption(true)` + `store.epoch_deadline_async_yield_and_update(1)`，lib.rs:833-836/851-854），**非** `Config::async_support`。在 epoch 模型下，**只有 async import（`func_wrap_async` 且 future 返回 `Poll::Pending`）才 yield**；sync `Func::wrap` 闭包在 host-call trampoline 内同步执行完毕，不 yield 到 epoch 调度器，不论 deadline 是否到期。`gc_collect` 不在 `core_async.rs`/`reentrant_async.rs` 的 async override 列表（lib.rs:398-432 的 `define_core_async` override 名单）—— 印证 sync 注册正确。

**故**：sync `gc_alloc_slow` 从 `$obj_new`（sync JS 执行）调用，不会触发 async reentry，不破坏 O2 线性语义。前提：实现时不得在闭包内 `.await` 或调 `call_wasm_callback_async`（回进 WASM）。

**AsyncOpGuard（scheduler.rs:44-76）**：仅用于 spawn 后台异步任务（如 fetch body pull，streams_fetch_body.rs）的 backpressure。纯同步 host 计算（GC）不需 guard —— 与现有 `gc_collect` 一致。

---

## 13. 旧代码删除计划（根除 duplicate owner）

| 现有代码 | 处置 |
|----------|------|
| `runtime_builtins.rs:trigger_gc` L2939-3223（compact） | **删除**；mark 逻辑迁移到 `mark_sweep/marker.rs`；sweep 重写为 non-moving |
| `runtime_builtins.rs:trace_runtime_side_table_roots_fixed_point` L2590-2918 | **迁移**到 `runtime_gc/roots.rs`，重构为 `RootProvider` |
| `host_imports/core.rs:gc_collect` L1218-1642（不完整） | **删除**；新增两个 host import 替代：`gc_alloc_slow(size,heap_type,cap)→i32`（§7.2）与 `gc_maybe_collect()`（§7.1 proactive）；`gc()` global 背后改调 `gc_algorithm.collect` |
| `runtime_heap.rs:mark_object_recursive` L577-761 | **迁移**到 `runtime_gc/mark.rs`，签名改 `&mut GcContext`，**递归改 worklist**（#11，见 §8.1） |
| `compiler_helpers.rs:$obj_new` L56-195 的 `memory.grow` OOM（73-109） | **改写**为 bump + handle_free_list 复用 + proactive counter 检查 + `gc_alloc_slow` slow-path |
| `compiler_array_helpers.rs:$arr_new` L11-146 的 `memory.grow` OOM（28-58） | **改写**同上 |
| `NativeCallable::GcCollect`（`gc()` global） | **保留**，背后改调 `gc_algorithm.collect`（host import 名 `gc_collect` 保留给 `gc()`；分配路径用新名 `gc_alloc_slow`/`gc_maybe_collect`，三者职责分离） |
| `bug.md` O2 状态 FIXED → **RESOLVED** | 更新文档（根因消除） |
| `AGENTS.md` GC 描述 | 更新：反映 non-moving + 可插拔框架 |

> **删除纪律**：每个迁移/删除在对应阶段（P5）完成后，grep 确认无残留引用。旧 `gc_collect` host import 被 `gc()` global 用 → 删除前先把 global 重接到框架入口（P4 同步完成）。

---

## 14. 实施阶段

| 阶段 | 内容 | 验证 |
|------|------|------|
| **P0** | 采集 fixture 对象 size 直方图；**验证**冻结的 `SIZE_CLASSES`（§9.1）覆盖率，必要时局部微调 | 直方图报告；class 覆盖率 ≥ 99%；微调不推翻结构（仅增删/移动边界） |
| **P1** | `wjsm-ir`: `ValueTy` + `tag_needs_root` + per-ValueId liveness pass（**块级 CFG join union + Phi 边分发**，#10） | 单元测试：liveness 正确性，含 if/else 汇合、loop 回边、嵌套控制流 |
| **P2** | Backend: safepoint spill 代码生成（**不接 GC**，仅验证 spill 不破坏语义）；函数级 `__shadow_sp` 不变量（#4）+ 容量检查（R2） | 现有 470+ fixture 全绿；dump-wat 检查 spill 序列与 sp 复位 |
| **P3** | `runtime_gc/` 框架: trait + `GcContext`（Caller 注入，#9）+ `MarkSweepCollector`（**mark worklist**，#11）+ `SegregatedFreeList` + roots（回调式，#6） | 单元测试：mock `GcContext` 注入假 roots 验证 mark/sweep/free-list；**深链表 10000 层不栈溢出**（R8） |
| **P4** | 改 `$obj_new`/`$arr_new` 为 bump + handle_free_list 复用（#7）+ proactive counter（#2）+ `gc_alloc_slow`/`gc_maybe_collect`（sync `Func::wrap`）；`gc()` 重接 | fixture 全绿 + 新增长循环不 OOM 测试 + streams_byob 系列全绿（R4） |
| **P5** | 删除旧 `trigger_gc` / `core.rs gc_collect`；迁移 fixed-point tracer；grep 无残留 | 编译通过；fixture 全绿；无死代码 |
| **P6** | 预留 hook 默认 impl 落地（零成本）+ CLI `--gc-algorithm` | `--gc-algorithm mark-sweep` 切换测试；默认值文档化 |

**阶段独立性**：P1/P2 可在不碰 runtime 的情况下推进；P3 可在不碰 backend 的情况下推进（mock ctx）；P4 是集成点。每个阶段有独立验证，可单独提交。

**正确性阻塞项已内化到阶段验收**（来自审查 #1/#2/#3/#7/#9/#11）：P3 验 worklist 不栈溢出；P4 验 handle 复用 + proactive 触发 + sync 不 reentry；P2 验 sp 不变量。

---

## 15. 风险与缓解

| 风险 | 缓解 |
|------|------|
| **R1** safepoint spill 代码膨胀 | (a) liveness 精确化（死值不 spill）；(b) non-moving 无需 reload（省一半）；(c) 纯算术函数 spill_set 为空；(d) P2 后 profile fixture spill 量，若膨胀严重再加 flow-sensitive 类型推断 |
| **R2** shadow stack 64KB 不够 | (a) spill 区 = `Σ live object locals × 8B`，函数级可估上限；(b) 容量检查 + 必要时分离独立 spill region；(c) 最坏回退：只 spill 函数参数（conservative） |
| **R3** segregated size class 不匹配 | P0 先跑 fixture 采 size 直方图定 class；class 覆盖率 ≥ 99% 才进入 P1 |
| **R4** async epoch reentry 残留 | (a) non-moving 消除 compaction → local 不失效；(b) `gc_alloc_slow`/`gc_maybe_collect` 注册为 sync `Func::wrap`，epoch 模型下不 yield（§12.3）；(c) 不在闭包内 `.await`/回进 WASM；(d) P4 后跑 streams_byob 全系列 + async generator 验证 |
| **R7** memory.grow 借用安全（#9） | (a) `GcContext` 不持 `&mut [u8]`，每阶段 `with_memory`/`with_memory_mut` 重借；(b) grow 经 `ctx.grow()` 不持 slice；(c) 模式对齐现有 compact 每轮独立 `data_mut`（runtime_builtins.rs:3187-3212）与唯一 grow 点（runtime_eval.rs:193-202 用 data_size 不持 slice） |
| **R8** mark 深递归栈溢出（#11） | (a) mark 改显式 worklist（堆上 Vec），深度仅受堆容量限制；(b) P3 单元测试构造深链表（10000 层）验证不栈溢出 |
| **R5** trait 过度设计 | (a) Decision Hygiene Review 已确认边界（§6.1）；(b) hook 默认实现零开销；(c) MarkSweep 是唯一真实算法，trait 边界经审查；(d) 接口稳定性承诺写入附录 D |
| **R6** liveness pass 正确性 | (a) 单元测试构造小 IR 断言 live 集合；(b) P2 spill 接入后 fixture 全绿是强校验（错误 liveness 会导致误回收 → fixture 崩溃） |

---

## 16. 非目标（明确排除）

- 不实现 generational / incremental / parallel GC（仅留 trait 扩展点）
- 不引入 WASM GC proposal（stack maps / externref）—— 保持 host 扫描模型
- 不改活动对象布局 / NaN-boxing / obj_table 间接（无算法需求；预留 future moving 的余地）
- 不做分代 write barrier 的真实实现（defer 到 generational，见 §12）
- 不改 ECMAScript 语义

---

## 17. 验收标准

1. **长循环不 OOM**：`for (let i=0;i<1e8;i++) arr.push({x:i})` 在固定 memory 上限下运行完成（GC 自动回收死对象）。
2. **fixture 零回归**：`cargo nextest run --workspace` 全绿（含 streams_byob / async / continuation / BYOB 系列）。
3. **safepoint 安全**：构造 fixture 让 WASM local 持有唯一对象引用，触发 GC 后对象仍可用（不被误回收）。
4. **可插拔验证**：CLI `--gc-algorithm mark-sweep` 可切换；框架单元测试用 mock `GcContext` 验证 trait 契约。
5. **无 duplicate owner**：grep 确认旧 `trigger_gc` / `core.rs gc_collect` 无残留引用。
6. **文档同步**：`bug.md` O2 → RESOLVED；AGENTS.md GC 描述更新。
7. **mark 不栈溢出**：10000 层链表/深对象图触发 GC，host 进程不崩溃（worklist 验证）。
8. **grow 安全**：GC 触发 grow 后无内存损坏（`GcContext` 不持 slice，#9）。
9. **handle 不无限膨胀**：sweep 回收的 handle 被 fast-path 复用（长循环后 `obj_table_count` 有上界，#7）。

---

## 18. 不变量与实现约束清单（实现必须维护）

> 集中声明，防止实现期遗忘。任何违反都导致 GC 不安全。

### 18.1 堆/对象层

| ID | 不变量 | 维护点 | 违反后果 |
|----|--------|--------|----------|
| INV-A | **obj_table 是堆块完整索引**：任何成功分配的对象，`(handle, ptr)` 必须在返回调用方前写入 obj_table | `$obj_new`/`$arr_new` fast-path、`gc_alloc_slow`、host alloc | sweep 漏块 → free list 幽灵块 → 覆盖活对象 |
| INV-B | **resize 重写 obj_table 槽到更高 ptr**（handle 不变） | `grow_array`/`grow_object`/`$obj_set` reallocate | 破坏 handle→ptr 单调性 → sweep 必须按 ptr sort（已纳入 §8.2） |
| INV-C | **对象永不动**（non-moving） | 所有算法实现 | 违反则 WASM locals 里的 handle/ptr 失效 → O2 复现 |
| INV-D | **活动对象布局不变**（16B header + payload） | 本次不改 | 若改，所有 obj_get/obj_set/mark 遍历需同步改 |

### 18.2 分配/触发层

| ID | 约束 | 维护点 |
|----|------|--------|
| IMPL-1 | fast-path 必须先取 `handle_free_list`，空才 `obj_table_count++`（#7） | `$obj_new`/`$arr_new` |
| IMPL-2 | proactive counter 在 **fast-path 与 slow-path 出口都递增检查**（#2） | WASM fast-path + `gc_alloc_slow` trampoline |
| IMPL-3 | `gc_alloc_slow`/`gc_maybe_collect` 注册为 **sync `Func::wrap`**，闭包内不 `.await`/不回进 WASM（#9/R4） | host_imports 注册 |
| IMPL-4 | `gc_alloc_slow` 返回 `Option<Handle>`，trap 仅在 import trampoline 层（#8） | gc_alloc_slow + trampoline |
| IMPL-5 | collect 发生在 safepoint spill 之后（fast-path：spill 在 bump 前；slow-path：spill 在 call 前） | 编译器 safepoint 代码生成 |

### 18.3 GC 算法层

| ID | 约束 | 维护点 |
|----|------|--------|
| IMPL-6 | mark 用**显式 worklist**，不递归（#11） | `mark.rs` |
| IMPL-7 | sweep **按 ptr sort**（resize 破坏单调性，#3） | `sweeper.rs` |
| IMPL-8 | `GcContext` 不持 `&mut [u8]`；每阶段 `with_memory`/`with_memory_mut` 重借；grow 经 `ctx.grow()`（#9） | `context.rs` |
| IMPL-9 | `continuation_table` 非 completed 条目的 `captured_vars` 作为顶层 root（§10） | `roots.rs` |
| IMPL-10 | `obj_table_count` 不缩减；空槽供 fast-path 复用（下标稳定，§8.3） | sweep + fast-path |

### 18.4 编译器层

| ID | 约束 | 维护点 |
|----|------|--------|
| IMPL-11 | liveness **块级 CFG join 取 union + Phi 边分发**（#10） | `wjsm-ir` liveness pass |
| IMPL-12 | safepoint spill：循环内每个 safepoint 独立 save/restore；`__shadow_sp` 函数入口=出口（#4） | `compiler_instructions.rs` |
| IMPL-13 | 函数 prologue 容量检查：`shadow_sp + spill_upper_bound + frame <= shadow_stack_end`（R2） | `compiler_module.rs` prologue |

---

## 附录 A：TaskIntentDraft

见 §1.3。

## 附录 B：BaselineReadSetHint

见 §1.4。

## 附录 C：ImpactStatementDraft

见 §1.5。

## 附录 D：后续算法接入契约（稳定性承诺）

本框架为 generational / incremental / parallel GC 预留接入点。后续算法实现时：

**不变（稳定性承诺）**：
- `GcAlgorithm` / `Allocator` / `Marker` / `Sweeper` / `RootProvider` / `HeapObjectQuery` trait 签名
- `GcContext` 字段集（只增不减；增字段为 backward compatible）
- `Handle` / `Value` 别名
- fast-path 物理边界（WASM bump + host slow）

**可变（后续算法允许）**：
- impl 上述 trait 的新 struct（新增算法文件，不改既有算法）
- 活动对象布局（moving 算法需加 forwarding pointer —— 届时引入，本次不改）
- 真实 `WriteBarrier` / `ReadBarrier` 实现（generational 需 dirty card —— 届时实现，本次 no-op）
- `HeapRegionManager` 具体化（region-based GC 需真实 region 划分）
- `mark_step` / `sweep_step` 真实步进（incremental 需 worklist 调度）

**接入示例（未来 generational）**：
```rust
struct GenerationalGc {
    young: SegregatedFreeList,
    old:  SegregatedFreeList,
    card_table: CardTable,
    marker: GenerationalMarker,
}
impl Allocator for GenerationalGc { /* young-first alloc */ }
impl Marker for GenerationalGc { /* remember-set aware mark */ }
impl WriteBarrier for GenerationalGc {
    fn on_write(&mut self, ctx, target, field, val) {
        self.card_table.mark_dirty(target);  // 真实 barrier, 此时有消费者
    }
}
impl GcAlgorithm for GenerationalGc { /* minor + major collect */ }
```

**不破坏既有 MarkSweep**：新算法独立 struct，通过 `GcRegistry::create("generational")` 注册，CLI `--gc-algorithm generational` 切换。既有 fixture 在 MarkSweep 下保持全绿。

---

**ADR 信号（待回填）**：本 spec 引入 GC 算法 trait 抽象这一持久架构决策。落地后应在 `docs/adr/` 记录 ADR，覆盖：trait 边界选型理由（§6.1）、分配路径物理边界（§7）、non-moving 决策（§2）、barrier defer 决策（§12）。baseline-sync 问题：AGENTS.md "mark-sweep GC" 描述需更新为反映可插拔框架。
