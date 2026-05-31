# JSON Implementation Plan (Corrected v4)

**Date**: 2026-05-31  
**Status**: Corrected after mandatory two-stage review + rust-style-guide enforcement + user-directed narrow exploration  
**Based on**: `docs/aegis/plans/2026-05-30-json-implementation.md` (v3) + subsequent discovery

## 根本问题（v3 计划的致命假设错误）

v3 计划的 Task 1~4 假设“把 semantic 层的 `builtin_call_signature` 参数数量改成 3/2”就能让 reviver/replacer/space 工作。

**这是错误的**：

- `builtin_call_signature` 返回值被 `wjsm-semantic/src/lowerer_async_eval.rs:1664` **严格当作 `min_js_args`** 使用：
  ```rust
  let (name, min_args) = builtin_call_signature(builtin);
  if call.args.len() < min_args {
      return Err(..., format!("{name} requires at least {min_args} argument"));
  }
  ```
- JSON.parse / stringify 按 ES §24.5.1 / §24.5.2 只有第一个参数是必填的（reviver、replacer、space 均为可选）。
- 把数量改成 3/2 会导致**所有正常单参数调用**在语义降低阶段直接失败（P0）。

此错误已被**强制两阶段审查（spec compliance + code quality）+ rust-style-guide 要求**在 Task 1 阶段捕获。原修改已回滚，semantic 层对这两个 builtin 必须永远保持 (1,1)。

## 窄探索关键发现（用户要求“再做一次更窄的探索”）

针对 `crates/wjsm-backend-wasm/src/compiler_builtins.rs` 进行窄聚焦探索（仅看 JSON 相关发射逻辑）：

**核心位置**（精确到行）：
```rust
// compiler_builtins.rs:138
Builtin::Fetch | Builtin::JsonStringify | Builtin::JsonParse => {
    let val = args
        .first()
        .with_context(|| format!("{builtin} expects 1 argument"))?;
    self.emit(WasmInstruction::LocalGet(self.local_idx(val.0)));
    let func_idx = self.builtin_func_indices.get(builtin).copied().unwrap_or(0);
    self.emit(WasmInstruction::Call(func_idx));
    ...
}
```

**发现**：
- 这个 arm 硬编码只 emit 第 1 个参数。
- 同文件内其他需要多参数的 builtin（CreateClosure、Eval、ScopeRecordAddBinding 等）都已经正确写 `args.get(0)`、`args.get(1)` 等。
- backend 中**不存在**任何独立的 “host arity / param count” 表格。参数数量完全由每个 match arm 硬编码决定。
- `builtin_func_indices` 来自 `HOST_IMPORT_SPECS`（host_import_registry.rs），因此正确的修改路径是：
  1. 改 registry 的 `type_idx`（json_stringify → 16，json_parse → 2）。
  2. 拆分 + 扩展上述 arm，让 JsonStringify emit 最多 3 个参数、JsonParse emit 最多 2 个参数，缺失的补 undefined。

**结论**：host 参数数量是**100% backend 责任**，semantic 层完全不该参与。

## 修正后的架构

- **Semantic 层**：`builtin_call_signature` 对 `JsonStringify` / `JsonParse` 永久保持返回 1（JS 最小必填参数数）。这是不可变的契约。
- **Backend 层**：唯一负责 host import 调用签名的地方。
  - `host_import_registry.rs` 的 `HOST_IMPORT_SPECS` 控制 WASM type index（已存在 Type 16 和 Type 2）。
  - `compiler_builtins.rs` 的发射逻辑控制实际 emit 几个参数（并在 IR 只提供部分参数时补 undefined）。
- Runtime / SIMD parser / reviver walk / 完整 stringify 实现**不受影响**（它们本来就期望正确的 host 签名）。

## 修正后的任务列表（大幅精简）

### 已退休的任务（不再执行）
- 原 Task 1（semantic 参数数量改 3/2）—— **永久退休**，已回滚并记录为计划缺陷。
- 原 Task 2、3、4 中任何涉及改 semantic `builtin_call_signature` 的部分 —— 全部作废。

### 新的最小 Backend Host Arity 任务（替代原 1-4）
**目标**：让 backend 能为 JSON builtins 正确传递最多 3/2 个参数给 host 函数。

1. **Task B1: 更新 Host Import Registry**
   - 文件：`crates/wjsm-backend-wasm/src/host_import_registry.rs`
   - 把 `json_stringify` 的 `type_idx` 改为 16（3 参数）
   - 把 `json_parse` 的 `type_idx` 改为 2（2 参数）
   - 验证：`cargo check -p wjsm-backend-wasm`

2. **Task B2: 拆分并扩展发射逻辑**
   - 文件：`crates/wjsm-backend-wasm/src/compiler_builtins.rs`
   - 把第 138 行的合并 arm 拆成三个独立 arm：
     - `Builtin::Fetch`：保持不变（只 emit 1 个）
     - `Builtin::JsonStringify`：emit 最多 3 个（value + replacer? + space?），缺失的 emit undefined
     - `Builtin::JsonParse`：emit 最多 2 个（text + reviver?），缺失的 emit undefined
   - 参考同文件中其他多参数 builtin 的写法（CreateClosure、Eval 等）
   - 验证：`cargo check -p wjsm-backend-wasm`

3. **Task B3: 端到端验证（在 runtime 实现就绪后）**
   - 结合后续 runtime 任务，手动或通过 fixture 验证 `JSON.stringify(value, replacer, space)` 和 `JSON.parse(text, reviver)` 能正确把可选参数传到 host。

### 后续任务（保持不变）
- 原 Task 5~9（SIMD 加速 parser + heap 构建 + reviver walk + delete helper）—— 继续
- 原 Task 10（接线到 timers_arrays.rs）—— 现在可以安全进行（因为 backend 签名已正确）
- 原 Task 11（完整 JSON.stringify 实现，包括 toJSON、replacer whitelist、space）—— 继续
- 原 Task 12~13（更新 fixtures + 最终验证）—— 继续

## 风险与偏差更新

- **已消除的风险**：semantic 层破坏所有 JSON 调用的 P0 风险（已通过审查 + 回滚消除）。
- **剩余偏差**（与原计划一致）：
  - SyntaxError 仍是字符串而非 Error 对象（runtime 限制）
  - 数组 reviver 返回 undefined 时写 undefined 而非创建 hole（dense array 限制）
  - 非字符串输入的 ToString 使用 `eval_to_string` 近似
  - space 字符串按 Unicode scalar 截断而非严格 UTF-16 code unit（极罕见边缘）

## 退休记录

| 旧内容 | 位置 | 处置 | 理由 |
|--------|------|------|------|
| Task 1（改 semantic 参数数量） | 原计划 v3 | **永久退休** | 违反 semantic min_js_args 契约，导致 P0 |
| 原 Task 2~4 中 semantic 相关部分 | 原计划 v3 | **作废** | 同上 |
| 合并的 Fetch\|Json* 发射 arm | compiler_builtins.rs:138 | **必须拆分** | 硬编码只支持 1 参数 |

## 下一步执行建议

1. 用户确认本修正计划（v4）。
2. 以本文件为权威依据，重新初始化 TodoWrite。
3. 按 `subagent-driven-development` 协议 + **rust-style-guide** 要求，启动修正后的任务执行（优先 Backend 两个小任务 B1、B2）。
4. 后续 SIMD / stringify 实现任务按原节奏继续。

---

**本计划已通过窄探索 + 强制审查验证，边界清晰、风险可控。**

建议直接以此文件作为后续执行的唯一权威计划。
