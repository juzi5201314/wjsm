# Owner 与单一事实来源

这一章汇总 wjsm 的 owner 约束和单一事实来源。

## 单一 owner

| 关注点 | Owner crate | 约束 |
| --- | --- | --- |
| Wasmtime Config 构造/mutation | `wjsm-host-wasm/src/engine_config.rs` | 其他 crate 不构造 Config |
| NaN-box 值编码 | `wjsm-ir/src/value.rs` | `BOX_BASE`、`TAG_*` 定义在这里 |
| ABI 常量 | `wjsm-ir/src/constants.rs` | global 名、类型索引等 |
| 快照格式 | `wjsm-snapshot-format` | 编解码和版本号 |
| GC 算法注册表 | `wjsm-gc/src/registry.rs` | `GcAlgorithmKind` |
| 内置模块表 | `wjsm-module/src/builtin_modules.rs` | `node:` 模块 |
| Builtin 拦截 | `wjsm-semantic/src/builtins.rs` | `Builtin` enum |

## 依赖边界

ADR 0011–0013 的约束：

- `wasmtime` 依赖只在 `wjsm-host-wasm`。
- `wjsm-builtins`、`wjsm-host`、`wjsm-gc`、`wjsm-module` 不依赖后端。
- `wjsm-runtime` 是 facade，只 re-export。
- `wjsm-gc` 通过 `GcContext` / `RootProvider` 接合层访问 wasmtime 内存，不直接依赖 wasmtime。

## 用户侧事实来源

| 关注点 | 来源 |
| --- | --- |
| 用户行为和 CLI | README.md 和 `wjsm --help` |
| 架构边界和不变量 | `docs/adr/`，尤其是 0010–0013 |
| 新后端契约 | `docs/backend-implementation-guide.md` |
| Fixture 和测试机制 | `build.rs`、`tests/`、`fixtures/`、`.config/nextest.toml` |

## ECMAScript 规范

ECMAScript 是语义的事实来源。不 ship 部分语义、跳过的边界情况或无效早期错误行为。语言问题用精确规范文本回答。

## 深入了解

- [核心不变量](invariants.md)
- [Workspace crate 地图](../foundations/crate-map.md)
- [ADR 导航](adr-index.md)
