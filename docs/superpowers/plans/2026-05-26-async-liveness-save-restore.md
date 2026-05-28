# Async Suspend Liveness 优化：实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 将 async suspend 点的绑定保存从「全量保存作用域链所有变量」优化为「只保存 resume 后仍被引用的活跃变量」，通过后向 liveness 分析实现。

**Architecture:** 两遍 lowering 方案——第一遍遇到 `await`/`yield`/`for-await-of` 时不发射 save/restore 指令，只记录 `PendingSuspend`；函数体 lowering 完成后，在干净 IR 上运行标准后向 liveness 分析，只为活跃变量发射 save/restore。

**Tech Stack:** Rust 2024, wjsm-ir, wjsm-semantic（不涉及 backend/runtime 变更）

---

### Task 1: 为 BasicBlock 添加 `instructions_mut` 方法

**Files:**
- Modify: `crates/wjsm-ir/src/lib.rs`

- [ ] **Step 1: 添加 `instructions_mut` 方法到 BasicBlock**

在 `pub fn instructions` 方法后添加：

```rust
pub fn instructions_mut(&mut self) -> &mut Vec<Instruction> {
    &mut self.instructions
}
```

- [ ] **Step 2: 编译验证**

```bash
cargo check -p wjsm-ir
```

- [ ] **Step 3: Commit**

```bash
git add crates/wjsm-ir/src/lib.rs
git commit -m "feat(ir): add instructions_mut to BasicBlock for async liveness optimization"
```

---

### Task 2: 添加 `PendingSuspend` 结构和 Lowerer 字段

**Files:**
- Modify: `crates/wjsm-semantic/src/lowerer_async_eval.rs`
- Modify: `crates/wjsm-semantic/src/lowerer_core.rs`

- [ ] **Step 1: 在 lowerer_async_eval.rs 顶部添加 PendingSuspend 结构**

在 `use super::*;` 之后、`impl Lowerer` 之前添加：

```rust
/// 推迟发射 save/restore 的 suspend 记录
struct PendingSuspend {
    /// Suspend 指令所在的 block
    suspend_block: BasicBlockId,
    /// resume 后执行起始 block
    resume_block: BasicBlockId,
    /// 该 suspend 点可见的所有绑定（async_visible_binding_names 结果）
    visible_bindings: Vec<String>,
}
```

- [ ] **Step 2: 在 Lowerer 结构体中添加 pending_suspends 字段**

在 `lowerer_core.rs` 的 `Lowerer` 构造函数和字段中添加：

```rust
// 在字段声明区域（约 line 64, async_closure_env_ir_name 之后）:
pending_suspends: Vec<PendingSuspend>,

// 在 new() 构造函数中:
pending_suspends: Vec::new(),
```

- [ ] **Step 3: 为 FunctionBuilder 添加 `blocks()` 方法**

在 `crates/wjsm-semantic/src/lib.rs` 的 `impl FunctionBuilder` 中，`block_mut` 方法旁添加：

```rust
fn blocks(&self) -> &[BasicBlock] {
    &self.blocks
}
```

- [ ] **Step 4: 编译验证**


- [ ] **Step 5: Commit**

```bash
git add crates/wjsm-semantic/src/lowerer_async_eval.rs crates/wjsm-semantic/src/lowerer_core.rs crates/wjsm-semantic/src/lib.rs
git commit -m "feat(semantic): add PendingSuspend struct, FunctionBuilder::blocks, and Lowerer field for deferred save/restore"
```

---

### Task 3: 修改 `lower_await_expr` — 推迟 save/restore

**Files:**
- Modify: `crates/wjsm-semantic/src/lowerer_async_eval.rs:812-905`

- [ ] **Step 1: 替换 save/restore 发射逻辑**

将 `lower_await_expr` 中当前的 save/restore 发射（约 line 847-866）改为记录 PendingSuspend。

当前代码（line 847-866）：
```rust
self.async_resume_blocks.push((next_state, resume_block));
let saved_bindings = self.async_visible_binding_names();
self.emit_save_async_bindings(block, &saved_bindings);

self.current_function.append_instruction(
    block,
    Instruction::Suspend {
        promise: promised,
        state: next_state,
    },
);

self.current_function.set_terminator(
    block,
    Terminator::Jump {
        target: continue_block,
    },
);

self.emit_restore_async_bindings(resume_block, &saved_bindings);
```

