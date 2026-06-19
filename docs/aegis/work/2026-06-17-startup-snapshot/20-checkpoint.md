# Startup Snapshot 执行 checkpoint

- Parent plan: `docs/aegis/plans/2026-06-17-startup-snapshot.md`
- Branch: `startup-snapshot-execution`
- Status: P0–P6 完成；P7 bench/默认策略未完成；P8 文档与 ADR 已同步。

## TodoCheckpointDraft

当前 todo：无（见 `90-evidence.md`）。

已完成：P0–P6（含 decode 加固、restore 失败重新实例化、capture 失败日志、cache wasm+ABI 键）。

## ResumeStateHint

若继续 P7：扩展 `bench_execute_phases` 与 snapshot-on 路径；默认开启仍待 `arr_proto_table_base` 统一。