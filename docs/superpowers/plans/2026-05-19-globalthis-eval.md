# globalThis 在 Eval 中可用

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 交换 `lower_ident` 和 `lower_assign` 中的回退检查顺序，使 eval 代码中 `globalThis`（以及其他 built-in globals）能从 `$0.$global` 正确解析，而非通过 eval 作用域桥返回 `undefined`。

**Architecture:** 仅修改 `crates/wjsm-semantic/src/lowerer_assignments.rs`。在 `lower_ident` 中将 `is_builtin_global` 检查提到 eval scope bridge 之前；在 `lower_assign` 中，在 scope bridge 检查内增加 builtin_global 回退来写入全局对象属性。

**Tech Stack:** Rust (edition 2024), `swc_core`, `wjsm-ir`

---

### Task 1: 交换 `lower_ident` 中的检查顺序

**Files:**
- Modify: `crates/wjsm-semantic/src/lowerer_assignments.rs:44-73`

- [ ] **Step 1: 交换两条 match arm 的顺序**

`lower_ident` 函数中，将 `is_builtin_global` 的 arm（当前 lines 50-73）移到 eval scope bridge 的 arm（当前 lines 44-48）之前。

修改前（lines 42-73）:
```rust
        let (scope_id, _kind) = match self.scopes.lookup(&name) {
            Ok(found) => found,
            Err(msg)
                if self.eval_scope_bridge_active() && msg.starts_with("undeclared identifier") =>
            {
                return Ok(self.lower_eval_env_read(&name, block));
            }
            Err(msg) if msg.starts_with("undeclared identifier") && is_builtin_global(&name) => {
                // 变量查找失败 → 从全局对象按名读取属性
                // 全局对象已在模块初始化阶段通过 CreateGlobalObject 创建并存入 $0.$global
                let global_obj = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::LoadVar {
                        dest: global_obj,
                        name: "$0.$global".to_string(),
                    },
                );
                let key_const = self.module.add_constant(Constant::String(name));
                let key_val = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::Const {
                        dest: key_val,
                        constant: key_const,
                    },
                );
                let dest = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::GetProp {
                        dest,
                        object: global_obj,
                        key: key_val,
                    },
                );
                return Ok(dest);
            }
            Err(msg) => return Err(self.error(ident.span, msg)),
        };
```

修改后:
```rust
        let (scope_id, _kind) = match self.scopes.lookup(&name) {
            Ok(found) => found,
            Err(msg) if msg.starts_with("undeclared identifier") && is_builtin_global(&name) => {
                // 变量查找失败 → 从全局对象按名读取属性
                // 全局对象已在模块初始化阶段通过 CreateGlobalObject 创建并存入 $0.$global
                let global_obj = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::LoadVar {
                        dest: global_obj,
                        name: "$0.$global".to_string(),
                    },
                );
                let key_const = self.module.add_constant(Constant::String(name));
                let key_val = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::Const {
                        dest: key_val,
                        constant: key_const,
                    },
                );
                let dest = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::GetProp {
                        dest,
                        object: global_obj,
                        key: key_val,
                    },
                );
                return Ok(dest);
            }
            Err(msg)
                if self.eval_scope_bridge_active() && msg.starts_with("undeclared identifier") =>
            {
                return Ok(self.lower_eval_env_read(&name, block));
            }
            Err(msg) => return Err(self.error(ident.span, msg)),
        };
```

- [ ] **Step 2: 编译检查**

```bash
cargo check -p wjsm-semantic
```

Expected: 编译成功，无错误。

- [ ] **Step 3: 提交**

```bash
git add crates/wjsm-semantic/src/lowerer_assignments.rs
git commit -F - <<'EOF'
fix: 交换 lower_ident 中 eval scope bridge 和 builtin_global 的检查顺序

使 eval 代码中 globalThis、Object、Array 等内置全局标识符优先从 $0.$global 解析，
而非走 eval 作用域桥返回 undefined。

Co-authored-by: CommandCodeBot <noreply@commandcode.ai>
EOF
```

---

### Task 2: 在 `lower_assign` 中增加 builtin_global 回退

