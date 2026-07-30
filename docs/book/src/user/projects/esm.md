# ES 模块

wjsm 默认按 ES 模块解析源码：`import` / `export` 直接可用，顶层 `await` 可用，`await` 是保留字。

```js
// a.mjs
export const v = 41;
export default function bump(x) {
  return x + 1;
}
```

```js
// main.mjs
import bump, { v } from "./a.mjs";
console.log(bump(v));
```

```bash
wjsm run main.mjs
```

## 支持的语法

| 形式 | 状态 |
| --- | --- |
| 具名导入导出 `import { a } from` / `export { a }` | 可用 |
| 默认导入导出 | 可用 |
| 命名空间导入 `import * as ns from` | 可用 |
| 重导出 `export * from` / `export { a } from` | 可用 |
| 动态 `import(expr)` | 可用，返回 Promise |
| 顶层 `await` | 可用 |
| `import.meta.resolve()` | 可用 |
| Import assertions / attributes | 未实现 |

动态导入在编译期不需要是字面量，运行期解析：

```js
const mod = await import("./a.mjs");
console.log(mod.v);
```

> <details><summary>动态 `import()` 为什么是返回 Promise 的「异步」？</summary>
>
> ESM 规范里 `import()` 故意设计成返回 Promise，目的是把模块加载的异步性暴露给调用方。考虑：
>
> - **网络模块**：跨网络加载的模块一定有网络延迟，Promise 是统一的「我稍后才能给结果」抽象。
> - **本地模块**：本地模块虽然快，但「不阻塞调用方代码」这件事仍然是好的——允许主流程继续执行、不在加载时卡死 UI。
>
> 这个设计在浏览器里是必须的（fetch 模块天然异步），在 Node.js 里是历史包袱（CJS 的 `require` 是同步的），在 wjsm 里是「选择了 ESM 就要接受这个语义」。
>
> 实际影响：你的代码里 `await import(...)` 是同步逻辑——`await` 会等 Promise resolve，所以读起来像同步，但底层是异步的。把它和 `require` 混用会出错。
>
> </details>

## 何时按 ESM 处理

格式判定的顺序是扩展名优先，然后看最近的 `package.json`：

| 条件 | 判定 |
| --- | --- |
| `.mjs` | ESM |
| `.cjs` | CommonJS |
| `.js` + `package.json` 有 `"type": "module"` | ESM |
| `.js` + `package.json` 有 `"type": "commonjs"` | CommonJS |
| `.js` + 无 `package.json`，且 AST 中出现 `require` / `module.exports` | CommonJS |
| `.js` + 无 `package.json`，无 CJS 语法 | ESM |
| `.ts` / `.tsx` / `.jsx` | 无 CJS 语法时为 ESM |

包目录里的 `.js` 文件按该包的 `type` 判定。给一个没有 `"type"` 字段的包写 `import`/`export` 会报 `Cannot use import/export syntax in CommonJS module`——这是格式判定的结果，不是语法错误。

## 解析规则

相对路径的扩展名可以省略，按 `js`、`ts`、`mjs`、`cjs`、`jsx`、`tsx` 顺序尝试；指向目录时找该目录下的 `index.<ext>`，顺序相同。裸包名走 `node_modules` 查找，见[包解析与条件导出](package-resolution.md)。

## 内置模块

Node.js 内置模块通过 `node:` 前缀或裸名导入：

```js
import path from "node:path";
import { EventEmitter } from "events";
```

完整清单与限制见 [Node.js 兼容能力](../runtime/node-compatibility.md)。

## 深入了解

- [模块语义与 lowering 处理](../../internals/frontend/module-semantics.md)
- [ESM 链接与求值顺序](../../internals/modules/esm-linking.md)
- [模块图与解析器实现](../../internals/modules/graph-and-resolution.md)
