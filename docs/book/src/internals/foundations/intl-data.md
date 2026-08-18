# 国际化数据契约

wjsm 把 CLDR / Unicode 数据 rustc 链接进 `wjsm` 与 `wjsm-exec` stub。数据 owner 是 `wjsm-intl-data`。portable `.wjsm` 与 startup snapshot 不含 ICU 数据。

Phase 2/3 已把 JS `Intl` 与 ECMA-262 locale 敏感方法接到本 crate。ICU 类型不泄漏给 host；抽象操作在 `wjsm-builtins`，安装与分派在 `wjsm-host-native`。

## 版本清单

| 项目 | 版本 |
| --- | --- |
| ICU4X | 2.2.0（`compiled_data` + `unstable`） |
| CLDR | 48.2 |
| Unicode / UCD | 17.0.0 |
| RegExp property escapes | regress 0.11.1（Unicode 17.0.0；与 UCD 同版本） |
| UTS #46 | Unicode 17.0，经 `idna` 1.1.0（WHATWG URL 参数化） |
| IANA tzdb | 2026a |
| ISO 4217 | CLDR 48.2 货币数据 |
| WHATWG Encoding | encoding_rs 0.8.35 |
| 覆盖 | `full`（不是 small-icu / 英文-only） |

canonical JSON 与 SHA-256 由 `wjsm_intl_data::canonical_json` / `manifest_sha256` 提供，升级 ICU4X 必须改测试常量。

## 覆盖矩阵

smoke locales：`en-US`、`zh-CN`、`de-DE`、`es-ES`、`ar`、`th`、`tr`、`ja-JP`。

| 类别 | 入口 | 备注 |
| --- | --- | --- |
| locale / likely subtags / fallback | `locale.rs` | ICU4X 2.2 无独立 LocaleMatcher；`ResolveLocale` 在 `wjsm-builtins` |
| calendar | `AnyCalendar` | 含 Gregorian / Japanese / Chinese / Buddhist / Hijri |
| collation | `Collator` | |
| numbering | `DecimalFormatter` | |
| date/time | `DateTimeFormatter` | |
| time zone | `IanaParser` + `zone.rs` | IANA 标识来自 ICU；UTC↔墙钟 / DST 由本 crate 的 `chrono-tz` 换算，host 不得另持一份 tzdb |
| plural | `PluralRules` | |
| list | `ListFormatter` | |
| display name | `RegionDisplayNames` / `LanguageDisplayNames` | `icu::experimental` |
| segmenter | `WordSegmenter` / grapheme / sentence | 含泰文、日文分词数据 |
| duration | `duration.rs` | JS `Intl.DurationFormat` 走规范 `PartitionDurationFormatPattern`，不直接用 ICU `DurationFormatter` 输出 |
| unit | `UnitsFormatter` | experimental |
| IDNA | `domain_to_ascii_uts46` / `domain_to_unicode_uts46` | UTS #46；供 `node:url` 与全局 `URL` |
| encoding labels | `encoding_rs::Encoding::for_label_no_replacement` | Phase 4 `TextDecoder` 经 `encoding_for_label` 消费 |
| RegExp `\p{...}` | `regress` 0.11（host-native / semantic early error） | UCD 与 manifest `unicode` 同为 17.0.0；不另建 fallback |

## 分发路径

```text
icu 2.2 compiled_data + idna + encoding_rs
  -> wjsm-intl-data
  -> wjsm-builtins / wjsm-host-native
  -> rustc 链接 wjsm 与 wjsm-exec stub
```

- `wjsm run`、当前宿主执行 portable `.wjsm`、native image cache、packed `native-executable` 共用这份 stub 数据。
- 不读 system ICU、`NODE_ICU_DATA` 或宿主 CLDR 目录，也不联网下载。
- debug `wjsm` 因 JS `Intl` 路径引用而链接 locale / format / text 数据；发行 stub 仍用 `keep_compiled_data` 留住尚未被引用的实验入口。

## 体积

ADR 0018 记录的发行 `wjsm-exec` stub 约 27 MiB（引入本阶段前）。引入 ICU4X full compiled_data 后，在 x86_64 Linux 上 `cargo build --release --bin wjsm-exec`（`lto = "thin"`、`strip = "symbols"`）测得 **29.8 MiB**（31,266,392 字节）。debug 体积不作合同。

| 制品 | 配置 | 大小 |
| --- | --- | --- |
| `wjsm-exec`（本阶段前，ADR 0018） | release | ~27 MiB |
| `wjsm-exec`（本阶段后） | release，x86_64 Linux，thin LTO，strip symbols | 29.8 MiB |

不得靠删除非英语 locale 压体积。

## 相关章节

- [ADR 0020](../../../../adr/0020-icu4x-baked-data.md)
- [项目目标与非目标](goals-and-non-goals.md)
- [跨 crate 所有权与依赖边界](ownership-and-dependencies.md)
