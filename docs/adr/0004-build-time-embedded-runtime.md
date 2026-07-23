# ADR 0004: Build-Time Embedded Runtime

**Status**: Workspace 全测试通过。P0/P1/P2.0-P2.5/P2.7/P3.0/P4 实施落地。运行时磁盘 startup cache 已退役。Normal mode 下 10 个 helper（obj_new/obj_get/obj_set/obj_delete/arr_new/elem_get/elem_set/string_eq/to_int32/get_proto_from_ctor）已从 wjsm_support import；bootstrap 阶段函数（wjsm_bootstrap_once / wjsm_init_function_props）保持 user wasm 内联（仅启动时调用一次，无 wasmtime compile 收益）。Eval mode 仍走 inline helper 路径（compiled eval 无独立 support instance）。当前等价于 ADR 0003 的 snapshot 能力上**改为 build-time 固化通道 + support module 双 instance**，不再依赖客户机器缓存。ManagedHeap / shared memory64 / engine fingerprint 与 support ABI 对齐见 [ADR 0010](0010-generational-zgc-managed-heap.md) 与 `wjsm-engine-config`（唯一 Wasmtime Config owner）；support cwasm 三 flavor 仍按 active GC 选择，且必须绑定 ManagedHeap host imports，禁止 memory32 动态对象堆 fallback。

**Date**: 2026-06-19

**Supersedes (partial)**: [ADR 0003](0003-startup-snapshot-boundary.md) 在 P2.8 完成后整体替代。

## Context

ADR 0003 把 startup snapshot 引入 wjsm，bootstrap heap 在 first-run 期 capture 后通过磁盘 cache 给后续运行复用。但仍有三类"出厂后不变"的制品在每次进程启动时付代价：

1. **startup snapshot 字节**：first-run 必须实际跑 cold bootstrap → capture → 写盘；之后才能 restore。无磁盘 cache 的环境（容器一次性启动、CI、`/tmp` 隔离）每次都付 cold path。
2. **wasm helper 函数体**：`compiler_helpers.rs` 当前每个 user wasm 都内联 ~1500 行 helper（`obj_*`, `arr_*`, `string_eq`, `to_int32`, `wjsm_bootstrap_once` 等）。wasmtime 每个 user 模块都重做编译，`module_only` 时间约 3ms（占 full-execute 75%）。
3. **未来内部 builtin JS API**（如 Promise.try、structuredClone 的 JS 实现）：当前没有任何承载位置；如果在运行时 lazy 装载会再付一次 compile + eval。

V8/Deno 用 startup snapshot + precompiled wasm 在构建期固化以上制品。本项目过去是单 crate 直接 `cargo run`，没有 cargo build-time 制品产线。

## Decision

引入 **build-time embedded runtime** 三类制品：

```
crates/wjsm-runtime-snapshot/   build.rs → OUT_DIR/wjsm_startup_snapshot.bin
crates/wjsm-runtime-support/    build.rs → OUT_DIR/wjsm_support.cwasm（wasmtime precompiled）
crates/wjsm-runtime/builtin_js/ manifest.rs 列 ordered (name, source)
crates/wjsm-snapshot-format/    pure 字节格式 + abi_hash（snapshot/runtime 共享，无 wasmtime 依赖）
```

入口 API（`wjsm-runtime`）：

```rust
pub fn install_embedded_startup_snapshot(snapshot_bytes: impl AsRef<[u8]>);
pub fn install_embedded_support_cwasm(cwasm_bytes: &'static [u8]);
pub fn embedded_startup_snapshot() -> Option<&'static [u8]>;  // ABI 校验通过的 view
pub fn embedded_support_cwasm() -> Option<&'static [u8]>;
pub fn build_embedded_startup_snapshot_bytes() -> Result<Vec<u8>>;
```

`wjsm-cli::main_entry` 启动时无条件 install 两份 embedded。未安装或 ABI 失配时只走 cold bootstrap，不在客户机器上 capture/write snapshot cache。

### 三类制品共享同一个 ABI hash 边界

`wjsm-snapshot-format::abi_hash()` 在 ADR 0003 原 6 项基础上追加 **external input** 单输入通道（`OnceLock<u64>`）：

```text
+ extra: u64 = combined_abi_external_input()
   = DefaultHasher( support_module_layout_hash || builtin_js_bundle_hash )
```

