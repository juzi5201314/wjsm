# 超时问题根治 — 设计文档

## 问题描述

5 个 fixture 在 9 秒后超时（nextest slow-timeout）：

| Fixture | 期望 | 实际 |
|---------|------|------|
| `weakref.js` | exit_code 2, WASM trap | 超时 |
| `finalization_registry.js` | exit_code 0, "registered" 等输出 | 超时 |
| `new_prototype_chain.js` | exit_code 0, "hello" | 超时 |
| `global_fn_visible_in_nested.js` | exit_code 0, "ok" | 超时 |
| `eval_exception_expression_contexts.js` | exit_code 0, 6 行输出 | 超时 |

## 诊断方法

- 编译并运行每个 fixture，确认均 hang 而非 crash
- 逐一对比工作 fixture（`new_prototype_fallback.js`）与 hang fixture（`new_prototype_chain.js`）的 IR 差异
- 定位到 `ConstructCall` 指令在 WASM 执行层进入无限循环

## 根本原因

两处 bug 交互作用导致 hang：

### Bug 1: FunctionRef 常量编译使用错误的表索引键

**位置**: `crates/wjsm-backend-wasm/src/compiler_data.rs:29-37`

```rust
Constant::FunctionRef(function_id) => {
    let wasm_idx = function_id.0;  // IR function ID（0, 1, 2, ...）
    let table_idx = self
        .function_table_reverse
        .get(&wasm_idx)            // 在 WASM 索引（384+）为键的 map 中查找
        .copied()
        .unwrap_or(wasm_idx);      // 回退直接用 IR ID 作表位置
    Ok(value::encode_function_idx(table_idx))
}
```

`function_table_reverse` 的键是 WASM function index（从 `actual_import_count=384` 开始递增）。但 `FunctionRef` 使用 IR function ID（`main=0`, `Base=1`, ...）作为查找键。

对于用户函数，IR ID 恰好等于表位置（因为函数是按 IR 顺序注册的），回退值 `unwrap_or(wasm_idx) = IR ID` 恰好对应正确表位置，所以大多数 fixture "碰巧工作"。但这是不正确的代码路径，对 main 函数（IR ID=0）会尝试用表位置 0 调用，而表位置 0 对应的函数签名是 Type 4（`() -> i64`）而非 Type 12（`(i64,i64,i32,i32)->i64`）。

### Bug 2: SetProto 的 tag 检查过窄

**位置**: `crates/wjsm-backend-wasm/src/compiler_instructions.rs:391-454`

`SetProto` 只接受 `TAG_OBJECT (0x8)` 和 `TAG_FUNCTION (0x9)`。但 `GetPrototypeFromConstructor`（定义在 `compiler_array_helpers.rs:318-406`）正确地接受 `TAG_CLOSURE (0xA)`, `TAG_ARRAY (0xB)`, `TAG_BOUND (0xC)`。

当构造函数的 `.prototype` 是闭包/数组/bound function 时，`SetProto` 静默丢弃该值，`__proto__` 保持 `-1`（null sentinel）——对象没有原型。后续原型链遍历进入未预期路径导致无限循环。

### 相互作用链

1. `ConstructCall` → `compile_call_with_new_target` → call_indirect
2. call_indirect 使用 FunctionRef 解析出的函数索引 → 如果索引错误（指向非 Type 12 函数），触发 WASM trap "indirect call type mismatch"
3. trap 被运行时 `main_ok` 逻辑捕获：`runtime_error` 被 `throw_fn` 设置 → `main_ok = false`
4. timer 事件循环运行 → 没有 timer → 应该 break 但某些条件下不 break
5. 程序 hang 直到 9 秒超时

## 修复方案（Approach A: 最小化定向修复）

### 修复 1: FunctionRef 正确解析函数索引

**涉及文件**:
- `crates/wjsm-backend-wasm/src/compiler_core.rs` — 新增 `function_id_to_wasm_idx` 字段
- `crates/wjsm-backend-wasm/src/compiler_module.rs` — populate 新 map
- `crates/wjsm-backend-wasm/src/compiler_data.rs` — 使用正确 map 查找

**方案**: 在 Compiler 中新增 `function_id_to_wasm_idx: HashMap<u32, u32>`，在 `compile_module` 的函数注册循环中 populate：

```rust
// compiler_module.rs, 在函数注册循环中:
self.function_id_to_wasm_idx.insert(function_id.0, wasm_idx);

// compiler_data.rs, encode_constant:
// 之前（错误）:
let wasm_idx = function_id.0;  // IR function ID
let table_idx = self.function_table_reverse.get(&wasm_idx).copied().unwrap_or(wasm_idx);

// 之后（正确）:
let wasm_idx = self.function_id_to_wasm_idx.get(&function_id.0).copied().unwrap_or(0);
let table_idx = self.function_table_reverse.get(&wasm_idx).copied().unwrap_or(0);
```

### 修复 2: SetProto 扩展 tag 检查

**文件**: `crates/wjsm-backend-wasm/src/compiler_instructions.rs`

在 SetProto 的 tag 检查中添加 `TAG_CLOSURE(0xA)`, `TAG_ARRAY(0xB)`, `TAG_BOUND(0xC)`，与 `GetPrototypeFromConstructor` 保持一致。

### 修复 3: ConstructCall 错误路径加固

**文件**: `crates/wjsm-backend-wasm/src/compiler_instructions.rs` 或运行时

- 确保 `compile_call_with_new_target` 中 `call_indirect` 陷阱后 `runtime_error` 被正确传播
- 确保 `main_ok == false` 路径正确收集输出和错误信息

### 修复 4（可选）: timer 事件循环防 hang

**文件**: `crates/wjsm-runtime/src/lib.rs`

timer 事件循环增加安全门：如果循环迭代超过一定次数或 timer 为空但没有 break，强制退出并返回错误。

## 验收标准

| Fixture | 期望行为 |
|---------|---------|
| `new_prototype_chain.js` | exit 0, stdout "hello" |
| `global_fn_visible_in_nested.js` | exit 0, stdout "ok" |
| `weakref.js` | exit 2, stderr containing "WASM trap: indirect call type mismatch" |
| `finalization_registry.js` | exit 0, stdout "registered", "cleaned: [object Object]", etc. |
| `eval_exception_expression_contexts.js` | exit 0, stdout "if", "seq", "arg", "binary", "new", "nested" |

## 回归风险

- 修复 1 改变了 `FunctionRef` 常量的索引解析路径，涉及所有用户函数引用
- 修复 2 扩展了 `SetProto` 的接受类型，可能暴露之前被静默忽略的原型链循环
- 建议修改后运行 `cargo nextest run --workspace` 全量测试

## 关联文件清单

| 文件 | 变更 |
|------|------|
| `crates/wjsm-backend-wasm/src/compiler_core.rs` | Fix 1: 新增 `function_id_to_wasm_idx` 字段 |
| `crates/wjsm-backend-wasm/src/compiler_module.rs` | Fix 1: populate 新 map |
| `crates/wjsm-backend-wasm/src/compiler_data.rs` | Fix 1: 使用正确 map 查找 |
| `crates/wjsm-backend-wasm/src/compiler_instructions.rs` | Fix 2: SetProto tag 检查 + Fix 3: ConstructCall 错误路径 |
| `crates/wjsm-runtime/src/lib.rs` | Fix 4: timer 循环安全门（可选） |
| `fixtures/happy/weakref.expected` | 确认/更新期望输出 |
