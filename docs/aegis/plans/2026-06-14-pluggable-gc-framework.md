**执行状态（R4 修订，review 后）**: P0 ✅ + P1 ✅ + P2 ✅（含 review fix e6ea856）+ P3 ✅ 已实现并提交（分支 gc-framework）。P2/P3 review 发现 3 个 P4-blocker（resize-abandoned 回收、closure env_obj 标记、RootProvider fixed-point）+ 1 个 P4-deferred（safepoint 集补全），已记入 P4 任务。P4-P6 待执行。

# 可插拔 GC 框架实施计划

**Goal**: 用 non-moving mark-sweep + segregated free list 替代当前 `memory.grow` 无限扩容，建立 `GcAlgorithm` trait 框架（预留 generational/incremental/parallel 接入点），并恢复自动触发 GC。同步长循环不 OOM，WASM locals 持有的对象在 alloc 触发 GC 时不被误回收。

**Architecture**: 见 `docs/aegis/specs/2026-06-14-pluggable-gc-framework-design.md`。三层改动：`wjsm-ir`（liveness + ValueTy pass）、`wjsm-backend-wasm`（safepoint spill 代码生成 + 分配路径 bump+slow）、`wjsm-runtime`（`runtime_gc/` 框架 + MarkSweep 实现）。

**Tech Stack**: Rust 2024, wasmtime（epoch interruption，sync `Func::wrap`）, wasm-encoder, swc_core. 测试用 `cargo nextest`（per-test 超时 9s）。

**Baseline/Authority Refs**:
- `docs/aegis/specs/2026-06-14-pluggable-gc-framework-design.md`（设计 spec，§18 不变量清单为硬约束）
- `bug.md` O2（根因：moving GC + WASM locals 不可见）
- `AGENTS.md`（NaN-boxing、对象布局、无 stub 硬规则）

**Compatibility Boundary**（必须保持）:
- 现有 470+ fixture 全绿
- 活动对象布局不变（16B header + payload）
- NaN-boxing / `obj_table` 间接不变
- `gc()` global 行为保持
- §18 全部 INV/IMPL 不变量

**Verification**:
- `cargo nextest run --workspace` 全绿（每阶段后）
- 新增 fixture：长循环不 OOM、safepoint 安全、深链表 mark 不栈溢出
- 新增单元测试：liveness 正确性、free list、mark/sweep（mock GcContext）

**ADR Signal**: 本计划落地 GC 算法 trait 抽象（持久架构决策）。完成后应在 `docs/adr/` 记录 ADR：trait 边界选型、分配路径物理边界、non-moving 决策、barrier defer。baseline-sync：AGENTS.md "mark-sweep GC" 描述需更新为可插拔框架。

---

## Plan Pressure Test

```text
- Owner / contract / retirement:
    Owner: 新建 runtime_gc/ 模块组（单一 canonical owner）；删除旧 trigger_gc + core.rs gc_collect（retirement，P5）。
    Contract: GcAlgorithm trait + GcContext（Caller 注入，不持 slice）；§18 INV/IMPL 不变量。
    Retirement: 旧 compact GC 在 P5 删除前，P4 已重接 gc() global 到框架，无断档。
- Verification scope: 每阶段独立可验证（P1 liveness 单测、P3 mock ctx 单测、P4 长循环 fixture）；fixture 全绿是跨阶段回归闸。
- Task executability: 每任务给出确切文件路径 + 完整代码 + 确切命令。
- Pressure result: proceed（owner 清晰、retirement 有序、验证分层）。
```

## Plan-Time Complexity Check

```text
- Target files:
    新建: crates/wjsm-runtime/src/runtime_gc/{mod,api,context,mark_bitmap,roots,mark_sweep/{mod,allocator,marker,sweeper,context}}.rs
    新建: crates/wjsm-ir/src/liveness.rs, value_ty.rs
    修改: wjsm-ir/src/{lib.rs,value.rs}, wjsm-ir/tests/liveness.rs（新）
    修改: wjsm-backend-wasm/src/{compiler_module,compiler_instructions,compiler_helpers,compiler_array_helpers,lib.rs,host_import_registry.rs}
    修改: wjsm-runtime/src/{lib.rs,runtime_heap.rs,runtime_builtins.rs,host_imports/core.rs,host_imports/mod.rs}
    修改: wjsm-cli/src/lib.rs
- Existing size / shape signals: runtime_builtins.rs 2500+行（trigger_gc L2939-3223），compiler_instructions.rs 大文件。
    标记迁移要抽到 runtime_gc/，不在 runtime_builtins.rs 加新代码。
- Owner fit: runtime_gc/ 是 GC 的唯一 owner；runtime_builtins.rs 的 trigger_gc 迁出后只剩其他职责。
- Add-in-place risk: liveness pass 必须是 wjsm-ir 新文件（不能塞 lib.rs）；safepoint spill 是 compiler_instructions.rs 的新 pass。
- Better file boundary: 抽 helper（liveness.rs / value_ty.rs 独立文件）；runtime_gc/ 模块组独立。
- Recommendation: add owner file（runtime_gc/）+ extract helper（liveness.rs）+ split task（7 阶段独立提交）。
```

## Tasks 总览

| 阶段 | 任务数 | 独立验证 |
|------|--------|----------|
| P0 ✅ | T0.1-T0.2 | size 直方图 + 冻结 SIZE_CLASSES（验证无需改动） |
| P1 ✅ | T1.1-T1.5 | ValueTy + tag_needs_root + liveness pass（16/16 绿） |
| P2 ✅ | T2.1-T2.4 + R-fix | safepoint spill 代码生成 + sp 不变量 + 容量检查（561/561 绿，review fix e6ea856） |
| P3 ✅ | T3.1-T3.8 | runtime_gc/ 框架 + MarkSweep + worklist + mock 单测（16/16 绿，561/561 无回归） |
| P4 | T4.1-T4.6 + **T4.b1/b2/b3**（review 新增 blocker） | 分配路径改造 + handle 复用 + proactive + 长循环 fixture + **3 个 P4-blocker** |
| P5 | T5.1-T5.3 | 删除旧 GC + 迁移 fixed-point + grep 无残留 |
| P6 | T6.1-T6.2 | 预留 hook 默认 impl + CLI --gc-algorithm |

### P4 新增 blocker 任务（review 发现，P3 记录详见上方）

| 任务 | 内容 | 阻塞原因 |
|------|------|----------|
| **T4.b1** | sweeper 回收 resize-abandoned 区域（abandoned list 或 gap 扫描） | INV-B vs §8.2 矛盾；长循环 OOM |
| **T4.b2** | marker 标记 closure env_obj + native_callable 内部引用 | 闭包捕获对象误回收 |
| **T4.b3** | RootProvider fixed-point（continuation_table.captured_vars 等） | IMPL-9；async/promise 路径漏 root |
| **T4.b4**（P2 deferred） | 补全 safepoint 集（ObjectSpread/CollectRestArgs/NewPromise/...） | 这些 alloc 点漏 spill |

---

# 阶段 P0：size 直方图 + 冻结 SIZE_CLASSES ✅ IMPLEMENTED

**Why**: segregated free list 的 size class table 是 allocator 核心。spec §9.1 已冻结初始值，P0 验证覆盖率 ≥ 99%，必要时局部微调（不重构）。

## T0.1 采集 fixture 对象 size 直方图

**Files**:
- create: `/tmp/gc_size_hist.rs`（临时探针脚本，不入库）

**Why**: 确认冻结的 SIZE_CLASSES（§9.1）覆盖 fixture 实际对象 size 分布。

**Impact/Compatibility**: 只读探针，不改任何代码。

**Verification**: 脚本输出 size 直方图 + SIZE_CLASSES 覆盖率。

**Steps**:

- [ ] **写探针脚本**。在 runtime 加一个临时 host import `__gc_probe_size(i32)`，编译期在每个 `$obj_new`/`$arr_new` 成功后插入 `call __gc_probe_size(size)`。脚本主体：

```rust
// /tmp/gc_size_hist.rs — 临时，不入库。放到 crates/wjsm-runtime/src/host_imports/ 下临时编译。
// 实现方式：在 host_imports/core.rs 临时加一个 Func::wrap("__gc_probe_size", |caller, size| {
//     caller.data_mut().size_histogram.lock().push(size as usize);
// });
// RuntimeState 临时加 size_histogram: Mutex<Vec<usize>>。
// 运行所有 happy fixture 后 dump 直方图。
```

