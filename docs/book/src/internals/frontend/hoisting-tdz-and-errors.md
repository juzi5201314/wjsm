# Hoisting、TDZ 与早期错误

这一章说明 wjsm 在编译期如何实现 hoisting、TDZ 和 ECMAScript 早期错误，以及静态判定带来的边界。

## Hoisting

预声明阶段按 `VarKind` 决定落点：

| 声明 | 落点 | 初始状态 |
| --- | --- | --- |
| `var` | 最近的 `ScopeKind::Function` | `initialised = true`，值为 `undefined` |
| `function` | 当前作用域 | `initialised = true`，函数体一并可用 |
| `let` / `const` / `class` | 当前作用域 | `initialised = false` |

`var` 在预声明阶段就被标为已初始化，因此 `console.log(x); var x = 1` 输出 `undefined` 而不报错；`let` 保持未初始化，触发 TDZ。

## TDZ

`ScopeTree::lookup` 在找到绑定时检查 `initialised`：

```rust
if !info.initialised {
    return Err(format!("cannot access `{name}` before initialisation"));
}
```

`lookup_for_assign` 做同样的检查，并额外拒绝 `const` 重新赋值。`mark_initialised` 在 lower 到实际声明语句时把绑定移出 TDZ。

`resolve_scope_id` 是唯一绕过 TDZ 检查的查找入口，供需要解析作用域归属但不读取值的场景使用。

## 静态判定的边界

TDZ 完全在 lowering 期判定，没有运行时 TDZ 检查。函数体在 lowering 时按词法位置解析标识符，此时后面的 `let`/`const`/`class` 尚未 `mark_initialised`，于是合法的前向引用被拒绝：

```js
function f() { return x }   // lowering 期报 TDZ
let x = 1;
console.log(f());           // Node 输出 1
```

类名在自身方法体内同样受影响（`class C { m() { return C.name } }`）。这是当前架构的已知取舍：要放开它需要引入运行时 TDZ 标记，涉及 IR 与后端两层。用户侧的表现和规避写法记录在[限制与已知差异](../../user/runtime/limitations.md)。

> <details><summary>为什么不用「运行时 TDZ 标记」？</summary>
>
> 运行时 TDZ 标记需要：
>
> 1. 在每个 let/const/class 声明处发射「标记 binding 为已初始化」指令。
> 2. 在每次读取前发射「检查 binding 是否已初始化」指令。
> 3. 检查失败时抛 ReferenceError。
>
> 这把 TDZ 检查从「编译期一次性」变成「每次访问都检查」，每次读变量多一次开销。
>
> wjsm 选择静态判定（编译期拒绝所有 TDZ 违规代码）的理由：
>
> - 性能：每次读变量少一次检查，对热代码影响大。
> - 简单：后端不需要支持「运行时 binding 状态」概念。
> - 严格：本来规范里 TDZ 就是「早期错误」（程序根本不该这么写），编译期拒绝更接近规范意图。
>
> 代价：合法的「延迟到声明后调用」模式被拒（如 `f()` 在 `let x = 1` 之后调 Node 会输出 1，wjsm 会拒绝编译）。这种模式在严格 TypeScript 项目里也越来越少见，是可接受的代价。
>
> </details>

## 早期错误

早期错误在 lowering 期通过上下文栈判定，各自有明确的 owner：

| 错误 | 触发条件 | owner |
| --- | --- | --- |
| `break outside of loop or switch` | 无 break 目标 | `lowerer_branching.rs` |
| `continue outside of loop` | 无 continue 目标 | `lowerer_branching.rs` |
| `duplicate label` | 标签重复 | `lowerer_branching.rs` |
| `super is only valid inside methods` | `super_allowed` 为假 | `lowerer_assignments/assign_super.rs` |
| `super() is only valid inside derived constructors` | 非派生构造器 | `lowerer_calls_eval/call_expr.rs` |
| `await is only valid in async functions` | `is_async_fn` 为假 | `lowerer_jsx_objects/jsx_expressions.rs` |
| `with statement is not supported in strict/static scope mode` | 出现 `with` | `lowerer_declarations/decl_misc.rs` |
| `cannot redeclare identifier` | 同作用域冲突声明 | `scope.rs` 的 `declare` |

`declare` 只允许 `var` 与 `var` 重复，其余组合一律报重复声明。

## 深入了解

- [作用域树与绑定表的结构](scopes-and-bindings.md)
- [预声明与 lower 两阶段的分工](two-phase-lowering.md)
- [诊断如何携带行列信息](diagnostics-and-spans.md)
