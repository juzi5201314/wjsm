# 限制与已知差异

这些是 wjsm 当前版本（`0.1.0`）与 ECMAScript 规范或 Node.js 行为之间的已知差异。大部分源于 Direct Cranelift 架构的编译策略：builtin 方法在 IR 中以调用形态存在，不作为可读属性暴露。遇到不确定的行为时，跑一遍比查表快：

```bash
wjsm run -e 'console.log(typeof [].map)'
```

## Builtin 方法拦截

wjsm 在语义层拦截内置方法调用，生成专用的 `CallBuiltin` 指令。这些方法在 IR 中不存在属性形态——取值时得到 `undefined`：

```js
typeof [].map        // "undefined"，但 [].map(x => x) 正常工作
typeof "abc".slice   // "undefined"，但 "abc".slice(1) 正常工作
typeof fetch         // "undefined"，但 fetch(url) 正常工作
```

直接调用能跑通；传递方法引用、解构赋值或 `Reflect.get` 取值则不行。

不同类型受影响范围不同，下文逐条说明。

## String 原型方法

`slice`、`concat`、`includes`、`startsWith`、`indexOf` 可取值传递。其余方法（`replace`、`split`、`match`、`trim` 等）取值得到 `undefined`，只能在调用点使用：

```js
"hello".replace("l", "L")      // 可用：直接调用
const fn = "hello".replace     // undefined：取值失败
```

## TypedArray / DataView 原型方法

TypedArray 原型方法和 DataView 访问器仅调用点可用，取值得到 `undefined`：

```js
const buf = new Uint8Array([1, 2, 3]);
buf.set([4, 5])                 // 可用：直接调用
const get = buf.subarray        // undefined：取值失败
```

## fetch 与 Streams 构造器

`fetch`、`Headers`、`Request`、`Response`、`ReadableStream`、`WritableStream`、`TransformStream`、`AbortController` 在全局名单中，但只能直接调用：

```js
fetch("https://example.com")           // 可用
const f = fetch                         // undefined
new Response("body")                   // 可用
const R = Response                      // undefined
```

## Map / Set / WeakMap / WeakSet 的 instanceof

这四个类型上的 `instanceof` 产生异常值（非布尔），不报错但结果不可靠：

```js
const m = new Map();
m instanceof Map    // 不是 true，也不是 false（异常值）
```

需要类型判断时用 `Object.prototype.toString.call(m) === "[object Map]"` 或 `typeof` + 构造器检查替代。

## TDZ 静态判定

`let` / `const` 的 Temporal Dead Zone 在 lowering 期静态判定。函数体内的前向引用会被拒绝——即使是合法的「延迟到声明后调用」模式：

```js
function f() {
  g();          // 合法调用，声明在前
  function g() {}
}
f();             // 上面这种没问题

function f() {
  g();           // 编译期拒绝：在 let 声明前访问
  let x = 1;
  function g() { return x }
}
```

类名引用在方法体内可用（延迟成员），但在静态字段初始值和 `extends` 等类定义期求值的位置仍报 TDZ。

## Intl 未实现

`Intl` 对象未实现，依赖它的方法会 trap。`Date.prototype.toLocaleString`、`Number.prototype.toLocaleString` 等 locale 敏感方法不提供 locale 定制，返回默认格式。

## URL / URLSearchParams 不是全局

`URL` 和 `URLSearchParams` 需要从 `node:url` 导入：

```js
import { URL, URLSearchParams } from "node:url";
```

`typeof globalThis.URL` 为 `undefined`。

## --format native-executable 未实现

`wjsm build --format native-executable` 当前返回稳定的未实现错误，退出码 1，不创建或覆盖输出文件。runtime 私有 native image 不是平台可执行文件，不能作为分发的二进制制品使用。

## 深入了解

- [语言功能矩阵](../reference/language-matrix.md)
- [Node.js 兼容矩阵](../reference/node-compatibility-matrix.md)
- [JavaScript 与 TypeScript 支持](javascript-and-typescript.md)
