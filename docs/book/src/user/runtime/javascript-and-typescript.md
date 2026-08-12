# JavaScript 与 TypeScript 支持

wjsm 按文件扩展名选择解析方式，`.ts`、`.tsx` 走 TypeScript 语法，`.jsx`、`.tsx` 支持 JSX 语法。类型注解在解析后被丢弃，不做类型检查：类型错误不会让 `wjsm check` 失败。

## 已验证可用的语言特性

下面这些都是直接跑出来的结果，不是特性清单推断：

```bash
wjsm run -e 'class A { #p = 1; static s = 2; get v() { return this.#p } }
  console.log(new A().v, A.s)'
```

```text
1 2
```

| 领域 | 实测可用 |
| --- | --- |
| 类 | 私有字段 `#x`、静态字段、getter/setter、继承与 `super` |
| 函数 | 箭头函数、默认参数、剩余参数、展开调用、生成器、async 生成器 |
| 运算符 | 可选链 `?.`、空值合并 `??`、逻辑赋值、指数运算 |
| 解构 | 对象/数组解构、嵌套解构、默认值、`for...of` 中解构 |
| 集合 | `Map`、`Set`、`WeakMap`、`WeakSet`、`WeakRef`、`FinalizationRegistry` |
| 二进制数据 | `ArrayBuffer`、`SharedArrayBuffer`、`DataView`、全部 TypedArray（含 `Float16Array`） |
| 元编程 | `Proxy`、`Reflect`、`Symbol`（含 well-known symbol） |
| 数值 | `BigInt`、`BigInt64Array`、`Number` 静态成员 |
| 正则 | 命名捕获组、`replaceAll`、具名组回填 |
| 其他 | 标签模板、`JSON` 往返、`Atomics`、迭代器辅助方法 |

> <details><summary>「实测可用」和「规范支持」之间有差距吗？</summary>
>
> 严格说，有。wjsm 通过的方式是「按规范实现 + 跑 fixture/test262 验证」。所以「实测可用」其实隐含了「已经通过对应测试用例」。
>
> 但「所有 fixture 都通过」不等于「所有 spec 行为都正确」——规范里有些行为是隐含的、不容易被某个具体测试用例覆盖的。遇到不确定的 corner case，最可靠的办法是写个最小复现用 `wjsm run -e` 跑一下。
>
> 列表里没列出来的特性不一定不能用——可能是「没人专门测过」、「fixture 覆盖少」、「依赖某条未实现的路径」。先跑一遍再说。
>
> </details>

## 全局对象

语义层维护一份内置全局名单（`crates/wjsm-semantic/src/builtins.rs` 的 `BUILTIN_GLOBALS`），其中包含 `console`、`process`、`Buffer`、`performance`、`structuredClone`、`queueMicrotask`、`atob`/`btoa`、`TextEncoder`/`TextDecoder`、`Intl`、`Iterator`、`setImmediate`，以及 Fetch/Streams 相关的 `Headers`、`Request`、`Response`、`ReadableStream`、`WritableStream`、`TransformStream`、`AbortController`。

`URL` 和 `URLSearchParams` 不是全局，需要从 `node:url` 导入：

```bash
wjsm run -e 'console.log(typeof globalThis.URL)'
```

```text
undefined
```

## TypeScript 的边界

```bash
wjsm check -e 'const n: number = "not a number"'
```

这条命令退出码为 `0`：语法合法，类型不匹配不属于 wjsm 的检查范围。需要类型检查请在流水线里单独跑 `tsc --noEmit`。

装饰器、`enum`、命名空间、构造器参数属性等需要运行时辅助代码的 TypeScript 特性均已降级。遇到尚未支持的形态时编译会报错，行为以实际运行结果为准。

## 深入了解

- [两阶段 Lowering 如何保证 hoisting 与 TDZ](../../internals/frontend/two-phase-lowering.md)
- [TypeScript 语法的降级处理位置](../../internals/frontend/functions-closures-and-classes.md)
- [语义层如何拦截内置方法调用](../../internals/host-runtime/javascript-builtins.md)
