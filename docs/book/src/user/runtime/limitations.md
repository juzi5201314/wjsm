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

`slice`、`concat`、`includes`、`startsWith`、`indexOf` 以及 locale 相关的 `normalize` / `toLowerCase` / `toUpperCase` / `toLocaleLowerCase` / `toLocaleUpperCase` / `localeCompare` 可取值传递。其余方法（`replace`、`split`、`match`、`trim` 等）取值得到 `undefined`，只能在调用点使用：

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

## TDZ 静态判定

`let` / `const` 的 Temporal Dead Zone 在 lowering 期静态判定。当前简单 declarator 的
initializer 本身是对象字面量时（允许括号及不改变求值时机的 TypeScript 类型包装），其
method/getter/setter 延迟执行体可以引用同一 binding；在初始化完成后调用这些方法时，
读取、`let` 写入及更新都会访问真实 binding。

```js
const set = {
  forEach(action) {
    action(set); // 支持：调用发生在 set 初始化完成之后
  },
};
set.forEach((value) => console.log(typeof value)); // "object"
```

边界仍然精确：紧接对象字面量的 member 读取、方法调用或 getter 访问属于立即求值，
不会开启逃逸；property value、computed key、spread，以及以调用、`new`、条件表达式、
数组等包裹对象字面量的非直接 initializer 也继续静态拒绝。任意后声明 binding 和
箭头/普通函数等其他函数形态仍由 [#372](https://github.com/juzi5201314/wjsm/issues/372)
及既有限制处理。这不是完整的运行时 TDZ 支持；类名仍使用独立的延迟方法体规则，
类定义期求值位置保持严格 TDZ。

## Intl 与 locale 敏感方法

`Intl` 命名空间、ECMA-402 构造器与 `String`/`Number`/`BigInt`/`Date`/`Array` 的 locale 敏感方法已实现，数据来自 `wjsm-intl-data`（ICU4X compiled_data）。默认 locale 读 `LC_ALL`/`LANG`，非法则 `en`。fixture 与可复现测试应显式传 locale。

实现定义差异（ILD/ILND）按规范允许，不把 Node full-icu 当作逐字符 oracle。Temporal 的 intl402 测试不在当前范围内。`intl-normative-optional` 的 legacy constructor 未实现。`localeMatcher: "best fit"` 当前与 Lookup 相同（规范允许实现定义）。`Intl.DisplayNames` 的 `calendar` / `dateTimeField` 目前只有英文表，其他 locale 回退为 code。`timeZoneName` 用 GMT 偏移与城市名，不是完整 ICU zone fieldset。

## URL / URLSearchParams 全局与 IDN

`URL` / `URLSearchParams` 可作为全局使用，也与 `import { URL } from "node:url"` 共享同一构造器。域名 hostname 走 UTS #46（`wjsm-intl-data` / `idna`），例如 `new URL("https://例え.テスト/")` 的 hostname 为 punycode。IPv4 / IPv6 字面量不跑 IDNA；`http://[::1]:8080/` 的 hostname 为 `::1`，host / href 带方括号。

```js
console.log(typeof globalThis.URL); // "function"
console.log(new URL("https://例子.测试/").hostname);
```
## --format native-executable 只覆盖当前宿主

`wjsm build --format native-executable` 在当前宿主上产出可直接运行的 ELF/PE：预链 `wjsm-exec` stub 加上 portable `.wjsm`、预编译 `NativeObject` 与制品内源码快照。Linux 上的 wjsm 出 ELF，Windows 上的 wjsm 出 PE。交叉编译、把 runtime-private object 改后缀冒充 executable，都不支持。打包失败不创建或覆盖输出文件。发行物需要同时带 `wjsm` 与 `wjsm-exec`。

packed exe 的源码 owner 是快照，不是主机目录。静态 `new Worker('./x')` / `fork('./x')` 会在打包期自动纳入；计算出来的入口仍须 `--include`。快照外模块与虚拟路径写操作明确失败。`wjsm run` 与 portable `.wjsm` 仍用主机路径。详见 [ADR 0017](../../../../adr/0017-native-executable-source-snapshot.md) 与 [ADR 0019](../../../../adr/0019-native-executable-application-contract.md)。

## 深入了解

- [语言功能矩阵](../reference/language-matrix.md)
- [Node.js 兼容矩阵](../reference/node-compatibility-matrix.md)
- [JavaScript 与 TypeScript 支持](javascript-and-typescript.md)
