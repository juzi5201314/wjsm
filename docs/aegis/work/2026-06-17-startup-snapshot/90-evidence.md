# Startup Snapshot 执行提测

- Parent plan: `docs/aegis/plans/2026-06-17-startup-snapshot.md`
- Branch: `startup-snapshot-execution`
- Result: P0–P6 核心实施完成；P7（扩展 bench + 默认开启策略）**未完成**；P8 文档与 ADR 已对齐实现（INDEX 含 ADR 链接）。

## TodoCheckpointDraft

当前 todo：无（checkpoint 见 `20-checkpoint.md`）。

已完成：

- P0: 阶段计时与开关基线
- P1: 拆分 wasm bootstrap/function-props
- P2: `function_props_base` 布局
- P3: 35 个 primordial 字符串（224–493，`USER_STRING_START=493`）
- P4: snapshot 格式 v2 + 58 种 `SnapshotNativeCallable`
- P5: capture/restore + decode 校验
- P6: cache（wasm+ABI 键）+ execute 接入 + `tests/startup_snapshot.rs` on/off/warm 一致性

未完成 / 延后：

- P7: `bench_execute_phases` 未覆盖 snapshot build/decode/restore/warm execute；**默认仍为 opt-in**（`arr_proto_table_base` 未统一前不默认开）
- P6 可选：`snapshot_cache_hit_skips_builder`、`snapshot_cache_abi_mismatch_rebuilds_once` 专项测试

## 已知限制

- **Seed `arr_proto_table_base` 不一致**：snapshot 开启时可能 `indirect call type mismatch`。需 per-module snapshot（已用 wasm bytes hash）或导出统一 `arr_proto_table_base` 后再考虑默认开启。

## 提测命令

```bash
cargo nextest run --workspace
cargo nextest run -p wjsm-runtime -E 'test(startup_snapshot)'
cargo test -p wjsm-runtime startup_snapshot_format -- --nocapture
WJSM_STARTUP_SNAPSHOT=1 cargo run -- eval "console.log('hello')"
```