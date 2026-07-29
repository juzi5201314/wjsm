# 两阶段 Lowering

`wjsm-semantic` 不是单遍遍历 AST。每个语句列表都先经过一次预声明扫描，再真正生成指令。这是 hoisting 与 TDZ 能同时正确的前提。

## 为什么需要两遍

JavaScript 的绑定可见性早于求值顺序：

```js
foo();                  // 合法：函数声明被提升
function foo() {}

console.log(typeof x);  // TDZ 错误：绑定已存在但未初始化
let x = 1;
```

单遍遍历遇到 `foo()` 时还不知道 `foo` 存在，遇到 `x` 时无法区分「未声明」和「已声明未初始化」。预声明先把所有绑定登记进作用域，第二遍才用真实的可见性信息生成指令。

## 第一阶段：预声明

入口是 `Lowerer::predeclare_stmts`（模块层）与 `predeclare_block_stmts`（块层），实现在 `lowerer_predeclare.rs`。它遍历语句列表，为每个声明登记绑定：

- `var` 与函数声明登记到最近的函数作用域，且 `initialised = true`（可提前访问）。
- `let` / `const` / `class` 登记到当前作用域，`initialised = false`（进入 TDZ）。
- 解构模式由 `extract_pat_bindings` 递归展开，数组、对象、rest、默认值都会收集到具体标识符。
- `export const/let/var/function/class` 会剥掉 `export` 外壳后按普通声明预声明，保证导出绑定同样有正确的 TDZ 行为。

`LexicalMode` 控制这一遍是否包含 `let`/`const`：顶层扫描用 `Include`，嵌套块扫描用 `Exclude`，避免把内层块的词法绑定错误地登记到外层。

## 第二阶段：Lower

第二遍按源码顺序生成 IR。词法声明求值到自己的初始化表达式时，调用 `ScopeTree::mark_initialised` 把绑定移出 TDZ。此后 `lookup` 才会返回它，否则报 `cannot access ... before initialisation`。

`Lowerer` 结构定义在 `lowerer_types.rs`，按语法域拆成多个 `impl` 模块：`lowerer_stmt`、`lowerer_binary_expr`、`lowerer_calls_eval`、`lowerer_classes_ts`、`lowerer_functions`、`lowerer_arrows`、`lowerer_async_eval`、`lowerer_jsx_objects` 等。它们共享同一个 `Lowerer` 状态，不各自持有作用域副本。

## 多模块场景

Bundling 时每个模块都要先预声明再 lower，且两个阶段之间必须回到同一个作用域。`ScopeTree::enter_scope` 提供这个能力：预声明时记录模块顶层作用域 id，lower 时用它重新进入，而不是 `push_scope` 新建一个。否则第二遍会看不到第一遍登记的绑定。

## 深入了解

- [作用域树与绑定登记规则](scopes-and-bindings.md)
- [Hoisting、TDZ 与早期错误的判定位置](hoisting-tdz-and-errors.md)
- [多模块 lowering 的作用域复用](../modules/program-bundling.md)
