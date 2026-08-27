# Crate 与公共 API 索引

这一章是 workspace crate 的公共 API 速查表。

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

## wjsm-backend-native

native 代码生成与磁盘 image cache。

| 导出 | 用途 |
| --- | --- |
| NativeCompiler | IR → 当前宿主 object / CLIF |
| NativeImageRepository | 进程内 image 池与可选 `.wnat` 磁盘缓存 |
| NativeCacheKey / NativeCacheStats | cache 键与统计 |
| CompiledImage | 已加载的可执行 image |

## wjsm-backend-jit

JIT 后端边界（未实现的扩展点）。

## wjsm-runtime

兼容 facade，只 re-export `wjsm-host-native` / `wjsm-gc`。

| 导出 | 用途 |
| --- | --- |
| NativeRuntime / NativeRuntimeConfig | 运行时与 `cache_dir` 配置 |
| execute_with_writer_with_options | 执行入口 |
| compile_source / compile_source_with_options | 源码 → portable artifact |
| RuntimeOptions / SourceCompileOptions | 执行与编译选项 |
| NativeExecution | stdout / stderr / exit / cache stats |

## wjsm-cli

命令行接口。subcommand：run、build、test、check、lint、eval、repl、fmt、install、cache、completions、init、version、dump-ast、dump-ir、dump-clif、validate、size、disasm。

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
| NativeRuntime / NativeRuntimeConfig | 运行时；`cache_dir: None` 关闭磁盘缓存 |
| execute / execute_with_writer_with_options | 执行入口 |
| RuntimeOptions / SourceCompileOptions | 配置 |
| NativeExecution | 可观察输出与 cache 统计 |

## wjsm-builtins

JavaScript builtins 算法。按域组织：object、collections、array、typedarray、string、promise、async、proxy、json、date、intl、fetch、weakref、modules、inspector、render、core。`intl` 只消费已规约的 Rust 类型，不持有 `NativeVmContext`。

## wjsm-intl-data

ICU4X compiled_data、UTS #46 与 WHATWG Encoding 标签的唯一入口。

| 导出 | 用途 |
| --- | --- |
| `DATA_MANIFEST` / `manifest_sha256` | 版本契约与稳定 hash |
| `normalize` / `case_map` / `locale_case_map` | Unicode 正规化与大小写映射 |
| `canonicalize_locale` / `supported_values` | 语言标签与 `supportedValuesOf` 数据 |
| `Owned*Formatter` / `OwnedDisplayNames` / `OwnedSegmenter` | Collator、Number、DateTime、Plural、List、RelativeTime、DisplayNames、Segmenter、Duration |
| `probe_locale` / `keep_compiled_data` | smoke 覆盖与发行 stub 链接保活 |
| `icu` / `idna` / `encoding_rs` | 测试与发行构建 re-export；debug `wjsm` 经 `Intl` 路径链接 locale 数据 |

## wjsm-gc

垃圾回收器。

| 导出 | 用途 |
| --- | --- |
| GenerationalZgc | 生产并发分代 ZGC collector |
| GcStats / CycleKind | 统计与周期类型 |
| Handle / Value | 类型别名 |
| StepBudget | 增量步进预算 |

## 深入了解

- [Owner 与单一事实来源](owners-and-sources-of-truth.md)
- [Workspace crate 地图](../foundations/crate-map.md)
- [跨 crate 所有权与依赖边界](../foundations/ownership-and-dependencies.md)
