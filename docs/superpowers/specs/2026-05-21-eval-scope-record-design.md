# Eval 规范补全 — ScopeRecord 作用域代理

## 目标

将 test262 直接 eval 套件通过率从 10%（29/286）提升至 ~67%（~192/286），
解决 EvalDeclarationInstantiation（arguments 冲突、TDZ）和 PerformEval（super、new.target）中的语义缺失。

## 方案：方法 B — ScopeRecord 宿主对象

将 eval 环境对象从填充了已初始化绑定的**扁平 JS 对象快照**替换为理解
TDZ、作用域链语义和调用上下文元数据的专用**宿主作用域记录对象**。

### 之前（扁平快照）
```
eval 模块 ──GetProp/SetProp──→ 普通 JS 对象 { x: 1, y: 2 }
                                              ↑ 仅已初始化绑定；无 TDZ；无元数据
```

### 之后（作用域记录）
```
eval 模块 ──EvalGetBinding/EvalSetBinding──→ ScopeRecord 宿主对象 {
                                                 bindings: { x: 1, y: TDZ, C: TDZ }
                                                 home_object: proto | None
                                                 new_target: constructor | None
                                                 has_arguments_binding: bool
                                                 is_strict: bool
                                              }
```

## 架构变更概览

| 层级 | 变更内容 |
|---|---|
| **IR** (`wjsm-ir`) | 7 个新的 `Builtin` 变体；1 个新的 NaN-boxing 标签 `TAG_SCOPE_RECORD` |
| **语义层** (`wjsm-semantic`) | `lower_direct_eval_call` 重写；`lower_super_prop`/`new.target` eval 路径；类预声明 TDZ 修复；新增 `visible_bindings_all()` |
| **后端** (`wjsm-backend-wasm`) | 为 7 个新宿主函数注册 WASM 导入 |
| **运行时** (`wjsm-runtime`) | `ScopeRecord` 分配/操作；TDZ 强制执行；const 不可变性；arguments AST 遍历；home_object 传播 |

---

## 第 1 部分：IR 变更

### 1.1 新 Builtin 变体 (`wjsm-ir/src/builtin.rs`)

```rust
pub enum Builtin {
    // ... 现有变体 ...

    /// 创建新的作用域记录宿主对象
    /// dest: i64 — scope record handle
    ScopeRecordCreate,

    /// 向作用域记录添加绑定
    /// args[0]: record, args[1]: name (string), args[2]: value (i64), args[3]: is_tdz (bool)
    ScopeRecordAddBinding,

    /// 按名称从作用域记录获取绑定值；若 TDZ 则抛出 ReferenceError
    /// dest: i64 — value (或 TAG_EXCEPTION，若在 TDZ 中)
    EvalGetBinding,

    /// 在作用域记录中写入绑定值；强制执行 const 不可变性
    /// dest: i64 — written value
    EvalSetBinding,

    /// 检查作用域记录中是否存在绑定名称（即使处于 TDZ 中也返回 true）
    /// dest: i64 — bool (0 或 1)
    EvalHasBinding,

    /// 从作用域记录的 [[HomeObject]] 获取 super base（原型）
    /// dest: i64 — prototype | undefined | TAG_EXCEPTION（若无 home）
    EvalSuperBase,

    /// 在作用域记录上设置元数据（home_object, new_target, is_strict, has_arguments 等）
    /// args[0]: record, args[1]: key (string constant), args[2]: value (i64)
    ScopeRecordSetMeta,
}
```

### 1.2 ScopeRecord NaN-Boxed 标签 (`wjsm-ir/src/value.rs`)

```rust
pub const TAG_SCOPE_RECORD: u64 = 0x11;
// 使用 encode_handle(TAG_SCOPE_RECORD, handle) / is_scope_record(val) / decode_scope_record_handle(val)
```

`TAG_MASK` 为 `0x1F`（bits 32-36），因此 0x11 在有效范围内且在 0x10（TAG_PROXY）之后是空闲的。

### 1.3 IR 显示格式 (`wjsm-ir/src/lib.rs`)

在 `Builtin` 的 `Display` 实现中添加新变体：
```
ScopeRecordCreate    → call builtin.scope_record_create(%dest)
ScopeRecordAddBinding → call builtin.scope_record_add_binding(%rec, key, val, tdz)
EvalGetBinding       → %dest = call builtin.eval_get_binding(%rec, key)
EvalSetBinding       → %dest = call builtin.eval_set_binding(%rec, key, val)
EvalHasBinding       → %dest = call builtin.eval_has_binding(%rec, key)
EvalSuperBase        → %dest = call builtin.eval_super_base(%rec)
ScopeRecordSetMeta   → call builtin.scope_record_set_meta(%rec, "key", val)
```

