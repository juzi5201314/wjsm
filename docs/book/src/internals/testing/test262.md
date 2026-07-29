# test262 一致性测试

这一章说明 wjsm 与 test262 的集成。

## test262 简介

test262 是 ECMAScript 官方一致性测试套件，覆盖语言的全部规范行为。wjsm 用 test262 验证语义实现的正确性。

## crate 组织

`crates/wjsm-test262/` 是 test262 集成 crate：

- `build.rs`：构建时处理 test262 测试用例。
- `src/`：测试 harness 和运行器。

crate 在构建期下载或引用 test262 测试用例，生成对应的 Rust 测试函数。每个 test262 用例变成一个独立的测试。

## 运行方式

```bash
cargo nextest run -p wjsm-test262
```

test262 测试通常较慢，nextest 的并行执行和超时配置帮助管理运行时间。

## 覆盖范围

test262 覆盖 ECMAScript 的核心行为：类型转换、运算符、语句、函数、对象、数组、字符串、正则、Promise、Symbol、迭代器、生成器、async、类、模块等。

wjsm 可能不通过所有 test262 用例——未实现的特性或不完整语义会失败。失败的用例记录在已知差异里（见[限制与已知差异](../../user/runtime/limitations.md)）。

## 深入了解

- [Fixture 测试框架](fixtures.md)
- [语义 IR 快照](semantic-snapshots.md)
- [用户侧的限制与已知差异](../../user/runtime/limitations.md)
