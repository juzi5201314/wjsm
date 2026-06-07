# 运行时侧表生命周期补全设计规格

**状态**: 待审批  
**日期**: 2026-06-07  
**范围**: `wjsm-runtime` 中 `continuation_table`、`promise_table`、`native_callables`、`combinator_contexts` 的长生命周期内存占用  
**权威来源**: `docs/superpowers/specs/2026-05-26-async-audit-refactor-design.md` §Perf 2 / §5；`docs/aegis/specs/2026-06-02-unified-async-execution-model-design.md`（async-only drain/GC 安全点）  
**ADR 信号**: 侧表回收策略与 GC 协调 — owner=`runtime_builtins`（GC sweep）、`runtime_microtask`（drain 安全点）、`runtime_promises`/`runtime_combinators`（分配/回收 API）；不变量=**handle 下标稳定，禁止压缩导致索引失效**

---

## 1. 问题陈述

### 1.1 背景核实（2026-06-07）

对「四张侧表 Vec 只 push 不 remove」的审计结论**部分过时**：

| 侧表 | 当前行为 | 是否仍构成长期泄漏 |
|------|----------|-------------------|
| `continuation_table` | `completed` + `drain_microtasks_async` 末尾 `retain(!completed)` | **是**（`retain` 与 handle 语义冲突，见 §2.1）；且回收依赖 `outer_promise` settled |
| `promise_table` | 按 object handle `resize_with`，无槽清理 | **是** |
| `native_callables` | `native_callable_free_slots` 复用槽位；`Vec` 不收缩 | **部分**（槽复用不完整，见 §2.3） |
| `combinator_contexts` | 仅 `push` + `settled` 标记 | **是** |
| `pending_unhandled_rejections` | 已实现 | 非侧表条目回收，**不在本 spec 范围** |

**影响场景**: 单次 CLI `wjsm run` 进程退出即释放；**长时间运行的 agent/共享 `RuntimeState`/循环执行脚本**下，`promise_table` 与 `combinator_contexts` 单调增长，构成实质内存泄漏。

### 1.2 目标（TaskIntentDraft）

- **Outcome**: 侧表在逻辑对象死亡或 combinator/continuation 完成后，槽位可复用或清空；`promise_table` 与 mark-sweep GC 同步清理未存活 Promise 对象对应槽位。
- **Success evidence**: 新增压力 fixture 或 runtime 单元测试证明表内「活跃条目数」有上界或随 GC/drain 回落；`cargo nextest run --workspace` 无回归。
- **Stop condition**: 四表策略按 §3 落地且验证通过；不引入 handle 失效或 UAF。
- **Non-goals**: 不改变 ECMAScript Promise/async 语义；不收缩 `native_callables`/`promise_table` 的 `Vec` 容量（仅逻辑槽回收）；不做全表 compaction；不修改 `wjsm-backend-wasm` / `wjsm-semantic`。

### 1.3 BaselineReadSetHint

- `docs/superpowers/specs/2026-05-26-async-audit-refactor-design.md` §5、§Perf 2  
- `crates/wjsm-runtime/src/runtime_builtins.rs` — `trigger_gc`  
- `crates/wjsm-runtime/src/runtime_microtask.rs` — `drain_microtasks_async`  
- `crates/wjsm-runtime/src/runtime_promises.rs` — `create_native_callable` / `recycle_native_callable`  
- `crates/wjsm-runtime/src/runtime_combinators.rs`  
- `crates/wjsm-runtime/src/host_imports/async_fn.rs` — continuation 编码  

### 1.4 ImpactStatementDraft

| 层 | 影响 |
|----|------|
| `RuntimeState` | 新增 `continuation_free_slots`、`combinator_context_free_slots`（与现有 `native_callable_free_slots` 同模式） |
| `trigger_gc` | sweep 后清理未标记 object handle 的 `promise_table` 槽 |
| `drain_microtasks_async` | 移除 `continuation_table.retain`；改为 tombstone + free-list |
| `async_fn` / `runtime_async_fn` | 统一 continuation 为 `encode_object_handle` 语义 |
| 测试 | 新增 `fixtures/happy/` 或 `crates/wjsm-runtime/tests/` 压力/回归用例 |