---

## 第 2 部分：语义层变更

### 2.1 类预声明 TDZ 修复 (`lowerer_predeclare.rs`)

**当前：** 类声明被注册为 `VarKind::Var` 且 `declared = true`（立即初始化）— 错误。
**修复：** 改为 `VarKind::Let` 且 `declared = false`（TDZ 直到类声明被求值）：

```rust
// 修复前 (lowerer_predeclare.rs:~155):
swc_ast::Decl::Class(class_decl) => {
    let name = class_decl.ident.sym.to_string();
    let _scope_id = self.scopes
        .declare(&name, VarKind::Var, true)   // ← 错误：已初始化
        .map_err(|msg| self.error(class_decl.span(), msg))?;
}

// 修复后:
swc_ast::Decl::Class(class_decl) => {
    let name = class_decl.ident.sym.to_string();
    let _scope_id = self.scopes
        .declare(&name, VarKind::Let, false)  // ← 正确：TDZ
        .map_err(|msg| self.error(class_decl.span(), msg))?;
}
```

`lower_class_decl`（实际的 lowering 过程）通过 `mark_initialised` 处理初始化。

### 2.2 新的 `visible_bindings_all()` (`scope.rs`)

当前，`visible_bindings()` 仅返回已初始化的绑定。需要第二个方法返回**所有**绑定，包括处于 TDZ 中的绑定：

```rust
/// 返回所有词法可见绑定，包括未初始化（TDZ）的 let/const/class。
/// 返回 (scope_id, name, is_initialised) 三元组。
pub fn visible_bindings_all(&self) -> Vec<(usize, String, bool)>;
```

### 2.3 新的 Lowerer 字段 (`lib.rs`)

```rust
pub(crate) struct Lowerer {
    // ... 现有字段 ...
    pub(crate) eval_scope_record: bool,
    /// 调用上下文是否有显式的 arguments 绑定（参数、var、let、function 声明）
    pub(crate) eval_caller_has_arguments: bool,
    /// 当前函数是否为方法（有 [[HomeObject]]）— 用于 super 检查
    pub(crate) is_method: bool,
}
```

### 2.4 `lower_direct_eval_call` 重写 (`lowerer_calls_eval.rs`)

新的实现：