`combined_abi_external_input` 由 `wjsm-runtime` 在 `startup_snapshot_enabled()`/`build_embedded_startup_snapshot_bytes()` 入口注册一次（OnceLock 重复 set 静默）。这样 build.rs 与 runtime 计算的 abi_hash 一致；任一 ABI 输入变化都使 embedded snapshot abi_hash 失配 → cold startup。

为什么用 `OnceLock` 而非 `LazyLock`：external input 来源（`wjsm-runtime-support` crate 的 hash）在 `wjsm-snapshot-format` 静态期不可知，必须在 runtime crate 加载后注入。这是 rs-lazylock 规则的"runtime input required"例外。

### support module ABI（P2.0）

固定常量集合（`wjsm-runtime-support::abi`）：

- `SUPPORT_MODULE_NAME = "wjsm_support"`
- `SUPPORT_VERSION = 3`
- `SUPPORT_TABLE_RESERVED_LEN = 64`：为 helper/table ABI 预留；当前 support module 不写 element section，user wasm 的 element section 从 table[0] 开始
- `ENV_GLOBALS`：19 个 imported env globals，与 user wasm global index 0..18 对齐，全部 mutable
- `SUPPORT_EXPORTS`：12 个 helper export 名
  （`obj_new`, `obj_get`, `obj_set`, `obj_delete`, `arr_new`, `elem_get`, `elem_set`, `string_eq`, `to_int32`, `get_proto_from_ctor`, `wjsm_bootstrap_once`, `wjsm_init_function_props`）
- `support_module_layout_hash() -> u64`：以 `DefaultHasher` 哈希上述四项，作为 ABI external input 的 support 部分

### user wasm 形态（P2.2 选择）

为避免改动 100+ 个 host 函数（它们通过 `WasmEnv::from_caller` 读 user instance 的 export memory），rev3 修订采用：

> user wasm `import "env" "memory"` 之后 **再 export 同一份 memory**

wasm 允许 re-export 自身的 import；wasm-encoder 一行写法。runtime 创建 shared memory + 19 globals + table 一次，通过 Linker 给 support 与 user 两个 instance；user re-export memory 后 `WasmEnv::from_caller` 零改动。

### Compatibility Boundary

- `wjsm-runtime` public API（`execute`/`execute_with_writer`/`compile_source`）签名与返回值完全不变；新增 install 函数是可选注入。
- 现有 fixture `.expected` 输出不变（fixture runner 只比 stdout/stderr，不比 wasm 结构）。
- `RuntimeState` 字段仍扁平（ADR 0002 不撤销）。
- snapshot 边界仍由 ADR 0003 定义：仅覆盖 pristine runtime startup heap；用户对象/promise/timer/scheduler 永不进 embedded 制品。
- 三个新 crate 都加 `embedded` cargo feature，默认开启；`--no-default-features` 时没有 build-time embedded 制品，运行时只走 cold bootstrap，不写 snapshot cache。

## Consequences

### Positive

- first-run 不付 cold bootstrap：embedded snapshot 直接 `restore`，不写磁盘 cache（不污染 `/tmp`）。
- 三个新 crate 各自单一 owner（snapshot bytes / support cwasm / builtin_js），改动半径明确。
- support cwasm 通过 `wasmtime::Engine::precompile_module` 一次预编译；P2.3-P2.6 切换 user wasm 为 import 形态后，每个 user 实例可经 `Module::deserialize` 跳过 wasmtime 编译，预期 `module_only` 降至旧值的 60%（P2.8 验收）。
- builtin JS bundle 走 manifest，append 即生效，无运行时 lazy 装载代价。

### Negative / Risks

- 三个 crate 引入额外 cargo build dependency 链；`build.rs` 在没有 wasmtime native deps 的 docker/CI 环境可能失败 → feature `embedded` 关闭即可降级到 cold bootstrap。
- ABI hash 输入扩到三类（snapshot/support/builtin-js），任一变更都触发 cold startup；维护时必须更新对应 layout/version 常量与单测断言。
- support cwasm 是 wasmtime 版本敏感字节；wasmtime 升级或 Cranelift 变化都需要 P2.7 重新 bake snapshot。
- 运行时磁盘 cache 已删除；库使用者未调用 `install_embedded_*` 时只付 cold bootstrap，不产生客户机器持久化状态。

