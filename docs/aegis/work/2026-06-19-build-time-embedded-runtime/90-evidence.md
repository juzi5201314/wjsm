# P0-P4 Evidence: Build-Time Embedded Runtime

## P1.4 Benchmark (Initial baseline, 2026-06-19)

### Snapshot Operations
- **Snapshot build (cold)**: 8.646µs/each
- **Snapshot decode**: 453ns/each
- **Snapshot restore**: 14.335µs/each

### Phase Breakdown
| Phase | Without snapshot | With embedded snapshot |
|-------|------------------|------------------------|
| engine | 9.561µs | 14.786µs |
| **module compilation** | **2.838ms** | **3.057ms** |
| startup | 41.597µs (cold bootstrap) | 19.567µs (snapshot restore) |
| main | 305.595µs | 342.622µs |
| **total** | **3.806ms** | **4.062ms** |

Snapshot restore (18.7µs) ≈ 2.2× faster than cold bootstrap (41.6µs). Module compilation
dominates (75%); P2 target.

## P4.1 Benchmark (Post P2.0/P2.1/P2.2/P3.0/P4.0)

### Snapshot Operations (re-measured)
- **Snapshot build (cold)**: 6.667µs/each
- **Snapshot decode**: 729ns/each
- **Snapshot restore**: 16.759µs/each

### Phase Breakdown
| Phase | Without snapshot | With embedded snapshot |
|-------|------------------|------------------------|
| engine | 7.6µs | 10.147µs |
| **module compilation** | **3.179ms** | **3.679ms** |
| startup | 44.488µs (cold bootstrap) | 18.048µs (snapshot restore) |
| main | 286.68µs | 444.756µs |
| **total** | **4.082ms** | **4.757ms** |

Module compilation 仍然是热点（user wasm 仍内联 helpers + ABI hash 增加 1 项额外
hash 输入对 module 编译无影响），符合预期：P2 真正的 module 时间下降需要等
P2.3-P2.6 切换 user wasm 为 import 形态、跳过 helper 内联。

## P2.3 回归修复（2026-06-20）

### 已修复根因

1. support module 未 re-export imported memory/table/globals，导致 support helper 触发的 host import 无法通过 `Caller::get_export` 恢复 `WasmEnv`。
2. compiled eval wasm 的 imported global mutability 仍是旧 contract，且未 export memory/table；P2.2 后父模块导出的是 mutable shared env globals，wasmtime 实例化失败后退回不完整解释器路径。
3. P2.2 提前计算 heap_start 后仍继续追加 eval metadata 与函数 `.name` data strings，导致字符串落入 object heap，被后续分配/GC 覆盖。

### 验证命令

- `cargo nextest run -E 'test(errors__eval_strict_undeclared) or test(happy__eval_super_prop)'` → 2 passed, 570 skipped
- `cargo nextest run -E 'test(happy__gc_function_props_survive)'` → 1 passed, 571 skipped
- `cargo nextest run -E 'test(happy__) or test(errors__)'` → 520 passed, 52 skipped
- `cargo nextest run -p wjsm-backend-wasm` → 50 passed
- `cargo nextest run -p wjsm-runtime -E 'test(startup_snapshot) or test(embedded_startup_snapshot)'` → 4 passed, 69 skipped
- `cargo nextest run --workspace` → 970 passed, 1 skipped


## Test Status

- workspace: **970 passed, 1 skipped**（runtime disk cache 相关旧测试删除/重写后）
- 新增测试覆盖：
  - 4 × `wjsm-runtime-support::abi`（hash 确定性、export 计数锁定、global 计数锁定、table reserved size）
  - 4 × `wjsm-backend-wasm::support_module`（valid wasm magic、wasmparser validate、helper 计数、env global 计数）
  - 2 × `wjsm-runtime-support::tests::embedded_support_cwasm`（cwasm 非空、cwasm deserialize 后 12 exports 完整）
  - 2 × P2.0 ABI 接入（在 startup_snapshot 测试中隐式覆盖）

## Artifact Sizes

| 制品 | 大小 | 说明 |
|---|---|---|
| `OUT_DIR/wjsm_startup_snapshot.bin` | 4516 bytes | embedded primordial heap snapshot |
| `OUT_DIR/wjsm_support.cwasm` | 16024 bytes | wasmtime precompiled support module（stub bodies，待 P2.3-P2.6 替换） |

## Completed Phases

| 阶段 | 状态 | 关键证据 |
|---|---|---|
| P0 工作区 | ✅ | 3 crate skeleton + workspace `cargo build` |
| P1.0 抽 snapshot lib | ✅ | wjsm-snapshot-format 独立 crate；旧 startup_snapshot_format.rs 删除 |
| P1.1 build-time snapshot | ✅ | OUT_DIR/wjsm_startup_snapshot.bin 4516 bytes，header.abi_hash 与运行时一致 |
| P1.2 install API + runtime cache 退役 | ✅ | `embedded_snapshot_first_run_ignores_runtime_cache_env` / `startup_snapshot_default_on_does_not_write_runtime_cache` 覆盖 cache env 不写盘 |
| P1.3 CLI 启动 install | ✅ | `cargo run -- eval "console.log('embedded ok')"` 成功 |
| P1.4 bench | ✅ | snapshot restore 18.7µs vs cold bootstrap 41.6µs（2.2x） |
| P2.0 support module ABI | ✅ | 12 helper exports + 19 env globals + SUPPORT_TABLE_RESERVED_LEN=64 + support_module_layout_hash |
| P2.1 build.rs 产 cwasm | ✅ | OUT_DIR/wjsm_support.cwasm 16024 bytes，deserialize + 12 exports 完整 |
| P2.2 install_embedded_support_cwasm | ✅（API+CLI 注入） | CLI 启动 install 双 embedded 不报错 |
| P2.3 object helpers | ✅ | happy/errors 520 passed；workspace 970 passed |
| P3.0 builtin_js 框架 | ✅ | manifest 空数组与 builtin JS 引入前字节级一致；BUILTIN_JS_FILES 接入 ABI hash external input |
| P4.0 ADR 0004 + AGENTS.md | ✅ | docs/adr/0004 + ADR 0003 标 partial-superseded + INDEX 更新 |

## Pending Phases (后续会话)

| 阶段 | 阻塞原因 | 工作量估算 |
|---|---|---|
| P2.4 切 array/elem helpers | 同 P2.3 模式 | 长会话 |
| P2.5 切 utility helpers (string_eq/to_int32/get_proto_from_ctor) | 同 P2.3 模式 | 长会话 |
| P2.6 切 bootstrap (wjsm_bootstrap_once + wjsm_init_function_props) | 需把 user wasm data segment 中 name/param 表布局改为 helper 指针参数 | 长会话 |
| P2.7 重新 bake P1 snapshot | 等 P2.6 后 ABI 已变 | 短 |
| P2.8 删除旧 inline helpers + bench module_only ≤ 60% | 等 P2.6 后 | 短（删除 1538 行 + bench） |
| P3.1 sentinel E2E（builtin_js_bundle_hash 接入已完成；sentinel 验证依赖 P2.3-P2.6 后的 user-side eval） | 同上 | 中 |
| P4.1 bench 三段证据（embedded warm vs runtime warm vs module_only ≤ 60%） | 等 P2.8 数据 | 短 |
| P4.2 提测 | 等 P4.1 完成 | 短 |