- [ ] **运行 happy fixtures 采集**：

```bash
cargo nextest run -E 'test(happy__)' 2>&1 | tail -5
# 然后用一个小 main 读 RuntimeState.size_histogram dump
```

- [ ] **计算覆盖率**。对照冻结的 SIZE_CLASSES：

```rust
const SIZE_CLASSES: &[usize] = &[
    16, 48, 80, 112, 144, 176, 208, 272, 336, 432,
    528, 640, 768, 1024, 1536, 2048, 4096, 8192, 16384,
];
// 覆盖率 = (落在某 class 精确匹配 or 进 big_list 的对象数) / 总对象数
// best-fit 允许向上取 class，所以所有 size 都有归属，覆盖率应 = 100%
// 关键指标：精确匹配率（size 恰好等于某 class）应高（减少分割）
```

- [ ] **记录结论到 spec §9.1 注释**。若某 size 区间集中但无 class 覆盖（如 fixture 大量 288B 对象但 class 是 272/336），局部增删 1-2 个 class。不改结构。

- [ ] **移除探针**（git checkout 还原 host_imports/core.rs + lib.rs），确认 `cargo nextest run --workspace` 仍全绿。

- [ ] **Commit**: `chore: GC P0 size histogram probe (temporary, not committed)`

> 注：探针是临时手段，**不入库**。结论（覆盖率数字 + 是否微调）写进 spec §9.1 注释并 commit spec。

## T0.2 确认 SIZE_CLASSES 定稿

**Files**:
- modify: `docs/aegis/specs/2026-06-14-pluggable-gc-framework-design.md`（§9.1 注释加覆盖率结论）

**Why**: 固化 P0 结论，P3 据此实现。

**Steps**:

- [ ] 在 §9.1 SIZE_CLASSES 上方加注释行：`// P0 验证：fixture 覆盖率 XX%，精确匹配率 YY%，class 未调整/微调（说明）`
- [ ] **Commit**: `docs: GC P0 freeze SIZE_CLASSES (coverage XX%)`

---

# 阶段 P1：IR 层 liveness + ValueTy 类型推断 ✅ IMPLEMENTED

**Status**: 已实现并提交（分支 gc-framework）。wjsm-ir 16/16 测试通过。

**Why**: safepoint spill（P2）需要"在 safepoint 哪些 live ValueId 是 Handle 类型"。wjsm-ir 当前无任何 per-ValueId 类型/liveness（经验证，从零建）。

**Why in wjsm-ir**: 零外部依赖，归属正确；wjsm-semantic 的 name-based liveness 后续可复用（去重，非本计划范围）。

**Critical（#10）**: liveness 必须**块级 CFG join 取 union + Phi 边分发**，否则 if/else/loop 汇合点 live 集合错误 → safepoint 误判（漏 spill 活值 → GC 误回收）。**已验证正确**（if/else join + loop backedge 测试通过）。

## 实现产物（已提交）

| 任务 | Commit | 文件 | 测试 |
|------|--------|------|------|
| T1.1 tag_needs_root | `abc5e01` | `crates/wjsm-ir/src/value.rs` | 2 (15 handle tags + 8 scalars) |
| T1.2 ValueTy | `f0aa8bc` | `crates/wjsm-ir/src/value_ty.rs` | 4 (handle/scalar/const/polymorphic) |
| T1.3-T1.4 liveness | `c308bbd` | `crates/wjsm-ir/src/liveness.rs` | 4 (linear/dead/if-else-join/loop-backedge) |
| T1.5 export | (同上) | `crates/wjsm-ir/src/lib.rs` (`pub mod liveness; pub mod value_ty;`) | — |

## 关键实现事实（供后续阶段引用）

- **API 名（已验证，勿再猜）**：`encode_function_idx`（非 `encode_function`）、`encode_closure_idx`、`encode_bound_idx`、`encode_bigint_handle`、`encode_symbol_handle`、`encode_regexp_handle`、`encode_proxy_handle`、`encode_scope_record_handle`、`encode_native_callable_idx`、`encode_runtime_string_handle`、`encode_object_handle`、`encode_handle(TAG_*, idx)`、`encode_exception`、`encode_undefined()`、`encode_null()`、`encode_bool(bool)`、`encode_string_ptr(ptr)`、`encode_f64(f64)`。**无** `UNDEFINED`/`NULL`/`TRUE`/`FALSE` 常量（都是函数）。无 `encode_array`（用 `encode_handle(TAG_ARRAY, idx)`）。
- **`is_runtime_string_handle(val)` 已存在**（value.rs:203），内部已 AND `is_string`，故 `tag_needs_root` 直接用 `is_runtime_string_handle(val)` 即可。
- **`Function::new(name, entry: BasicBlockId)`** — 只两参（无 params）。`Function` 无 `module()` 方法；`infer_value_ty` 签名为 `infer_value_ty(module: &Module, function: &Function)`，通过 `module.constants()[ConstantId.0 as usize]` 查 Constant。
- **`PhiSource { predecessor: BasicBlockId, value: ValueId }`**（lib.rs:855）。
- **`BasicBlockId` 无 `Ord`**（只有 `Eq + Hash`）— successors 去重用 `HashSet` 而非 `sort`/`dedup`。
- **`SwitchCaseTarget { constant: ConstantId, target: BasicBlockId }`**。
- **liveness 契约**：`compute_liveness(f)[(block_id, i)]` = 紧邻指令 `i` 执行**前**活跃集；`(block_id, len)` = 块出口（含 terminator uses）。细化起点 = `live_out ∪ terminator_uses`（terminator 在最后一条指令后执行）。**Phi 的 def/use 不在块内细化**（已由块级 + 边分发处理）。
- **`Instruction` 条件 dest**：`Call`/`CallBuiltin`/`SuperCall` 的 `dest: Option<ValueId>`；`ConstructCall` **无 dest**（void）；其余 producing 都有 `dest: ValueId`。

## 复现命令

```bash
cargo nextest run -p wjsm-ir   # 16 tests, all green
```

---

# 阶段 P2：Backend safepoint spill 代码生成 ✅ IMPLEMENTED

**Status**: 已实现并提交（ddf013c + review fix e6ea856）。561/561 非 network fixture 绿。

**Why**: 编译器在每个 safepoint（alloc 点）前把 live Handle local spill 到 shadow stack，让 GC root 集精确。**本阶段不接 GC**，只验证 spill 不破坏语义（fixture 全绿）。

**Critical（#4）**: `__shadow_sp` 函数入口=出口；循环内每个 safepoint 独立 save/restore。

## 实现产物（已提交）

| 任务 | Commit | 文件 | 说明 |
|------|--------|------|------|
| T2.1 cursor | `ddf013c` | `compiler_control.rs`（8 个 compile_instruction 调用点全设 cursor）、`compiler_core.rs`（Compiler 字段）、`lib.rs`、`compiler_module.rs` | emit-position cursor `(current_emit_block_idx, current_emit_instr_idx)`；`current_spill_locals()` 查 liveness |
| T2.2 spill emit | `ddf013c` | `compiler_instructions.rs` | `emit_safepoint_spill_prologue/epilogue` 包裹 NewObject/NewArray/Call/CallBuiltin/SuperCall/ConstructCall |
| T2.3 容量检查 | `ddf013c` | `compiler_module.rs` | `compute_max_spill_bytes` + `emit_safepoint_capacity_check`（函数 prologue 一次） |
| T2.4 验证 | `ddf013c` | fixture 更新 | `eval_exception_expression_contexts.expected`（仅 wasm addr padding） |
| **R-fix** | `e6ea856` | 同上 | **修正 spill epilogue：subtract-restore → save/restore**（见下方偏差） |

## 偏差与修正（review 发现）

1. **liveness map 重整为嵌套**（T2.1 偏差）：`compute_liveness` 返回扁平 `(block_id, instr_idx)→Set`；在 `compile_function`/`compile_js_function` 重整为 `HashMap<BasicBlockId, HashMap<usize, Set>>` 便于 `current_spill_locals` 查询。**原计划未指定扁平/嵌套**——实现选择嵌套，无 API 影响。

