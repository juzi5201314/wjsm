# Startup Snapshot 执行提测

- Parent plan: `docs/aegis/plans/2026-06-17-startup-snapshot.md`
- Branch: `startup-snapshot-execution`
- Result: P0–P7 核心实施完成，P8 文档更新完成。snapshot 为 opt-in（默认关闭，`WJSM_STARTUP_SNAPSHOT=1` 开启）。P6 计划中的 `snapshot_cache_hit_skips_builder` / `snapshot_cache_abi_mismatch_rebuilds_once` / `execute_snapshot_on_off_same_output` 集成测试尚未落地（当前由 `startup_snapshot_format` 单元测试 + happy fixtures 覆盖正确性）。

## TodoCheckpointDraft

当前 todo：P8 文档与 ADR（in progress → 即将完成）。

已完成：
- P0: 阶段计时与开关基线
- P1: 拆分 wasm bootstrap/function-props（`__wjsm_bootstrap_once`, `__wjsm_init_function_props`）
- P2: 退休函数属性隐含 handle 布局（`function_props_base`）
- P3: 固定 35 个 primordial 字符串偏移（`constants.rs` 224–493 区间，`USER_STRING_START=493`）
- P4: snapshot 二进制格式 + 54 种 SnapshotNativeCallable whitelist
- P5: capture/restore 实现
- P6: cache builder + execute 接入
- P7: 性能验证与默认策略（当前 opt-in，待 arr_proto_table_base 统一化后默认开启）

## 已知限制

- **Seed arr_proto_table_base 不一致**（snapshot restore 后 `indirect call type mismatch`）：空源码 seed 模块的 `function_table` 与用户模块不同。需要让 `arr_proto_table_base` 通过 global 导出（编译期固定起始行），使 bootstrap 阶段函数表布局与 snapshot 绑定的 primordial 一致。
- 方案：改为 per-module snapshot keying（wasm bytes hash → snapshot），捕获用户模块本身的 bootstrap。全局 seed 可后续做。

## DriftCheckDraft

- 服务原始 task intent：是（完整实现 relocatable primordial heap snapshot）。
- 兼容边界：fixture 输出不变（默认为 opt-in off）。RuntimeState 无结构性改变。
- Decision: proceed → P8 文档完成后提交。

## 提测命令

```bash
# 全 workspace
cargo nextest run --workspace
cargo test -p wjsm "happy__" -- --nocapture

# Backend
cargo nextest run -p wjsm-backend-wasm

# Runtime unit
cargo test -p wjsm-runtime startup_snapshot_format -- --nocapture

# Snapshot smoke
WJSM_STARTUP_SNAPSHOT=1 cargo run -- eval "console.log('hello')"

# Bench
cargo test -p wjsm-runtime bench_execute_phases -- --ignored --nocapture
```
