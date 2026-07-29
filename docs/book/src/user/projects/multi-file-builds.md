# 多文件构建

多个模块被编译成**一个** WebAssembly 模块。wjsm 从入口出发构建模块图，把所有依赖 bundle 进同一份 IR，
再一次性生成 Wasm。

```text
src/
  main.js          import { add } from "./util/index.js"
  util/
    index.js       export { add } from "./math.js"
    math.js        export const add = (a, b) => a + b
```

```bash
wjsm run src/main.js
wjsm build src/main.js -o app.wasm
```

入口的依赖会被自动发现，不需要列出每个文件。

## `--root` 的作用

`--root <DIR>` 指定模块解析的根目录。不给时以入口文件所在位置为基准。

```bash
wjsm build src/main.js --root . -o app.wasm
```

`-v` 会打印 `Bundling modules...`，据此可以确认走的是 bundling 路径而不是单文件路径。

## 求值顺序与循环依赖

依赖按深度优先顺序求值：被依赖的模块先执行完，再回到引用它的模块。

循环依赖不会报错，但会暴露「尚未初始化」的绑定。当前实现在这种情况下读到的是绑定的零值，
而不是按规范抛 `ReferenceError`：

```js
// cyc1.js
import { b } from "./cyc2.js";
export const a = 1;
console.log("in cyc1, b =", b);

// cyc2.js
import { a } from "./cyc1.js";
export const b = 2;
console.log("in cyc2, a =", a);
```

```text
in cyc2, a = 0
in cyc1, b = 2
```

`cyc2` 先求值，此时 `cyc1` 的 `a` 还没赋值，读到 `0`。把跨模块访问推迟到函数调用时就没有这个问题：

```js
// cyc4.js
export function log() { console.log("deferred a =", a); }
```

```text
deferred a = 1
```

设计上不要依赖循环依赖中的顶层求值时序。

## 产物体积

bundle 进来的模块只增加 `Code` 和 `Data` 段。上面三文件示例编译出 26174 字节，其中 `Code` 段 2564 字节；
单个 `console.log(1)` 的产物是 25686 字节。差值就是你的代码，其余是固定的宿主 ABI 声明开销。

## 与 npm 包一起构建

`node_modules` 里的依赖同样被 bundle 进产物，不需要在运行时提供。这意味着一个 `.wasm` 是自包含的
（除了 wjsm 宿主 import），但也意味着依赖变更后必须重新编译。

## 深入了解

- [模块图与 Bundling 阶段](../../internals/pipeline/bundle.md)
- [IR Program Bundling 的合并规则](../../internals/modules/program-bundling.md)
- [循环依赖、缓存与求值顺序](../../internals/modules/cycles-and-cache.md)