```rust
pub(crate) fn lower_direct_eval_call(
    &mut self, call: &swc_ast::CallExpr, block: BasicBlockId,
) -> Result<(ValueId, BasicBlockId), LoweringError> {
    self.current_function.mark_has_eval();

    // 1. 降低 code 参数
    let (code_val, eval_block) = self.lower_eval_code_arg(call, block)?;

    // 2. 创建 ScopeRecord
    let all_bindings = self.scopes.visible_bindings_all();
    let env_val = self.alloc_value();
    self.current_function.append_instruction(
        eval_block,
        Instruction::CallBuiltin {
            dest: Some(env_val),
            builtin: Builtin::ScopeRecordCreate,
            args: vec![self.const_val_i64(eval_block, all_bindings.len() as i64)],
        },
    );

    // 3. 添加所有绑定（包括未初始化的 TDZ 绑定）
    for (scope_id, name, is_initialised) in &all_bindings {
        let binding = CapturedBinding::new(name.clone(), *scope_id);
        let value = self.load_binding_for_eval(eval_block, &binding, *is_initialised)?;
        let is_tdz = !*is_initialised;
        let key_const = self.module.add_constant(Constant::String(name.clone()));
        let key_val = self.const_val(eval_block, key_const);
        let tdz_val = self.const_val(eval_block, 
            self.module.add_constant(Constant::Bool(is_tdz)));

        self.current_function.append_instruction(
            eval_block,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::ScopeRecordAddBinding,
                args: vec![env_val, key_val, value, tdz_val],
            },
        );
    }

    // 4. 设置元数据（通过 ScopeRecordSetMeta + 预定义保留键名）
    // 保留键：`__wjsm_is_strict`、`__wjsm_has_arguments`、
    //          `__wjsm_home_object`、`__wjsm_new_target`
    for (key_str, val_const) in [
        ("__wjsm_is_strict", Constant::Bool(self.strict_mode)),
        ("__wjsm_has_arguments", Constant::Bool(self.eval_caller_has_arguments)),
    ] {
        let key_cid = self.module.add_constant(Constant::String(key_str.to_string()));
        let key_v = self.alloc_value();
        self.current_function.append_instruction(eval_block, Instruction::Const {
            dest: key_v, constant: key_cid,
        });
        let val_cid = self.module.add_constant(val_const);
        let val_v = self.alloc_value();
        self.current_function.append_instruction(eval_block, Instruction::Const {
            dest: val_v, constant: val_cid,
        });
        self.current_function.append_instruction(eval_block,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::ScopeRecordSetMeta,
                args: vec![env_val, key_v, val_v],
            });
    }
    if self.is_method {
        let home = self.alloc_value();
        self.current_function.append_instruction(
            eval_block,
            Instruction::GetSuperBase { dest: home },
        );
        let hk_cid = self.module.add_constant(Constant::String("__wjsm_home_object".to_string()));
        let hk_v = self.alloc_value();
        self.current_function.append_instruction(eval_block, Instruction::Const {
            dest: hk_v, constant: hk_cid,
        });
        self.current_function.append_instruction(eval_block,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::ScopeRecordSetMeta,
                args: vec![env_val, hk_v, home],
            });
    }
    if !self.is_arrow_fn_stack.last().unwrap_or(&false) {
        let nt_val = self.alloc_value();
        self.current_function.append_instruction(
            eval_block,
            Instruction::CallBuiltin {
                dest: Some(nt_val),
                builtin: Builtin::NewTarget,
                args: vec![],
            },
        );
        let nk_cid = self.module.add_constant(Constant::String("__wjsm_new_target".to_string()));
        let nk_v = self.alloc_value();
        self.current_function.append_instruction(eval_block, Instruction::Const {
            dest: nk_v, constant: nk_cid,
        });
        self.current_function.append_instruction(eval_block,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::ScopeRecordSetMeta,
                args: vec![env_val, nk_v, nt_val],
            });
    }

    // 5. 调用 eval
    let dest = self.alloc_value();
    self.current_function.append_instruction(
        eval_block,
        Instruction::CallBuiltin {
            dest: Some(dest),
            builtin: Builtin::Eval,
            args: vec![code_val, env_val],
        },
    );

    // 6. 异常分支（与之前相同）
    // 7. 回写：对于非 TDZ 绑定，将更改从记录读回
    let merge_block = self.emit_eval_writeback(eval_block, dest, env_val, &all_bindings)?;
    Ok((dest, merge_block))
}
```

### 2.5 Eval 模块 lowering 变更

`lower_eval_module_with_scope()` 设置 `eval_scope_record = true`。在 eval 模块 lowering 过程中：

- **标识符读取**（`lower_ident`）：作用域桥回退使用 `EvalGetBinding` 代替 `GetProp`
- **标识符写入**（`lower_assign`）：作用域桥回退使用 `EvalSetBinding` 代替 `SetProp`
- **`typeof` 操作符**：使用 `EvalHasBinding` 来判断绑定是否存在于作用域中（即使在 TDZ 中），若不存在则回退到全局查找
- **`super.prop`**（`lower_super_prop`）：在 eval 模式下，使用 `EvalSuperBase` 代替 `GetSuperBase`
- **`super()`**：同之前，在 eval 模式下为 SyntaxError，除非在派生构造器中（未来工作）
- **`new.target`**：从 `eval_scope_env`（作用域记录元数据）读取，而非发出 `NewTarget` 内建指令（在 eval 模块中不可用）

### 2.6 Arguments 绑定检测

在 `lowerer_core.rs` 中 lowering 函数入口时，设置 `eval_caller_has_arguments`：

```rust
fn check_caller_arguments_context(&mut self, params: &[Param]) {
    // 1) 是否有参数名为 "arguments"？
    let has_param_arguments = params.iter().any(|p| {
        if let Pat::Ident(binding) = &p.pat {
            binding.id.sym.as_ref() == "arguments"
        } else { false }
    });

    // 2) 函数体中是否有显式的 arguments 绑定？
    //    在 lowering 过程中由 emit_arguments_init 处理 ——
    //    若 declare("arguments") 返回 Err，则 arguments 已被显式声明。
    //    此处通过 scope lookup 检查是否已存在 arguments 绑定。
    let has_explicit_arguments = self.scopes.lookup("arguments").is_ok();

    self.eval_caller_has_arguments = has_param_arguments || has_explicit_arguments;
```

