# 运行时侧表生命周期补全 — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use subagent-driven-development or executing-plans to implement task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** 补全 2026-05-26 Phase 4 未落地的侧表回收：`promise_table` GC 清槽、`continuation_table`/`combinator_contexts` free-list、continuation 编码统一、`native_callables` resolving 函数回收。

**Architecture:** Handle 下标稳定 — 禁止 `Vec::retain` 压缩；Promise 在 `trigger_gc` sweep 后按 mark bit 清槽；continuation/combinator 用 free-list 复用；drain 末尾回收 completed continuation。

**Tech Stack:** Rust 2024, wasmtime 43, wjsm-runtime

**Baseline/Authority Refs:**
- `docs/aegis/specs/2026-06-07-runtime-side-table-lifecycle-design.md`
- `docs/superpowers/specs/2026-05-26-async-audit-refactor-design.md` §5

**Compatibility Boundary:** ECMAScript 语义与现有 fixture stdout 不变；不改变 NaN-box handle 编码规则。

**Verification:** `cargo nextest run --workspace`；新增 `crates/wjsm-runtime/tests/side_table_lifecycle.rs`

---

## Plan Basis

| 项 | 内容 |
|----|------|
| Problem | 长驻 `RuntimeState` 下 `promise_table`/`combinator_contexts` 单调增长；`continuation_table.retain` 与 handle 语义冲突 |
| Owner files | `lib.rs`, `runtime_builtins.rs`, `runtime_microtask.rs`, `runtime_async_fn.rs`, `host_imports/async_fn.rs`, `runtime_combinators.rs`, `runtime_promises.rs` |
| Retirement | 删除 `continuation_table.retain`；不新增 compaction 路径 |

## Plan Pressure Test

- **Owner / contract / retirement:** GC 钩子属 `runtime_builtins::trigger_gc`；free-list API 属各表 owner 模块 — **proceed**
- **Verification scope:** 新 Rust 测试 + 全量 nextest — **proceed**
- **Task executability:** 每 task ≤5 步 — **proceed**

## Plan-Time Complexity Check

- **Target files:** `runtime_builtins.rs` (~2700 行)、`lib.rs` (~2800 行) — 仅追加 sweep 块与 `RuntimeState` 字段，**edit-in-place**
- **Recommendation:** 不新建大模块；可选 `sweep_promise_table_slots(caller, mark_snapshot, obj_table_count)` 私有函数放在 `runtime_builtins.rs` 末尾

---

### Task 1: RuntimeState 字段 + Clone 传播

**Files:**
- Modify: `crates/wjsm-runtime/src/lib.rs`

**Why:** free-list 基础设施。

**Impact:** 新字段默认空 Vec，无行为变化直至 Task 2–4 接线。

- [ ] **Step 1:** 在 `RuntimeState` 中 `native_callable_free_slots` 旁增加：

```rust
continuation_free_slots: Arc<Mutex<Vec<u32>>>,
combinator_context_free_slots: Arc<Mutex<Vec<usize>>>,
```

- [ ] **Step 2:** 在 `RuntimeState::new`（或等价构造）初始化：

```rust
continuation_free_slots: Arc::new(Mutex::new(Vec::new())),
combinator_context_free_slots: Arc::new(Mutex::new(Vec::new())),
```

- [ ] **Step 3:** 在 `impl Clone for RuntimeState` 中 clone 上述两个 `Arc` 字段（与 `native_callable_free_slots` 相同模式）。

- [ ] **Step 4:** 验证

```bash
cargo check -p wjsm-runtime
```

Expected: Finished, 0 errors.

- [ ] **Step 5:** Commit

```bash
git add crates/wjsm-runtime/src/lib.rs
git commit -m "feat(runtime): add continuation and combinator context free-slot lists"
```

---

### Task 2: Continuation 编码统一 + free-list + 移除 retain

**Files:**
- Modify: `crates/wjsm-runtime/src/host_imports/async_fn.rs`
- Modify: `crates/wjsm-runtime/src/runtime_microtask.rs`
- Modify: `crates/wjsm-runtime/src/runtime_async_fn.rs`（若 `enqueue` 路径仍用裸下标）

