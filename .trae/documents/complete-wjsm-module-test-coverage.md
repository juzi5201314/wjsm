# 补全 wjsm-module 测试覆盖率至 100%

## 概述

为 `wjsm-module` crate 补全所有缺失的单元测试，目标是 100% 的函数/分支覆盖率。

## 当前状态分析

**现有测试：17 个**，全部通过，但覆盖率约 55-60%。

各文件未覆盖的关键代码路径：

### resolver.rs（约 40% 覆盖率）
- `resolve_path`：非相对路径错误、带扩展名路径、找不到模块错误、parent 为空场景
- `resolve`：缓存命中路径、CJS 模块检测+转换路径
- `get_module`：存在/不存在两种情况
- `all_modules`：迭代器
- `get_id_by_path`：存在/不存在两种情况
- `ensure_default_export_for`：已有默认导出跳过、无导出跳过、正常添加
- `add_synthetic_default_export`：Named/Declaration/Default/All 各 ExportEntry 类型、export_names 为空跳过
- `extract_imports`：Named import（含 as）、Default import、Namespace import、非 Import 的 ModuleItem
- `extract_exports`：ExportNamed（含 src / 不含 src）、ExportDefaultExpr、ExportDefaultDecl（Class/Fn/TsInterface）、ExportAll、ExportDecl（Class/Fn/Var/Ts 系列）、非 ModuleDecl

### graph.rs（约 60% 覆盖率）
- `build`：CJS 模块默认导入 ESM 模块时添加合成默认导出的路径
- `topological_order`：与入口不连通的模块处理路径
- `visit_module`：Visited/Visiting/None 三种状态
- `get_module`：不存在情况
- `entry_id`：基本访问

### semantic.rs（约 70% 覆盖率）
- `analyze_module_links`：ExportEntry::All（wildcard re-export）路径
- `CollectedExports::supports_name`：wildcard 为 true 时的路径
- `analyze_module_links`：namespace import（`*`）跳过缺失导出校验
- `analyze_module_links`：重复 default 导出

### cjs_transform.rs（约 75% 覆盖率）
- `CjsDetector::visit_assign_expr`：`is_module_exports_member` 路径、`is_exports_ident` 路径、`is_module_exports_member_no_prop` 路径
- `RequireCollector::visit_var_decl`：`__cjs_req_` 前缀变量跳过路径、已有 specifier 跳过路径、非 Ident Pat 跳过路径
- `RequireCollector::visit_call_expr`：已有 specifier 跳过路径
- `CjsTransformer::transform_module`：ModuleDecl 项原样保留
- `transform_stmt_into`：Decl 分支、other 分支
- `transform_stmt`：Block/If/While/DoWhile/For/ForIn/ForOf/Switch/Try/Labeled/Return/Throw/With/other 各分支
- `transform_decl`：非 Var 的 Decl
- `transform_var_decl`：非 require 的 var decl、非 Ident Pat 的 var decl、非 direct_import 的 require
- `try_transform_expr_stmt`：非 Assign 表达式、非 `=` 赋值操作、非 Simple target、非 Member target、computed property（字符串字面量）、computed property（非字符串字面量返回 None）、非 Ident/Computed 的 MemberProp
- `transform_expr`：Call（非 require）/Member/Bin/Unary/Update/Assign/SimpleAssignTarget 非 Member/AssignTarget::Pat/Cond/Seq/Array/Object/Arrow/BlockStmtOrExpr::Expr/Paren/Tpl/OptChain/OptChainBase::Call/New/Await/Yield/Fn/Class/TaggedTpl/other 各分支
- `transform_block`：ExportDecl/ExportDefaultExpr/其他 ModuleDecl 在 block 中的处理
- `is_module_exports_member`：非 Member 表达式、obj 非 Ident、prop 非 Ident
- `is_module_exports_member_no_prop`：obj 非 Ident、prop 非 Ident
- `is_exports_member`：非 Member 表达式、obj 非 Ident
- `is_exports_ident`：非 Ident 表达式
- `create_import_default_decl`、`create_synthetic_default_export`、`create_let_decl`：间接覆盖
- `transform_with_prefix`：有 export_names 但 has_default_export 为 true 时不生成合成导出

### bundler.rs（约 10% 覆盖率）
- `ModuleBundler::new`：创建
- `bundle`：完整流程（需要 `lower_modules` 支持）

### lib.rs（约 0% 覆盖率）
- `bundle` 公共函数

## 实施方案

### 文件修改清单

仅修改 `/workspace/crates/wjsm-module/src/` 下的各源文件中的 `#[cfg(test)] mod tests` 块。不创建新文件。

---

### 1. resolver.rs — 新增约 15 个测试

