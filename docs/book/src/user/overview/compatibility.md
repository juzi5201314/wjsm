# 兼容性与支持范围

## 平台支持

| 目标 | 状态 |
| --- | --- |
| x86_64 Linux | 生产支持 |
| x86_64 Windows | 生产支持 |
| 其他 target | fail-closed |

native compiler 初始化时若检测到不支持的宿主，返回结构化 capability error，不会回退到另一执行后端。没有 Wasm sandbox——artifact verifier、checked lowering、strict relocation、symbol allowlist 与 W^X 是受信编译/加载边界，不等同于进程隔离。

## ECMAScript 兼容

ECMAScript 是子集，不是完整实现。已覆盖作用域与 TDZ、闭包、类、异常、生成器、`async`/`await`、Promise、集合、TypedArray、Proxy/Reflect 等大量语义，但完整度没有静态承诺——以 `fixtures/`、crate 测试与 Test262 runner 的实际覆盖为准。尚未覆盖的语义视为不受支持。

## Node.js 兼容

内置 24 个 Node.js 模块封装，`node:` 前缀和裸名都能解析。这些模块是 wjsm 自有的 JS 实现，不是 Node.js 移植——每个模块覆盖的是常用 API 子集，行为不一定与 Node 逐字一致。

全局对象方面，`process`、`Buffer`、`TextEncoder`、`TextDecoder`、`structuredClone`、`queueMicrotask`、`atob`、`btoa`、`performance`、`setImmediate`、`clearImmediate` 可用；`fetch` 与 Streams 构造器只能直接调用，取值得到 `undefined`。`Intl` 未实现，依赖它的方法会 trap。

## TypeScript

TypeScript 语法参与解析与 lowering，类型注解、`interface`、类型别名、泛型、`as` 断言、`satisfies`、装饰器、`enum`、JSX/TSX 都能编译；类型本身不做检查——这是 `tsc` 的职责。少数形态（如构造器参数属性 `constructor(public a)`）不可用，需显式赋值。

## 怎么判断某个 API 能不能用

直接跑：

```bash
wjsm run -e 'import { join } from "node:path"; console.log(join("a", "b"))'
```

看错误信息比查表快。`Unknown built-in module` 表示没有内置封装；运行时 trap 表示该 API 尚未实现。配合 [语言功能矩阵](../reference/language-matrix.md) 和 [Node.js 兼容矩阵](../reference/node-compatibility-matrix.md) 可以缩小范围，但最终以实际运行结果为准。

## 深入了解

- [语言功能矩阵](../reference/language-matrix.md)
- [Node.js 兼容矩阵](../reference/node-compatibility-matrix.md)
- [限制与已知差异](../runtime/limitations.md)