## Alternatives Considered

### Per-process 全量再编译

放弃。3ms wasmtime compile 对短命 CLI 是可观开销，每个进程都付。

### Helper inline 保持现状

保留旧路径会导致：(1) compiler_helpers.rs 1538 行只为生成 wasm bytecode；(2) 每个 user wasm 都重新编译这些 helpers；(3) 体积膨胀。删除 inline helpers（P2.8）是这项 ADR 的退役条件。

### 把 builtin JS 做成运行时 lazy 装载

放弃。运行时 lazy 会让 first-call 付 compile + eval 代价；snapshot 期 eval → 结果固化进 embedded snapshot 是 V8/Deno 已验证的范式。

### 用 sha2 或 blake3 做 ABI hash

放弃。ABI hash 不需要密码学强度，只需稳定的 mismatch detection。`std::collections::hash_map::DefaultHasher` 与现有 `support_module_layout_hash` 风格一致，零依赖。

## Status of Implementation

| P0 工作区准备（3 crate skeleton + workspace 注册） | ✅ | `cargo build --workspace` 通过 |
| P1.0 wjsm-snapshot-format crate 抽出 | ✅ | 全测试通过 |
| P1.1 build-time 生成 snapshot 字节 | ✅ | OUT_DIR/wjsm_startup_snapshot.bin 4516 bytes |
| P1.2 install_embedded_startup_snapshot + 退役 runtime cache | ✅ | embedded_snapshot_first_run_ignores_runtime_cache_env 通过 |
| P1.3 wjsm-cli 启动时 install | ✅ | `cargo run -- eval "console.log('embedded ok')"` 输出正确 |
| P1.4 bench：embedded vs runtime first-run | ✅ | snapshot restore 18.7µs vs cold bootstrap 41.6µs（2.2x） |
| P2.0 设计 support module ABI（abi.rs） | ✅ | 4 ABI 单测通过；support_module_layout_hash 接入 abi_hash |
| P2.1 build.rs 产 support.wasm + cwasm | ✅ | OUT_DIR/wjsm_support.cwasm 生成，deserialize 成功，12 exports 完整 |
| P2.2 shared memory/table/globals + 双 instance | ✅ | user wasm import + re-export memory 链路完整 |
| P2.3 obj_new/obj_get/obj_set/obj_delete/string_eq/to_int32 import | ✅ | 6 个 helper 已从 wjsm_support import |
| P2.4 arr_new/elem_get/elem_set import | ✅ | 3 个 helper 已从 wjsm_support import |
| P2.5 get_proto_from_ctor import | ✅ | Normal mode 下已从 wjsm_support import |
| P2.6 bootstrap 阶段函数迁移 | ⊘ (by design) | 保持 user wasm 内联（仅启动调用一次） |
| P2.7 重新 bake snapshot | ✅ | 全 workspace 测试通过 |
| P2.8 删除旧 inline helpers + bench | ⊘ (部分) | Normal mode 10 个 helper 已从 import；Eval mode 保留 inline |
| P3.0 builtin_js 框架 + manifest | ✅ | 空 manifest 不破坏现有行为；BUILTIN_JS_FILES 接入 ABI hash |
| P3.1 sentinel 端到端验证 | ⏳ | hash 通道已通；snapshot restore 保留 builtin JS 全局待后续 |
| P4.0 文档（本 ADR + ADR 0003 supersede + AGENTS.md） | ✅ | 本文件 |
| P4.1 全工作区验证 + bench | ✅ | 970 passed, 1 skipped |
当前测试: **workspace 970 passed, 1 skipped**（含 4 ABI + 4 emit_support + 2 cwasm deserialize + 7 startup/embedded snapshot）。

## References

- [ADR 0002: RuntimeState stays flat](0002-runtimestate-stays-flat.md)
- [ADR 0003: Startup Snapshot Boundary](0003-startup-snapshot-boundary.md)
- 实施计划: `docs/aegis/plans/2026-06-19-build-time-embedded-runtime.md`
- 工作日志: `docs/aegis/work/2026-06-19-build-time-embedded-runtime/`
- Deno `cli/snapshot/build.rs` + `runtime/snapshot.rs::create_runtime_snapshot`
- V8 custom startup snapshots