2. **spill epilogue 用 save/restore 而非 subtract**（review 修正，e6ea856）：原实现 `shadow_sp -= n*8` 复位。**Bug**：SuperCall `forward_args` 分支把 `shadow_sp` 重置为 caller `args_base`（local 2），非 spill 前值，subtract 得错误指针。P2 被 GC 未触发掩盖，P4 会崩。**修正**：新增 `safepoint_sp_saved_idx`（i32 local，local_count+4）保存 spill 前 `shadow_sp`，epilogue 恢复。同时修 slot 冲突：`eval_var_base_local_idx` 从 local_count+4 移到 +5，`total_i32_locals` 2→3。

3. **CompileMode::Eval 走 compile_function**（验证）：eval module 也经 `compile_function`（compiler_module.rs:345），liveness + spill + 容量检查全覆盖。

## P4-deferred（P2 安全不阻塞，P4 必须补）

**safepoint 集不完整**：当前 `is_safepoint()` 只含 NewObject/NewArray/Call/CallBuiltin/SuperCall/ConstructCall。下列也分配但未 wrap，P2（GC 未触发）无碍，**P4 GC 接通后会漏 spill → 误回收**：

- `ObjectSpread`（compile_object_spread，调 obj_new）
- `CollectRestArgs`（compiler_instructions.rs:735，调 arr_new + ArrayPush）
- `NewPromise`（调 promise_create，host alloc）
- `PromiseResolve`/`PromiseReject`（可能 host alloc）
- `StringConcatVa`（产 runtime string handle）
- `SetProto`（可能触发 grow_object）

**P4 任务**：扩展 `is_safepoint()` + 在对应 compile 分支包裹 spill。`compile_builtin_call`（CallBuiltin 走此）已包裹，但 builtin 内部若调 host-alloc 的（PromiseCreate/StringConcat 等）经由 CallBuiltin 已被 wrap——**需验证**：CallBuiltin 的 spill 是否覆盖 builtin 内部的 alloc（是的，CallBuiltin 是 safepoint，spill 在 call 前，覆盖 builtin 内部所有 alloc 点）。

## 已验证的 Compiler 字段（lib.rs:100-163，review 后更新）

- `shadow_sp_global_idx: u32`（= 4，WASM global）
- `shadow_sp_scratch_idx: u32`（i32 local，call 期间保存 sp）
- `shadow_stack_end_global_idx: u32`（= 8）
- `ssa_local_base: u32`（main=0，JS fn=8）
- `local_idx(val_id) -> u32` = `val_id + ssa_local_base`（compiler_module.rs:6）
- `compile_instruction(&mut self, module, instruction) -> Result<bool>`（compiler_instructions.rs:5）
- `self.emit(WasmInstruction::*)`（emit 单条指令）
- `alloc_counter_global_idx`（已存在，global 5）

## T2.1 emit-position cursor + safepoint liveness 查找

**Files**:
- modify: `crates/wjsm-backend-wasm/src/lib.rs`（Compiler 加字段）
- modify: `crates/wjsm-backend-wasm/src/compiler_module.rs`（compile_function 计算 liveness + 重置 cursor）
- modify: `crates/wjsm-backend-wasm/src/compiler_control.rs`（调用 compile_instruction 前更新 cursor）

**Why（修正 global_idx）**: 因 structured 编译非线性，不能用单一 `global_idx`。改用 **(current_block_idx, current_instr_idx)** cursor：编译器在 emit 每条指令前设置 cursor，compile_instruction 内 alloc 指令用 cursor 查 liveness。

**Steps**:

- [ ] **Compiler 加字段**（lib.rs Compiler struct）:

```rust
/// 当前函数的 per-instruction liveness（P1 已实现，wjsm_ir::liveness::compute_liveness）。
/// compile_function 入口计算一次。
current_fn_liveness: std::collections::HashMap<(wjsm_ir::BasicBlockId, usize), std::collections::HashSet<wjsm_ir::ValueId>>,
/// 当前函数的 ValueTy（P1 已实现，wjsm_ir::value_ty::infer_value_ty）。
current_fn_value_ty: std::collections::HashMap<wjsm_ir::ValueId, wjsm_ir::value_ty::ValueTy>,
/// 当前 emit 位置的 IR block 索引（block 在 function.blocks() 中的下标）。
current_emit_block_idx: usize,
/// 当前 emit 位置在当前 block 内的指令下标。
current_emit_instr_idx: usize,
```

- [ ] **compile_function 入口计算 liveness + ValueTy**（compiler_module.rs:529，在 assign_var_locals 之后）:

```rust
// 复用 P1 实现
self.current_fn_liveness = wjsm_ir::liveness::compute_liveness(function);
self.current_fn_value_ty = wjsm_ir::value_ty::infer_value_ty(module, function);
self.current_emit_block_idx = 0;
self.current_emit_instr_idx = 0;
```

- [ ] **在 compile_structured 的 block 遍历处设置 block cursor**（compiler_control.rs:233 附近，`let block = &blocks[idx];` 之后）:

```rust
self.current_emit_block_idx = idx;
self.current_emit_instr_idx = 0;
```

> 同样在所有其他 compile_instruction 调用循环处（if/else/loop/switch 分支体）设置 `current_emit_block_idx`（每个分支体入口）+ 每条指令前 `current_emit_instr_idx = i`。调用点清单：compiler_control.rs L236-237（主循环）、L385-386/392-393（if/else）、L719-720（loop 体）、L1101-1102、L1164-1165（switch case）。每处 `for (i, instruction) in block.instructions().enumerate()` 改为遍历时 set cursor。

- [ ] **封装"取当前 safepoint spill 集合"helper**（新方法，compiler_instructions.rs 或 compiler_module.rs）:

```rust
/// 返回当前 emit 位置（alloc 指令前）需 spill 的 local idx 列表。
/// = live ValueId ∩ Handle 类型 → local_idx。
fn current_spill_locals(&self) -> Vec<u32> {
    let key = (wjsm_ir::BasicBlockId(self.current_emit_block_idx as u32),
               self.current_emit_instr_idx);
    let Some(live) = self.current_fn_liveness.get(&key) else { return vec![]; };
    let mut spill: Vec<u32> = live.iter()
        .filter(|v| {
            // ValueTy 缺失（Unknown）保守当 Handle
            self.current_fn_value_ty.get(v)
                .map_or(true, |t| *t == wjsm_ir::value_ty::ValueTy::Handle)
        })
        .map(|v| self.local_idx(v.0))
        .collect();
    spill.sort();
    spill.dedup();
    spill
}
```

> 注：BasicBlockId 的值 = block 在 blocks() 中的下标（IR 约定，block_by_id O(1) by index）。需确认 BasicBlockId(u32) 的 .0 是否等于下标（读 lib.rs block_by_id 确认；若不等，改用 block.id() 映射）。

- [ ] **编译确认**: `cargo build -p wjsm-backend-wasm` 通过。

## T2.2 safepoint spill emit（在 compile_instruction 的 alloc 分支）

**Files**:
- modify: `crates/wjsm-backend-wasm/src/compiler_instructions.rs`

**Why**: 在 NewObject/NewArray/Call/CallBuiltin/SuperCall/ConstructCall 的 alloc call 前后包 spill 序列。

**Steps**:

- [ ] **加 spill emit helper**（compiler_instructions.rs）:

```rust
/// 在 alloc call 前调用：save sp + spill 所有 live handle locals。
/// 返回 spill 的 local 数（供 epilogue 用）。
fn emit_safepoint_prologue(&mut self, spill: &[u32]) {
    if spill.is_empty() { return; }
    // save shadow_sp 到 scratch
    self.emit(WasmInstruction::GlobalGet(self.shadow_sp_global_idx));
    self.emit(WasmInstruction::LocalSet(self.shadow_sp_scratch_idx));
    // spill each live handle local
    for &local in spill {
        self.emit(WasmInstruction::GlobalGet(self.shadow_sp_global_idx));
        self.emit(WasmInstruction::LocalGet(local));
        self.emit(WasmInstruction::I64Store(wasm_encoder::MemArg {
            offset: 0, align: 3, memory_index: 0,
        }));
        self.emit(WasmInstruction::GlobalGet(self.shadow_sp_global_idx));
        self.emit(WasmInstruction::I64Const(8));
        self.emit(WasmInstruction::I64Add);
        self.emit(WasmInstruction::GlobalSet(self.shadow_sp_global_idx));
    }
}

/// 在 alloc call 后调用：restore shadow_sp（non-moving 无需 reload local 值）。
fn emit_safepoint_epilogue(&mut self, spill: &[u32]) {
    if spill.is_empty() { return; }
    self.emit(WasmInstruction::LocalGet(self.shadow_sp_scratch_idx));
    self.emit(WasmInstruction::GlobalSet(self.shadow_sp_global_idx));
}
```

