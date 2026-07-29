# 语义 Lowering 阶段

`wjsm-semantic`（55 文件，约 23000 行）把 AST 降级为 IR。这是整条流水线里语义密度最高的一层：作用域、TDZ、hoisting、闭包捕获、类语义、内置方法拦截都在这里定型。

## 公开入口

| 函数 | 用途 |
| --- | --- |
| `lower_module` | 最小入口，无源码上下文 |
| `lower_module_with_source` | 带源码与文件名，错误可渲染代码片段 |
| `lower_module_with_debug_source` | 额外在语句入口发射 `DebugCheck` |
| `lower_eval_module` | eval 编译模式 |
| `lower_eval_module_with_scope` | eval 且需要作用域桥接 |

CLI 统一走 `lower_module_with_debug_source`（`lower_parsed_module`），`emit_debug_checks` 由 `--inspect` / `--inspect-brk` 驱动。开启 debug 但没有 `source` 时行列无法解析，插桩会被跳过而不是报错。

## 模块划分

lowering 按语法域拆成独立模块，避免单文件膨胀：

- `lowerer_core.rs`：核心分发
- `lowerer_predeclare.rs`：预声明阶段
- `lowerer_declarations`、`lowerer_function_decls`、`lowerer_functions.rs`：声明与函数
- `lowerer_classes_ts`：类与 TypeScript 构造
- `lowerer_stmt`、`lowerer_branching.rs`：语句与控制流
- `lowerer_binary_expr`、`lowerer_assignments`、`lowerer_construct.rs`：表达式
- `lowerer_arrows.rs`、`lowerer_jsx_objects`：箭头函数与 JSX
- `lowerer_async_eval`、`lowerer_calls_eval`、`eval_scan.rs`、`scan_await.rs`：eval 与 await 扫描
- `scope.rs`：作用域树与绑定表
- `builtins.rs`：内置全局名单与 Builtin 映射
- `wk_symbol_map.rs`、`wk_symbol_names.rs`：well-known symbol

## 错误类型

`LoweringError` 承载失败，`Diagnostic` 负责渲染。`Diagnostic` 实现 `Display`，输出与解析错误同构（`error:` + `-->` + 源码行 + caret）。这让 `wjsm check` 无需区分错误来源。

## 与 IR 校验的关系

`lower_parsed_module` 在返回前调用 `verify_ir_for_pipeline`，只有 `--verify-ir` 时才真正执行 `program.verify()`。校验默认关闭，因为它是 O(指令数) 的额外遍历。

## 相关章节

- [两阶段 Lowering 的预声明契约](../frontend/two-phase-lowering.md)
- [作用域树、绑定与名称解析](../frontend/scopes-and-bindings.md)
- [Hoisting、TDZ 与早期错误](../frontend/hoisting-tdz-and-errors.md)
- [IR 校验与不变量](../ir/validation-and-invariants.md)
