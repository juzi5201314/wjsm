# 测试项目

wjsm 自带的 `test` 命令是一个运行器，不是断言框架：它逐个执行测试文件，按退出码判定成功或失败。

## 文件发现

目录参数会递归查找四种后缀：`*.test.js`、`*.test.ts`、`*_test.js`、`*_test.ts`。其他后缀不参与发现，`login.test.mjs` 这样的文件不会被匹配，需要改名或直接把路径作为参数传入。

```bash
wjsm test ./tests
wjsm test ./tests/login.test.ts
```

## 编写测试

没有内置的 `describe` / `it`。判定标准是「进程是否以退出码 0 结束」，所以断言用抛异常表达：

```js
function assertEqual(actual, expected, label) {
  if (actual !== expected) {
    throw new Error(`${label}: expected ${expected}, got ${actual}`);
  }
}

assertEqual(1 + 2, 3, "sum");
console.log("sum ok");
```

任何未捕获异常都会让该文件失败，其余文件继续执行。

## 结果汇总

每个文件的结果打印到标准错误，最后一行是汇总：

```text
FAILED ./c.test.js (exit ExitCode(unix_exit_status(2)))
test result: 1 passed; 1 failed
```

只要有一个文件失败，`wjsm test` 就以退出码 1 结束，可直接用于 CI 判定。`-v` 会额外打印每个文件的开始与通过记录。

> <details><summary>不内置断言库，但怎么写测试不痛苦？</summary>
>
> 几行 helper 就能用得很顺手：
>
> ```js
> // tests/_helpers.js
> export function assert(cond, msg) {
>   if (!cond) throw new Error("assertion failed: " + (msg ?? ""));
> }
> export function assertEqual(a, b, msg) {
>   if (a !== b) throw new Error(`${msg ?? "values differ"}: expected ${b}, got ${a}`);
> }
> export async function assertRejects(p, msg) {
>   try { await p; throw new Error(`${msg ?? "expected rejection, got fulfillment"}`); }
>   catch (e) { /* 期望的拒绝 */ }
> }
> ```
>
> 然后在每个测试文件里 `import { assert, assertEqual } from "./_helpers.js";` 就好。20 行的 helper 覆盖 80% 的断言需求。
>
> 想要更完整的库？目前生态里没有「为 wjsm 设计的」测试框架。直接用 `node:test`（Node 内置的）写测试、用 wjsm 跑——`node:test` 的 API 是稳定的子集，在 wjsm 下基本都能跑。
>
> </details>

## 与 package.json 脚本的关系

不带路径参数执行 `wjsm test` 时，如果 `package.json` 里定义了 `test` 脚本，wjsm 会执行该脚本而不是自己发现文件。这让你可以把测试委托给别的工具，同时保留 `wjsm test` 作为统一入口。

## 深入了解

- [Fixture 测试框架如何驱动 1500+ 条端到端用例](../../internals/testing/fixtures.md)
- [test262 一致性测试的运行方式与超时模型](../../internals/testing/test262.md)
