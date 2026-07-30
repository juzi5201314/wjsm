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

> <details><summary>为什么 lowering 需要这么多子模块？</summary>
>
> 单文件装不下。JavaScript 语法有几千种形态（声明、表达式、语句、控制流、异常、类、对象、模式匹配……），每种都有 lowering 逻辑。23000 行不是「写得啰嗦」，是「JS 本身就这么复杂」。
>
> 子模块按「语法域」切（声明、函数、类、表达式……），不按 owner 切（解析、IR 生成、清理……）。原因是「同一类语法会反复出现在不同地方」——比如函数声明、函数表达式、箭头函数、async 函数都是函数形态，但分散在不同子模块里；按域切能让「函数相关的所有 lowering」集中在一起。
>
> 替代方案是「状态机式 lowering」——一个 dispatch 循环，遍历 AST 时根据节点类型调用对应 handler。这种方案在大型编译器里（rustc、Swift）常见，但代码写起来冗长、可读性差。wjsm 选「按域拆文件」是因为代码量已经大到必须拆，而 IR 状态机本身又不像 rustc 那么深。
>
> </details>

## 错误类型

`LoweringError` 承载失败，`Diagnostic` 负责渲染。`Diagnostic` 实现 `Display`，输出与解析错误同构（`error:` + `-->` + 源码行 + caret）。这让 `wjsm check` 无需区分错误来源。

## 与 IR 校验的关系

`lower_parsed_module` 在返回前调用 `verify_ir_for_pipeline`，只有 `--verify-ir` 时才真正执行 `program.verify()`。校验默认关闭，因为它是 O(指令数) 的额外遍历。

## 相关章节

- [两阶段 Lowering 的预声明契约](../frontend/two-phase-lowering.md)
- [作用域树、绑定与名称解析](../frontend/scopes-and-bindings.md)
- [Hoisting、TDZ 与早期错误的判定位置](../frontend/hoisting-tdz-and-errors.md)
- [IR 校验与不变量](../ir/validation-and-invariants.md)
