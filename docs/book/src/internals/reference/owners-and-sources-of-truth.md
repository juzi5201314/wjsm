# Owner 与单一事实来源

这一章汇总 wjsm 的 owner 约束和单一事实来源。

## 单一 owner

| 关注点 | Owner crate | 约束 |
| --- | --- | --- |
| NaN-box 值编码 | `wjsm-ir/src/value.rs` | `BOX_BASE`、`TAG_*` 定义在这里 |
| ABI 常量 | `wjsm-ir/src/constants.rs` | global 名、类型索引等 |
| GC 算法注册表 | `wjsm-gc/src/registry.rs` | `GcAlgorithmKind` |
| 内置模块表 | `wjsm-module/src/builtin_modules.rs` | `node:` 模块 |
| Builtin 拦截 | `wjsm-semantic/src/builtins.rs` | `Builtin` enum |
| Cranelift ISA 构造 | `wjsm-backend-native/src/isa_config.rs` | 唯一 ISA/flags owner |
| CLDR/Unicode 数据 | `wjsm-intl-data` | 唯一 ICU4X compiled_data / IDNA / Encoding provider |

ADR 0014 的约束：

- Cranelift 依赖只在 `wjsm-backend-native` 和 `wjsm-host-native`。
- `wjsm-builtins`、`wjsm-host`、`wjsm-gc`、`wjsm-module`、`wjsm-intl-data` 后端无关。
- `wjsm-runtime` 是 facade，只 re-export。
- `wjsm-gc` 通过 `GcContext` / `RootProvider` 接合层访问内存。

## 用户侧事实来源

| 关注点 | 来源 |
| --- | --- |
| 用户行为和 CLI | README.md 和 `wjsm --help` |
| 架构边界和不变量 | `docs/adr/`，尤其是 0010、0012、0014、0020 |
| Fixture 和测试机制 | `build.rs`、`tests/`、`fixtures/`、`.config/nextest.toml` |

## ECMAScript 规范

ECMAScript 是语义的事实来源。不 ship 部分语义、跳过的边界情况或无效早期错误行为。语言问题用精确规范文本回答。

## 深入了解

- [核心不变量](invariants.md)
- [Workspace crate 地图](../foundations/crate-map.md)
- [ADR 导航](adr-index.md)
