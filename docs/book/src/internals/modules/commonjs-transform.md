# CommonJS 转换

CJS 不是独立的执行路径。`cjs_transform.rs` 在 AST 层把 CommonJS 改写成 ESM 风格 AST，之后所有阶段只认 ESM。

## 检测

`is_commonjs_module` 先做一次否决判断：模块体含任意 `ModuleDecl`（`import` / `export`）即返回 false，不可能是 CJS。之后用 `CjsDetector` 访问器查找 `require` / `exports` / `module.exports`。

文件后缀的判定权在 `module_format.rs`，不在这里：`.cjs` 恒为 CommonJS，`.mjs` 恒为 ESM，`.js` 看最近 `package.json` 的 `type`，无 `package.json` 时才回落到 AST 检测结果。

## require 站点分析

`cjs_require_analysis.rs` 先把所有 `require()` 调用分成两类：

| 类型 | 条件 | 处理 |
| --- | --- | --- |
| `HoistableStatic` | 顶层、无控制流包裹、字面量 specifier | 改写成 `import` |
| `Runtime` | 在函数/类体内、控制流内，或参数非字面量 | 保留为运行时调用 |

站点用 `RequireSiteKey { lo, hi }`（SWC span 字节区间）标识，因为同一 specifier 可能出现在多个位置且处理方式不同。`hoistable` 是 `BTreeMap<String, String>`，有序保证改写结果稳定。

## 改写规则

- 顶层 `const x = require('./p')` → `import x from './p'`，直接复用用户变量名。
- 其他可提升 `require('./p')` → `import __cjs_req_N from './p'`，原位置替换为该标识符。
- `module.exports.x = v` 和 `exports.x = v` → `let <prefix>__cjs_x = v`，并记入命名导出。
- `module.exports = obj` → `export default obj`。
- `module.exports.nested.deep = v`：深层赋值不支持，原样保留。

存在命名导出时合成一个默认导出对象 `export default { x: <var>, ... }`，让 ESM 侧 `import m from` 拿到与 Node 一致的形状。

`transform_with_prefix` 的 `export_prefix` 用于多模块 bundle 时避免不同模块的合成变量名相撞。

## 运行时 require

保留下来的 `Runtime` 站点由运行时模块加载器处理，这是 ADR 0006 定义的边界。相关实现见运行时侧章节。

> <details><summary>为什么「深层赋值 `module.exports.nested.deep = v`」不支持？</summary>
>
> 改写 CJS 的目的是把 `module.exports.foo = bar` 转成 ESM 风格的 `export const foo = bar`。这是个一一映射——每个 `exports` 上的属性都对应一个具名导出。
>
> 但 `module.exports.nested.deep = v` 是「先建一个嵌套对象，再在嵌套对象上赋值」：lowering 阶段不知道 `nested` 对象是什么形状，只能假设它最终会构造出来。要正确改写需要：
>
> 1. 见到 `module.exports.nested = {}` 时建一个空对象
> 2. 见到 `module.exports.nested.deep = v` 时在那个对象上设 `deep` 属性
> 3. 见到 `module.exports.nested.other = w` 时在同一对象上设 `other` 属性
> 4. 最后 `module.exports = {nested: <合并后的对象>}` —— 但这要求降级期能追踪「之前所有的 `nested` 赋值是同一对象」
>
> 实现复杂度远超收益（这种写法在 CJS 里本身就比较罕见，且往往可以通过浅层属性绕开）。当前选择「原样保留」——意味着这段代码会留在产物里按 CJS 语义执行。降级后表现为 undeclared identifier（因为 `module` 在 ESM 路径不注入）。
>
> 用户遇到这个错误时改写一下就行：`const nested = {deep: v, other: w}` 然后 `module.exports = {nested}`。
>
> </details>

## 深入了解

- [用户视角的 CJS 与 Node 模块行为](../../user/projects/commonjs-and-node.md)
- [运行时 require 与动态加载的实现](../runtime-features/module-loading.md)
- [模块图如何消费改写后的 AST](graph-and-resolution.md)
