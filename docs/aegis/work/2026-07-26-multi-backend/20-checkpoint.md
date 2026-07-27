# TodoCheckpointDraft

- 当前 todo：P5 Streams 全家迁 builtins。
- 并行候选：P7 Modules/CJS；因没有匹配的实现型子代理，由主线程在 P5 后执行。
- 已完成：P0 基线；P1 Promise combinators；P2 render/json；P3 core；P4 Atomics/SAB；P1–P4 review 修复。
- 证据：`cargo nextest run -p wjsm-builtins -p wjsm-host-wasm` → 145 passed；核心与 Atomics 定向夹具 → 8 passed；`cargo check -p wjsm-host-wasm -p wjsm-builtins` → OK。
- 阻塞：无。
- 下一步：建立 Streams/Modules 类型、调用与 owner 边界；执行 P5。

## ResumeStateHint

恢复时先读本文件、`10-intent.md` 与父计划 P5–P8 段；检查 todo；从 P5 当前实现状态继续，不重做 P1–P4。

## DriftCheckDraft

- 原始意图：一致。
- 兼容边界：一致；未增加 fallback 或双 owner。
- 退役轨道：旧 `runtime_values::strict_eq`、旧 `runtime_values::to_number` 与数组 accessor 拒绝路径已删除。
- 决策：continue。