```rust
// resolve_path 测试
fn resolve_path_rejects_non_relative_specifier()       // 非 . 开头的 specifier 应报错
fn resolve_path_finds_js_extension()                    // 自动添加 .js 扩展名
fn resolve_path_finds_file_with_extension()             // 已有扩展名直接使用
fn resolve_path_fails_when_module_not_found()           // 所有候选都不存在时报错
fn resolve_path_resolves_parent_directory()             // ../ 相对路径

// resolve 测试
fn resolve_returns_cached_id_on_second_call()           // 第二次 resolve 同一模块返回缓存 ID
fn resolve_detects_cjs_module()                         // CJS 模块 is_cjs=true
fn resolve_parses_esm_module()                          // ESM 模块 is_cjs=false

// 访问器测试
fn get_module_returns_some_for_existing()               // 已解析模块返回 Some
fn get_module_returns_none_for_missing()                // 不存在的 ID 返回 None
fn all_modules_iterates_all()                           // 迭代所有模块
fn get_id_by_path_returns_some_for_visited()            // 已访问路径返回 Some
fn get_id_by_path_returns_none_for_unknown()            // 未知路径返回 None

// ensure_default_export_for 测试
fn ensure_default_export_adds_when_no_default()         // 无 default 但有其他导出时添加
fn ensure_default_export_skips_when_has_default()       // 已有 default 时不添加
fn ensure_default_export_skips_when_no_exports()        // 无任何导出时不添加

// extract_imports 间接测试（通过 resolve）
fn extract_imports_handles_named_import()               // import { x } from
fn extract_imports_handles_default_import()             // import x from
fn extract_imports_handles_namespace_import()           // import * as ns from
fn extract_imports_handles_aliased_named_import()       // import { x as y } from

// extract_exports 间接测试（通过 resolve）
fn extract_exports_handles_named_export()               // export { x }
fn extract_exports_handles_default_expr_export()        // export default expr
fn extract_exports_handles_default_fn_export()          // export default function
fn extract_exports_handles_default_class_export()       // export default class
fn extract_exports_handles_declaration_export()         // export const/let/var/function/class
fn extract_exports_handles_export_all()                 // export * from './foo'
fn extract_exports_handles_re_export_with_source()      // export { x } from './foo'
```

### 2. graph.rs — 新增约 6 个测试

```rust
fn build_creates_correct_dependency_edges()             // 验证 imports 列表正确关联
fn build_handles_cjs_importing_esm_default()            // CJS 导入 ESM 默认导出时添加合成导出
fn get_module_returns_none_for_invalid_id()             // 无效 ID 返回 None
fn entry_id_returns_entry_module()                      // 返回入口模块 ID
fn single_module_graph_topological_order()              // 单模块图的拓扑排序
fn topological_order_handles_disconnected_modules()     // 与入口不连通模块的处理（理论上不会出现，但覆盖代码路径）
```

### 3. semantic.rs — 新增约 5 个测试

```rust
fn wildcard_reexport_allows_any_import()                // export * from 后导入任意名称不报错
fn namespace_import_skips_missing_check()               // import * as ns 跳过缺失导出校验
fn duplicate_default_export_detected()                  // 重复 default 导出检测
fn link_result_contains_correct_export_names()          // 验证 export_names 包含所有导出名
fn empty_module_links_successfully()                    // 无导入导出的模块链接成功
```

### 4. cjs_transform.rs — 新增约 20 个测试

