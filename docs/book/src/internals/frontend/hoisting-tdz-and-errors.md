# Hoisting、TDZ 与早期错误

这一章说明 wjsm 如何实现 hoisting、TDZ 和 ECMAScript 早期错误：同函数内的 TDZ 违规在编译期拒绝，跨函数前向引用降级为运行时检查。

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

## 混合判定：同函数静态拒绝 + 跨函数运行时检查

同函数内的直线前向引用（如 `console.log(x); let x = 1`）执行时必然违规，lowering 期直接拒绝，零运行时开销。

跨函数前向引用（延迟执行的函数体读取后声明的 `let`/`const`/`class`）静态无法判定调用是否先于声明执行，按规范降级为运行时检查：

```js
function f() { return x }   // 读取点发射 TdzCheck
let x = 1;
console.log(f());           // 输出 1；若在声明前调用 f() 则抛 ReferenceError
```

机制（`runtime_tdz_binding` / `lower_tdz_checked_read`，`lowerer_core.rs`）：

1. 标识符解析因 TDZ 失败时，若绑定属于外层函数（跨函数前向引用），改为发射经
   env 链的受检读取；同函数内仍保持编译期拒绝。
2. 闭包环境快照（shared env / iteration env）对仍处 TDZ 的绑定写入
   `Constant::Uninitialized` 哨兵（`TAG_UNINITIALIZED`），声明执行时由
   `store_binding_value` 覆盖为真实值。
3. 读取点发射 `Builtin::TdzCheck(value, name)`：值为哨兵时宿主构造
   `ReferenceError: Cannot access 'name' before initialization`，否则原样返回。
4. 赋值 / 复合赋值 / 逻辑赋值 / update 的前向引用路径在 GetValue/SetValue
   处同样发射 TdzCheck；const 重赋值仍在编译期拒绝。
5. `direct_call` pass 跳过 TDZ 受检读取的 `Const(FunctionRef)` 替换，
   保证类声明等绑定的运行时哨兵可被观察。

哨兵永不暴露给用户代码：只有 TdzCheck 会消费它，检查通过后向下游传递的是真实值。运行时开销只出现在「静态无法证明已初始化」的读取点，热路径的普通局部读取不受影响。

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
