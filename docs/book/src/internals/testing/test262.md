# test262 一致性测试

这一章说明 wjsm 与 test262 的集成。

## test262 简介

test262 是 ECMAScript 官方一致性测试套件，覆盖语言的全部规范行为。wjsm 用 test262 验证语义实现的正确性。

## crate 组织

`crates/wjsm-test262/` 提供独立的一致性测试 runner：

- `src/read.rs` 读取 test262 元数据、harness 和测试树。
- `src/exec.rs` 为每个用例启动独立 wjsm 子进程，执行超时和内存限制并汇总结果。
- `src/main.rs` 提供套件、单文件、并行度和报告选项。

test262 用例不会由 `build.rs` 转换成 nextest 测试函数；runner 直接遍历已检出的 `test262/` 子模块。

## 运行方式

```bash
git submodule update --init test262
cargo run --release -p wjsm-test262 -- run --suite test/built-ins --plain
```

runner 默认给每个独立子进程 60s 超时，并按内存预算限制并发；可用 `--timeout-secs` 或 `WJSM_TEST262_TIMEOUT_SECS` 调整。`cargo nextest run -p wjsm-test262` 只运行 runner 自身的 Rust 单元测试，不执行 test262 套件。

## 覆盖范围

test262 覆盖 ECMAScript 的核心行为：类型转换、运算符、语句、函数、对象、数组、字符串、正则、Promise、Symbol、迭代器、生成器、async、类、模块等。

wjsm 可能不通过所有 test262 用例——未实现的特性或不完整语义会失败。失败的用例记录在已知差异里（见[限制与已知差异](../../user/runtime/limitations.md)）。

## 深入了解

- [Fixture 测试框架](fixtures.md)
- [语义 IR 快照](semantic-snapshots.md)
- [用户侧的限制与已知差异](../../user/runtime/limitations.md)