- [ ] **在 NewObject 分支包裹**（compiler_instructions.rs:373）:

```rust
Instruction::NewObject { dest, capacity } => {
    let spill = self.current_spill_locals();  // 在 call 前
    self.emit_safepoint_prologue(&spill);
    // ... 原 emit（call obj_new 等）...
    self.emit_safepoint_epilogue(&spill);
    // ... 原 LocalSet(dest) ...
}
```

- [ ] **同样包裹 NewArray（L532）、Call（L346）、CallBuiltin、SuperCall、ConstructCall**。注意 Call/CallBuiltin/SuperCall 的 dest 是 Option；ConstructCall 无 dest。spill 在 call 指令前，epilogue 在 call 后（LocalSet 前/后均可，因 non-moving 不改值）。

> **关键**：current_spill_locals() 必须在 emit 该指令**之前**调用（此时 cursor 指向该指令）。若 compile_instruction 内部在 emit 前已 set cursor，则 OK；否则在调用 current_spill_locals 前 self.current_emit_instr_idx 已被 compile_structured 设置。

- [ ] **编译确认**: `cargo build -p wjsm-backend-wasm` 通过。

## T2.3 shadow stack 容量检查（函数 prologue）

**Files**:
- modify: `crates/wjsm-backend-wasm/src/compiler_module.rs`（compile_function prologue）

**Why（R2）**: 防止 spill 区溢出覆盖对象堆。spill_upper_bound = max spill 集合大小 × 8（编译期已知）。

**Steps**:

- [ ] **compile_function 入口算 spill_upper_bound**:

```rust
// 遍历所有 safepoint，取最大 live-handle-local 数 × 8
let spill_upper_bound = self.compute_max_spill_bytes(module, function);
```

```rust
fn compute_max_spill_bytes(&self, module: &IrModule, function: &IrFunction) -> usize {
    let live = wjsm_ir::liveness::compute_liveness(function);
    let ty = wjsm_ir::value_ty::infer_value_ty(module, function);
    let mut max = 0usize;
    let mut global_idx = 0usize;
    for bb in function.blocks() {
        for (i, ins) in bb.instructions().enumerate() {
            if self.is_safepoint(ins) {
                let key = (bb.id(), i);
                let cnt = live.get(&key)
                    .map(|s| s.iter().filter(|v| ty.get(v).map_or(true, |t| *t == wjsm_ir::value_ty::ValueTy::Handle)).count())
                    .unwrap_or(0);
                max = max.max(cnt);
            }
            global_idx += 1;  // 注：global_idx 仅用于此估算，不用于 emit 查找
        }
    }
    max * 8
}
```

- [ ] **prologue 加容量检查**（compile_function，local 声明之后、body 之前）:

```rust
// if shadow_sp + frame_size + spill_upper_bound > shadow_stack_end: unreachable
self.emit(WasmInstruction::GlobalGet(self.shadow_sp_global_idx));
self.emit(WasmInstruction::I32Const((frame_size + spill_upper_bound as i32).max(0)));
self.emit(WasmInstruction::I32Add);
self.emit(WasmInstruction::GlobalGet(self.shadow_stack_end_global_idx));
self.emit(WasmInstruction::I32GtU);
self.emit(WasmInstruction::If(BlockType::Empty));
self.emit(WasmInstruction::Unreachable);
self.emit(WasmInstruction::End);
```

> `frame_size` = 该函数 shadow stack frame 大小（params + eval vars；从现有 prologue 的 sp 推进量算）。确认 align 用 `BlockType::Empty`（import 自 wasm_encoder）。

- [ ] **编译确认**: `cargo build -p wjsm-backend-wasm`。

## T2.4 验证 spill 不破坏语义（fixture 全绿）

**Files**: 无（仅验证）

**Why**: 本阶段不接 GC，spill 只是写 shadow stack 又复位 sp，应无语义影响（值写了但 GC 不读，因 P2 无 GC）。验证 spill 代码生成正确 + sp 复位 + 容量不溢出。

**Verification**: fixture 全绿 + dump-wat 检查。

**Steps**:

- [ ] **跑全部 fixture**:

```bash
cargo nextest run --workspace
```

- [ ] **若失败**：用 dump-wat 检查出问题的 fixture（spill 序列、sp 复位、cursor 对齐）:

```bash
cargo run -- dump-wat fixtures/happy/<fail>.js > /tmp/out.wat
```

- [ ] **dump-wat 抽查一个有 alloc 的 fixture**（如 hello.js）确认 spill 序列存在且 sp 复位。

- [ ] **Commit**: `feat(backend): safepoint spill codegen (no GC yet, semantics-preserving)`

---

# 阶段 P3：runtime_gc/ 框架 + MarkSweep 实现 ✅ IMPLEMENTED

**Status**: 已实现并提交（034cbfb）。16/16 GC 单测绿；561/561 非 network fixture 绿（无回归）。

**Why**: 建立 GcAlgorithm trait 框架 + MarkSweep（worklist mark + sweep 重建 free list）。本阶段用 mock GcContext 单测，不接 backend（P4 集成）。

**Critical（#9）**: GcContext 持 Caller 不持 slice；with_memory/grow 分离。
**Critical（#11）**: mark worklist 不递归。

## 实现产物（已提交 034cbfb）

| 任务 | 文件 | 测试 |
|------|------|------|
| T3.1 api.rs | `runtime_gc/api.rs`（trait 全集 + GcContext + GcStats） | — |
| T3.2 mark_bitmap | `runtime_gc/mark_bitmap.rs` | 5（mark/dedup/grow/popcount/reset） |
| T3.3 context.rs | `runtime_gc/context.rs`（heap-meta helpers + global readers） | — |
| T3.4 SegregatedFreeList | `runtime_gc/mark_sweep/allocator.rs` | 7（exact/split/fallback/big/too-small/empty/size_class） |
| T3.5 Marker | `runtime_gc/mark_sweep/marker.rs` | 4（linear/dead/cycle/deep-chain-R8-10000） |
| T3.6 Sweeper | `runtime_gc/mark_sweep/sweeper.rs` | （集成测试待 P4） |
| T3.7 MarkSweepCollector | `runtime_gc/mark_sweep/mod.rs`（collect + collect_with_roots） | — |
| T3.8 RuntimeRoots | `runtime_gc/roots.rs`（shadow stack scan + function props） | — |

## 偏差（review 记录）

1. **GcContext.with_memory_mut 签名收窄**（T3.3 偏差）：原设计 `with_memory_mut(f: FnOnce(&mut Caller, &mut [u8]))`。**借用冲突**：`memory.data_mut(&mut *self.caller)` 与 `f(&mut *self.caller, data)` 双重可变借 caller。**修正**：改为 `with_memory_mut(f: FnOnce(&mut [u8]))`，不传 caller（caller 经 self 后续访问）。调用方（sweeper write_obj_table_slot）相应去掉 `_caller` 参数。

2. **global reader 需 `&mut Caller`**（T3.3 偏差）：wasmtime `Global::get` 需 `AsContextMut`（`&mut Caller`），非 `&Caller`。故 GcContext 的 `obj_table_ptr/count/heap_ptr/shadow_sp/...` 等读方法全改 `&mut self`（经 `&mut *self.caller`）。marker/sweeper/collect 调用处相应调整。`Global::get` 返回 `Val`（非 `Result`），`.i32()` 链式取值。

3. **handle_free_list 字段加在 RuntimeState**（T4.2 前置）：P3 实现 `MarkSweepCollector::collect` 时需把 `freed_handles` 推入 RuntimeState 供 fast-path 复用。**提前**在 P3 加了 `handle_free_list: Arc<Mutex<Vec<u32>>>` 字段（lib.rs）+ `handle_free_list_for_gc()` accessor。原计划放 T4.2，实现提前到 P3 因 collect 已需它。

4. **mark_children 用 `tag_needs_root` 过滤**（T3.5 实现）：移植自 `collect_child_from_value`，但用 P1 新增的 `tag_needs_root` 统一过滤，替代分散的 `is_object||is_array||is_function` 检查。`is_object/is_array → decode_object_handle`；`is_function → low32`；其他 handle tag（closure/native_callable/bigint/...）**当前不解析**（见 P4-blocker #2）。

