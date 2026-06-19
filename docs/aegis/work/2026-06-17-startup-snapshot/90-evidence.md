# Startup Snapshot 执行提测

- Parent plan: `docs/aegis/plans/2026-06-17-startup-snapshot.md`
- Branch: `startup-snapshot-execution`
- Result: P0–P8 完成；startup snapshot 已切换为默认开启，保留 `WJSM_STARTUP_SNAPSHOT=0/false/off` 显式关闭。

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
- P7: release bench 覆盖 snapshot build/decode/restore/warm execute；cache miss 先查 cache 再 instantiate，避免 miss 时重复实例化；默认开启策略落地
- P8: ADR / AGENTS / evidence 同步

未完成 / 延后：

- 无。

## 已知限制

- Snapshot 仍只覆盖 pristine runtime startup heap，不捕获用户对象、promise/timer/microtask/fetch/stream 活动状态、`SharedRuntimeState`、`eval_cache` 或 scheduler 状态。

## P7 性能证据

`cargo test -p wjsm-runtime --lib --release --no-run` 后直接运行 release libtest：

```text
target/release/deps/wjsm_runtime-62f61669d4d4e2b0 bench_execute_phases --ignored --nocapture

run 1:
BENCH full execute off       : 4.465112ms/each
BENCH full execute on warm   : 3.940045ms/each
BENCH real on cache lookup   : 4.901µs/each
BENCH real on decode         : 622ns/each
BENCH real on restore        : 14.64µs/each

run 2:
BENCH full execute off       : 3.951771ms/each
BENCH full execute on warm   : 3.863878ms/each
BENCH real on cache lookup   : 4.727µs/each
BENCH real on decode         : 611ns/each
BENCH real on restore        : 14.731µs/each

final verification run:
BENCH full execute off       : 4.161183ms/each
BENCH full execute on warm   : 3.935557ms/each
BENCH real on cache lookup   : 4.152µs/each
BENCH real on decode         : 613ns/each
BENCH real on restore        : 13.689µs/each
```

Root cause / fix：默认开启前，cache miss 在 `execute_with_writer_shared_inner` 中先实例化一次、发现 miss 后再实例化一次；现改为先查 cache，再实例化当前 run，miss 直接走 cold bootstrap。内存 cache 命中也不再重复 decode 验证，只在磁盘载入时校验。

## 提测命令

```bash
cargo nextest run --workspace
cargo nextest run -p wjsm-runtime -E 'test(startup_snapshot)'
cargo test -p wjsm-runtime startup_snapshot_format -- --nocapture
cargo run -- eval "console.log('hello')"
```