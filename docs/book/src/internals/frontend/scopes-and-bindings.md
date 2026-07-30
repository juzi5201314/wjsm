# 作用域树、绑定与名称解析

作用域是 lowering 的核心数据结构。它决定一个标识符解析到哪个变量、是否处于 TDZ、能否重新赋值。owner 是 `crates/wjsm-semantic/src/scope.rs`。

## 数据结构

`ScopeTree` 是 arena：`arenas: Vec<Scope>` 存所有作用域，`current: usize` 指向当前作用域。作用域用索引而非指针互相引用，避免生命周期纠缠。

```rust
pub(crate) struct Scope {
    pub(crate) parent: Option<usize>,
    pub(crate) kind: ScopeKind,
    pub(crate) id: usize,
    pub(crate) variables: HashMap<String, VarInfo>,
}
```

`ScopeKind` 有三种：`Block`、`Function`、`Module`。根作用域是 `Function`，id 为 0。

`VarInfo` 记录三件事：

- `kind`：`Var` / `Let` / `Const`，决定登记位置和可变性。
- `initialised`：`false` 表示处于 TDZ。
- `implicit_arguments`：仅对 `emit_arguments_init` 创建的隐式 `arguments` 为 true，用于区分显式声明的 `arguments`。


<details>
<summary>为什么用 <code>usize</code> 索引而不是 <code>Rc&lt;Scope&gt;</code>？</summary>

<code>Rc&lt;Scope&gt;</code> 看起来更「现代」——自动管理生命周期、可变引用计数。但 lowering 阶段会反复修改作用域（添加绑定、移出 TDZ、嵌套进入退出），<code>Rc</code> 每次都涉及引用计数更新。

用 `usize` 索引（arena 模式）的代价是手动管理生命周期：删除作用域要手动从 arena 移除，引用了已删除 id 的代码要小心。好处：

- 没有引用计数开销，纯整数比较。
- 父链遍历是 <code>Vec&lt;Option&lt;usize&gt;&gt;</code> 的索引跳转，缓存友好。
- 调试时可以打印出整个作用域树（arena 是个普通 Vec）。

这是经典的「用裸数据结构换性能」取舍。lowering 阶段对作用域访问频繁（每次 `lookup` 都走父链），值得为它优化。
</details>

## 登记规则

`declare` 按声明种类选择目标作用域：

- `let` / `const` 进当前（最内层）作用域。
- `var` 沿父链上溯到最近的 `ScopeKind::Function`，由 `function_scope_for_scope` 定位。

同一作用域内重复声明会报 `cannot redeclare identifier ... in the same scope`。唯一例外是 `var` 重复声明 `var`，这与 ECMAScript 一致，直接返回已有作用域 id。

## 查找路径

四个查找入口对应不同语义：

| 方法 | 用途 | TDZ 检查 | const 检查 |
| --- | --- | --- | --- |
| `lookup` | 读取标识符 | 是 | 否 |
| `lookup_for_assign` | 赋值 | 是 | 是 |
| `resolve_scope_id` | 只要作用域归属 | 否 | 否 |
| `visible_bindings_all` | 枚举可见绑定（含 TDZ） | 不适用 | 不适用 |

`lookup_for_assign` 把 const 检查和 TDZ 检查合并到一次父链遍历里。原先 `lower_assign` 先 `check_mutable` 再 `lookup`，遍历两次；合并后在深层嵌套下减少约一半的 HashMap 查找。`check_mutable` 保留但已标记 `#[allow(dead_code)]`。

查不到名字时统一报 `undeclared identifier`。

## 作用域限定名

IR 中的变量名带作用域前缀，形如 `$0.x`、`$2.a`。前缀是作用域 id，因此同名变量在不同作用域不会互相覆盖，IR dump 也能直接读出绑定归属：

```bash
wjsm dump-ir -e 'let x = 1; { let x = 2; }'
```

`$this` 与 `$env` 是特殊名，分别表示 this 绑定和闭包环境参数。

## 深入了解

- [两阶段 lowering 如何驱动作用域登记](two-phase-lowering.md)
- [TDZ 的静态判定与已知限制](hoisting-tdz-and-errors.md)
- [闭包如何捕获跨作用域变量](functions-closures-and-classes.md)
