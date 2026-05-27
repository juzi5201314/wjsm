# Async Suspend 绑定保存优化：Liveness Analysis

日期: 2026-05-26
状态: 待审批
范围: wjsm-semantic 的 async lowering 流程优化

## 概述

当前每次 async suspend（`await`、`yield`、`for-await-of`）时无条件调用
`async_visible_binding_names()` 收集作用域链中所有可见变量，全部通过
`ContinuationSaveVar`/`ContinuationLoadVar` 保存/恢复。大量变量在 suspend
点之后不再被引用（已死），却仍占用 slot 和 runtime 内存。

本设计通过**推迟 save/restore 发射 + 标准后向 liveness 分析**，只保存
在 resume 后会被读取的变量，实现精准的绑定性活跃分析。

## 当前实现

```
lower_await_expr():
  saved = async_visible_binding_names()    // 所有可见变量
  emit_save_async_bindings(saved)          // 立即发射 LoadVar → SaveVar
  emit Suspend
```

`async_visible_binding_names()` 从当前作用域向上走到函数作用域，
收集所有声明的变量名（排除 `$env`、`$state` 等内部变量）。

## 目标设计

### 两遍 Lowering

**第一遍（现有 lowering 流程修改）**：
遇到 `await`/`yield`/`for-await-of` 时不发出 save/restore 指令，
改为记录 `PendingSuspend`：

```rust
struct PendingSuspend {
    suspend_block: BasicBlockId,   // Suspend 指令所在 block
    resume_block: BasicBlockId,    // resume 后执行起始 block
    visible_bindings: Vec<String>, // async_visible_binding_names() 的结果
    promise: ValueId,              // Promise 值
    state: u32,                    // 状态号
}
```

Suspend 指令仍然立即发出（连同 Jump terminator），但 save/restore 延迟。

**第二遍（函数体 lowering 完成后）**：
1. 在干净 IR（无 save/restore 的 CFG）上运行后向 liveness 分析
2. 对每个 `PendingSuspend`，`live = visible_bindings ∩ liveness[suspend_point]`
3. 在 `suspend_block` 的 Suspend 指令之前插入 save 指令（仅 live 变量）
4. 在 `resume_block` 开头插入 restore 指令（仅 live 变量）

### Liveness 分析

标准后向迭代数据流分析：

```
对每个基本块 B:
  use[B]  = { 在 B 中先读后写的变量 }
  def[B]  = { 在 B 中被写的变量 }

  // 排除 async 内部变量（$env, $state, $resume_val, $is_rejected,
  // $promise, $generator, $closure_env）

迭代至不动点:
  live_out[B] = ∪ { live_in[S] | S ∈ successors(B) }
  live_in[B]  = use[B] ∪ (live_out[B] - def[B])
```

**CFG 构建特殊处理**：
- Suspend block 的逻辑 successor 是 `resume_block`（不是 terminator 中的 `Jump(continue_block)`），因为 Suspend 后函数退出，resume 时从 resume_block 继续执行
- 需要在构建 CFG 时为含 Suspend 的 block 用对应的 resume_block 替换 terminator successor

**Suspend 点的活跃集**：
Suspend 指令不读写用户变量，其活跃集 = `live_out[suspend_block]`（即 resume 后需要的变量集合）。

### Slot 分配

推迟 save/restore 后，slot 分配也推迟到第二遍。由于只给活跃变量分配 slot：

- Slot 0-3 保留（state, is_rejected, promise, env）
- 用户变量从 slot 4 开始
- 每个唯一活跃绑定分配一个槽位（和现在一样，但死变量不分配）
- `total_slots` = 4 + unique_active_bindings_count
- 自然实现 slot compaction，无浪费

### 受影响的调用点

全部在 `wjsm-semantic`：

| 文件 | 函数 | 修改内容 |
|------|------|---------|
| `lowerer_async_eval.rs` | `lower_await_expr` | 推迟 save/restore，记录 PendingSuspend |
| `lowerer_async_eval.rs` | `lower_yield_expr` (async generator yield) | 同上 |
| `lowerer_stmt.rs` | `lower_for_await_of` | 同上 |
| `lowerer_async_eval.rs` | 新增 `compute_liveness` | CFG 构建 + 后向 liveness |
| `lowerer_async_eval.rs` | 新增 `resolve_pending_suspends` | 为活跃变量发射 save/restore |
| `lowerer_core.rs` | `Lowerer` 结构体 | 新增 `pending_suspends: Vec<PendingSuspend>` |
| `lowerer_arrows.rs` | async arrow lowering 完成处 | 调用 `resolve_pending_suspends` |
| `lowerer_functions.rs` | async fn expr lowering 完成处 | 同上 |
| `lowerer_function_decls.rs` | async fn decl lowering 完成处 | 同上 |

### 不变更的部分

- IR 层（`wjsm-ir`）：指令不变，`Suspend`/`ContinuationSaveVar`/`ContinuationLoadVar` 不变
- 后端（`wjsm-backend-wasm`）：不变
- 运行时（`wjsm-runtime`）：不变（slot 数可能更少，但协议不变）

## 正确性论证

- **保守性**：liveness 分析对分支取并集（一个变量在任一分支上活即视为活），宁可多保存不遗漏
- **async 内部变量排除**：`$env`、`$state`、`$resume_val`、`$is_rejected`、`$promise`、`$generator` 等通过 `is_async_internal_binding()` 排除，不受 liveness 影响
- **现有测试覆盖**：~28 个 async fixture 测试。若 liveness 误判活变量为死，resume 后变量为 `undefined`，现有测试会失败
- **IR snapshot 测试**：`.ir` 文件会反映 save/restore 指令数量变化，需手动更新

## 测试策略

1. **现有 async fixture 测试**：`cargo nextest run -E 'test(happy__async_)'`，全部通过
2. **IR snapshot 测试**：`cargo test -p wjsm-semantic`，手动更新受影响的 `.ir` 文件
3. **全量 fixture 测试**：`cargo nextest run`，确保无回归
4. **不需新增专用测试**：正确性由现有 fixture 覆盖

## 风险与降级

- **风险**：liveness 分析实现 bug 导致活变量未保存 → resume 后变量为 `undefined` → 功能测试失败
- **缓解**：liveness 分析是标准编译器算法，实现简单（~100 行），可独立单元测试
- **降级**：如有问题，可临时恢复全量保存行为（`resolve_pending_suspends` 中直接用 `visible_bindings` 而非 `live ∩ visible`）