---

## 第 3 部分：后端变更

### 3.1 WASM 导入 (`compiler_core.rs`)

`HOST_IMPORT_NAMES` 数组新增 7 个条目（索引 348-354）：

| 索引 | 名称 | 签名 |
|---|---|---|
| 348 | `scope_record_create` | `(i64) → i64` |
| 349 | `scope_record_add_binding` | `(i64, i64, i64, i64) → void` |
| 350 | `eval_get_binding` | `(i64, i64) → i64` |
| 351 | `eval_set_binding` | `(i64, i64, i64) → i64` |
| 352 | `eval_has_binding` | `(i64, i64) → i64` |
| 353 | `eval_super_base` | `(i64) → i64` |
| 354 | `scope_record_set_meta` | `(i64, i64, i64) → void` |

`builtin_func_indices` 映射：
```rust
builtin_func_indices.insert(Builtin::ScopeRecordCreate, 348);
builtin_func_indices.insert(Builtin::ScopeRecordAddBinding, 349);
builtin_func_indices.insert(Builtin::EvalGetBinding, 350);
builtin_func_indices.insert(Builtin::EvalSetBinding, 351);
builtin_func_indices.insert(Builtin::EvalHasBinding, 352);
builtin_func_indices.insert(Builtin::EvalSuperBase, 353);
builtin_func_indices.insert(Builtin::ScopeRecordSetMeta, 354);
```

`HOST_IMPORT_NAMES` 数组长度从 348 增加到 355。

### 3.2 编译器内建指令生成 (`compiler_builtins.rs`)

每个新内建指令遵循现有模式：从参数列表读取 args，发出 `LocalGet` 从本地槽位获取，然后发出 `Call` 调用相应的 WASM 导入索引。不需要特殊的代码生成——所有逻辑都在运行时宿主函数中。

### 3.3 Eval 模式模块 globals

之前，eval 模块导入了 13 个父模块 globals。ScopeRecord 不需要额外的 globals——作用域记录句柄通过 eval 入口参数 `(scope_env: i64) → i64` 传递。

---

## 第 4 部分：运行时变更

### 4.1 ScopeRecord 数据结构 (`runtime_eval.rs`)

```rust
/// 基于宿主堆的作用域记录，实现类似规范的作用域行为。
struct ScopeRecord {
    bindings: Vec<(String, i64, bool, bool)>, // (name, value, initialized, is_const)
    home_object: Option<i64>,
    new_target: Option<i64>,
    has_arguments_binding: bool,
    is_strict: bool,
}
```

分配在宿主堆上，以 `TAG_SCOPE_RECORD` 作为 NaN-boxed 句柄暴露。宿主函数 `scope_record_free(handle)` 在 GC 时释放记录（或立即释放，若适用）。

### 4.2 宿主函数实现

**`scope_record_create(caller, capacity) → i64`：**
1. 分配带有给定初容量的 `ScopeRecord`
2. 返回 NaN-boxed 句柄

**`scope_record_add_binding(caller, record, name: i64, value: i64, is_tdz: i64) → void`：**
1. 解码 record 句柄，解码 name 字符串，解码 is_tdz 布尔值
2. 若 `is_tdz` 为 true：`bindings.push((name, value, false, is_const))` — 未初始化
3. 若 `is_tdz` 为 false：`bindings.push((name, value, true, is_const))` — 已初始化
4. 若 binding 来自 `var` 声明 → `is_const = false`

**`eval_get_binding(caller, record, name: i64) → i64`：**
1. 解码记录，解码 name 字符串
2. 在 `bindings` 中按 name 查找
3. 若找到且未初始化（`initialized == false`）：
   - 创建 `ReferenceError: Cannot access '<name>' before initialization`
   - 通过 `set_runtime_error` 设置
   - 返回 `TAG_EXCEPTION` 句柄
4. 若找到且已初始化：返回值
5. 若未找到：返回 `undefined`

**`eval_set_binding(caller, record, name: i64, value: i64) → i64`：**
1. 解码记录，解码 name
2. 在 bindings 中查找
3. 若找到，`is_const` 且 `initialized`：
   - 返回 `TypeError: assignment to constant '<name>'`
4. 若找到：更新 value，设置 `initialized = true`，返回 value
5. 若未找到且为非严格模式：添加新 binding，返回 value
6. 若未找到且为严格模式：返回 `ReferenceError`（赋值给未声明变量）