5. **测试用纯 buffer 而非 mock GcContext**（T3.5 测试策略）：`mark_drain_on_buffer(mark_bits, data, obj_table_ptr, count, roots)` 在 `&[u8]` 上跑完整 mark（seed+drain），不依赖 wasmtime。4 个 marker 测试（linear/dead/cycle/deep-chain）用此。**R8 验证**：10000 层链表不栈溢出（worklist Vec）。

## P4-blockers（P3 mock 测试不暴露，P4 GC 接通前必须解决）

### P4-blocker #1：sweeper 不回收 resize-abandoned 区域

**问题**：INV-B 声称 "resize 后旧块成为死块，由 sweep 回收"。但 §8.2 sweep 算法只遍历 `obj_table`，而 `grow_array`/`grow_object`（runtime_values.rs:222-226/273-277）把已有 handle 的 ptr 重写到更高位置后，**旧 ptr 区域不在 obj_table 中**（handle 已指向新 ptr）。sweep 看不到旧区域 → 无法回收 → 凸堆永久泄漏。

**影响**：长循环（spec 验收 #1）数组频繁 push→resize，每次 resize 泄漏旧区域 → OOM。**P4 阻塞**。

**修复方案（P4 T4.x 新增）**：sweep 在按 ptr sort 的块扫描后，补充一次 gap 扫描：
- 收集所有 obj_table 块的 `[ptr, ptr+size)` 区间（sort 后）
- 扫 [object_heap_start, max_block_end)：未被任何块覆盖的 gap → add_free_region
- 或：让 grow_array/grow_object 在重写 ptr 前，把旧 (ptr, size) 注册到一个 host 维护的 "abandoned list"，sweep 读取并入 free list（更精确，避免 gap 扫描的 O(n²)）

**推荐**：abandoned list（O(1) 注册，sweep O(n) 合并）。需改 grow_array/grow_object + RuntimeState 加 `abandoned_regions: Vec<(ptr, size)>`。

### P4-blocker #2：marker 漏标 closure/native_callable 的内部引用

**问题**：`push_value_handle`（marker.rs:153）只处理 object/array/function。**closure**（TAG_CLOSURE）持有 `env_obj`（对象 handle），存在对象属性/数组元素里时 marker 遍历到但不解析 → env_obj 对象漏标 → 误回收。`native_callable` 同理（内部可能持有对象引用）。

**现有 compact GC 处理**：`mark_runtime_value_recursive`（runtime_heap.rs:471-490）对 closure 查 `RuntimeState.closures[closure_idx].env_obj` 递归标记。

**修复方案（P4 T4.x 新增）**：marker 的 `collect_child_handles` 对 closure 值，经 `ctx.with_state` 查 `closures` 表取 `env_obj`，作为 object handle 加入候选。native_callable 类似（查 `native_callables` 表）。需把 `collect_child_handles` 改为 `ctx.with_memory` + `ctx.with_state` 混合（当前只 with_memory）。

**影响**：闭包持有捕获变量的对象会被误回收 → P4 fixture（任何用闭包的）崩。**P4 阻塞**。

### P4-blocker #3：RootProvider fixed-point 未实现

**问题**：`RuntimeRoots::for_each_host_table_root`（roots.rs）只提供 function props（0..num_ir_functions）作为稳定 root。**fixed-point host 侧表追踪**（microtask/promise reactions/continuation_table.captured_vars/streams/BYOB/async_generator/combinator）未实现。spec §10 要求 continuation_table 非 completed 条目的 captured_vars 作为**顶层 root**（IMPL-9）。

**修复方案（P4 T4.x）**：移植 `trace_runtime_side_table_roots_fixed_point`（runtime_builtins.rs:2590-2918）到 `for_each_host_table_root`，或在 `collect_with_roots` 中分轮注入 roots（mark → fixed-point loop：再 mark host table roots until popcount 不变）。

## T3.1 runtime_gc 模块骨架 + api.rs（trait 定义）

**Files**:
- create: `crates/wjsm-runtime/src/runtime_gc/mod.rs`
- create: `crates/wjsm-runtime/src/runtime_gc/api.rs`
- modify: `crates/wjsm-runtime/src/lib.rs`（加 `mod runtime_gc;`）

**Why**: trait 框架是可插拔的基础（spec §6）。

**Steps**:

- [ ] **创建 mod.rs**:

```rust
//! 可插拔 GC 框架（spec §6）。单一 canonical owner: 本模块组。
pub mod api;
pub mod context;
pub mod mark_bitmap;
pub mod roots;
pub mod mark_sweep;
```

- [ ] **创建 api.rs**：贴 spec §6 的完整 trait 定义（GcAlgorithm/Allocator/Marker/Sweeper/RootProvider/WriteBarrier/ReadBarrier/HeapRegionManager/GcContext/GcStats/Handle/Value/MarkProgress）。GcContext 用 spec §6 R1 版本（持 Caller，with_memory/with_memory_mut/grow/with_state）。

> 注：GcContext 持 `&mut Caller<'b, RuntimeState>` + `Memory`。需 `use wasmtime::{Caller, Memory};`。

- [ ] **编译确认**: `cargo build -p wjsm-runtime`（trait 无 impl，可能有 unused warning，正常）。

## T3.2 MarkBitmap（从 RuntimeState 提取/封装）

**Files**:
- create: `crates/wjsm-runtime/src/runtime_gc/mark_bitmap.rs`

**Why**: 现有 `gc_mark_bits: Arc<Mutex<Vec<u64>>>`（lib.rs:1047）封装为独立类型。

**Steps**:

- [ ] **实现 MarkBitmap**:

```rust
//! Handle 标记位图（1 bit per handle）。
pub struct MarkBitmap {
    bits: Vec<u64>,
}
impl MarkBitmap {
    pub fn new() -> Self { Self { bits: vec![] } }
    pub fn reset(&mut self, count: usize) {
        let words = count.div_ceil(64);
        if self.bits.len() < words { self.bits.resize(words, 0); }
        else { self.bits[..words].fill(0); self.bits[words..].fill(0); }
    }
    pub fn mark(&mut self, h: u32) {
        let (w, b) = (h as usize / 64, h as usize % 64);
        if w >= self.bits.len() { self.bits.resize(w + 1, 0); }
        self.bits[w] |= 1u64 << b;
    }
    pub fn is_marked(&self, h: u32) -> bool {
        let (w, b) = (h as usize / 64, h as usize % 64);
        w < self.bits.len() && (self.bits[w] & (1u64 << b)) != 0
    }
    pub fn popcount(&self) -> usize { self.bits.iter().map(|w| w.count_ones() as usize).sum() }
}
```

- [ ] **编译确认**。

## T3.3 GcContext 实现（context.rs）

**Files**:
- create: `crates/wjsm-runtime/src/runtime_gc/context.rs`

**Why**: 桥接 Caller + Memory，提供 with_memory/grow/with_state（spec §6）。

**Steps**:

- [ ] **实现 GcContext**（按 spec §6 R1 版本）。关键方法 with_memory/with_memory_mut/grow/with_state。grow 内部 `self.memory.grow(&mut *self.caller, pages)`。

- [ ] **编译确认**。

## T3.4 SegregatedFreeList（mark_sweep/allocator.rs）

**Files**:
- create: `crates/wjsm-runtime/src/runtime_gc/mark_sweep/mod.rs`
- create: `crates/wjsm-runtime/src/runtime_gc/mark_sweep/allocator.rs`
- test: `crates/wjsm-runtime/src/runtime_gc/mark_sweep/allocator.rs`（#[cfg(test)] mod tests）

**Why**: free list 数据结构 + alloc_slow（best-fit）+ add_free_region。size class 用 §9.1 冻结值。

**Verification**: 单测 alloc/dealloc/复用/分割。

**Steps**:

- [ ] **写失败测试**（#[cfg(test)]）:

