# ADR 0020: ICU4X compiled_data 嵌入 rustc 链接的 stub

## Status

Accepted（2026-08-17）

## Context

Node.js v24 默认 full-icu：CLDR/Unicode 数据打进官方二进制，不依赖宿主机 ICU，也不是英文-only 的 small-icu。wjsm 要对齐这条分发合同，作为后续 ECMA-402 `Intl`、locale 敏感方法与 URL IDN 的数据地基。

当时的实现是双轨：

- `wjsm-builtins` 依赖 `icu_normalizer` 2.2 `compiled_data`，但生产 dispatcher 不调用。
- `wjsm-host-native` 的 `String.prototype.normalize` 走 `unicode-normalization`。
- 没有 CLDR / UCD / UTS #46 / tzdb 版本清单，也没有跨 crate 的单一 provider。
- 手册非目标写「不引入完整 ICU」。

ADR 0003 的 startup snapshot 是强制 restore 的最小种子，不是 ICU 数据载体。portable `.wjsm` 不含机器码，也不应含 CLDR。ADR 0016 的 `wjsm-exec` stub 由 rustc 链接，packed overlay 只带 IR / native object / 源码快照。

## Decision

### 1. `wjsm-intl-data` 是唯一数据 provider

新建后端无关 crate `wjsm-intl-data`。只有它直接依赖 `icu` 2.2 `compiled_data`（含 `unstable` 实验组件）、`idna` 1.1 与 `encoding_rs`。`wjsm-builtins` 与 `wjsm-host-native` 只消费本 crate，禁止再引入 `icu_*`、`unicode-normalization` 或第二套 normalizer。

数据算法与访问层保持 backend-independent：禁止 Cranelift、`NativeVmContext`、native ABI 类型进入该 crate。

### 2. compiled_data 进 rustc 链接的 `wjsm` / `wjsm-exec`

生产数据是 ICU4X 发布的 compiled_data，经 rustc 链进 `wjsm` 与预链 `wjsm-exec` stub。不跑 icu4x-datagen，不裁非英语 locale。

`NativeAgentState::new` 调用 `wjsm_intl_data::keep_compiled_data()`。发行构建通过 `#[used]` constructor 指针把各类数据入口留在 rustc 链接图里，避免 DCE 在 `Intl` JS API 落地前把 locale 数据从 stub 删掉；它不在启动路径上真正构造全部 formatter。debug / test 构建不强制保留未引用的 locale 数据，以免 debug `wjsm` 体积拖垮 3s fixture 门禁。

禁止：

- 运行时读 system ICU / 宿主 CLDR 路径
- `NODE_ICU_DATA`、`--icu-data-dir` 类环境变量作为主路径或回退
- 运行时联网下载
- 把 ICU 数据放进 startup snapshot 或 portable `.wjsm`
- packed overlay 再带一份数据文件

`wjsm run`、portable `.wjsm` 的当前宿主执行、native image cache 命中、packed `native-executable` 使用同一份 rustc 链接数据。

### 3. 覆盖是 full，不是 small-icu

覆盖矩阵至少包含 `en-US`、`zh-CN`、`de-DE`、`es-ES`、`ar`、`th`、`tr`、`ja-JP`，数据类别覆盖 locale / likely subtags / calendar / collation / numbering / date-time / time zone / plural / list / display name / segmenter / duration / unit / UTS #46 / WHATWG Encoding 标签。

体积用 DCE 与共享 compiled_data 控制，不得靠删除非英语 locale 达标。体积增量记录在手册与本 ADR。

### 4. 版本契约可审计

`DataManifest` 固定记录 ICU4X、CLDR、Unicode、UTS #46、tzdb、ISO 4217（CLDR 货币）与 Encoding 版本。canonical JSON 的 SHA-256 纳入测试；升级数据必须改清单，禁止静默混用。

本阶段基线：ICU4X **2.2.0**、CLDR **48.2**、Unicode **17.0.0**、tzdb **2026a**、`idna` 1.1.0、`encoding_rs` 0.8.35。

### 5. 本阶段不实现 `Intl` JS API

ECMA-402 对象、`localeCompare`、`toLocaleString`、URL IDN 接线、非 UTF-8 `TextDecoder` 属于后续阶段。它们必须消费本 crate，不得另起数据 owner。

## Consequences

- 生产 `String.prototype.normalize` 与 builtins 算法共用 ICU4X normalizer；`unicode-normalization` 从 workspace 删除。
- `wjsm-exec` stub 体积随 full compiled_data 增加；packed exe 继承 stub，overlay 不再重复带 ICU。x86_64 Linux release（thin LTO、strip symbols）测得 29.8 MiB（31,266,392 字节）；引入前 ADR 0018 记录约 27 MiB。
- 手册非目标改为「不引入 ICU4C / 宿主 ICU」，不再把 ICU4X 当成否决项。

## Verification

- `wjsm-intl-data` smoke matrix（8 locale × 数据类别）与 manifest hash 测试。
- `NODE_ICU_DATA` 不影响探测结果。
- `happy/string_normalize*` 与 packed `wjsm-exec` 的 `String.prototype.normalize`。
- workspace 测试作为合入门槛。

## References

- [ADR 0003](0003-startup-snapshot-boundary.md) — snapshot 不是 ICU 载体
- [ADR 0014](0014-direct-cranelift-portable-artifact.md) — 后端无关边界
- [ADR 0016](0016-native-executable-stub-overlay.md) — rustc 链接 stub
- Node.js v24.15.0 [Internationalization support](https://nodejs.org/docs/v24.15.0/api/intl.html)
- ICU4X 2.2（CLDR 48.2 / TZDB 2026a）
