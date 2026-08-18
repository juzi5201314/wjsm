# ADR 0020: ICU4X compiled_data 嵌入 rustc 链接的 stub

## Status

Accepted（2026-08-17）

**Amended**: 2026-08-18（墙钟换算收进 `wjsm-intl-data`；IANA 标识仍以 ICU 为准）

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

`NativeAgentState::new` 调用 `wjsm_intl_data::keep_compiled_data()`。发行构建通过 `#[used]` constructor 指针把尚未被 JS 路径引用的实验数据留在 rustc 链接图里。Phase 2/3 的 `Intl` 与 locale 敏感方法会直接引用 `locale` / `format` / `text` 模块，因此这些模块在 debug 与发行构建中都编译；`keep_compiled_data()` 仍只在发行构建强制留住尚未被引用的入口。

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

`DataManifest` 固定记录 ICU4X、CLDR、Unicode、UTS #46、tzdb、ISO 4217（CLDR 货币）、Encoding 与 RegExp（`regress` Unicode property escapes）版本。canonical JSON 的 SHA-256 纳入测试；升级数据必须改清单，禁止静默混用。

本阶段基线：ICU4X **2.2.0**、CLDR **48.2**、Unicode **17.0.0**、tzdb **2026a**、`idna` 1.1.0、`encoding_rs` 0.8.35、`regress` 0.11.1（RegExp UCD 同为 Unicode 17）。

### 5. Phase 2/3 消费本 crate，不另起数据 owner

ECMA-402 `Intl` 对象与 ECMA-262 locale 敏感方法（`localeCompare`、`toLocale*`、`normalize` 的大小写映射）由 `wjsm-builtins` 抽象操作 + `wjsm-host-native` 安装/分派实现，ICU 包装只留在本 crate。URL IDN 与 WHATWG `TextDecoder`（含非 UTF-8 标签）同样必须消费本 crate。

对象模型沿用现有 console / Date / Map 模式（普通对象、lazy prototype、侧表内部槽），不新开架构 ADR。

### 6. 墙钟时区换算只在数据 crate

IANA 标识的解析、规范化与可用性仍走 ICU `IanaParser` / compiled_data。UTC↔墙钟与 DST 换算由 `wjsm-intl-data` 拥有（`chrono` + `chrono-tz`），`wjsm-host-native` 不得再依赖 `chrono-tz` 或用 `chrono::Local` 冒充 resolved `timeZone`。

未知 IANA 名 fail-closed，不得静默当 UTC。默认 `timeZone` 是宿主 IANA 标识（`TZ` / `/etc/localtime`），`resolvedOptions().timeZone` 与 `format` 使用同一值。

## Consequences

- 生产 `String.prototype.normalize` 与 builtins 算法共用 ICU4X normalizer；`unicode-normalization` 从 workspace 删除。
- 墙钟换算与 IANA 标识分属同一数据 crate 的两层：ICU 认标识，`chrono-tz` 做 DST；host-native 只调用 `wjsm-intl-data` 的 zone API。
- `wjsm-exec` stub 体积随 full compiled_data 增加；packed exe 继承 stub，overlay 不再重复带 ICU。x86_64 Linux release（thin LTO、strip symbols）测得 29.8 MiB（31,266,392 字节）；引入前 ADR 0018 记录约 27 MiB。
- 手册非目标改为「不引入 ICU4C / 宿主 ICU」，不再把 ICU4X 当成否决项。

## Verification

- `wjsm-intl-data` smoke matrix（8 locale × 数据类别）与 manifest hash 测试。
- `NODE_ICU_DATA` 不影响探测结果。
- `happy/string_normalize*`、`happy/intl_*`、`errors/intl_*` 与 packed `wjsm-exec` 的 `String.prototype.normalize`。
- test262 allowlist 含本阶段用到的 ECMA-402 feature；不含光秃 `"Intl"`（以免前缀误选 Temporal）、`canonical-tz`、`intl-normative-optional`。
- 构造器与 locale 方法目录套件作为实现期回归；全量 `test/intl402` 非 Temporal 子集仍是后续合入门槛，不作为本阶段关闭条件。
- workspace 测试作为合入门槛。

## References

- [ADR 0003](0003-startup-snapshot-boundary.md) — snapshot 不是 ICU 载体
- [ADR 0014](0014-direct-cranelift-portable-artifact.md) — 后端无关边界
- [ADR 0016](0016-native-executable-stub-overlay.md) — rustc 链接 stub
- Node.js v24.15.0 [Internationalization support](https://nodejs.org/docs/v24.15.0/api/intl.html)
- ICU4X 2.2（CLDR 48.2 / TZDB 2026a）