```rust
#[test]
fn alloc_from_free_list_best_fit() {
    let mut fl = SegregatedFreeList::new();
    fl.add_free_region(1000, 144);  // 进 class 144
    assert_eq!(fl.alloc(144), Some(1000));  // 精确匹配
    assert_eq!(fl.alloc(144), None);  // 已用完
}

#[test]
fn alloc_splits_oversized_block() {
    let mut fl = SegregatedFreeList::new();
    fl.add_free_region(2000, 272);  // class 272
    let p = fl.alloc(144);  // 从 class 272 取，分割
    assert_eq!(p, Some(2000));
    // 剩余 128 进对应 class
    assert_eq!(fl.alloc(112), Some(2000 + 144));  // 从分割块取
}

#[test]
fn alloc_falls_back_to_higher_class() {
    let mut fl = SegregatedFreeList::new();
    fl.add_free_region(3000, 528);  // class 528
    assert_eq!(fl.alloc(144), Some(3000));  // class 144 空，向上取 528
}
```

- [ ] **运行 RED**: 编译失败。
- [ ] **实现 SegregatedFreeList**（spec §9.1 SIZE_CLASSES + §9.2 数据结构 + §9.3 alloc_slow + §9.4 add_free_region）。
- [ ] **运行 GREEN**。
- [ ] **Commit**: `feat(runtime): SegregatedFreeList allocator`

## T3.5 Marker（worklist，移植 mark_object_recursive）

**Files**:
- create: `crates/wjsm-runtime/src/runtime_gc/mark_sweep/marker.rs`

**Why（#11）**: mark 用显式 worklist 不递归。移植 `mark_object_recursive`（runtime_heap.rs:577-**585**，**仅 9 行委派器**）调用的 `mark_object_recursive_with_funcs`（runtime_heap.rs:**587+**，**真实子对象收集逻辑**），但把递归改 worklist。

**Steps**:

- [ ] **实现 Marker for MarkSweepCollector**:

```rust
//! Mark phase：worklist（不递归，#11）。移植自 runtime_heap mark_object_recursive。
impl Marker for MarkSweepCollector {
    fn mark(&mut self, ctx: &mut GcContext, roots: &mut dyn Iterator<Item = Handle>) {
        let mut worklist: Vec<Handle> = Vec::new();
        // seed roots
        for h in roots { if self.mark_bits.mark_if_new(h, &mut worklist) {} }
        // drain
        while let Some(h) = worklist.pop() {
            self.mark_children(ctx, h, &mut worklist);
        }
    }
    fn is_marked(&self, h: Handle) -> bool { self.mark_bits.is_marked(h) }
}
```

> `mark_children` 读单对象的引用（proto/props/elements/env_obj），推入 worklist。移植 mark_object_recursive_with_funcs 的子收集逻辑（runtime_heap.rs:618-750），但 collect 后 push 而非递归。每批 with_memory 借用。

- [ ] **移植子收集逻辑**（从 runtime_heap.rs:618-750 的 children_to_mark 收集代码）。

- [ ] **编译确认**。

## T3.6 Sweeper（按 ptr sort + 线性重建 free list）

**Files**:
- create: `crates/wjsm-runtime/src/runtime_gc/mark_sweep/sweeper.rs`

**Why（#3）**: sweep 必须按 ptr sort（resize 破坏单调性），线性合并相邻 unmarked。

**Steps**:

- [ ] **实现 Sweeper for MarkSweepCollector**（spec §8.2 算法）: 收集 blocks → sort_by_ptr → 线性扫描合并 → add_free_region → 清空 unmarked handle 槽（推入 handle_free_list）→ process weak refs。

- [ ] **编译确认**。

## T3.7 MarkSweepCollector + impl GcAlgorithm

**Files**:
- modify: `crates/wjsm-runtime/src/runtime_gc/mark_sweep/mod.rs`

**Steps**:

- [ ] **组装 MarkSweepCollector**（持有 SegregatedFreeList + MarkBitmap）+ impl GcAlgorithm（collect = reset mark → mark roots（经 RootProvider 回调）→ fixed-point → sweep → weak refs）。

- [ ] **编译确认**。

## T3.8 RootProvider 实现 + mock 单测 + 深链表测试

**Files**:
- create: `crates/wjsm-runtime/src/runtime_gc/roots.rs`
- test: `crates/wjsm-runtime/src/runtime_gc/mark_sweep/mod.rs`（#[cfg(test)]）

**Why**: RootProvider 回调式（#6）；mock ctx 单测 mark/sweep；深链表不栈溢出（R8）。

**Verification**: mock 单测 + 深链表 10000 层。

**Steps**:

- [ ] **实现 RuntimeRoots**（impl RootProvider）：for_each_shadow_stack_root（with_memory 扫描）、for_each_host_table_root（含 continuation_table captured_vars 顶层 root，§10）。

- [ ] **写 mock 单测**：构造假 memory（手写 obj_table + 对象 header），mock RootProvider，跑 mark/sweep，断言 marked/sweipped 正确。

- [ ] **写深链表测试（R8）**:

```rust
#[test]
fn mark_deep_chain_no_stack_overflow() {
    // 构造 10000 层链表（每对象有个 next 属性指向下一个）
    // mock ctx + roots，跑 mark，断言不栈溢出（worklist）
    // 断言 10000 个全 marked
}
```

- [ ] **运行**: `cargo nextest run -p wjsm-runtime -E 'test(runtime_gc)'`。
- [ ] **Commit**: `feat(runtime): GC framework + MarkSweep (worklist, sweep, roots, mock tests)`

---

# 阶段 P4：分配路径集成 + handle 复用 + proactive GC

**Why**: 把 $obj_new/$arr_new 改为 bump + gc_alloc_slow，接框架，恢复 proactive GC（#2），handle 复用（#7）。本阶段后 GC 真正工作。

**Critical（IMPL-3）**: gc_alloc_slow/gc_maybe_collect 注册 sync Func::wrap。
**Critical（#8）**: gc_alloc_slow 返回 Option<Handle>，trap 仅 trampoline。

> **P2/P3 review 发现的 P4-blocker（必须在本阶段解决，详见上方 P3/P2 章节）**：
> - **T4.b1** sweeper 回收 resize-abandoned 区域（INV-B vs §8.2 矛盾）
> - **T4.b2** marker 标记 closure env_obj + native_callable 内部引用
> - **T4.b3** RootProvider fixed-point（continuation_table.captured_vars 等）
> - **T4.b4** 补全 safepoint 集（ObjectSpread/CollectRestArgs/NewPromise/...）
>
> 这 4 个在 P2/P3 被 "GC 未触发" 掩盖，P4 GC 接通后会立即暴露为 fixture 崩溃或长循环 OOM。**建议执行顺序：先 T4.b1/b2/b3/b4（修框架正确性），再 T4.1-T4.6（接通）**。

## T4.b1 sweeper 回收 resize-abandoned 区域（P4-blocker #1）

**Files**:
- modify: `crates/wjsm-runtime/src/runtime_gc/mark_sweep/sweeper.rs`
- modify: `crates/wjsm-runtime/src/runtime_values.rs`（grow_array/grow_object 注册旧区域）
- modify: `crates/wjsm-runtime/src/lib.rs`（RuntimeState 加 `abandoned_regions`）

**Why**: INV-B 声称 resize 旧块由 sweep 回收，但 sweep 只遍历 obj_table，看不到被重写 ptr 后的旧区域 → 凸堆永久泄漏 → 长循环 OOM。

**Steps**:
- [ ] RuntimeState 加 `abandoned_regions: Arc<Mutex<Vec<(usize, usize)>>>`（ptr, size）。
- [ ] `grow_array`/`grow_object`（runtime_values.rs:222/273）在重写 obj_table 槽前，把旧 `(ptr, old_size)` push 到 `abandoned_regions`。注：`$obj_set` reallocate（compiler_helpers.rs:1037）同理。
- [ ] sweeper 在 §8.2 块扫描后，读 `abandoned_regions`，对每条 `(ptr, size)` add_free_region（按 ptr merge 进已 sort 的块序列，或单独入表后让 alloc_slow 合并）。
- [ ] sweep 结束清空 `abandoned_regions`。
- [ ] **编译 + 单测**：构造 resize 场景（数组 push 触发 grow），验证旧区域进 free list。
- [ ] **Commit**: `fix(gc): sweep回收 resize-abandoned 区域 (P4-blocker #1)`

## T4.b2 marker 标记 closure env_obj（P4-blocker #2）

**Files**:
- modify: `crates/wjsm-runtime/src/runtime_gc/mark_sweep/marker.rs`

**Why**: closure（TAG_CLOSURE）持有 env_obj（对象 handle），存在对象属性/数组元素里时 marker 遍历到但不解析 → env_obj 漏标 → 误回收。