```rust
// CJS 检测补充
fn detects_cjs_via_assign_to_exports_ident()           // exports.x = 1 在赋值表达式中
fn does_not_detect_cjs_for_member_access()              // 只读取 module.exports 不写入

// transform 补充
fn transform_preserves_module_decl_items()              // ModuleDecl 项原样保留
fn transform_with_prefix_adds_prefix_to_var_names()     // export_prefix 生效
fn transform_skips_synthetic_default_when_has_default() // 有 default export 时不生成合成导出

// require 收集器补充
fn multiple_require_same_specifier_uses_first()        // 同一 specifier 多次 require 复用
fn non_ident_var_decl_require_generates_auto_name()    // 解构 require 生成 __cjs_req_N
fn require_in_non_var_context_generates_auto_name()    // 非变量声明中的 require

// try_transform_expr_stmt 补充
fn non_assign_expr_stmt_not_transformed()               // 非赋值表达式不转换
fn compound_assign_not_transformed()                    // += 等复合赋值不转换
fn non_member_assign_not_transformed()                  // 非成员赋值不转换
fn computed_string_property_exports()                   // exports['key'] = value 转换
fn computed_non_string_property_not_transformed()       // exports[expr] = value 不转换
fn non_ident_non_computed_prop_not_transformed()        // 非标识符非计算的属性不转换

// transform_expr 各分支
fn transform_expr_handles_binary()                      // 二元表达式
fn transform_expr_handles_unary()                       // 一元表达式
fn transform_expr_handles_update()                      // 更新表达式
fn transform_expr_handles_conditional()                 // 三元表达式
fn transform_expr_handles_sequence()                    // 逗号表达式
fn transform_expr_handles_arrow_with_body()             // 箭头函数含块体
fn transform_expr_handles_arrow_expr_body()             // 箭头函数表达式体
fn transform_expr_handles_template()                    // 模板字符串
fn transform_expr_handles_new()                         // new 表达式
fn transform_expr_handles_paren()                       // 括号表达式
fn transform_expr_handles_object_spread()               // 对象展开
fn transform_expr_handles_opt_chain_member()            // 可选链成员访问
fn transform_expr_handles_opt_chain_call()              // 可选链调用
fn transform_expr_handles_await()                       // await 表达式
fn transform_expr_handles_yield()                       // yield 表达式
fn transform_expr_handles_fn_expr()                     // 函数表达式
fn transform_expr_handles_class_expr()                  // 类表达式
fn transform_expr_handles_tagged_template()             // 标签模板

// transform_stmt 各分支
fn transform_stmt_handles_block()                       // 块语句
fn transform_stmt_handles_if()                          // if 语句
fn transform_stmt_handles_while()                       // while 语句
fn transform_stmt_handles_for()                         // for 语句
fn transform_stmt_handles_switch()                      // switch 语句
fn transform_stmt_handles_try_catch()                   // try-catch 语句
fn transform_stmt_handles_return()                      // return 语句
fn transform_stmt_handles_throw()                       // throw 语句
fn transform_stmt_handles_labeled()                     // labeled 语句

// transform_var_decl 补充
fn var_decl_with_non_require_init_preserved()           // 非 require 初始化保留
fn var_decl_with_destructuring_require_generates_auto() // 解构 require 生成自动名

// 辅助函数
fn is_module_exports_member_returns_false_for_non_member()  // 非 Member 表达式
fn is_exports_member_returns_false_for_non_member()         // 非 Member 表达式
fn is_exports_ident_returns_false_for_non_ident()           // 非 Ident 表达式
fn is_module_exports_member_no_prop_returns_false_for_non_ident_obj() // 非 Ident obj
```

### 5. bundler.rs — 新增约 2 个测试

```rust
fn bundler_new_creates_instance()                       // 创建 bundler 实例
fn bundle_simple_modules_produces_wasm()                // 完整 bundle 流程（使用 fixtures/modules/simple）
```

### 6. lib.rs — 新增约 1 个测试

```rust
fn public_bundle_function_works()                       // 测试 lib.rs 的 bundle 公共函数
```

## 测试策略

1. **resolver.rs / graph.rs / semantic.rs** 的测试使用临时文件系统（延续现有 `create_temp_project` + `write_file` 模式）
2. **cjs_transform.rs** 的测试使用 `wjsm_parser::parse_module` 直接解析源代码（延续现有 `parse()` 辅助函数模式）
3. **bundler.rs / lib.rs** 的测试使用 `/workspace/fixtures/modules/` 下的现有 fixture 文件
4. 每个测试聚焦于一个具体的代码路径或分支
5. 辅助函数（`create_temp_project`、`write_file`、`parse`）在各模块的 test 中各自定义（延续现有模式）

## 实施顺序

1. `cjs_transform.rs` — 最多新增测试，影响最大
2. `resolver.rs` — 核心解析逻辑
3. `semantic.rs` — 语义链接
4. `graph.rs` — 依赖图
5. `bundler.rs` — 集成测试
6. `lib.rs` — 公共 API

## 验证步骤

1. `cargo test -p wjsm-module` — 所有测试通过
2. `cargo test -p wjsm-module -- —nocapture` — 查看输出无异常
3. 目测确认每个 `#[cfg(test)] mod tests` 块中的测试覆盖了对应模块的所有公开/私有函数和分支

## 假设与决策

- **不引入 `cargo-llvm-cov`**：环境安装 llvm-tools 较慢，通过代码审查确认覆盖率
- **bundler 测试依赖 `lower_modules`**：如果 `lower_modules` 尚未完整实现，bundler 测试可能需要标记 `#[ignore]` 或仅测试到语义链接步骤
- **不修改任何非测试代码**：仅在各文件的 `#[cfg(test)] mod tests` 块中添加测试
- **辅助函数复用**：每个模块的 test 块中独立定义 `create_temp_project`/`write_file`/`parse`，不跨模块共享（延续现有模式）