**Repair Track:** 根因 = `async_function_start` 传裸 `cont_handle` 与 `decode_object_handle` 不一致；`retain` 破坏 handle 稳定性。

**Verification:** `cargo nextest run -p wjsm-runtime -E 'test(side_table)'`（Task 5 后）； interim: `cargo nextest run -E 'test(happy__async)'` 或现有 async fixture 子集。

- [ ] **Step 1:** `async_fn.rs` — `async_function_start` 中 `Microtask::AsyncResume` 的 `continuation` 改为：

```rust
continuation: value::encode_object_handle(cont_handle),
```

（`cont_handle` 仍为 `c_table.len() as u32` push 前的长度。）

- [ ] **Step 2:** 新增辅助函数（放在 `runtime_async_fn.rs` 或 `runtime_microtask.rs`）：

```rust
pub(crate) fn recycle_completed_continuations(state: &RuntimeState) {
    let mut table = state
        .continuation_table
        .lock()
        .expect("continuation table mutex");
    let mut free = state
        .continuation_free_slots
        .lock()
        .expect("continuation free slots mutex");
    for (idx, entry) in table.iter_mut().enumerate() {
        if entry.completed {
            entry.completed = false;
            entry.fn_table_idx = 0;
            entry.outer_promise = value::encode_undefined();
            entry.captured_vars.clear();
            free.push(idx as u32);
        }
    }
}
```

- [ ] **Step 3:** `runtime_microtask.rs` — 将 drain 末尾块：

```rust
c_table.retain(|entry| !entry.completed);
```

替换为：

```rust
recycle_completed_continuations(ctx.state_mut());
```

- [ ] **Step 4:** `async_fn.rs` — `continuation_create`（及 `async_function_start` push）分配改为优先 free-list：

```rust
let handle = if let Some(slot) = state.continuation_free_slots.lock().expect("...").pop() {
    let h = slot as usize;
    if h < table.len() {
        table[h] = ContinuationEntry { /* ... */ };
        slot
    } else {
        // fall through push
    }
} else {
    let h = table.len() as u32;
    table.push(ContinuationEntry { /* ... */ });
    h
};
```

（实现时合并为清晰分支，避免重复 `ContinuationEntry` 构造。）

- [ ] **Step 5:** 验证 + commit

```bash
cargo nextest run --workspace 2>&1 | tail -20
git add crates/wjsm-runtime/src/host_imports/async_fn.rs crates/wjsm-runtime/src/runtime_microtask.rs crates/wjsm-runtime/src/runtime_async_fn.rs
git commit -m "fix(runtime): continuation handle encoding and free-list recycle (no retain)"
```

---

### Task 3: promise_table GC 槽清理

**Files:**
- Modify: `crates/wjsm-runtime/src/runtime_builtins.rs`

**Why:** 未 mark 的 Promise 对象死亡后释放 `promise_table` 槽。

**Verification:** Task 5 单元测试 `promise_slot_cleared_after_gc`

- [ ] **Step 1:** 在 `trigger_gc` 末尾、`alloc_counter` 重置之前，添加函数调用：

```rust
sweep_dead_promise_slots(
    caller,
    &mark_snapshot,
    obj_table_count as usize,
);
```

- [ ] **Step 2:** 在同文件实现：

```rust
fn sweep_dead_promise_slots(
    caller: &mut Caller<'_, RuntimeState>,
    mark_snapshot: &[u64],
    obj_table_count: usize,
) {
    let mut table = caller
        .data()
        .promise_table
        .lock()
        .expect("promise table mutex");
    if table.len() < obj_table_count {
        table.resize_with(obj_table_count, PromiseEntry::empty);
    }
    for handle_idx in 0..obj_table_count {
        let word_idx = handle_idx / 64;
        let bit_idx = handle_idx % 64;
        let marked = word_idx < mark_snapshot.len()
            && (mark_snapshot[word_idx] & (1u64 << bit_idx)) != 0;
        if !marked && table[handle_idx].is_promise {
            table[handle_idx] = PromiseEntry::empty();
        }
    }
}
```

- [ ] **Step 3:** `cargo check -p wjsm-runtime`

- [ ] **Step 4:** Commit

```bash
git add crates/wjsm-runtime/src/runtime_builtins.rs
git commit -m "feat(runtime): sweep promise_table slots for unmarked handles in GC"
```

