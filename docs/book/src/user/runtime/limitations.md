# 限制与已知差异

这一章列出与 Node.js 或 ECMAScript 预期不一致的地方。遇到异常行为时先在这里找，能省掉一轮调试。

## 内置原型方法不能作为值取出

`String`、TypedArray、`DataView` 的原型方法由语义层在**调用点**识别并转成 Builtin 调用，它们不是可读取的属性：

```bash
wjsm run -e 'const s = "ab"; console.log(typeof s.slice)'   # undefined
wjsm run -e 'const s = "ab"; console.log(s.slice(1))'       # b
```

直接调用正常，取值、解构、传递为回调则拿到 `undefined`：

```js
const slice = "ab".slice;        // undefined
[1, 2].map(new Uint8Array(2).fill);  // 不可用
```

`Array.prototype` 是例外，它的方法是真实属性，`typeof [].map === "function"`、`Array.prototype.map` 都成立。

`DataView` 的访问器同样受此影响：`dv.byteLength` 读到 `undefined`，但 `dv.getUint8(0)`、`dv.setUint32(0, x)` 可用。

## 内置构造器的 instanceof

用户定义的类、`Object`、`Array`、`RegExp`、`Promise` 的 `instanceof` 正常。`Map`、`Set`、`WeakMap`、`WeakSet` 上的 `instanceof` 会抛 `TypeError: Function has non-object prototype property`：

```bash
wjsm run -e 'console.log([] instanceof Array)'      # true
wjsm run -e 'console.log(new Map() instanceof Map)' # TypeError
```

判定这些类型时改用行为检测，或 `Object.prototype.toString.call(value)`。

## Intl 与 locale 敏感方法

`Intl` 出现在全局名单中，但没有实现，`typeof Intl` 为 `undefined`。依赖它的方法会在运行时触发 WASM trap，而不是抛出 JavaScript 异常：

- `Number.prototype.toLocaleString`
- `String.prototype.localeCompare`

`Date.prototype.toLocaleDateString` 可以调用，返回固定的 `YYYY-MM-DD` 形式（示例环境下为 `2026-07-29`），不接受 locale 定制。`String.prototype.normalize` 已实现，`"e\u0301".normalize("NFC").length` 得到 `1`。

## URL 与 URLSearchParams

两者都不是全局对象，`new URL(...)` 会在 lowering 阶段报 `undeclared identifier`。需要解析 URL 时使用 `node:url` 和 `node:querystring`。

## Fetch 与 Streams 只能直接调用

`fetch`、`Headers`、`Request`、`Response`、`ReadableStream`、`WritableStream`、`TransformStream`、`AbortController` 都是语义层拦截的 Builtin。直接调用和 `new` 可用，读成值得到 `undefined`：

```bash
wjsm run -e 'console.log(typeof globalThis.fetch)'  # undefined
```

需要把 `fetch` 传给其他函数或做能力探测时，自己包一层箭头函数。

## 内联源码不支持 import

`-e` 传入的源码不经过模块图，`import` 语句会报 `undeclared identifier`。要用模块就写成文件：

```bash
wjsm run -e 'import fs from "node:fs"; console.log(1)'   # 报错
printf 'import fs from "node:fs";\nconsole.log(typeof fs.readFileSync);\n' > /tmp/a.mjs
wjsm run /tmp/a.mjs                                       # function
```

`require` 在内联模式下同样不可用（`typeof require` 为 `undefined`）。

## eval 与 REPL 只接受表达式

`wjsm eval` 和默认模式的 `wjsm repl` 会把输入包进 `console.log((...))`，语句和多语句序列会报语法错误。需要执行语句用 `wjsm run -e`，或 `wjsm repl --script`。

## 函数体不能前向引用后声明的 let/const/class

TDZ 在 lowering 阶段静态判定，不生成运行时检查。函数体只要引用了词法上更晚声明的 `let`、`const` 或 `class`，即使调用发生在声明之后，也会被编译期拒绝：

```js
function f() { return x }
let x = 1;
f();               // Node 输出 1；wjsm 报 cannot access `x` before initialisation
```

这条规则同样命中类体内引用自身类名的写法，因为类绑定在类定义求值完成前处于 TDZ：

```js
class C {
  static s = 1;
  m() { return C.s }   // 报 cannot access `C` before initialisation
}
```

改写方式是把引用换成不依赖外层绑定的形式：类内用 `this.constructor.s` 或 `new.target`，普通函数把变量作为参数传入，或把声明提到使用之前。

## TypeScript 构造器参数属性不生效

`constructor(public a)` 的参数属性简写会被丢弃：形参不出现在 lowering 结果里，实例上也没有对应字段。

```bash
wjsm run -e 'class A { constructor(public a: number) {} } console.log(new A(1).a)'   # undefined
```

需要显式声明字段并赋值：

```ts
class A {
  a: number;
  constructor(a: number) { this.a = a }
}
```

## 没有类型检查

TypeScript 语法参与解析和 lowering，但类型不做检查，类型错误的代码可以正常编译运行。类型检查请交给 `tsc`。

## 其他

- `--target jit` 未实现，传入会报 `JIT backend is not implemented yet`。
- `debugger` 语句是编译期空操作，`wjsm lint` 会就此告警。
- `wjsm run --watch` 不支持 package script。
- 未被 fixture 或 test262 覆盖的语义应视为不受支持，不要假定与 V8 逐位一致。

## 深入了解

- [语义层如何拦截内置方法调用](../../internals/frontend/expressions-and-statements.md)
- [Builtin 与 Host Import 的注册边界](../../internals/host-runtime/host-imports.md)
- [JIT 后端当前的接入契约](../../internals/backend/jit-backend.md)
