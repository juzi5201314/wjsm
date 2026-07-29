# 核心不变量

这一章汇总 wjsm 必须遵守的核心不变量。

## GC 不变量

`wjsm-gc/src/api.rs` 定义：

- **INV-C1**：JS 值层引用是 handle；`obj_table[h]` 是唯一 ptr truth。
- **INV-C2**：raw ptr 不跨潜在 moving/collect GC 点；跨越必须重新 resolve。

所有三种回收器都遵守这两条不变量。mark-sweep 虽然不移动对象，但代码仍遵守，保持与移动回收器的兼容性。

## 统一 ManagedHeap

ADR 0010 确立：

- 统一 ManagedHeap，三种回收器共用。
- 8 字节句柄（V2），不用 4 字节句柄（V1）。
- shared memory64 对象堆，不用 memory32。
- 不引入 dual-heap fallback 或第二个运行时 owner。

## 后端边界

ADR 0011–0013：

- wasmtime 依赖只在 `wjsm-host-wasm` 和 `wjsm-backend-wasm`。
- `wjsm-builtins`、`wjsm-host`、`wjsm-gc`、`wjsm-module` 后端无关。
- 包装层（host import 函数）只做类型转换，不做语义决策。
- GC 算法通过 `GcContext` / `RootProvider` 接合层访问内存。

## Engine owner

`wjsm-host-wasm/src/engine_config.rs` 是唯一构造和 mutation wasmtime `Config` 的地方。所有 profile 固定开启 `threads` / `shared-memory` / `memory64` / `multi-memory` / `bulk-memory`。

## 快照格式

- `SNAPSHOT_FORMAT_VERSION` 任何 wire 改动必须递增。
- 快照 ABI 哈希由 `support_abi_union_hash` + `builtin_js_bundle_hash` + `compatibility_fingerprint` 组成。
- `ManagedHeapV2ArtifactAbi` 生成时自校验。

## 执行模型

- NaN-boxed 值（`i64`），标签在 bits 32-37。
- 两阶段 lowering（预声明 + lower），保证 TDZ + hoisting。
- 作用域名 `$scope_id.name`。
- 启动快照默认开启，ABI hash 校验。

## 深入了解

- [Owner 与单一事实来源](owners-and-sources-of-truth.md)
- [GC 选择、配置与不变量](../gc/configuration-and-invariants.md)
- [ADR 导航](adr-index.md)