**Steps**:
- [ ] `collect_child_handles` 改为接收 `ctx`（或拆成两步：with_memory 收集 raw values，再 with_state 解析 closure）。
- [ ] 对 closure 值：`ctx.with_state(|st| st.closures.lock().get(idx).map(|e| e.env_obj))`，env_obj 作为 object handle 加入候选。
- [ ] native_callable：查 `native_callables` 表（若内部持有对象引用，如 bound_objects）。
- [ ] **测试**：构造闭包持有捕获对象的对象图，验证 env_obj 被 marked。
- [ ] **Commit**: `fix(gc): marker 标记 closure env_obj + native_callable (P4-blocker #2)`

## T4.b3 RootProvider fixed-point（P4-blocker #3）

**Files**:
- modify: `crates/wjsm-runtime/src/runtime_gc/roots.rs`
- 移植自: `runtime_builtins.rs:trace_runtime_side_table_roots_fixed_point`（L2590-2918）

**Why**: continuation_table 非 completed 条目的 captured_vars 必须作为顶层 root（IMPL-9/spec §10）。当前 for_each_host_table_root 只提供 function props。

**Steps**:
- [ ] 移植 fixed-point tracer 到 `RuntimeRoots::for_each_host_table_root`：遍历 microtask_queue / promise reactions / continuation_table.captured_vars / stream readers/controllers / BYOB views / async_generator queue / combinator contexts。
- [ ] 在 `collect_with_roots`（mark_sweep/mod.rs）实现 fixed-point loop：mark → 再注入 host table roots → until popcount 不变。
- [ ] **测试**：构造 pending continuation（无 reaction 引用）场景，验证 captured_vars 对象存活。
- [ ] **Commit**: `fix(gc): RootProvider fixed-point (continuation/promise/streams) (P4-blocker #3)`

## T4.b4 补全 safepoint 集（P2-deferred → P4）

**Files**:
- modify: `crates/wjsm-backend-wasm/src/compiler_instructions.rs`（is_safepoint + 各 handler 包裹）

**Why**: ObjectSpread/CollectRestArgs/NewPromise/PromiseResolve/Reject/StringConcatVa/SetProto 也分配但未 wrap spill。P2（GC 未触发）无碍，P4 会漏 spill → 误回收。

**Steps**:
- [ ] `is_safepoint()` 加 ObjectSpread/CollectRestArgs/NewPromise/PromiseResolve/PromiseReject/StringConcatVa/SetProto。
- [ ] 各 handler 分支包裹 `emit_safepoint_spill_prologue/epilogue`（参照 NewObject 模式）。
- [ ] **验证**：fixture 全绿（spill 不破坏语义）。
- [ ] **Commit**: `fix(backend): 补全 safepoint 集 (P2-deferred, P4)`

## T4.1 host imports: gc_alloc_slow + gc_maybe_collect + gc_take_freed_handle

**Files**:
- modify: `crates/wjsm-runtime/src/host_imports/core.rs`（或新 gc.rs）
- modify: `crates/wjsm-backend-wasm/src/host_import_registry.rs`（注册 SpecialHostImport）

**Steps**:

- [ ] **实现 gc_alloc_slow import**（sync Func::wrap，spec §7.2 trampoline）：调 GcContext + gc_alloc_slow → Option → Some 返 handle / None trap。
- [ ] **实现 gc_maybe_collect import**（sync Func::wrap，无参）：调 gc_algorithm.collect（fast-path proactive 触发，spec §7.1）。
- [ ] **实现 gc_take_freed_handle import**（sync Func::wrap）：从 host handle_free_list pop，返 i32（-1 表空）。
- [ ] **在 host_import_registry.rs 注册** 3 个新 SpecialHostImport 变体（`GcAllocSlow` / `GcMaybeCollect` / `GcTakeFreedHandle`）**并加 HOST_IMPORT_SPECS 条目**（host_import_registry.rs:13 enum + :66 specs）。注：`GcCollect` 已存在（lib.rs:27）无需重加。否则 `special_host_import_indices` 不会被 compiler_core.rs:194-201 填充，导致索引缺失。
- [ ] **编译确认**。

## T4.2 RuntimeState: gc_algorithm + handle_free_list + gc_threshold

**Files**:
- modify: `crates/wjsm-runtime/src/lib.rs`

**Steps**:

- [ ] RuntimeState（lib.rs:1040）加字段：
  - `gc_algorithm: Arc<Mutex<Box<dyn GcAlgorithm + Send + Sync>>>`（默认 `MarkSweepCollector::new()`；trait 加 `Send + Sync` bound；Arc<Mutex> 因 host fn 经 &Caller 访问需内部可变性）
  - `handle_free_list: Arc<Mutex<Vec<u32>>>`（沿用 `native_callable_free_slots` 约定 lib.rs:1066；非裸 Vec<u32>）
  - **gc_threshold 已存在**（lib.rs:1052，类型 **u64 非 usize**，非 Arc/Mutex 包装）—— **不重新声明**，仅调初值 1000
  - alloc_counter（Arc<Mutex<u64>> lib.rs:1049）、gc_mark_bits（Arc<Mutex<Vec<u64>>> lib.rs:1047）**也已存在**，复用
- [ ] **编译确认**。

## T4.3 改 $obj_new：bump + handle_free_list + proactive + gc_alloc_slow

**Files**:
- modify: `crates/wjsm-backend-wasm/src/compiler_helpers.rs`（$obj_new L56-195）

**Steps**:

- [ ] **改写 $obj_new**（spec §7.1）: bump fast-path（先 take_or_alloc_handle：call gc_take_freed_handle，-1 则 count++）+ init_header + proactive（__alloc_counter++ 检查 __gc_threshold 调 gc_maybe_collect）+ OOM 走 gc_alloc_slow。删除 memory.grow OOM（L73-109）。
- [ ] **改写 $arr_new**（compiler_array_helpers.rs）同上。
- [ ] **编译确认**。

## T4.4 gc() global 重接到框架

**Files**:
- modify: `crates/wjsm-runtime/src/runtime_builtins.rs`（NativeCallable::GcCollect L1854-1859）

**Steps**:

- [ ] NativeCallable 枚举定义在 **lib.rs:2172**（非 runtime_builtins.rs）；`GcCollect` 变体在 lib.rs:2270。其 match arm 在 runtime_builtins.rs:1856-1859，改调 `gc_algorithm.collect`（经 GcContext），不调旧 trigger_gc。
- [ ] **编译确认**。

## T4.5 长循环 fixture + safepoint 安全 fixture

**Files**:
- create: `fixtures/happy/gc_long_loop.js` + `.expected`
- create: `fixtures/happy/gc_safepoint_local.js` + `.expected`

**Why**: 验收标准 #1（长循环不 OOM）+ #3（safepoint 安全）。

**Steps**:

- [ ] **写 gc_long_loop.js**:

```js
let arr = [];
for (let i = 0; i < 1000000; i++) { arr.push({ x: i }); }
// 不 OOM 即通过（GC 回收死对象；arr 本身活着但内部对象轮换）
console.log("done");
```

- [ ] **写 gc_safepoint_local.js**（WASM local 持有唯一引用，触发 GC 后仍可用）:

```js
function hold() {
    let obj = { val: 42 };
    let dummy = { a: 1 };  // 触发 alloc，可能 GC
    return obj.val;        // obj 仍可用（spill 保护）
}
console.log(hold());
// expected: 42
```

- [ ] **生成 .expected**:

```bash
WJSM_UPDATE_FIXTURES=1 cargo nextest run -E 'test(happy__gc_)'
```

- [ ] **Commit**: `test: GC long-loop + safepoint-safety fixtures`

## T4.6 集成验证

**Files**: 无

**Verification**: fixture 全绿 + 长循环 + streams_byob。

**Steps**:

- [ ] **跑全 fixture**:

```bash
cargo nextest run --workspace
```

- [ ] **重点验证 streams_byob 系列**（R4 async 安全）:

```bash
cargo nextest run -E 'test(streams_byob) | test(fetch_http_byob)'
```

- [ ] **Commit**: `feat: GC integration (bump+slow alloc, proactive, handle reuse)`

---

# 阶段 P5：删除旧 GC + 迁移 fixed-point tracer

**Why**: 根除 duplicate owner（旧 trigger_gc + core.rs gc_collect）。框架已接管（P4）。

## T5.1 删除 trigger_gc + 迁移 fixed-point tracer

