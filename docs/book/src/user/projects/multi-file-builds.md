# 多文件构建

单文件入口直接 `wjsm run app.ts` 就够。多文件项目需要让 wjsm 知道模块解析根在哪里，其余按 ESM / CJS 标准语义处理。

## 模块解析根

`--root <DIR>` 指定模块解析根目录。不带 `--root` 时，入口文件所在目录自动成为根：

```bash
wjsm run src/main.ts                 # root = src/
wjsm run --root . src/main.ts        # root = .
```

根目录影响的是 bundling 阶段：wjsm 从入口开始 DFS 遍历 `import` / `require`，把整个模块图收集起来，按拓扑序合并为一个 Program。根目录之外的非入口模块不会被自动拉入图中，除非入口可达。

多文件入口一律走 bundling，即使入口只有一个文件——只要它 `import` 了别的模块。

## ESM 与 CJS 混用

wjsm 允许在同一个项目里混用 ES 模块和 CommonJS 模块。格式判定按扩展名和 `package.json` 的 `type` 字段：

```js
// math.mjs — ESM
export function add(a, b) { return a + b; }
```

```js
// util.cjs — CommonJS
module.exports = { log: (x) => console.log(x) };
```

```js
// main.mjs — 混用
import { add } from "./math.mjs";
const util = require("./util.cjs");
util.log(add(1, 2));
```

CJS 模块在编译期被重写为 ESM 等价形式（`module.exports` → `export default`，顶层 `require` → `import`），因此运行时只走一条 ESM 路径。从 ESM 导入 CJS 时，`module.exports` 作为默认导出，具名导入不会被静态分析出来。

详细规则见 [ES 模块](esm.md) 和 [CommonJS 与 Node 模块](commonjs-and-node.md)。

## 循环依赖

wjsm 允许循环依赖，行为与 V8、Node 一致：循环成员按拓扑序先执行的那个，会看到后执行模块的绑定尚未初始化。

### 可观察行为

直接读未初始化的绑定得到 `undefined`；延迟到函数调用时读，则拿到正确值。原因是 ESM 绑定是引用而非拷贝——调用时绑定已就位。

```js
// a.mjs
import { b } from "./b.mjs";
export const a = 1;
console.log("a.mjs sees b =", b);   // b 还没初始化 → undefined
```

```js
// b.mjs
import { a } from "./a.mjs";
export const b = 2;
export function readA() { return a; }
console.log("b.mjs sees a =", a);   // a 已经初始化 → 1
```

```bash
$ wjsm run a.mjs
a.mjs sees b = undefined
b.mjs sees a = 1
```

执行顺序是 `a.mjs` 先跑（入口优先）。`a.mjs` 顶层执行时 `b.mjs` 还没跑完，`b` 的绑定是 `undefined`。之后 `b.mjs` 跑完，`a` 已经赋值。如果 `a.mjs` 里不直接读 `b`，而是在函数里延迟读：

```js
// a2.mjs
import { readB } from "./b2.mjs";
export const a = 1;
console.log("a2.mjs calls readB() =", readB());  // b 已就位 → 2
```

```js
// b2.mjs
import { a } from "./a2.mjs";
export const b = 2;
export function readB() { return b; }
console.log("b2.mjs sees a =", a);
```

```bash
$ wjsm run a2.mjs
b2.mjs sees a = 1
a2.mjs calls readB() = 2
```

延迟到函数调用时读，绑定已经就位，拿到正确值。

### 什么会出问题

在初始化时序里直接读未初始化的绑定会拿到 `undefined`（NaN-box 编码），这不是报错——程序会继续跑，但行为可能不符合预期。如果循环成员之间只在函数体内互相引用（不在顶层读），就不会踩到这个问题。

## 深入了解

- [ES 模块](esm.md)
- [CommonJS 与 Node 模块](commonjs-and-node.md)
- [循环依赖、缓存与求值顺序](../../internals/modules/cycles-and-cache.md)
