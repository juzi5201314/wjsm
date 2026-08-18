# Fixture 测试框架

这一章说明 `fixtures/` 目录的组织和 fixture 测试机制。

## 四类 fixture

| 目录 | 内容 | 用途 |
| --- | --- | --- |
| `fixtures/happy/` | `.js` + `.expected` 对 | 验证正确行为 |
| `fixtures/errors/` | `.js` + `.expected` 对 | 验证错误行为 |
| `fixtures/modules/` | 多文件项目 | 验证模块行为 |
| `fixtures/slow/` | `.js` + `.expected` 对 | 验证复杂度和压力边界，不进入默认快速套件 |

每个可执行 fixture 是一对文件：`foo.js` 是输入，`foo.expected` 记录退出码、标准输出和标准错误。测试框架运行 wjsm 并比较完整快照。没有 `.expected` 的输入不会生成运行时测试；它可以作为模块依赖或语义 IR 快照的源码，例如 `fixtures/happy/labeled.js`。

## 测试生成

`build.rs` 扫描 `happy`、`errors`、`modules` 和 `slow` 四个目录，只为同时存在 `.expected` 的 `.js`、`.ts` 或 `.tsx` 输入生成测试。生成函数由 `tests/integration.rs` 引入，名称是 `happy__foo`、`errors__foo`、`modules__foo` 或 `slow__foo`。

```bash
cargo nextest run -E 'test(happy__hello)'
WJSM_UPDATE_FIXTURES=1 cargo nextest run -E 'test(happy__<name>)'
cargo nextest run -P slow -E 'test(slow__)'
```

## 更新 fixture

`WJSM_UPDATE_FIXTURES=1` 环境变量让测试框架更新 `.expected` 文件而不是比较。修改行为后用这个变量更新期望输出，但要审查变更——不要通过修改 fixture 来避开正确逻辑。

## nextest profile

`.config/nextest.toml` 定义三个互补入口：

- 默认 profile：无独占进程、网络、PTY、Loom、大地址空间或压力负载的快速正确性测试。
- `full`：全部测试，用于提交前和跨 crate 验证。
- `slow`：默认 profile 排除的资源、并发和压力测试，可独立运行。

三个 profile 都保留 3s 单例硬超时；需要端口、共享缓存或高 CPU 的测试仍通过 test group 与 `threads-required` 隔离。`fail-fast = false`，一次运行会报告全部失败。

## 深入了解

- [语义 IR 快照](semantic-snapshots.md)
- [Backend 与 Runtime 定向测试](backend-and-runtime-tests.md)
- [用户侧的测试工作流](../../user/workflows/testing.md)
