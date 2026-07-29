# Fixture 测试框架

这一章说明 `fixtures/` 目录的组织和 fixture 测试机制。

## 三类 fixture

| 目录 | 内容 | 用途 |
| --- | --- | --- |
| `fixtures/happy/` | `.js` + `.expected` 对 | 验证正确行为 |
| `fixtures/errors/` | `.js` + `.expected` 对 | 验证错误行为 |
| `fixtures/modules/` | 多文件项目 | 验证模块行为 |

每个 fixture 是一对文件：`foo.js` 是输入，`foo.expected` 是期望的 stdout 输出。测试框架运行 `wjsm run foo.js`，比较实际输出与 `.expected`。

## 测试生成

`tests/` 目录的 `integration.rs` 和 `unit.rs` 通过 `build.rs` 自动生成 fixture 测试。每个 fixture 变成一个独立的 test 函数，名字是 `happy__foo` 或 `errors__foo`（目录前缀 + 文件名）。

```bash
cargo nextest run -E 'test(happy__hello)'
WJSM_UPDATE_FIXTURES=1 cargo nextest run -E 'test(happy__<name>)'
```

## 更新 fixture

`WJSM_UPDATE_FIXTURES=1` 环境变量让测试框架更新 `.expected` 文件而不是比较。修改行为后用这个变量更新期望输出，但要审查变更——不要通过修改 fixture 来避开正确逻辑。

## nextest 配置

`.config/nextest.toml` 配置测试运行：

- 默认 3s 硬超时，超过视为死锁或性能回归。
- 资源隔离组：`cluster-ipc`、`runtime-cache-delete`、`perf-hooks-runtime`、`async-hooks-load`、`worker-runtime` 等，每组 `max-threads = 1`。
- `fail-fast = false`，运行所有测试即使有失败。

## 深入了解

- [语义 IR 快照](semantic-snapshots.md)
- [Backend 与 Runtime 定向测试](backend-and-runtime-tests.md)
- [用户侧的测试工作流](../../user/workflows/testing.md)