**Files**:
- modify: `crates/wjsm-runtime/src/runtime_builtins.rs`（删 trigger_gc L2939-3223）
- 已迁移：trace_runtime_side_table_roots_fixed_point → runtime_gc/roots.rs（T3.8）

**Steps**:

- [ ] 确认 trigger_gc 无调用方（GcCollect 已重接，T4.4）。
- [ ] **删除 trigger_gc**。
- [ ] **删除 sweep_dead_promise_slots**（已并入 sweeper）。注：它当前被 `trigger_gc` 在 runtime_builtins.rs:3219 调用 —— 由于 T5.1 同时删除 trigger_gc，调用点一并消失，无残留引用。
- [ ] **编译确认**: `cargo build -p wjsm-runtime`。

## T5.2 删除 core.rs gc_collect

**Files**:
- modify: `crates/wjsm-runtime/src/host_imports/core.rs`（删 gc_collect L1218-1642）

**Steps**:

- [ ] 确认 gc_collect import 无 WASM 调用方（$obj_new 已改用 gc_alloc_slow，T4.3）。
- [ ] **删除 gc_collect import** + linker 注册。
- [ ] **编译确认**。

## T5.3 grep 无残留 + 全 fixture

**Steps**:

- [ ] **grep 确认**:

```bash
# 无残留引用
grep -rn "trigger_gc\|gc_collect" crates/wjsm-runtime/src/ | grep -v "gc_alloc_slow\|gc_maybe_collect\|gc_collect_" 
# 应只剩 gc_alloc_slow/gc_maybe_collect/gc_take_freed_handle（新 imports）
```

- [ ] **全 fixture**:

```bash
cargo nextest run --workspace
```

- [ ] **Commit**: `refactor: remove legacy compact GC (trigger_gc, gc_collect); consolidate to framework`

---

# 阶段 P6：预留 hook 默认 impl + CLI --gc-algorithm

## T6.1 预留 hook 默认 impl 落地

**Files**:
- modify: `crates/wjsm-runtime/src/runtime_gc/api.rs`

**Why**: WriteBarrier/ReadBarrier/HeapRegionManager/mark_step/sweep_step 默认 impl 确认存在（trait 定义已含默认，本任务确认 + 加 doc）。

**Steps**:

- [ ] 确认 trait 默认 impl 完整（WriteBarrier::on_write 等 no-op，mark_step/sweep_step 一次性）。
- [ ] **Commit**: `docs(runtime_gc): confirm barrier/region hooks default impls (zero-cost)`

## T6.2 CLI --gc-algorithm

**Files**:
- modify: `crates/wjsm-cli/src/lib.rs`

**Why**: 运行期切换 GC 算法（调试 + 未来 generational）。

**Steps**:

- [ ] **加 enum**（lib.rs，参照 Target/Stage 模式）:

```rust
#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum GcAlgorithmChoice {
    MarkSweep,
    // 未来：Generational, Incremental
}
```

- [ ] **加 Cli 字段**:

```rust
#[arg(long, global = true, default_value = "mark-sweep")]
gc_algorithm: GcAlgorithmChoice,
```

- [ ] **threading**: 从 execute/run_pipeline 把 choice 传入 runtime 初始化（RuntimeState.gc_algorithm 按 choice 构造）。

> 注：`backend_wasm::compile` 定义在 **crates/wjsm-backend-wasm/src/lib.rs:23**（签名 `compile(program: &Program) -> Result<Vec<u8>>`），调用点在 wjsm-cli/src/lib.rs:818/1132。GC choice 是 runtime 期（非编译期），**compile() 签名不变**。在 RuntimeState 构造处按 choice 选 algorithm。

- [ ] **测试**: `cargo run -- run fixtures/happy/gc_long_loop.js --gc-algorithm mark-sweep` 通过。
- [ ] **Commit**: `feat(cli): --gc-algorithm flag`

---

# 收尾：文档同步 + ADR

## T-final.1 文档更新

**Files**:
- modify: `bug.md`（O2 → RESOLVED）
- modify: `AGENTS.md`（GC 描述更新为可插拔框架）
- modify: `docs/aegis/specs/2026-06-14-pluggable-gc-framework-design.md`（状态 → Implemented）

**Steps**:

- [ ] bug.md O2 状态 FIXED → RESOLVED，加注"根因（moving GC）已消除，non-moving + safepoint spill"。
- [ ] AGENTS.md "mark-sweep GC" 行更新为"可插拔 GC 框架（non-moving mark-sweep + segregated free list + safepoint spill；GcAlgorithm trait 预留 generational/incremental）"。
- [ ] **Commit**: `docs: GC framework implemented (O2 resolved, AGENTS.md updated)`

## T-final.2 ADR

**Files**:
- create: `docs/adr/0002-pluggable-gc-framework.md`

**Why**: 持久架构决策记录（spec ADR signal）。

**Steps**:

- [ ] 写 ADR：context（O2 根因）、decision（non-moving + trait + WASM bump/host slow）、alternatives（moving+spill reload / 全 shadow stack）、consequences（trait 稳定性承诺见 spec 附录 D）、baseline-sync（AGENTS.md）。
- [ ] **Commit**: `docs(adr): 0002 pluggable GC framework`

---

## Risks（实施期）

| 风险 | 阶段 | 缓解 |
|------|------|------|
| spill 破坏语义 | P2 | fixture 全绿闸；dump-wat 抽查 |
| liveness Phi 边错误 | P1 | 3 单测（if/else/loop）；P2 fixture 误回收会崩 |
| mark 栈溢出 | P3 | worklist + 深链表单测（R8） |
| grow 借用 UB | P3/P4 | GcContext 不持 slice（#9）；with_memory 重借 |
| async reentry | P4 | sync Func::wrap（§12.3）；streams_byob 验证 |
| handle 无限膨胀 | P4 | take_or_alloc_handle（#7） |
| 旧 GC 残留 | P5 | grep + fixture |

## Retirement

- P5 删除 trigger_gc + core.rs gc_collect（框架 P4 已接管，无断档）
- spec §18 INV/IMPL 是实现期硬约束，违反即 GC 不安全

## Self-Review 结论（R4 — P2/P3 实现后 review 修订）

- **实现进度**：P0-P3 ✅ 已实现并提交（ddf013c P2、e6ea856 P2 review-fix、034cbfb P3）。测试：wjsm-ir 16/16、runtime_gc 16/16、非 network fixture 561/561 全绿。
- **Spec 覆盖**：spec §14 的 P0-P6 + §18 全部 INV/IMPL 不变量在计划中均有对应任务。
- **Review 发现 4 个 P4 阻塞项**（P2/P3 被 "GC 未触发" 掩盖）：
  - **T4.b1**：sweeper 不回收 resize-abandoned 区域（INV-B vs §8.2 算法矛盾 —— spec 自身不完整）。→ 长循环 OOM。
  - **T4.b2**：marker 漏标 closure env_obj / native_callable 内部引用。→ 闭包捕获对象误回收。
  - **T4.b3**：RootProvider fixed-point 未实现（continuation_table.captured_vars 等）。→ async/promise 漏 root。
  - **T4.b4**：safepoint 集不完整（ObjectSpread/CollectRestArgs/NewPromise/...）。→ 这些 alloc 点漏 spill。
- **已修正的实现偏差**（P2/P3 review 记录，详见各阶段章节）：
  - P2 spill epilogue：subtract-restore → save/restore（forward_args bug，e6ea856）。
  - P2 liveness map：扁平 → 嵌套（便于查询）。
  - P3 with_memory_mut：去掉 caller 参数（借用冲突）。
  - P3 global readers：`&self` → `&mut self`（wasmtime AsContextMut）。
  - P3 handle_free_list：T4.2 前置到 P3（collect 已需它）。
- **类型一致**：GcContext 持 Caller 贯穿；gc_alloc_slow → Option<Handle> 贯穿；RootProvider 回调式贯穿。
- **兼容性**：§18 为硬约束；fixture 全绿跨阶段闸；活动对象布局/NaN-boxing/obj_table 不变。
- **验证**：每阶段确切命令（cargo nextest run -E ... / cargo build -p ...）。
- **双轨**：P5 Retirement（删旧 GC，P4 先接管无断档）。
- **结论**：P0-P3 实现稳定可验证。P4 在接通 GC 前**必须先解决 T4.b1-b4 四个 blocker**（否则 fixture 立即崩或长循环 OOM）。P5-P6 待执行。