替换为：
```rust
self.async_resume_blocks.push((next_state, resume_block));
let visible_bindings = self.async_visible_binding_names();

// 推迟 save/restore —— 由 resolve_pending_suspends 在函数体 lowering 完成后统一处理
self.pending_suspends.push(PendingSuspend {
    suspend_block: block,
    resume_block,
    visible_bindings,
});

self.current_function.append_instruction(
    block,
    Instruction::Suspend {
        promise: promised,
        state: next_state,
    },
);

self.current_function.set_terminator(
    block,
    Terminator::Jump {
        target: continue_block,
    },
);
// 注意：不再调用 emit_restore_async_bindings
```

- [ ] **Step 2: 编译验证**

```bash
cargo check -p wjsm-semantic
```

- [ ] **Step 3: Commit**

```bash
git add crates/wjsm-semantic/src/lowerer_async_eval.rs
git commit -m "feat(semantic): defer save/restore in lower_await_expr to PendingSuspend"
```

---

### Task 4: 修改 `lower_yield_expr` (async generator) — 推迟 save/restore

**Files:**
- Modify: `crates/wjsm-semantic/src/lowerer_async_eval.rs:936-973`

- [ ] **Step 1: 替换 save/restore 发射逻辑**

将 `lower_yield_expr` 中 async generator 分支的 save/restore（约 line 944-973）改为记录 PendingSuspend。

当前代码：
```rust
self.async_resume_blocks.push((next_state, resume_block));
let saved_bindings = self.async_visible_binding_names();
self.emit_save_async_bindings(block, &saved_bindings);

let promised = self.alloc_value();
self.current_function.append_instruction(
    block,
    Instruction::CallBuiltin { ... AsyncGeneratorNext ... },
);
self.current_function.append_instruction(
    block,
    Instruction::Suspend { ... },
);
self.current_function.set_terminator(block, Terminator::Jump { target: continue_block });

self.emit_restore_async_bindings(resume_block, &saved_bindings);
```

替换为（相同模式，记录 PendingSuspend，移除 save/restore 调用）：

```rust
self.async_resume_blocks.push((next_state, resume_block));
let visible_bindings = self.async_visible_binding_names();

self.pending_suspends.push(PendingSuspend {
    suspend_block: block,
    resume_block,
    visible_bindings,
});

let promised = self.alloc_value();
self.current_function.append_instruction(
    block,
    Instruction::CallBuiltin {
        dest: Some(promised),
        builtin: Builtin::AsyncGeneratorNext,
        args: vec![gen_val, value],
    },
);
self.current_function.append_instruction(
    block,
    Instruction::Suspend {
        promise: promised,
        state: next_state,
    },
);
self.current_function.set_terminator(
    block,
    Terminator::Jump {
        target: continue_block,
    },
);
// 不调用 emit_restore_async_bindings
```

- [ ] **Step 2: 编译验证**

```bash
cargo check -p wjsm-semantic
```

- [ ] **Step 3: Commit**

```bash
git add crates/wjsm-semantic/src/lowerer_async_eval.rs
git commit -m "feat(semantic): defer save/restore in async generator yield to PendingSuspend"
```

---

### Task 5: 修改 `lower_for_await_of` — 推迟 save/restore

**Files:**
- Modify: `crates/wjsm-semantic/src/lowerer_stmt.rs:767-792`

- [ ] **Step 1: 替换 save/restore 发射逻辑**

将 `lower_for_await_of` 中的 save/restore（约 line 773-792）替换为记录 PendingSuspend。

```rust
self.async_resume_blocks.push((next_state, resume_block));
let visible_bindings = self.async_visible_binding_names();

self.pending_suspends.push(PendingSuspend {
    suspend_block: header,
    resume_block,
    visible_bindings,
});

self.current_function.append_instruction(
    header,
    Instruction::Suspend {
        promise: next_result,
        state: next_state,
    },
);

let continue_after_await = self.current_function.new_block();
self.current_function.set_terminator(
    header,
    Terminator::Jump {
        target: continue_after_await,
    },
);
// 不调用 emit_restore_async_bindings
```

- [ ] **Step 2: 编译验证**

