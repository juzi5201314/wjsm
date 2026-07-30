# 语言功能矩阵

下表记录本文档核对时逐条实测的结果，对应二进制版本 `wjsm 0.1.0`。矩阵不是兼容性承诺：没有列出的语义以 `fixtures/` 和 test262 的实际覆盖为准。

## 声明与作用域

| 功能 | 状态 | 说明 |
| --- | --- | --- |
| `var` / `let` / `const` | 可用 | 同作用域重复声明按早期错误拒绝 |
| TDZ 检查 | 可用（静态） | 在 lowering 期判定，函数体内前向引用会被拒绝 |
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
| 私有字段 `#x` | 可用 | |
| 继承与 `super` | 可用 | |
| getter / setter | 可用 | 含类原型与 `defineProperty` |
| 计算属性名 | 可用 | |
| 方法体内引用类名 | 不可用 | 静态 TDZ 拒绝，用 `this.constructor` 替代 |

> <details><summary>「不可用」不一定是真的不能跑</summary>
>
> 矩阵里写「不可用」是「在 wjsm 编译期会被拒绝」。意思是：你写出来代码后 `wjsm check` 会报错，编译不能通过。
>
> 实际可能是「实现代价高」（如构造器参数属性、类名引用）、「与当前架构不兼容」（如 TDZ 静态判定的某些边角）、「还没人写」（如某些不常用的 TypeScript 形态）。
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
| `Object`、`Array`、`Function` | 可用 | `Array.prototype` 方法是真实属性 |
| `String` 方法 | 仅调用点 | 取值得到 `undefined` |
| `Map` / `Set` / `WeakMap` / `WeakSet` | 可用 | 这四者上的 `instanceof` 会抛 `TypeError` |
| `Promise` 及组合子 | 可用 | 含 `allSettled`、`withResolvers` |
| `Proxy` / `Reflect` | 可用 | |
| `BigInt` | 可用 | |
| `Symbol` | 可用 | |
| `RegExp`（含命名捕获组） | 可用 | 由 `regress` 提供 |
| `JSON` | 可用 | |
| TypedArray、`ArrayBuffer`、`SharedArrayBuffer` | 可用 | 原型方法仅调用点可用 |
| `DataView` | 可用 | 访问器取值得到 `undefined` |
| `Atomics` | 可用 | |
| `WeakRef`、`FinalizationRegistry` | 可用 | |
| `Date` | 可用 | locale 定制不可用 |
| `Intl` | 未实现 | 依赖它的方法会 trap |
| `URL` / `URLSearchParams` | 未提供 | 用 `node:url`、`node:querystring` |

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
| 构造器参数属性 | 不可用 | `constructor(public a)` 不生成字段，需显式赋值 |
| JSX / TSX | 可用 | 降级为对象 |

## 深入了解

- [两阶段 Lowering 如何建立作用域与 TDZ](../../internals/frontend/two-phase-lowering.md)
- [Hoisting、TDZ 与早期错误的判定规则](../../internals/frontend/hoisting-tdz-and-errors.md)
- [TypeScript 与类语法的 lowering 归属](../../internals/frontend/functions-closures-and-classes.md)
- [Fixture 与 test262 如何界定实际覆盖范围](../../internals/testing/fixtures.md)