**兼容性边界**: 现有 fixture 的 stdout/语义不变；NaN-boxed promise/object handle 编码不变；GC 仅在既有 `trigger_gc` / 分配阈值路径触发。

---

## 2. 根因与设计约束

### 2.1 Handle 下标稳定（硬约束）

以下表的**下标**会写入 NaN-boxed 值或存入 reaction/native callable 载荷，**禁止**对整张 `Vec` 做 `retain`/压缩导致下标漂移：

- `promise_table[object_handle]`
- `continuation_table[handle]`（`continuation_create` → `encode_object_handle(handle)`）
- `combinator_contexts[context_idx]`（`NativeCallable::PromiseCombinatorReaction { context, ... }`）
- `native_callables[idx]`（`encode_native_callable_idx`）

**允许**: 按槽 tombstone（`PromiseEntry::empty()`）、free-list 复用槽、逻辑字段 `completed`/`settled`。

**禁止**: `Vec::retain` 删除中间元素（当前 `continuation_table.retain` 违反此约束）。

### 2.2 Continuation 编码不一致（须修复）

| 路径 | continuation 传值 |
|------|-------------------|
| `async_function_start` | `cont_handle as i64`（裸下标） |
| `continuation_create` | `encode_object_handle(handle)` |
| `resume_async_function_async` | `decode_object_handle(continuation)` |

裸下标与 object-handle 解码混用会导致 `completed` 标记与表项错位。**本 spec 要求**: 所有 continuation 微任务与 WASM 边界统一为 **`encode_object_handle` / `decode_object_handle`**（与 `continuation_create` 一致）。

### 2.3 2026-05-26 设计勘误

原设计 §Perf 2 写「NativeCallable: 表过大时做 compaction」；同文档 §5 与实施计划 Task 4.4 已改为 **free-list、禁止 compaction**。**以 §5 / Task 4.4 为准**，本 spec 不实现 compaction。

---

## 3. 方案（推荐：Index-Stable Free-List + GC 槽清理）

### 3.1 `promise_table` — GC 同步清槽

在 `trigger_gc` 完成 mark 且 live object 压缩**之后**（主线程、与现有 GC 同安全点）：

对每个 `handle_idx in 0..obj_table_count`：

- 若 mark bit **未**设置，且 `promise_table[handle_idx].is_promise`，则 `table[handle_idx] = PromiseEntry::empty()`。

**根集**: 沿用现有 `trigger_gc` roots（shadow stack、IR 函数对象、timer、closure env、module namespace 等）。Promise **对象**若仍被引用，其 object handle 会被 mark；仅当 Promise 对象死亡后清槽。

**不收缩** `promise_table.len()`（与 object table 最大 handle 对齐）。

### 3.2 `continuation_table` — Free-list + `completed`

1. `RuntimeState` 增加 `continuation_free_slots: Arc<Mutex<Vec<u32>>>`。
2. **分配**: `create` 时优先 `free_slots.pop()`，否则 `push` 新条目；返回 `encode_object_handle(handle)`。
3. **完成**: `resume_async_function_async` 在 `outer_promise` settled 时设 `completed = true`（保持）。
4. **回收（drain 末尾）**: 遍历表，对 `completed == true` 的槽：`captured_vars` 清空、`completed` 保持 false 语义下的 tombstone，**索引 push 到 `continuation_free_slots`**；**不** `retain`。
5. **统一编码**: `async_function_start` 的 `Microtask::AsyncResume.continuation` 改为 `encode_object_handle(cont_handle)`。

### 3.3 `combinator_contexts` — Free-list

