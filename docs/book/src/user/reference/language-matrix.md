# 语言功能矩阵

下表记录本文档核对时逐条实测的结果，对应二进制版本 `wjsm 0.1.0`。矩阵不是兼容性承诺：没有列出的语义以 `fixtures/` 和 test262 的实际覆盖为准。

## 声明与作用域

| 功能 | 状态 | 说明 |
| --- | --- | --- |
| `var` / `let` / `const` | 可用 | 同作用域重复声明按早期错误拒绝 |
| TDZ 检查 | 可用（混合） | 同函数内前向引用在 lowering 期拒绝；跨函数前向引用（函数先于声明执行）在运行时抛 ReferenceError |
| 解构（对象、数组、嵌套） | 可用 | 含默认值 |
| 默认参数、rest 参数 | 可用 | |
| 展开（调用、数组、对象） | 可用 | |
| 可选 catch 绑定 | 可用 | |
| 带标签语句、`continue label` | 可用 | |

## 函数与类

| 功能 | 状态 | 说明 |
| --- | --- | --- |
| 箭头函数、闭包 | 可用 | |
| 生成器、异步生成器 | 可用 | 含 `for await` |
| `async` / `await` | 可用 | |
| 类字段、静态字段 | 可用 | |
| 私有字段 `#x` | 可用 | 含 `#x in obj` brand 检查 |
| 继承与 `super` | 可用 | |
| getter / setter | 可用 | 含类原型与 `defineProperty` |
| 计算属性名 | 可用 | |
| 方法体内引用类名 | 可用（运行时 TDZ） | 方法、构造器、getter/setter、实例字段初始化器内可用；静态字段初始值、`extends` 等类定义期求值的位置仍报编译期 TDZ |

> <details><summary>「不可用」不一定是真的不能跑</summary>
>
> 矩阵里写「不可用」是「在 wjsm 编译期会被拒绝」。意思是：你写出来代码后 `wjsm check` 会报错，编译不能通过。
>
> 实际可能是「实现代价高」（如类名引用）、「与当前架构不兼容」（如 TDZ 静态判定的某些边角）、「还没人写」（如某些不常用的 TypeScript 形态）。
>
> 矩阵不会写「将来会实现」——所有状态都是「在当前二进制上跑得通/跑不通」。要跟踪进展看 [GitHub releases](https://github.com/juzi5201314/wjsm/releases) 或仓库 commit log。
>
> </details>

## 表达式与运算符

| 功能 | 状态 | 说明 |
| --- | --- | --- |
| 可选链、`??`、逻辑赋值 | 可用 | |
| `**` | 可用 | |
| 模板字面量、标签模板 | 可用 | `String.raw` 未实现 |
| `for-in` / `for-of` | 可用 | |
| 自定义 `Symbol.iterator` | 可用 | |

## 内置对象

| 功能 | 状态 | 说明 |
| --- | --- | --- |
| `Object`、`Array`、`Function` | 可用 | `Array.prototype` 方法是真实属性；`toLocaleString` 委托元素的 locale 方法 |
| `String` 方法 | 多数仅调用点 | `normalize` / `toLowerCase` / `toUpperCase` / `toLocale*` / `localeCompare` 是真实原型方法；`slice`/`concat`/`includes`/`startsWith`/`indexOf` 可取值传递，其余方法取值得到 `undefined` |
| `Map` / `Set` / `WeakMap` / `WeakSet` | 可用 | 同构造器与跨构造器的 `instanceof` 返回布尔；其他构造器的 `instanceof` 边界仍以 fixture/test262 覆盖为准 |
| `Promise` 及组合子 | 可用 | 含 `allSettled`、`withResolvers` |
| `Proxy` / `Reflect` | 可用 | |
| `Number` | 可用 | `toLocaleString` 委托 `Intl.NumberFormat` |
| `BigInt` | 可用 | `toLocaleString` 委托 `Intl.NumberFormat` |
| `Symbol` | 可用 | |
| `RegExp`（含命名捕获组与 Unicode property escapes） | 可用 | 由 `regress` 提供；`\p{...}` / `\P{...}` 字符属性需 `/u`；UCD Unicode 17 与 Phase 1 manifest 一致。`Script(_Extensions)=Unknown`/`Zzzz` 与 `regexp-v-flag` property-of-strings 暂未纳入（regress 缺口） |
| `JSON` | 可用 | |
| TypedArray、`ArrayBuffer`、`SharedArrayBuffer` | 可用 | 原型方法可取值并经 `call`/`apply`/`bind` 复用，各构造器 `prototype` 对象可用；TypedArray `toLocaleString` 委托与 Array 相同的 Intl 路径 |
| `DataView` | 可用 | get/set 全族（含 `getBigInt64`/`getBigUint64`/`setBigInt64`/`setBigUint64`）可取值传递 |
| `Atomics` | 可用 | |
| `WeakRef`、`FinalizationRegistry` | 可用 | |
| `Date` | 可用 | `toLocale*` 委托 `Intl.DateTimeFormat` |
| `Intl` | 可用 | ECMA-402 核心构造器与 `getCanonicalLocales` / `supportedValuesOf`；不含 Temporal intl402、`intl-normative-optional` 遗留构造器 |
| `URL` / `URLSearchParams` | 支持（含 IDN） | 全局与 `node:url` 同引用 |

## TypeScript

| 功能 | 状态 | 说明 |
| --- | --- | --- |
| 类型注解、`interface`、类型别名 | 可用 | 解析后擦除，不做检查 |
| `enum` | 可用 | |
| 泛型 | 可用 | |
| `as` 断言、非空断言 | 可用 | |
| `satisfies` | 可用 | |
| `abstract`、`implements` | 可用 | |
| `namespace` | 可用 | |
| 装饰器 | 可用 | |
| 构造器参数属性 | 可用 | `constructor(public a)` 生成形参与字段赋值，在 `super()` 后、实例字段初始化器前发射 |
| JSX / TSX | 可用 | 降级为对象 |

## 深入了解

- [两阶段 Lowering 如何建立作用域与 TDZ](../../internals/frontend/two-phase-lowering.md)
- [Hoisting、TDZ 与早期错误的判定规则](../../internals/frontend/hoisting-tdz-and-errors.md)
- [TypeScript 与类语法的 lowering 归属](../../internals/frontend/functions-closures-and-classes.md)
- [Fixture 与 test262 如何界定实际覆盖范围](../../internals/testing/fixtures.md)