```bash
cargo check -p wjsm-semantic
```

- [ ] **Step 3: Commit**

```bash
git add crates/wjsm-semantic/src/lowerer_stmt.rs
git commit -m "feat(semantic): defer save/restore in lower_for_await_of to PendingSuspend"
```

---

### Task 6: 实现 CFG 构建 + 后向 liveness 分析

**Files:**
- Create/Modify: `crates/wjsm-semantic/src/lowerer_async_eval.rs`（新增函数）

- [ ] **Step 1: 实现 CFG 构建函数**

在 `lowerer_async_eval.rs` 中新增 `build_cfg` 函数：

```rust
/// 构建 CFG：返回 successors、predecessors 映射
/// 特殊处理：Suspend block 的 successor 为 resume_block（而非 terminator 的 Jump 目标）
fn build_cfg(
    blocks: &[BasicBlock],
    pending_suspends: &[PendingSuspend],
) -> (Vec<Vec<BasicBlockId>>, Vec<Vec<BasicBlockId>>) {
    let block_count = blocks.len();
    // 建立 suspend_block → resume_block 的映射
    let suspend_to_resume: std::collections::HashMap<BasicBlockId, BasicBlockId> = pending_suspends
        .iter()
        .map(|ps| (ps.suspend_block, ps.resume_block))
        .collect();

    let mut successors: Vec<Vec<BasicBlockId>> = vec![Vec::new(); block_count];
    let mut predecessors: Vec<Vec<BasicBlockId>> = vec![Vec::new(); block_count];

    for block in blocks {
        let bid = block.id();
        let targets: Vec<BasicBlockId> = if let Some(&resume) = suspend_to_resume.get(&bid) {
            // Suspend block: 逻辑 successor 是 resume_block
            vec![resume]
        } else {
            // 普通 block: 从 terminator 提取 targets
            match block.terminator() {
                Terminator::Jump { target } => vec![*target],
                Terminator::Branch { true_block, false_block, .. } => vec![*true_block, *false_block],
                Terminator::Switch { cases, default_block, .. } => {
                    let mut targets: Vec<BasicBlockId> = cases.iter().map(|c| c.target).collect();
                    targets.push(*default_block);
                    targets
                }
                Terminator::Return { .. } | Terminator::Throw { .. } | Terminator::Unreachable => vec![],
            }
        };

        for &t in &targets {
            successors[bid.0 as usize].push(t);
            predecessors[t.0 as usize].push(bid);
        }
    }

    (successors, predecessors)
}
```

- [ ] **Step 2: 实现 compute_use_def 函数**

```rust
/// 计算每个 block 的 use 和 def 集合（只考虑用户变量，排除 async 内部绑定）
fn compute_use_def(blocks: &[BasicBlock]) -> (Vec<HashSet<String>>, Vec<HashSet<String>>) {
    let mut use_sets: Vec<HashSet<String>> = vec![HashSet::new(); blocks.len()];
    let mut def_sets: Vec<HashSet<String>> = vec![HashSet::new(); blocks.len()];

    for block in blocks {
        let bid = block.id().0 as usize;
        let mut local_def: HashSet<String> = HashSet::new();

        for instr in block.instructions() {
            match instr {
                Instruction::LoadVar { name, .. } => {
                    if !Lowerer::is_async_internal_binding(name) && !local_def.contains(name) {
                        use_sets[bid].insert(name.clone());
                    }
                }
                Instruction::StoreVar { name, .. } => {
                    if !Lowerer::is_async_internal_binding(name) {
                    if !Self::is_async_internal_binding(name) {
                        local_def.insert(name.clone());
                        def_sets[bid].insert(name.clone());
                    }
                }
                _ => {}
            }
        }
    }

    (use_sets, def_sets)
}
```

- [ ] **Step 3: 实现标准后向迭代 liveness**

