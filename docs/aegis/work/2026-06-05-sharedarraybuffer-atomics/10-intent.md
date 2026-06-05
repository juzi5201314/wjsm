# TaskIntent — SharedArrayBuffer + Atomics

**Outcome:** 按 `docs/aegis/plans/2026-06-05-sharedarraybuffer-atomics.md` 完成 §25.2/§25.4 可观测行为。

**Scope:** wjsm-runtime owner `shared_buffer.rs`、SAB/Atomics wiring、fixtures、IR/backend registry。

**Non-goals:** 完整 §29 candidate execution graph；test262 全量。

**Baseline:** `docs/aegis/specs/2026-06-05-sharedarraybuffer-atomics-design.md`、计划 Repair/Retirement track。

**Resume:** Task 1–3 声称完成但仓库无 `shared_buffer.rs`；`SharedArrayBufferConstructor` 仍 `undefined`。从 Task 2 重做并继续 Task 4–11。