**Files:**
- Modify: `crates/wjsm-semantic/src/lowerer_assignments.rs:365-403`

- [ ] **Step 1: 在 scope bridge arm 中增加 builtin_global 检查**

`lower_assign` 函数中，在 eval scope bridge 匹配后、strict mode 检查前，增加 `is_builtin_global` 检查。如果是 builtin global，生成 IR 写入 `$0.$global` 属性。

修改前（lines 363-403）:
```rust
        let (scope_id, kind) = match self.scopes.lookup_for_assign(&name) {
            Ok(found) => found,
            Err(msg)
                if self.eval_scope_bridge_active() && msg.starts_with("undeclared identifier") =>
            {
                if self.strict_mode {
                    // strict eval: 对未声明变量赋值 → ReferenceError
                    let msg_const = self.module.add_constant(Constant::String(
                        format!("assignment to undeclared variable '{name}'"),
                    ));
                    let msg_val = self.alloc_value();
                    self.current_function.append_instruction(
                        block,
                        Instruction::Const {
                            dest: msg_val,
                            constant: msg_const,
                        },
                    );
                    let error_val = self.alloc_value();
                    self.current_function.append_instruction(
                        block,
                        Instruction::CallBuiltin {
                            dest: Some(error_val),
                            builtin: Builtin::ReferenceErrorConstructor,
                            args: vec![msg_val],
                        },
                    );
                    // 创建 dummy 值（在 throw 终止块之前分配）
                    let dummy = self.alloc_value();
                    self.current_function.append_instruction(
                        block,
                        Instruction::Const {
                            dest: dummy,
                            constant: self.module.add_constant(Constant::Undefined),
                        },
                    );
                    self.emit_throw_value(block, error_val)?;
                    // emit_throw_value 已终止块；返回的 dummy 不会被使用
                    return Ok(dummy);
                }
                return self.lower_assign_eval_env(assign, block, &name);
            }
            Err(msg) => return Err(self.error(assign.span, msg)),
        };
```

修改后:
```rust
        let (scope_id, kind) = match self.scopes.lookup_for_assign(&name) {
            Ok(found) => found,
            Err(msg)
                if self.eval_scope_bridge_active() && msg.starts_with("undeclared identifier") =>
            {
                if is_builtin_global(&name) {
                    // 对 builtin global 的赋值 → 写入 $0.$global 属性
                    let global_obj = self.alloc_value();
                    self.current_function.append_instruction(
                        block,
                        Instruction::LoadVar {
                            dest: global_obj,
                            name: "$0.$global".to_string(),
                        },
                    );
                    let key_const = self.module.add_constant(Constant::String(name));
                    let key_val = self.alloc_value();
                    self.current_function.append_instruction(
                        block,
                        Instruction::Const {
                            dest: key_val,
                            constant: key_const,
                        },
                    );
                    let rhs = self.lower_expr(assign.right.as_ref(), block)?;
                    self.current_function.append_instruction(
                        block,
                        Instruction::SetProp {
                            object: global_obj,
                            key: key_val,
                            value: rhs,
                        },
                    );
                    return Ok(rhs);
                }
                if self.strict_mode {
                    // strict eval: 对未声明变量赋值 → ReferenceError
                    let msg_const = self.module.add_constant(Constant::String(
                        format!("assignment to undeclared variable '{name}'"),
                    ));
                    let msg_val = self.alloc_value();
                    self.current_function.append_instruction(
                        block,
                        Instruction::Const {
                            dest: msg_val,
                            constant: msg_const,
                        },
                    );
                    let error_val = self.alloc_value();
                    self.current_function.append_instruction(
                        block,
                        Instruction::CallBuiltin {
                            dest: Some(error_val),
                            builtin: Builtin::ReferenceErrorConstructor,
                            args: vec![msg_val],
                        },
                    );
                    // 创建 dummy 值（在 throw 终止块之前分配）
                    let dummy = self.alloc_value();
                    self.current_function.append_instruction(
                        block,
                        Instruction::Const {
                            dest: dummy,
                            constant: self.module.add_constant(Constant::Undefined),
                        },
                    );
                    self.emit_throw_value(block, error_val)?;
                    // emit_throw_value 已终止块；返回的 dummy 不会被使用
                    return Ok(dummy);
                }
                return self.lower_assign_eval_env(assign, block, &name);
            }
            Err(msg) => return Err(self.error(assign.span, msg)),
        };
```

