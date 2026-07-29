# 表达式与语句

这一章说明表达式和语句如何变成 IR 指令，以及 Builtin 拦截为什么放在 lowering 而不是运行时。

## 语句降级的返回值

每个 `lower_*` 语句函数返回 `StmtFlow`：

- `StmtFlow::Open(block)` — 控制流仍在延续，后续语句写入 `block`。
- `StmtFlow::Terminated` — 当前路径已被 `return` / `throw` / `break` / `continue` 终结。

`ensure_open` 把 `Terminated` 转成诊断，避免在已终结的路径后继续追加指令。这个约定让 CFG 在构造期就保持每个基本块恰有一个终结器。

语句实现按类型分布在 `lowerer_stmt/stmt_core.rs`（顺序语句）、`lowerer_stmt/stmt_loops.rs`（循环）、`lowerer_branching.rs`（分支、`try`、标签与展开）。

## Builtin 拦截

语义层识别一批已知调用形态，直接发射 `Instruction::CallBuiltin`，跳过运行时属性查找。`crates/wjsm-semantic/src/builtins.rs` 按接收者种类提供拦截表：

| 函数 | 拦截形态 |
| --- | --- |
| `builtin_from_global_ident` | `fetch(...)`、`parseInt(...)`、`new Map(...)` 等全局标识符 |
| `builtin_from_static_member` | `Math.max`、`Object.keys`、`Reflect.set`、`Promise.withResolvers` 等静态成员 |
| `builtin_from_array_proto_method` | `arr.map(...)`、`arr.with(...)` |
| `builtin_from_string_proto_method` | `str.slice(...)` |
| `builtin_from_typedarray_proto_method` | `ta.fill(...)` |
| `builtin_from_dataview_proto_method` | `dv.getUint8(...)` |
| 另有 `object` / `regexp` / `promise` / `number` / `boolean` / `sharedarraybuffer` 变体 | 对应原型方法 |

这套拦截以调用点为单位。它换来的是省掉一次属性查找和一次间接调用，代价是这些方法不作为可读属性存在——用户可观察的后果记录在[限制与已知差异](../../user/runtime/limitations.md)。

## 表达式分派

表达式入口按域拆分：

- `lowerer_binary_expr/binary_unary.rs`：二元、一元、`delete`、`typeof`。
- `lowerer_calls_eval/call_expr.rs`：调用、`new`、`super()`、直接 `eval`。
- `lowerer_assignments/`：赋值、复合赋值、解构赋值、捕获变量写回。
- `lowerer_jsx_objects/`：对象字面量、JSX 元素与表达式容器。
- `lowerer_async_eval/`：`await`、`yield`、动态 `import()`。

`Instruction::Binary` / `Unary` / `Compare` 只承载数值和比较语义，其余全部落到 `CallBuiltin` 或专用指令（如 `StringConcatVa`）。

## 早期错误

一批 ECMAScript 早期错误在这一层判定，而不是留给运行时：

- `break` / `continue` 不在合法目标内。
- 重复标签。
- `super` 出现在非方法位置，`super()` 出现在非派生构造器。
- `await` 出现在非 async 函数体。
- `with` 语句（静态作用域模型下不支持）。
- `delete` 作用于不支持的目标形态。

## 深入了解

- [Hoisting、TDZ 与早期错误的判定顺序](hoisting-tdz-and-errors.md)
- [控制流与异常如何构造 CFG](control-flow-and-exceptions.md)
- [Instruction 与 Constant 的完整定义](../ir/instructions-and-constants.md)
