# `test`

运行测试文件，或直接跑一段内联测试代码。

```text
wjsm test [OPTIONS] [INPUT]
```

`INPUT` 可以是文件，也可以是目录；省略时按下面的顺序决定行为。

## 解析顺序

1. 给了 `-e <CODE>`：直接把这段代码当测试跑，不做文件发现。
2. 没给 `INPUT`，而且向上能找到定义了 `test` 脚本的 `package.json`：执行那个 npm 脚本。
3. 其余情况：`INPUT` 缺省为 `.`，按目录发现测试文件。

## 文件发现规则

目录会被递归遍历，命中四种后缀的文件按路径排序后逐个执行：

- `*.test.js`
- `*.test.ts`
- `*_test.js`
- `*_test.ts`

一个都没命中时报错退出，不会静默成功。

## 单个测试的判定

每个文件按 `wjsm run` 的方式编译执行，进程退出码为 0 记通过，否则记失败并打印 `FAILED <path>`。没有断言库或测试框架介入，测试文件自己用 `throw` 或 `process.exit` 表达失败。

```bash
wjsm test -e 'if (1 + 1 !== 2) throw new Error("bad"); console.log("ok")'
```

结束时汇总一行（`-q` 可关闭）：

```text
test result: 3 passed; 0 failed
```

只要有一个文件失败，整体退出码为 1。

> <details><summary>为什么 wjsm 的 `test` 不内置断言库？</summary>
>
> 设计取舍：wjsm 的定位是「可执行 ECMAScript 规范子集」，断言库和测试框架属于「怎么写测试」这一层——每个人的偏好不同（Mocha、Vitest、Jest、自写），强行内置会让一部分用户觉得多余，另一部分觉得不够。
>
> 当前的「进程退出码 0 = 通过」约定在 Node.js 生态里是事实标准：CI 系统（GitHub Actions、GitLab CI）都靠退出码判断成败。这种约定的好处是 zero-dep，坏处是写起来没有 `expect(x).toBe(y)` 那么顺手。
>
> 实际项目里常见的做法是：wjsm test 跑 happy-path 测试（不需要 mock、不需要 fake timer），复杂的集成测试用 Vitest 在 Node 里跑，wjsm 只负责跑 build 和 lint。
>
> </details>

## 相关选项

`--root`、`--script` 与 `run` 同义。`-v` 会额外打印每个文件开始和通过的记录。

## 深入了解

- [Fixture 测试框架如何驱动仓库自身的用例](../../internals/testing/fixtures.md)
- [Backend 与 Runtime 定向测试的分层](../../internals/testing/backend-and-runtime-tests.md)