- [ ] **Step 2: 编译检查**

```bash
cargo check -p wjsm-semantic
```

Expected: 编译成功，无错误。

- [ ] **Step 3: 提交**

```bash
git add crates/wjsm-semantic/src/lowerer_assignments.rs
git commit -F - <<'EOF'
fix: 在 lower_assign 的 eval scope bridge 中增加 builtin_global 回退

eval 代码中对 globalThis、Object 等内置全局标识符的赋值现在会写入
$0.$global 属性，而非走 eval 作用域桥（后者只在 non-strict 模式下
写入桥对象，不会持久化到全局对象）。

Co-authored-by: CommandCodeBot <noreply@commandcode.ai>
EOF
```

---

### Task 3: 运行完整测试套件验证

**Files:**
- Test: 无需创建新测试文件

- [ ] **Step 1: 运行所有现有测试确保无回归**

```bash
cargo nextest run
```

Expected: 所有已有测试通过，无新增失败。

- [ ] **Step 2: 运行 test262 globalThis 相关测试**

（`globalThis` 已在 `SUPPORTED_FEATURES` 中，无需额外配置）

```bash
# 全局对象测试（2 个）
cargo run -p wjsm-test262 -- run --suite test/built-ins/global --all --plain 2>&1 | grep -E "(PASS|FAIL)"

# eval-code/direct 中的 globalThis 测试（~148 个，只看 summary）
cargo run -p wjsm-test262 -- run --suite test/language/eval-code/direct \
  --filter "globalThis" --all --plain 2>&1 | tail -5
```

Expected: `built-ins/global/` 下 2 个 globalThis 特性测试全部 PASS；eval-code/direct 中 `features: [globalThis]` 的测试（之前全部 FAIL）现在大量 PASS（具体数量取决于 async/arguments 等其他特性是否已实现）。

- [ ] **Step 3: 运行 eval 测试套件总览**

```bash
cargo run -p wjsm-test262 -- run --suite test/language/eval-code/direct --all --plain 2>&1 | tail -5
cargo run -p wjsm-test262 -- run --suite test/language/eval-code/indirect --all --plain 2>&1 | tail -5
```

Expected: 直接 eval 通过率提升（之前 11%），间接 eval 通过率不变或提升。

- [ ] **Step 4: 提交测试结果（如果测试通过率提升显著）**

```bash
git add -u
git commit -F - <<'EOF'
test: globalThis 在 eval 中可用，更新 test262 通过率

Co-authored-by: CommandCodeBot <noreply@commandcode.ai>
EOF
```

---

### Task 4: 更新快照（如有需要）

**Files:**
- Verify: `fixtures/semantic/*.ir`

- [ ] **Step 1: 检查是否需要更新 IR 快照**

```bash
WJSM_UPDATE_FIXTURES=0 cargo test -p wjsm-semantic 2>&1 | tail -20
```

Expected: 如果所有语义快照测试通过，无需操作。如果有失败：
- 检查失败的 `.ir` 文件 diff，确认修改符合预期
- 运行 `WJSM_UPDATE_FIXTURES=1 cargo test -p wjsm-semantic` 更新快照

- [ ] **Step 2: 如果更新了快照，提交**

```bash
git add fixtures/semantic/
git commit -F - <<'EOF'
test: 更新 semantic IR 快照以反映 globalThis eval 优先级变更

Co-authored-by: CommandCodeBot <noreply@commandcode.ai>
EOF
```

---

### Task 5: 更新 test262 SUPPORTED_FEATURES（已确认无需操作）

`"globalThis"` 已在 `crates/wjsm-test262/src/config.rs:76` 的 `SUPPORTED_FEATURES` 列表中。无需修改。
