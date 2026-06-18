# Startup Snapshot 执行 checkpoint

- Parent plan: `docs/aegis/plans/2026-06-17-startup-snapshot.md`
- Branch: `startup-snapshot-execution`（用户要求不使用 worktree）
- Requested outcome: 完整实现 relocatable startup snapshot，保持 runtime async-only API、RuntimeState 扁平结构、fixture 输出不变。
- Scope: P3-P8；P0-P2 已在前序会话完成。
- Non-goals: 不快照 Wasmtime Instance/Store；不捕获用户运行态 async/host/shared 状态；不重组 RuntimeState。

## TodoCheckpointDraft

当前 todo：P3 固定 primordial 字符串表与 ABI hash 输入（in progress）。

已完成：
- P0 阶段计时与开关基线（prior session）。
- P1 拆分 wasm bootstrap/function-props（prior session + 本会话修复 eval 回归）。
- P2 退休函数属性隐含 handle 布局（本会话）。
- GC 变量 rooting 修复（本会话 pre-existing bug）。

阻塞：无。

## 计划偏差（已采纳）

- P1 relocation 跨入 runtime + backend helpers（与 bootstrap 拆分耦合）。
- P2 用集成 fixture 替代合成单测、新增 async 原型 root、未加 WasmEnv 字段（get_export 已够，P5 再加）。
- GC 变量 rooting 修复不在原 P0-P8 内，已用户确认立即修复。
- P3 计划写 `crates/wjsm-backend-wasm/src/constants.rs` 实际不存在，常量改在 `crates/wjsm-ir/src/constants.rs` 维护。

## DriftCheckDraft

- 服务原始 task intent：是。
- 兼容边界：P3 只增加固定 data section 字符串；不改变对象堆布局；不影响 fixture 输出。
- 当前无新增 owner/fallback；后续 snapshot format/cache 会新增独立 owner 文件。
- Decision: continue。

## ResumeStateHint

若中断：从 P3 开始，需重新确认 `wjsm-ir/src/constants.rs` 与 `compiler_module.rs` 的当前字符串预写区未变。