```rust
/// 标准后向迭代 liveness 分析
/// 返回每个 block 的 live_in 集合（变量在该 block 入口处是活跃的）
fn compute_liveness(
    blocks: &[BasicBlock],
    successors: &[Vec<BasicBlockId>],
    use_sets: &[HashSet<String>],
    def_sets: &[HashSet<String>],
) -> Vec<HashSet<String>> {
    let block_count = blocks.len();
    let mut live_in: Vec<HashSet<String>> = use_sets.to_vec();
    let mut live_out: Vec<HashSet<String>> = vec![HashSet::new(); block_count];

    loop {
        let mut changed = false;

        for block in blocks {
            let bid = block.id().0 as usize;

            // live_out[B] = union of live_in of all successors
            let mut new_live_out: HashSet<String> = HashSet::new();
            for &succ in &successors[bid] {
                for var in &live_in[succ.0 as usize] {
                    new_live_out.insert(var.clone());
                }
            }

            if new_live_out != live_out[bid] {
                live_out[bid] = new_live_out;
                changed = true;
            }

            // live_in[B] = use[B] ∪ (live_out[B] - def[B])
            let mut new_live_in = use_sets[bid].clone();
            for var in &live_out[bid] {
                if !def_sets[bid].contains(var) {
                    new_live_in.insert(var.clone());
                }
            }

            if new_live_in != live_in[bid] {
                live_in[bid] = new_live_in;
                changed = true;
            }
        }

        if !changed {
            break;
        }
    }

    live_in
}
```

- [ ] **Step 4: 编译验证**

```bash
cargo check -p wjsm-semantic
```

- [ ] **Step 5: Commit**

```bash
git add crates/wjsm-semantic/src/lowerer_async_eval.rs
git commit -m "feat(semantic): implement CFG construction and backward liveness analysis"
```

---

### Task 7: 实现 `resolve_pending_suspends`

**Files:**
- Modify: `crates/wjsm-semantic/src/lowerer_async_eval.rs`（新增函数）

- [ ] **Step 1: 实现 resolve_pending_suspends 函数**

```rust
/// 在函数体 lowering 完成后调用：
/// 1. 在 FunctionBuilder 的 blocks 上运行 liveness 分析
/// 2. 对每个 PendingSuspend，取 visible_bindings ∩ live_at_suspend
/// 3. 在 suspend_block 的 Suspend 指令前插入 save 指令
/// 4. 在 resume_block 开头插入 restore 指令
pub(crate) fn resolve_pending_suspends(&mut self) {
    if self.pending_suspends.is_empty() {
        return;
    }
    let pending = std::mem::take(&mut self.pending_suspends);

    // Phase 1: liveness 分析 —— 以不可变引用读取 blocks
    // live_in 是 Vec<HashSet<String>>，拥有所有数据，不持有 blocks 引用
    let (live_in, _use_sets, _def_sets) = {
        let blocks = self.current_function.blocks();
        let (successors, _predecessors) = build_cfg(blocks, &pending);
        let (use_sets, def_sets) = compute_use_def(blocks);
        let live_in = compute_liveness(blocks, &successors, &use_sets, &def_sets);
        (live_in, use_sets, def_sets)
    }; // 不可变借用在此结束

    // Phase 2: 插入 save/restore 指令 —— 以可变引用修改 blocks

    // 对每个 PendingSuspend：
    // - 活跃变量 = visible_bindings ∩ live_in[suspend_block]
    // - 在 suspend_block 的 Suspend 指令前插入 save
    // - 在 resume_block 开头插入 restore
    for ps in &pending {
        let suspend_live = &live_in[ps.suspend_block.0 as usize];
        let live_bindings: Vec<String> = ps
            .visible_bindings
            .iter()
            .filter(|name| suspend_live.contains(*name))
            .cloned()
            .collect();

        // 插入 save 指令到 suspend_block（Suspend 指令之前）
        self.insert_save_before_suspend(ps.suspend_block, &live_bindings);

        // 插入 restore 指令到 resume_block 开头
        self.insert_restore_at_start(ps.resume_block, &live_bindings);
    }
}
```

- [ ] **Step 2: 实现 insert_save_before_suspend 辅助函数**

