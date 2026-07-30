# CommonJS 与 Node 模块

CommonJS 与 ES 模块可以在同一个项目里混用，wjsm 会按文件格式分别处理并打通互操作。

## 编写 CommonJS

```js
// lib.cjs
let n = 0;
module.exports = { bump: () => ++n, name: "lib" };
```

```js
// main.cjs
const lib = require("./lib.cjs");
console.log(lib.name, lib.bump(), lib.bump());
```

```text
lib 1 2
```

模块作用域内可用：`require`、`module`、`exports`、`__dirname`、`__filename`。

## `require` 的能力

| 形式 | 状态 |
| --- | --- |
| `require("./rel.cjs")` | 可用 |
| `require("pkg")` | 可用，走 `node_modules` 解析 |
| `require("node:path")` / `require("path")` | 可用 |
| `require("./data.json")` | 可用，解析为对象 |
| `require.resolve(spec)` | 可用，返回解析后的路径字符串 |
| `require.cache` | 可用，实时反映已加载模块 |

模块只在首次 `require` 时求值，后续拿到同一个 `module.exports`。上面的例子里 `bump()` 两次返回 1 和 2，说明 `n` 是模块级共享状态。

> <details><summary>「CommonJS 转换」到底做了什么？</summary>
>
> 严格说 wjsm 没有 CJS 运行时——它把 CJS 在编译期「重写」成 ESM 风格：
>
> - 顶层的 `const x = require('./p')` → 改写成 `import x from './p'`（直接用用户给的变量名）。
> - 控制流里的 `require(...)`（如函数体内的）保留为运行时调用——这部分不能静态改写。
> - `module.exports = obj` → 改写成 `export default obj`。
> - `module.exports.x = v` → 改写成 `let __cjs_x = v`，同时记入命名导出。
>
> 这样原本写 CJS 的代码在 wjsm 里的运行时路径和 ESM 完全一致——一套语义、一套模块图、一套 bundle 流程。代价是某些 CJS 极端用法（动态改 `module.exports`、运行时确定导出名）不支持。
>
> 这套机制的好处是：用户写 CJS，但 wjsm 内部只需要维护 ESM 一条路。坏处是：用户偶尔会写出「CJS 转换不识别」的代码，原样保留在产物里——这种情况下报错信息可能是「`module` is not defined」之类的，调试时要意识到「这是 wjsm 跳过了 CJS 转换」。
>
> </details>

## 从 ESM 导入 CommonJS

```js
// use.mjs
import lib from "./lib.cjs";
console.log(lib.name, lib.bump());
```

`module.exports` 作为默认导出。CommonJS 模块没有静态导出记录，因此不要指望具名导入（`import { bump } from "./lib.cjs"`）能被静态分析出来。

## Node.js 内置模块

24 个内置模块，`node:` 前缀和裸名两种写法都可以：

```js
import path from "node:path";
import { EventEmitter } from "events";
```

可用清单：`path`、`util`、`events`、`assert`、`url`、`querystring`、`os`、`fs`、`fs/promises`、`crypto`、`stream`、`http`、`net`、`https`、`zlib`、`child_process`、`dgram`、`tls`、`worker_threads`、`inspector`、`cluster`、`vm`、`async_hooks`、`perf_hooks`。

带 `node:` 前缀导入清单外的名字会在构建模块图时就失败：

```text
Failed to build module graph: Unknown built-in module 'node:not_real'
```

不带前缀的未知名字会按普通包名去 `node_modules` 查找，报的是「找不到模块」。

每个内置模块实现的是常用子集，不是完整 Node API。逐模块状态见 [Node.js 兼容能力](../runtime/node-compatibility.md)。

## 深入了解

- [CommonJS 到 ESM 的转换实现](../../internals/modules/commonjs-transform.md)
- [运行时模块加载与 require 缓存](../../internals/runtime-features/module-loading.md)
- [Node.js Built-in 模块的组织方式](../../internals/runtime-features/node-builtins.md)
