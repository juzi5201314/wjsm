# 修改 WASM ABI

这一章说明修改 user wasm 与 host 之间的 ABI 需要改动哪些地方。

## ABI 组成

user wasm 与 host 的 ABI 由几部分组成：

- **共享类型部分**（type section）：函数类型定义。
- **三块内存**：`memory`、`shadow_memory`、`heap_memory`（shared memory64）。
- **约 27 个 env global**：`__heap_ptr`、`__obj_table_ptr`、`__gc_phase`、`__good_color` 等。
- **约 507 个 host import**：`env.*` 函数。
- **导出**：`memory`、`shadow_memory`、`__table`、global 等。

## 改动步骤

1. **IR 常量**：`wjsm-ir` 的 `constants.rs` 或 `value.rs` 定义 ABI 常量（标签、类型索引、global 名等）。修改常量。
2. **codegen**：`wjsm-backend-wasm` 的 `module.rs` / `func_table.rs` / `host_imports.rs` 生成新的 ABI 结构。
3. **support module**：`support_module.rs` 修改 support module 的导出和 import。
4. **runtime 接合**：`wjsm-host-wasm/src/wasm_env.rs` 的 `WasmEnv` 结构更新，`extract_wasm_env` 读取新的 global/memory。
5. **ABI 哈希**：`runtime_support/abi.rs` 的 ABI hash 计算更新。
6. **快照格式**：如果 ABI 变化影响快照布局，`wjsm-snapshot-format` 的格式版本递增。
7. **build.rs**：重新生成嵌入工件。
8. **测试**：全 workspace 测试。

## 影响范围

ABI 变化影响所有层：IR、semantic、backend、host-wasm、snapshot-format、build.rs。这是最跨层的改动类型，需要[跨层变更检查清单](cross-layer-checklist.md)。

## 递增格式版本

快照 wire 改动必须递增 `SNAPSHOT_FORMAT_VERSION`。当前是 9。递增后旧快照无法解码，build.rs 会重新生成。

## 深入了解

- [Import、Export 与主模块 ABI](../backend/imports-exports-and-abi.md)
- [修改快照与嵌入工件](changing-snapshots.md)
- [跨层变更检查清单](cross-layer-checklist.md)