**`eval_has_binding(caller, record, name: i64) → i64`：**
1. 解码记录，解码 name
2. 若 binding 在 `bindings` 中存在（无论初始状态）：返回 `true`
3. 否则：返回 `false`

**`eval_super_base(caller, record: i64) → i64`：**
1. 解码记录
2. 若 `home_object` 为 `Some(home)`：返回 `GetPrototypeOf(home)`
3. 若 `None`：返回 `TAG_EXCEPTION`（TypeError: super 在此上下文中不可用）

 **`scope_record_set_meta(caller, record, key: i64, value: i64) → void`：**
 1. 解码 record 句柄，解码 key 字符串
 2. 根据预定义保留键名设置字段：
    - `"__wjsm_is_strict"` → `record.is_strict = bool(value)`
    - `"__wjsm_has_arguments"` → `record.has_arguments_binding = bool(value)`
    - `"__wjsm_home_object"` → `record.home_object = Some(value)`
    - `"__wjsm_new_target"` → `record.new_target = Some(value)`
 3. 其他键被忽略（静默无操作）

### 4.3 Arguments 冲突检测

替换当前 `runtime_eval.rs:249-264` 中的字符串匹配：

```rust
// 从作用域记录读取 has_arguments_binding
let has_arguments = scope_record.has_arguments_binding;

if has_arguments {
    // 遍历解析后的 eval AST，检测 var/function arguments 声明
    if eval_module_declares_arguments(&module) {
        let msg = "SyntaxError: declaring 'arguments' in eval code is invalid";
        set_runtime_error(caller.data(), msg.to_string());
        return value::encode_undefined();
    }
}
```

`eval_module_declares_arguments()` 遍历 AST body，在每个 `ModuleItem::Stmt(Stmt::Decl(Decl::Var(..)))`、
`ModuleItem::Stmt(Stmt::Decl(Decl::Fn(..)))` 中查找 `arguments`，检查声明名称。

### 4.4 错误格式化 (`format_eval_error`)

为新的错误模式添加特定于 eval 的错误消息映射：

```rust
"cannot access 'arguments' before initialization" → 
    "ReferenceError: Cannot access 'arguments' before initialization"
```

已存在用于 `"cannot access …"` 的通用处理程序（第 375 行），它映射到 `ReferenceError`。

### 4.5 与现有 eval 缓存的集成

compiled eval 缓存键哈希目前包括 `(code, has_scope_bridge, var_writes_to_scope, data_base)`。
这保持不变——代码是哈希的一部分，而 `has_scope_bridge` 区分了有桥和无桥的 eval。
相同的代码字符串结合不同的调用上下文作用域记录不会导致不同的编译输出；
eval 编译是独立于调用者作用域的。正确性依赖于作用域记录在运行时正确反映调用者的绑定。

---

## 第 5 部分：测试策略

### 5.1 IR 快照测试

为新的内建指令添加语义快照：
```
fixtures/happy/eval-scope-record-create.js → fixtures/semantic/eval-scope-record-create.ir
fixtures/happy/eval-super-prop-method.js  → fixtures/semantic/eval-super-prop-method.ir
fixtures/happy/eval-new-target-fn.js      → fixtures/semantic/eval-new-target-fn.ir
```

### 5.2 E2E 集成测试

```
fixtures/happy/eval-tdz-let.js     — let x; typeof x 应抛出 ReferenceError
fixtures/happy/eval-arguments-ok.js — 无预先存在的 arguments 绑定，eval 声明应成功
fixtures/happy/eval-super-prop.js  — 方法中的 super.test262 应返回原型值
fixtures/happy/eval-new-target.js  — 构造函数中的 new.target 应返回构造函数
fixtures/errors/eval-arguments-conflict.js — 参数名 arguments + eval("var arguments") → SyntaxError
fixtures/errors/eval-tdz-class.js  — typeof C; class C {} → ReferenceError
```

### 5.3 test262 预期增量

| 故障类别 | 测试数 | 之前通过 | 预期通过 | 方法 |
|---|---|---|---|---|
| arguments binding | ~144 | 0 | ~130 | 作用域记录 + AST 遍历 |
| TDZ (let/const/class) | ~7 | 0 | 7 | 类预声明修复 + EvalGetBinding/EvalHasBinding |
| super keyword | ~10 | 0 | ~8 | EvalSuperBase；super() 错误路径 |
| new.target | ~3 | 0 | 3 | 作用域记录 new_target 元数据 |
| var/function hoisting | ~6 | ~3 | ~5 | 现有的 eval_scan + 最后函数优先语义 |
| parse-failure | ~6 | ~2 | ~4 | 现有的 swc 解析器 |
| block/switch decls | ~9 | ~1 | ~6 | 现有的 lowering + TDZ 修复 |
| **总计** | **~185** | **~6** | **~163** | — |

