# 模块系统（ES `import` / `export`）Spec

## Why
当前 `todo.md` 中“模块系统 — ES”尚未实现，导致多文件代码无法以标准 ES Module 方式组织与复用。需要补齐 `import` / `export` 的最小可用链路，以支持模块化开发与后续动态模块能力扩展。

## What Changes
- 解析器新增 ES 模块声明语法识别：`ImportDecl` 与 `ExportDecl`（含默认导出与命名导出）
- 语义层新增模块级符号收集与依赖图构建，完成导入绑定校验与导出表生成
- 运行时新增模块加载、实例化与执行流程，支持缓存与循环依赖下的基础可用行为
- CLI/执行入口接入模块解析模式（文件作为模块入口时按模块语义执行）
- 新增覆盖解析、链接、运行时行为的测试用例与 fixtures

## Impact
- Affected specs: 模块解析、符号绑定、模块加载执行、测试基线
- Affected code: `crates/wjsm-parser`、`crates/wjsm-semantic`、`crates/wjsm-module`、`crates/wjsm-runtime`、`crates/wjsm-cli`、`fixtures/modules`

## ADDED Requirements
### Requirement: 支持 ES 模块导入导出
系统 SHALL 支持基础 ES Module 语义，包括 `import` / `export` 解析、链接与执行。

#### Scenario: 命名导出与命名导入
- **WHEN** 用户在 `lib.js` 中使用 `export const x = 1` 并在 `main.js` 中使用 `import { x } from "./lib.js"`
- **THEN** `main.js` 可以读取到 `x` 的值，且与导出绑定一致

#### Scenario: 默认导出与默认导入
- **WHEN** 用户在模块中使用 `export default expr`，并在其他模块中使用 `import v from "./mod.js"`
- **THEN** 导入值等于对应默认导出绑定

#### Scenario: 模块缓存
- **WHEN** 同一模块被多个导入方加载
- **THEN** 该模块仅实例化与执行一次，后续加载复用缓存命名空间

#### Scenario: 导入绑定校验失败
- **WHEN** 用户导入不存在的命名导出
- **THEN** 系统在链接阶段返回明确错误，阻止继续执行

## MODIFIED Requirements
### Requirement: 脚本执行入口支持模块模式
现有执行入口在处理入口文件时 SHALL 能区分脚本模式与模块模式；当检测到模块语法或显式模块入口时，走模块加载与链接流程而非单文件脚本直执行。

## REMOVED Requirements
### Requirement: 无
**Reason**: 本变更为能力新增，不移除既有需求。  
**Migration**: 无需迁移。
