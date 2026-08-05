# 兼容性与支持范围

这一章说明可以对 wjsm 抱有什么预期，以及哪些预期会落空。核心结论：wjsm `0.1.0` 是实验性运行时，不是 Node.js 的替代品，也不保证任意 npm 包能跑。

## 判断标准

wjsm 不维护静态的兼容性 Roadmap 表。实际支持范围由三类可执行证据界定：

| 证据 | 位置 | 含义 |
| --- | --- | --- |
| 行为 fixture | `fixtures/happy`（676 项）、`fixtures/errors`（78 项） | 已验证的可观察行为与错误信息 |
| 模块 fixture | `fixtures/modules`（71 项） | 已验证的模块加载与包解析行为 |
| Test262 | `wjsm-test262` runner + `test262` 子模块 | ECMAScript 一致性覆盖情况 |

没有被这三者覆盖的语义，应当视为未受支持——即使它看起来「应该能工作」。

## 源码语言

按扩展名选择解析模式：`.js`、`.mjs`、`.cjs`、`.jsx`、`.ts`、`.tsx`。

TypeScript 语法参与解析和降级，类型注解被擦除。wjsm **不是类型检查器**：类型错误不会被报告，`tsc` 该报的错这里不报。想要类型检查请单独跑 `tsc --noEmit`。

> <details><summary>「wjsm 不做类型检查」具体意味着什么？</summary>
>
> ```ts
> const x: number = "hello"  // tsc 会报错；wjsm 不会
> const y: string = 42       // 同样
> ```
>
> `wjsm check` 对这两条都返回成功（退出码 0），因为它们语法合法、TypeScript 类型擦除后剩下的 JavaScript 也是合法的。wjsm 只关心代码「能不能跑」，不关心类型「匹不匹配」。
>
> 生产项目里正确的做法是同时跑：
>
> ```bash
> wjsm check src/ --root .   # 检查语法和语义
> tsc --noEmit              # 检查类型
> ```
>
> 两个工具各管一摊，不要假设 wjsm 帮你抓了类型错。
>
> </details>

## 执行后端

`--target` 有两个取值，但只有一个可用：

- `wasm`（默认）：唯一的生产路径。
- `jit`：仓库中只有静态接入契约，没有实现。传 `--target jit` 会失败。

## 语言语义

已覆盖的范围包括作用域与 TDZ、闭包、类与私有字段、异常、生成器、`async`/`await`、Promise 及其组合器、`Map`/`Set`/`WeakMap`/`WeakSet`、`ArrayBuffer`/`DataView`/TypedArray、`Proxy`/`Reflect`、`Symbol`、正则表达式、`JSON`。

locale 敏感的方法（`toLocaleString` 系列的完整语言支持等）不承诺与任何具体 ICU 数据一致。

## 模块与包

- ESM 与 CommonJS 都支持，包括动态 `import(expr)`、`require`、`require.cache`、`import.meta.resolve()`。
- `node_modules` 解析、`exports` 条件导出、`browser` 字段映射可用。
- `wjsm install` 可以从 npm registry 拉取包。

能装上不等于能跑：依赖原生插件（N-API）、依赖未实现的 Node API、或依赖 V8 特有行为的包会在运行时失败。

## Node.js 与 Web API

已实现的部分包括 `console`、`process`、Fetch、Streams、定时器、`node:vm`、`node:async_hooks`、`node:worker_threads`、`node:perf_hooks`、文件系统、子进程、网络与 TLS。

这些实现覆盖各自模块的常用面，不是完整实现。`process.versions` 同时报告一个 `node` 版本号和 wjsm 自身版本号，前者是为了让检测 Node 环境的库能继续工作，不代表 API 面与该 Node 版本对齐。

## 安全边界

文件系统与子进程默认受限，不是默认开放：

- 文件写入需要 `WJSM_FS_ALLOW_WRITE=1`。
- 额外的读取根目录通过 `WJSM_FS_ALLOW_READ` 追加。
- `child_process` 默认禁用，需要 `WJSM_CHILD_PROCESS_ALLOW` 列出命令或设为 `*`。

## 许可证

仓库当前没有 `LICENSE` 文件。除维护者另行授权，不要假定代码已按某个开源许可证发布。

## 深入了解

- [多后端契约与 JIT 后端边界](../../internals/backend/jit-backend.md)
- [Test262 一致性测试的运行方式与统计口径](../../internals/testing/test262.md)
- [Fixture 测试框架如何界定可观察行为](../../internals/testing/fixtures.md)
- [Node.js Built-in 模块的组织与实现范围](../../internals/runtime-features/node-builtins.md)
