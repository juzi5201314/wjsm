# Plan: 实现 CommonJS 模块系统 (`require` / `module.exports`)

## Summary

在现有 ES Module bundling 基础上，新增 CommonJS 模块支持。通过 AST 预处理将 CJS 模块转换为 ESM 风格，复用现有 bundler、语义层、WASM 后端和运行时链路。CLI 自动检测入口文件类型（ESM/CJS/Script）。

## Current State Analysis

### 已有基础设施

1. **ES Module 系统已完整实现**（`wjsm-module` crate）：
   - `ModuleResolver`：解析文件、提取 import/export、缓存模块
   - `ModuleGraph`：构建依赖图、拓扑排序、支持循环依赖
   - `ModuleBundler`：bundle 入口模块及其依赖为单一 WASM
   - `analyze_module_links()`：校验导入导出绑定
   - `lower_modules()`：多模块 lowering 为单一 IR Program

2. **解析器**（`wjsm-parser`）：使用 swc 解析 TypeScript 语法，输出 `swc_ast::Module`

3. **语义层**（`wjsm-semantic`）：单模块 `lower_module()` + 多模块 `lower_modules()`，支持 import/export lowering

4. **WASM 后端**（`wjsm-backend-wasm`）：116 个 host import，Type 3 签名 `(i64) -> i64`

5. **运行时**（`wjsm-runtime`）：wasmtime 执行，host function 通过 `Func::wrap()` 注册

6. **CLI**（`wjsm-cli`）：`contains_es_module_syntax()` 检测 ESM 语法，自动走 bundler

### 缺失部分

- CJS 语法检测（`require()`、`module.exports`、`exports`）
- CJS 到 ESM 的 AST 转换
- CJS 模块的依赖解析（`require()` 调用提取）
- CLI 入口检测支持 CJS

## Proposed Changes

### 1. 新增 `wjsm-module/src/cjs_transform.rs` — CJS AST 转换器

**What**: 将 CJS 模块的 `swc_ast::Module` 转换为等价的 ESM 风格 `swc_ast::Module`。

**How**:
- 遍历 AST，收集所有 `require()` 调用（包括条件分支中的）
- 对每个唯一的 `require('./path')` 调用，生成对应的 `import` 声明
- 将 `require('./path')` 表达式替换为导入的本地变量引用
- 将 `module.exports.x = value` / `exports.x = value` 转换为 `export let x = value`
- 将 `module.exports = obj` 转换为 `export default obj`

**Key transformations**:
```javascript
// Before (CJS)
const foo = require('./foo');
exports.bar = 42;
module.exports.baz = foo.baz;

// After (ESM-style AST)
import * as __cjs_req_0 from './foo';
const foo = __cjs_req_0;
export let bar = 42;
export let baz = foo.baz;
```

**Edge cases**:
- `require()` 在表达式中：`const x = require('./a') + require('./b')` → 多个 import + 变量替换
- `module.exports = expr` → `export default expr`
- `exports` 是 `module.exports` 的别名，需统一处理
- 条件分支中的 `require()`：静态收集所有可能路径

### 2. 修改 `wjsm-module/src/resolver.rs` — CJS 依赖提取

**What**: 在 `ModuleResolver::resolve()` 中，解析 CJS 模块时先进行 AST 转换，再提取 import/export。

**How**:
- 新增 `is_commonjs_module(ast: &swc_ast::Module) -> bool`：检测是否存在 `require()` / `module.exports` / `exports`
- 在 `resolve()` 中，如果检测到 CJS 语法：
  1. 调用 `cjs_transform::transform(ast)` 转换为 ESM 风格 AST
  2. 用转换后的 AST 提取 import/export
- 转换后的 AST 存入 `ResolvedModule.ast`

**Changes to `ResolvedModule`**:
- 无需新增字段，复用现有 `ast: swc_ast::Module`

### 3. 修改 `wjsm-module/src/graph.rs` — CJS 模块依赖图构建

**What**: 复用现有 `ModuleGraph::build()`，因为 CJS 转换后已变为 ESM 风格 AST。

**How**: 无需修改。`ModuleGraph::build()` 通过 `resolver.resolve()` 递归解析依赖，CJS 模块转换后已有 `ImportEntry`，自然参与依赖图构建。

### 4. 修改 `wjsm-module/src/bundler.rs` — 支持 CJS 入口

**What**: `ModuleBundler::bundle()` 无需修改，因为 CJS 模块在 resolver 阶段已完成转换。

**How**: 无需修改。`bundle()` 调用 `ModuleGraph::build()` → `analyze_module_links()` → `lower_modules()`，CJS 模块已透明地作为 ESM 参与。

### 5. 修改 `wjsm-cli/src/lib.rs` — CLI 入口检测

**What**: 扩展 `contains_es_module_syntax()` 为 `detect_module_type()`，支持 CJS 检测。

**How**:
- 新增 `contains_commonjs_syntax(source: &str) -> Result<bool>`：检测 `require(`、`module.exports`、`exports.`
- 修改 `build_compile_plan()`：
  - 如果有 `--root` 参数，直接走 Bundle（与现有行为一致）
  - 否则，读取文件内容：
    - 如果有 ESM 语法 → `Bundle`（推断 root 为父目录）
    - 如果有 CJS 语法 → `Bundle`（推断 root 为父目录）
    - 否则 → `SingleSource`

### 6. 新增 fixtures 和测试

**What**: 在 `fixtures/modules/` 下新增 CJS 测试用例。

**Test cases**:
- `cjs_simple/`：`require()` + `module.exports`
- `cjs_exports_alias/`：`exports.x = y`（验证 exports 别名）
- `cjs_default_export/`：`module.exports = obj`
- `cjs_circular/`：CJS 循环依赖
- `cjs_conditional_require/`：条件分支中的 `require()`
- `cjs_mixed_esm/`：CJS 模块 require ESM 模块（混用）

## Assumptions & Decisions

1. **编译时静态转换**：CJS 模块在编译时转换为 ESM 风格，运行时无额外开销
2. **不支持运行时动态 require()**：`require(dynamicPath)` 无法静态解析，编译时报错
3. **exports 别名处理**：`exports` 视为 `module.exports` 的引用，转换时统一处理
4. **循环依赖**：复用现有 ESM bundler 的循环依赖处理（拓扑排序允许回边）
5. **模块缓存**：复用现有 `ModuleResolver` 的 `visited` 缓存
6. **不新增 IR/WASM/Runtime 改动**：完全复用现有 ESM 链路

## Verification Steps

1. 新增 CJS fixtures 能通过编译和执行
2. `cargo test` 全部通过（无回归）
3. CJS 模块的 `require()` 能正确解析依赖
4. `module.exports` 和 `exports` 的导出能被其他模块正确导入
5. 循环依赖场景下行为可预测
6. CLI 自动检测 CJS 入口并走 bundler

## Files to Modify

| File | Change |
|------|--------|
| `crates/wjsm-module/src/cjs_transform.rs` | 新增：CJS AST 转换器 |
| `crates/wjsm-module/src/resolver.rs` | 修改：CJS 检测 + AST 转换集成 |
| `crates/wjsm-module/src/lib.rs` | 修改：导出 cjs_transform 模块 |
| `crates/wjsm-cli/src/lib.rs` | 修改：CLI 入口 CJS 检测 |
| `fixtures/modules/cjs_*/` | 新增：CJS 测试 fixtures |
| `tests/integration/fixtures.rs` | 修改：新增 CJS 集成测试 |
