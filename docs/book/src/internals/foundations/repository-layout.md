# 仓库布局

这一章说明顶层目录各自承担什么，改动某类内容时应该落在哪里。

## 顶层结构

```text
wjsm/
├── src/main.rs              根二进制入口，仅调用 wjsm_cli::main_entry()
├── build.rs                 生成 fixture 测试用例列表
├── Cargo.toml               workspace 定义 + 依赖版本 + profile
├── crates/                  全部 15 个 workspace 成员
├── fixtures/                端到端行为用例与 IR 快照
├── tests/                   根级集成测试与 fixture runner
├── docs/                    ADR、设计文档、本手册
├── test262/                 Test262 子模块（按需初始化）
├── fuzz/                    fuzz target
└── .config/nextest.toml     测试超时与 test-group 配置
```

## 各目录的判断标准

| 要改的内容 | 落点 |
| --- | --- |
| 某个 crate 的实现 | `crates/<crate>/src/` |
| 用户可观察行为 | `fixtures/happy`、`fixtures/errors`、`fixtures/modules` + `.expected` |
| Lowering 结果 | `fixtures/semantic` IR 快照 |
| 架构决策 | `docs/adr/`，新增编号文件 |
| 测试超时与分组 | `.config/nextest.toml` |
| 依赖版本与 profile | 根 `Cargo.toml` |

## fixtures 三个套件

`build.rs` 扫描 `happy`、`errors`、`modules` 三个套件并生成测试用例列表，所以新增 fixture 不需要手写测试函数，放好 `.js` 与 `.expected` 即可。

| 套件 | 内容 | 规模 |
| --- | --- | --- |
| `fixtures/happy` | 成功路径的可观察输出 | 约 1357 项 |
| `fixtures/errors` | 期望报错的用例 | 约 158 项 |
| `fixtures/modules` | 模块系统与运行时加载 | 约 71 项 |
| `fixtures/semantic` | IR lowering 快照 | 由 `lowering_snapshots` 使用 |

## 生成物与临时文件

构建产物在 `target/`，本手册的构建输出写 `/tmp`，不提交 `docs/book/book/`。临时验证脚本不要落在仓库里，用 `-e` 传内联源码或写 `/tmp`。

## 相关章节

- [Workspace crate 地图](crate-map.md)
- [Fixture 测试框架](../testing/fixtures.md)
- [`build.rs` 工件流水线](../build-release/build-script.md)