---

### Task 4: combinator_context free-list + try_recycle

**Files:**
- Modify: `crates/wjsm-runtime/src/runtime_combinators.rs`

- [ ] **Step 1:** 改写 `create_combinator_context` — 优先 `combinator_context_free_slots.pop()`，复用槽时重置 `CombinatorContext { result_promise, result_array, remaining, settled: false }`。

- [ ] **Step 2:** 添加：

```rust
pub(crate) fn try_recycle_combinator_context(state: &RuntimeState, context: usize) {
    let mut contexts = state.combinator_contexts.lock().expect("combinator context mutex");
    let Some(entry) = contexts.get(context) else { return };
    if !entry.settled || entry.remaining != 0 {
        return;
    }
    drop(entry);
    if let Some(slot) = contexts.get_mut(context) {
        *slot = CombinatorContext {
            result_promise: value::encode_undefined(),
            result_array: value::encode_undefined(),
            remaining: 0,
            settled: false,
        };
    }
    state
        .combinator_context_free_slots
        .lock()
        .expect("combinator context free slots mutex")
        .push(context);
}
```

- [ ] **Step 3:** 在 `handle_combinator_reaction` 中，当 `decrement_combinator_remaining` 返回 `Some` 且已 `mark_combinator_settled` 的路径末尾，在 `recycle_native_callable(state, handler)` 之后调用 `try_recycle_combinator_context(state, context)`。

- [ ] **Step 4:** `cargo nextest run --workspace` + commit

```bash
git add crates/wjsm-runtime/src/runtime_combinators.rs
git commit -m "feat(runtime): combinator context free-list recycling"
```

---

### Task 5: NativeCallable PromiseResolving 回收（最小安全点）

**Files:**
- Modify: `crates/wjsm-runtime/src/runtime_promises.rs`
- Modify: `crates/wjsm-runtime/src/host_imports/promise.rs`（若 resolving 在 settle 后仍可达）

- [ ] **Step 1:** 在 `settle_promise` 成功写入 settled state 后，扫描本次 settlement 是否使某 promise 终结；若 `constructor_resolver` 关联的 resolve/reject native callables 仅服务于该 promise，在 settle 后 `recycle_native_callable`（需读 `NativeCallable::PromiseResolvingFunction` 的 `promise` 字段匹配）。

- [ ] **Step 2:** 保守策略：仅在 `PromiseResolvingFunction` handler 被调用且 `already_resolved` 置 true **之后** recycle 该 handler（在 host import resolve/reject 路径），避免 settle 前回收。

- [ ] **Step 3:** 创建 `crates/wjsm-runtime/tests/side_table_lifecycle.rs`：
  - 测试 combinator：多次 `Promise.all` 逻辑（可用内联 Rust 构造 state + mock settle，或调用现有 host 路径的集成测）
  - 测试 promise GC 槽：分配 promise object + `is_promise`，模拟 unmarked handle，`sweep_dead_promise_slots` 后 `is_promise == false`

- [ ] **Step 4:**

```bash
cargo nextest run -p wjsm-runtime -E 'test(side_table)'
cargo nextest run --workspace
```

Expected: 0 failures.

- [ ] **Step 5:** Commit

```bash
git add crates/wjsm-runtime/
git commit -m "feat(runtime): recycle promise resolving native callables; side table lifecycle tests"
```

---

## Risks & Retirement

| 风险 | 回滚 |
|------|------|
| Promise 清槽过早 | revert Task 3；加「仅当 state 已 settled」守卫 |
| Continuation 编码 | revert Task 2 单提交 |

**Baseline-sync 问题（完成后）:** `docs/aegis/baseline/` 是否记录「侧表 handle-stable + GC promise sweep」为运行时不变量。

---

## Execution Handoff

Plan complete and saved to `docs/aegis/plans/2026-06-07-runtime-side-table-lifecycle.md`.

**1. Subagent-Driven（推荐）** — 每 task 独立 subagent + 任务间 review  

**2. Inline Execution** — 本会话按 Task 1→5 连续实现  

请选择执行方式；spec 已写入 `docs/aegis/specs/2026-06-07-runtime-side-table-lifecycle-design.md`，INDEX 已更新。