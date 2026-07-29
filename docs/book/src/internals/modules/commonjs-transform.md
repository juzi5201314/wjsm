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

## 深入了解

- [用户视角的 CJS 与 Node 模块行为](../../user/projects/commonjs-and-node.md)
- [运行时 require 与动态加载的实现](../runtime-features/module-loading.md)
- [模块图如何消费改写后的 AST](graph-and-resolution.md)