```rust
/// 在指定 block 的 Suspend 指令之前插入 save 指令序列
fn insert_save_before_suspend(&mut self, block_id: BasicBlockId, bindings: &[String]) {
    if bindings.is_empty() {
        return;
    }

    let fb = &mut self.current_function;
    let Some(block) = fb.block_mut(block_id) else { return };

    // 找到 Suspend 指令的索引
    let suspend_idx = block.instructions().iter()
        .position(|instr| matches!(instr, Instruction::Suspend { .. }));
    let Some(suspend_idx) = suspend_idx else { return };

    // 构建 save 指令序列
    let continuation = self.alloc_value();
    let mut save_instrs: Vec<Instruction> = Vec::new();

    save_instrs.push(Instruction::LoadVar {
        dest: continuation,
        name: format!("${}.$env", self.async_env_scope_id),
    });

    for binding in bindings {
        let slot = self.async_binding_slot(binding);
        let slot_const = self.module.add_constant(Constant::Number(slot as f64));
        let slot_val = self.alloc_value();
        let value = self.alloc_value();

        save_instrs.push(Instruction::Const {
            dest: slot_val,
            constant: slot_const,
        });
        save_instrs.push(Instruction::LoadVar {
            dest: value,
            name: binding.clone(),
        });
        save_instrs.push(Instruction::CallBuiltin {
            dest: None,
            builtin: Builtin::ContinuationSaveVar,
            args: vec![continuation, slot_val, value],
        });
    }

    // 在 Suspend 指令前插入
    let instrs = block.instructions_mut();
    instrs.splice(suspend_idx..suspend_idx, save_instrs);
}
```

- [ ] **Step 3: 实现 insert_restore_at_start 辅助函数**

```rust
/// 在指定 block 开头插入 restore 指令序列
fn insert_restore_at_start(&mut self, block_id: BasicBlockId, bindings: &[String]) {
    if bindings.is_empty() {
        return;
    }

    let fb = &mut self.current_function;
    let Some(block) = fb.block_mut(block_id) else { return };

    let continuation = self.alloc_value();
    let mut restore_instrs: Vec<Instruction> = Vec::new();

    restore_instrs.push(Instruction::LoadVar {
        dest: continuation,
        name: format!("${}.$env", self.async_env_scope_id),
    });

    for binding in bindings {
        let Some(&slot) = self.captured_var_slots.get(binding) else {
            continue;
        };
        let slot_const = self.module.add_constant(Constant::Number(slot as f64));
        let slot_val = self.alloc_value();
        let value = self.alloc_value();

        restore_instrs.push(Instruction::Const {
            dest: slot_val,
            constant: slot_const,
        });
        restore_instrs.push(Instruction::CallBuiltin {
            dest: Some(value),
            builtin: Builtin::ContinuationLoadVar,
            args: vec![continuation, slot_val],
        });
        restore_instrs.push(Instruction::StoreVar {
            name: binding.clone(),
            value,
        });
    }

    // 在 block 开头插入（index 0）
    let instrs = block.instructions_mut();
    // prepend: 在 index 0 处插入，不删除任何元素
    instrs.splice(0..0, restore_instrs);
}
```

- [ ] **Step 4: 编译验证**

```bash
cargo check -p wjsm-semantic
```

- [ ] **Step 5: Commit**

```bash
git add crates/wjsm-semantic/src/lowerer_async_eval.rs
git commit -m "feat(semantic): implement resolve_pending_suspends with save/restore insertion"
```

---

### Task 8: 在 async fn 表达式 lowering 中调用 resolve

**Files:**
- Modify: `crates/wjsm-semantic/src/lowerer_functions.rs`（`lower_async_fn_expr`）

- [ ] **Step 1: 在函数体 lowering 完成后调用 resolve_pending_suspends**

在 `lower_async_fn_expr` 中，函数体 lowering 完成（`inner_flow` 处理完后）、drain `async_resume_blocks` 之前，插入：

```rust
// 在 StmtFlow::Open(b) 的处理之后，line ~421（let resume_blocks = ...）之前:

// ── 推迟的 save/restore：运行 liveness 分析并插入 save/restore ──
self.resolve_pending_suspends();
```

- [ ] **Step 2: 编译验证**

```bash
cargo check -p wjsm-semantic
```

- [ ] **Step 3: Commit**

```bash
git add crates/wjsm-semantic/src/lowerer_functions.rs
git commit -m "feat(semantic): wire resolve_pending_suspends into lower_async_fn_expr"
```

---

### Task 9: 在 async fn 声明 lowering 中调用 resolve

**Files:**
- Modify: `crates/wjsm-semantic/src/lowerer_function_decls.rs`（`lower_async_fn_decl`）

- [ ] **Step 1: 在函数体 lowering 完成后调用 resolve_pending_suspends**

