# build.rs 工件流水线

这一章说明 `build.rs` 如何在构建期生成测试用例和工件。

## build.rs 的职责

根 `build.rs` 扫描 `fixtures/` 目录，把每个 `.js` + `.expected` 对变成一个 Rust 测试函数：

| 目录 | 生成 |
| --- | --- |
| `fixtures/happy/` | `happy__<name>` 测试函数 |
| `fixtures/errors/` | `errors__<name>` 测试函数 |
| `fixtures/modules/` | `modules__<name>` 测试函数 |

新增 fixture 不需要手写测试代码——放好 `.js` 与 `.expected` 文件，`build.rs` 自动扫描生成。

## 测试用例生成流程

```text
build.rs
  1. 扫描 fixtures/happy/*.js
  2. 为每个 .js 查找对应的 .expected
  3. 生成 fn happy__<name>() 测试函数
  4. 写入 $OUT_DIR/fixture_tests.rs
  5. tests/integration.rs 和 tests/unit.rs include! 这个文件
```

生成的测试函数名规则：目录前缀 `__` 文件名（去掉 `.js` 后缀，特殊字符替换为 `_`）。

## wjsm-test262 的 build.rs

`crates/wjsm-test262/build.rs` 在构建期处理 test262 测试用例，生成对应的 Rust 测试函数。test262 子模块按需初始化。

## 深入了解

- [仓库布局](../foundations/repository-layout.md)
- [Fixture 测试框架](../testing/fixtures.md)
- [生成文件与缓存边界](generated-artifacts.md)
