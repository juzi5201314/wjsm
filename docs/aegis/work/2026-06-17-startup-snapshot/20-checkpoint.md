# Startup Snapshot 执行 checkpoint

- Parent plan: `docs/aegis/plans/2026-06-17-startup-snapshot.md`
- Branch: `startup-snapshot-execution`
- Status: P0–P8 完成；startup snapshot 默认开启，opt-out / corrupt cache rebuild / warm bench 已验证。

## TodoCheckpointDraft

当前 todo：无（见 `90-evidence.md`）。

已完成：P0–P8（含 decode 加固、restore 失败重新实例化、capture 失败 debug 日志、cache wasm+ABI 键、默认开启策略、P7 release bench、文档与 ADR 同步）。

## ResumeStateHint

无需继续 P7；后续只在新增 builtin / NativeCallable / primordial string / Array.prototype 方法表时维护 ABI hash 与专项回归。