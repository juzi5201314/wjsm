# Startup Snapshot 执行 checkpoint

- Parent plan: `docs/aegis/plans/2026-06-17-startup-snapshot.md`
- Branch: `startup-snapshot-execution`（用户要求不使用 worktree）
- Requested outcome: 完整实现 relocatable startup snapshot，保持 runtime async-only API、RuntimeState 扁平结构、fixture 输出不变。
- Scope: P0-P8；review 因子代理 429 不可用改由主控自查 + 实现（用户指令）。期间发现并（经用户确认）修复 pre-existing GC 缺陷。
- Non-goals: 不快照 Wasmtime Instance/Store；不捕获用户运行态 async/host/shared 状态；不重组 RuntimeState。

## TodoCheckpointDraft

当前 todo：等用户决定——(a) 修 #2/#3 pre-existing bug；(b) 推进 snapshot 计划 P3-P8；(c) 提交当前成果。

已完成：
- P0 阶段计时与开关基线（prior session，review ✅）。
- P1 拆分 wasm bootstrap/function-props（prior session 实现 + 本会话修复 eval 回归）。
- P2 退休函数属性隐含 handle 布局（本会话实现）。
- **GC 变量 rooting 修复**（本会话，原 #1 容器跨 GC 损坏）：根因 = GC safepoint spill 只 root per-ValueId liveness 的 Handle 值；变量 local（StoreVar/LoadVar 是 liveness 空洞）不被 spill → 被 sweep。修复 = `compute_var_liveness` + `infer_value_and_var_ty`（每变量 ValueTy），`current_spill_locals` 补 spill「存活且 Handle」变量 local，`compute_max_spill_bytes` 计入。详见记忆 [[wjsm-gc-variable-rooting]]。

证据：
- P1：eval 导入 `__bootstrap_done/__function_props_done/__function_props_base`（全局 13/14/15），修复 `compile_eval_exports_entry_and_imports_runtime_state` + `happy__eval_super_prop`。
- P2：`roots.rs` 稳定 root 改 `function_props_base..+n` + 显式 async 原型 root；`push_value_roots` 函数值 handle 加 base；`GcContext::function_props_base()`；`handle_index_of` 统一函数→属性对象 handle 重定位（含 defineProperty 扩容写槽）。
- 回归 fixture：`gc_function_props_survive.js`、`gc_container_survives.js`，均用 git stash 反证「修复前失败、修复后通过」。
- 验证：backend 43/43；workspace 950/950；GC fixtures 0.355s 无超时。

阻塞：无。

## 计划偏差（已采纳）
- P1 relocation 跨入 runtime + backend helpers（与 bootstrap 拆分耦合）；P2 用集成 fixture 替代合成单测、新增 async 原型 root（计划遗漏）、未加 WasmEnv 字段（get_export 已够，P5 再加）。
- GC 变量 rooting 修复不在原 P0-P8 内——排查 P1/P2 时发现的 pre-existing 严重 GC 缺陷（自 a3f103b 起 spill 即 SSA-only），用户选择立即精确修复。

## 仍存在的 pre-existing bug（独立，未修，超范围）
- #2 函数 `Object.defineProperty` 超容量 8 扩容丢属性（clean HEAD 丢 .name）。
- #3 computed-key（`o["p"+i]`）多属性丢失（20 次迭代不触发 GC，非 GC 问题）。

## DriftCheckDraft
- 服务原始 task intent：是。
- 兼容边界：GC 修复只增加 rooting（更保守）；标量变量被 ValueTy 过滤；eval/memory 变量不受影响；无 runtime/sweeper 改动；fixture 输出不变（+2 回归）。
- canonical owner：`current_spill_locals` 是 spill 集合唯一来源（12 callsites 共用），变量 rooting 在此补齐，无 fallback/重复 owner。
- Decision: 暂停，报告并确认下一步。
