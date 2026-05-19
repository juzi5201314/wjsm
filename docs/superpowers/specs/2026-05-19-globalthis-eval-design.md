# globalThis 在 Eval 中的实现

## 问题

eval 代码中引用内置全局标识符（`Object`、`Array`、`JSON`、`globalThis` 等）时返回 `undefined`。
核心原因是 eval 作用域桥（scope bridge）在标识符查找失败时拦截了所有"未声明标识符"的查找，导致
`$0.$global` 属性回退路径永远不会被触发。

test262 中 ~148 个 eval-code/direct 测试依赖 `globalThis.arguments` 来验证全局 `arguments` 状态，
这些测试无法通过。另外 2 个 `built-ins/global/` 下的 `globalThis` 特性测试也无法通过。

## 根因

`lowerer_assignments.rs` 中 `lower_ident` 的函数内，查找失败后的两条回退路径顺序错误：

```rust
// 当前顺序（错误）：
Err(msg) if eval_scope_bridge_active() && msg.starts_with("undeclared identifier") => {
    // 路径 1: 从 eval 作用域桥对象读取 → globalThis 不在桥中 → undefined
}
Err(msg) if is_builtin_global(&name) => {
    // 路径 2: 从 $0.$global 读取 → 永远不会到达
}
```

`globalThis`（以及 `Object`、`Array` 等）不在 ScopeTree 中预声明，也不在
`visible_bindings()` 结果中，因此不在 eval 作用域桥对象中。
eval 代码中 `scopes.lookup("globalThis")` 失败 → 被路径 1 拦截 → 桥对象中没有 → `undefined`。

## 方案

### 修改 `lower_ident`（标识符读取）

交换两条回退路径的顺序 —— 先检查 `is_builtin_global`，再检查 eval 作用域桥：

```rust
// 修正后顺序：
Err(msg) if msg.starts_with("undeclared identifier") && is_builtin_global(&name) => {
    // 路径 1: 从 $0.$global 读取 — 适用于所有内置全局
}
Err(msg) if self.eval_scope_bridge_active() && msg.starts_with("undeclared identifier") => {
    // 路径 2: 从 eval 作用域桥对象读取 — 仅用于用户声明的变量
}
```

### 修改 `lower_assign`（标识符赋值）

对于赋值路径（`lowerer_assignments.rs:355-370`），当前 scope bridge 检查之后直接报错，
缺少 `is_builtin_global` 回退。需要增加：如果标识符是 built-in global，则写入 `$0.$global` 属性。

```rust
Err(msg)
    if self.eval_scope_bridge_active() && msg.starts_with("undeclared identifier") =>
{
    if is_builtin_global(&name) {
        // 写入 $0.$global 属性 — 与 lower_ident 中的读取路径对称
        // LoadVar "$0.$global" → Const key → lower expr for value → SetProp
        return self.assign_to_global_property(assign, block, &name);
    }
    // 继续原有的 strict/non-strict eval 赋值逻辑
    if self.strict_mode { ... }
    return self.lower_assign_eval_env(assign, block, &name);
}
```

`assign_to_global_property` 生成如下 IR：
```
LoadVar  → $0.$global  (获取全局对象)
Const    → name        (属性名字符串常量)
lower_expr → value     (降低赋值表达式的右侧)
SetProp  → object=$0.$global, key=name, value=... 
return value            (赋值表达式返回被赋的值)
```

### 为什么正确

- 每个 eval 模块都会调用 `CreateGlobalObject` 创建 `$0.$global`，该对象上已设置了
  `Object`、`Array`、`Function`、`globalThis`（自引用）等属性
- 规范规定 eval 在同一 realm 中运行，eval 的全局对象就是 realm 的全局对象
- 已声明到 ScopeTree 中的局部变量不受影响 —— 它们会匹配 `Ok(found)` 分支

### 不做什么

- **不处理 `this` 在 script 模式下的初始化问题**（这是一个已存在的独立 bug）
- **不为 `BUILTIN_GLOBALS` 中的 stub（Math、JSON 等）添加特殊处理**—— 这些 stubs 在普通代码中也是 `undefined`，保持现状

## 影响范围

| 文件 | 修改内容 |
|---|---|
| `crates/wjsm-semantic/src/lowerer_assignments.rs` | `lower_ident`: 交换 scope bridge 和 builtin_global 检查顺序 |
| `crates/wjsm-semantic/src/lowerer_assignments.rs` | `lower_assign`: 在 scope bridge 检查后增加 builtin_global 回退 |

## 测试

通过的测试：
- `test262/test/built-ins/global/global-object.js` — globalThis 基本功能
- `test262/test/built-ins/global/property-descriptor.js` — globalThis 属性描述符
- `test262/test/language/eval-code/direct/` 下所有 `features: [globalThis]` 的 async arguments 测试（~148 个）

## 风险

低风险。修改仅影响 eval 中未声明标识符的查找优先级。标识符既不在 scope 中也不是 builtin-global
时，行为不变（仍然通过 scope bridge）。