与 Task 8 相同模式——在 `lower_async_fn_decl` 中函数体处理完、drain resume_blocks 之前，插入 `self.resolve_pending_suspends();`。

找到对应位置（类似 line 415-420 区域），插入调用。

- [ ] **Step 2: 编译验证**

```bash
cargo check -p wjsm-semantic
```

- [ ] **Step 3: Commit**

```bash
git add crates/wjsm-semantic/src/lowerer_function_decls.rs
git commit -m "feat(semantic): wire resolve_pending_suspends into lower_async_fn_decl"
```

---

### Task 10: 在 async arrow lowering 中调用 resolve

**Files:**
- Modify: `crates/wjsm-semantic/src/lowerer_arrows.rs`（`lower_async_arrow_expr`）

- [ ] **Step 1: 在函数体 lowering 完成后调用 resolve_pending_suspends**

与 Task 8 相同模式——在 `lower_async_arrow_expr` 中函数体处理完、drain resume_blocks 之前（约 line 434），插入 `self.resolve_pending_suspends();`。

- [ ] **Step 2: 编译验证**

```bash
cargo check -p wjsm-semantic
```

- [ ] **Step 3: Commit**

```bash
git add crates/wjsm-semantic/src/lowerer_arrows.rs
git commit -m "feat(semantic): wire resolve_pending_suspends into lower_async_arrow_expr"
```

---

### Task 11: 在 top-level await (async main) 中调用 resolve

**Files:**
- Modify: `crates/wjsm-semantic/src/lowerer_async_eval.rs`（`finalize_async_main`）

- [ ] **Step 1: 在 finalize_async_main 开头调用 resolve_pending_suspends**

在 `finalize_async_main` 函数最开始（drain async_resume_blocks 之前），插入：

```rust
pub(crate) fn finalize_async_main(&mut self) -> Result<(), LoweringError> {
    // ── 推迟的 save/restore：运行 liveness 分析并插入 save/restore ──
    self.resolve_pending_suspends();

    let dispatch_block = self
        .async_dispatch_block
        .expect("async_dispatch_block not set");
    // ... 后续代码不变
```

- [ ] **Step 2: 编译验证**

```bash
cargo check -p wjsm-semantic
```

- [ ] **Step 3: Commit**

```bash
git add crates/wjsm-semantic/src/lowerer_async_eval.rs
git commit -m "feat(semantic): wire resolve_pending_suspends into finalize_async_main for TLA"
```

---

### Task 12: 运行测试并更新快照

- [ ] **Step 1: 构建项目确保编译通过**

```bash
cargo build
```

- [ ] **Step 2: 运行 IR snapshot 测试**

```bash
cargo test -p wjsm-semantic
```

预期：部分 IR 快照测试失败（因为 save/restore 指令数量变化）。需要手动检查变更是否合理。

- [ ] **Step 3: 更新 IR 快照**

对于因 save/restore 变化导致的失败测试，检查新的 IR 输出是否正确（死变量不再 save/restore），然后手动更新 `fixtures/semantic/*.ir`。

- [ ] **Step 4: 运行 async fixture 测试**

```bash
cargo nextest run -E 'test(happy__async_)'
```

预期：全部通过。

- [ ] **Step 5: 运行 for_await fixture 测试**

```bash
cargo nextest run -E 'test(happy__for_await)'
```

预期：全部通过。

- [ ] **Step 6: 运行 TLA (top-level await) fixture 测试**

```bash
cargo nextest run -E 'test(happy__tla)'
```

预期：全部通过。

- [ ] **Step 7: 运行全量 fixture 测试（排除已知挂起的测试）**

```bash
cargo nextest run -E 'not test(happy__new_prototype_chain) & not test(happy__global_fn_visible_in_nested) & not test(happy__eval_exception_expression_contexts) & not test(happy__weakref) & not test(happy__finalization_registry)'
```

预期：全部通过，无回归。

- [ ] **Step 8: 验证优化效果（可选）**

选择一个 async fixture（如 `async_multi_await`），检查生成的 IR snapshot 中 save/restore 指令数量是否减少。

- [ ] **Step 9: Commit**

```bash
git add fixtures/semantic/
git commit -m "test: update IR snapshots for async liveness optimization"
```