这将在总计约 286 个测试中带来约 192 个总通过数（29 个之前通过 + 163 个新通过 = 192），即 **67%** 通过率。
剩余的失败主要是 Annex B 测试（在 test262 中单独计算），以及间接 eval 的边缘情况。

---

## 第 6 部分：迁移与回滚安全

### 实现顺序（8 步）

| 步骤 | 范围 | 破坏性？ | 累积收益 |
|---|---|---|---|
| 1. | IR：添加 7 个 Builtin 变体 + TAG_SCOPE_RECORD + Display impls | 否 | 0 个测试 |
| 2. | 后端：WASM 导入注册 + 内建指令 codegen | 否 | 0 个测试 |
| 3. | 运行时：ScopeRecord 结构体 + 7 个宿主函数 + 注册 | 否 | 0 个测试 |
| 4. | 语义层：类预声明 TDZ 修复 | 否 | +2 个测试 |
| 5. | 语义层：`visible_bindings_all` + `lower_direct_eval_call` 重写 | **是** | +150 个测试 |
| 6. | 语义层：`lower_super_prop`/`new.target` eval 路径 | 否 | +11 个测试 |
| 7. | 运行时：arguments AST 遍历冲突检测 | 否 | +130 个测试 |
| 8. | 清理：var/function 提升边缘情况、parse-failure | 否 | +6 个测试 |

第 5 步是破坏性的，因为它重写了 eval 环境对象路径。若 scope record 宿主函数故障，
eval 调用会返回 `TAG_EXCEPTION`——现有代码会通过现有的异常处理分支（`lower_direct_eval_call` 中的
`IsException` 检查）传播，不会导致崩溃。

### 回退到 GetProp/SetProp

作用域记录并未完全移除 GetProp/SetProp 路径——这些对于通用属性访问仍然是必需的。
Flat-object 路由保留用于：
- 间接 eval（无范围桥）
- 无桥的严格直接 eval（仅元数据，无绑定传播）

---

## 影响范围总结

| 文件 | 变更内容 |
|---|---|
| `crates/wjsm-ir/src/builtin.rs` | 添加 7 个 Builtin 变体 |
| `crates/wjsm-ir/src/lib.rs` | 更新 `Display` 实现 |
| `crates/wjsm-ir/src/value.rs` | 添加 `TAG_SCOPE_RECORD = 0x11` 及编解码函数 |
| `crates/wjsm-semantic/src/lowerer_calls_eval.rs` | 重写 `lower_direct_eval_call` |
| `crates/wjsm-semantic/src/lowerer_assignments.rs` | 更新 eval 作用域桥为使用 EvalGet/EvalSet |
| `crates/wjsm-semantic/src/lowerer_async_eval.rs` | 为 eval 添加 `lower_super_prop` 路径；添加 `new.target` eval 路径 |
| `crates/wjsm-semantic/src/lowerer_predeclare.rs` | 修复类预声明 TDZ（Let + declared=false） |
| `crates/wjsm-semantic/src/scope.rs` | 添加 `visible_bindings_all()` |
| `crates/wjsm-semantic/src/lib.rs` | 添加 `eval_scope_record`、`eval_caller_has_arguments`、`is_method` 字段；初始化 |
| `crates/wjsm-semantic/src/lowerer_core.rs` | 设置 `is_method` 和 `eval_caller_has_arguments` |
| `crates/wjsm-backend-wasm/src/compiler_core.rs` | 向 `HOST_IMPORT_NAMES` 添加 7 个导入；`builtin_func_indices` 映射 |
| `crates/wjsm-backend-wasm/src/compiler_builtins.rs` | 为新内建指令添加 codegen |
| `crates/wjsm-runtime/src/runtime_eval.rs` | `ScopeRecord` 结构体；7 个宿主函数实现；arguments AST 遍历；更新 `format_eval_error` |
| `crates/wjsm-runtime/src/lib.rs` | 注册 7 个宿主函数；`TAG_SCOPE_RECORD` 在类型判断中的处理 |
| `fixtures/happy/` | 6-8 个新 E2E 夹具 |
| `fixtures/semantic/` | 3-4 个新 IR 快照 |
