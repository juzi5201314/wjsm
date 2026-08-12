# Crate 与公共 API 索引

这一章是 15 个 crate 的公共 API 速查表。

## wjsm-parser

SWC 解析边界。

| 导出 | 用途 |
| --- | --- |
| parse | 解析源码为 SWC AST |

## wjsm-semantic

语义分析与 lowering。

| 导出 | 用途 |
| --- | --- |
| lower | SWC AST → 语义 IR |
| builtins | builtin 拦截识别 |

## wjsm-ir

中间表示与常量。

| 导出 | 用途 |
| --- | --- |
| Program / Module / Function | IR 结构 |
| Instruction | 指令枚举 |
| value | NaN-box 值编码（`BOX_BASE`、`TAG_*`） |
| constants | ABI 常量（`SHADOW_MEMORY_NAME` 等） |

## wjsm-backend-wasm

WASM 代码生成。

| 导出 | 用途 |
| --- | --- |
| emit_support_module | 生成 support module WASM |
| GcFlavor | GC flavor 枚举 |

## wjsm-backend-jit

JIT 后端边界（未实现的扩展点）。

## wjsm-runtime

兼容 facade。

| 导出 | 用途 |
| --- | --- |
| execute / execute_with_options | 执行 WASM |
| compile_source / compile_source_with_debug | 编译源码 |
| RuntimeOptions / InspectConfig / PrecompiledEntry | 配置 |
| module_cache_stats / clear_module_cache | 缓存管理 |
| validate_artifact / artifact_metadata | Artifact 工具 |

## wjsm-cli

命令行接口。subcommand：run、build、test、check、lint、eval、repl、fmt、install、cache、completions、init、version、dump-ast、dump-ir、dump-wat、validate、size、disasm。

## wjsm-test262

test262 集成。

## wjsm-module

模块系统与 bundler。

| 导出 | 用途 |
| --- | --- |
| builtin_modules | `node:` 模块表 |
| resolution_options | 解析条件 |

## wjsm-gc-bench

GC 基准。

## wjsm-snapshot-format

快照二进制格式。

| 导出 | 用途 |
| --- | --- |
| encode_snapshot / decode_snapshot | 快照编解码 |
| ManagedHeapV2ArtifactAbi | 工件 ABI |
| SNAPSHOT_MAGIC / SNAPSHOT_FORMAT_VERSION | 格式常量 |

## wjsm-host

后端无关的 host trait。

| 导出 | 用途 |
| --- | --- |
| ExecContext / HeapContext / JsBackend | trait 契约 |
| Value / Handle | 值与句柄 |


## wjsm-host-native

Cranelift 后端实现。

| 导出 | 用途 |
| --- | --- |
| execute_with_options | 执行入口 |
| process_exit_code / process_exit_diagnostics | 退出码 |
| CRANELIFT_VERSION | Cranelift 版本常量 |
## wjsm-builtins

JavaScript builtins 算法。按域组织：object、collections、array、typedarray、string、promise、async、proxy、json、date、fetch、weakref、modules、inspector、render、core。

## wjsm-gc

垃圾回收器。

| 导出 | 用途 |
| --- | --- |
| GcAlgorithmKind | 算法枚举（MarkSweep / G1 / Zgc） |
| GcStats / CycleKind | 统计与周期类型 |
| Handle / Value | 类型别名 |
| StepBudget | 增量步进预算 |

## 深入了解

- [Owner 与单一事实来源](owners-and-sources-of-truth.md)
- [Workspace crate 地图](../foundations/crate-map.md)
- [跨 crate 所有权与依赖边界](../foundations/ownership-and-dependencies.md)