1. `combinator_context_free_slots: Arc<Mutex<Vec<usize>>>`。
2. `create_combinator_context`: 优先复用 free 槽，否则 `push`。
3. 当 `mark_combinator_settled` 且 `remaining == 0`（在 `decrement_combinator_remaining` 返回 `Some` 的最终路径），且该 context 上所有 combinator reaction 的 `native_callable` 已 `recycle_native_callable` 后：槽逻辑重置，`idx` 入 `combinator_context_free_slots`。
4. 实现细节：`CombinatorContext` 可增加 `recyclable: bool` 或在 `settled && remaining==0` 时由 `handle_combinator_reaction` 末次调用触发 `try_recycle_combinator_context(context)`。

### 3.4 `native_callables` — 补全 recycle 面

保持现有 free-list；**扩展** `recycle_native_callable` 调用至：

- `PromiseResolvingFunction`：`settle_promise` 之后 resolve/reject 函数不再可被调用时回收（需确认 `already_resolved` 与 then 链无悬挂引用；通常 settle 后即可回收该 capability 的 resolve/reject native callable）。
- 已有 combinator reaction 路径保持。

**不**收缩 `native_callables` Vec。

### 3.5 安全点顺序

与 2026-05-26 一致：

1. **drain 结束**: continuation tombstone + free-list（无压缩）。
2. **GC 周期**: promise 槽清理。
3. combinator/native 回收发生在 settle/decrement **之后**的微任务或同步 handler 返回点，不与其他线程并发（RuntimeState 仍在单线程 store 模型下）。

---

## 4. 备选方案（未采纳）

| 方案 | 优点 | 缺点 | 结论 |
|------|------|------|------|
| A. 仅文档声明「CLI 无泄漏」 | 零代码 | 不解决 agent/长驻 | 拒绝 |
| B. 全局 `retain` 压缩四表 | 实现简单 | UAF / handle 失效 | 拒绝 |
| C. 本 spec：free-list + GC 清槽 | 与 2026-05-26 §5 一致 | 需修 continuation 编码 | **采纳** |

---

## 5. 验证策略

1. **回归**: `cargo nextest run --workspace`
2. **单元/集成**:
   - GC：构造 object + promise，丢引用，`trigger_gc`，断言 `promise_table[h].is_promise == false`
   - Combinator：循环 `Promise.all` N 次，断言 `combinator_contexts` 活跃计数不随 N 线性永久增长（允许 Vec len ≥ 峰值活跃数，free 槽增加）
   - Continuation：多 async 函数交错 await，无错位（现有 async fixture + 可选新 fixture）
3. **可选**: `fixtures/happy/runtime_side_table_gc_stress.js`（循环 Promise + 周期性 GC 若暴露给 JS）

---

## 6. 风险与回滚

| 风险 | 缓解 |
|------|------|
| Promise 槽清理过早 | 仅清未 mark 的 handle；清前 `is_promise` 检查 |
| Continuation 编码变更 | 单提交内统一 async_function_start + 全路径 decode |
| Resolving function 过早 recycle | 在 settle 且 capability 逻辑终点回收；加单元测试 then 链 |

回滚：按任务 revert；无 schema 迁移。

---

## 7. 实施顺序

1. 修复 continuation 编码 + 移除 `retain` + continuation free-list  
2. `trigger_gc` promise 槽清理  
3. combinator free-list  
4. native resolving function recycle  
5. 测试与文档 INDEX/plan  

---

## 8. Product Risk Lens

- **Value**: 长驻运行时内存可预测，对齐 Phase 4 未完成项。  
- **Non-goals**: 不改为分代 GC、不暴露 JS 层 WeakRef 给侧表。  
- **Trade-offs**: `Vec` 容量可能仍偏大，但活跃槽可回收。  
- **Decision needed**: 无（方案 C 已选）；实施前用户审批本 spec。

---

## 9. Plan-Time Complexity Check

- **Better file boundary**: 回收辅助函数新建 `runtime_side_table_gc.rs`（可选）或集中在 `runtime_builtins.rs`（GC）+ `runtime_combinators.rs` + `runtime_promises.rs`。  
- **Recommendation**: GC 钩子在 `runtime_builtins.rs`；free-list API 与表字段在 `lib.rs` + 各 owner 模块 **edit-in-place**，避免再拆大文件。