# this 绑定与跨函数异常传播设计

## 概述

修复两个制约 test262 eval-direct 测试通过率的架构级问题：

1. **this 绑定错误**：`--script` 模式下顶层 `this` 为未初始化的数值（0），应为全局对象
2. **异常传播缺陷**：`Terminator::Throw` 直接发射 WASM trap，导致跨函数异常的 try/catch 无法捕获

同时完成关联重构：创建真正的全局对象、迁移 builtins 和 `globalThis` 到该对象上。

---

## 问题 1：this 绑定与全局对象

### 现状

- `--script` 标志只在 parser 层使用，随后的 `parse_script_as_module` 把 Script 包装为 Module，script/module 区别就此丢失
- `lower_module()` 不接收 script 标志，不声明 `$this`
- `main` 函数 WASM 签名为 `() -> ()`，无 `this` 参数
- 无全局对象。`Object`、`Array` 等 builtins 通过作用域链上的 `StoreVar`/`LoadVar` 访问，底层由运行时 `get_builtin_global` 宿主函数按需创建
- `globalThis` 是 `StubGlobal`，非真实全局对象引用

### 设计

#### 管道贯穿 script 标志

```
wjsm-cli --script
  → compile_source(script: bool)
    → semantic::lower_module(module, script: bool)
    → 在 Program 中存储 script_mode: bool
    → backend_wasm::compile(&program)
      → 控制 main 签名与初始化
    → runtime::execute_with_writer()
      → 检查 main 返回值
```

`CompileMode` 新增 `Script` 变体（当前有 `Normal`、`Eval`）。

#### 全局对象创建：CreateGlobalObject 宿主函数

新增 host function import（索引 316），签名 `() -> i64`。

行为：
1. 分配空的 host object（容量 60）
2. 遍历 builtins 列表，为每个创建 `NativeCallable` 并设为属性
3. 设置 `globalThis = self`
4. 返回 object handle

builtins 创建逻辑复用现有 `get_builtin_global`（import 312）中的创建代码。包含：

  - **构造函数型**（Object、Array、Number、Boolean、Error 等）：创建 `NativeCallable` 并设为属性
  - **对象型**（Math、JSON、Reflect）：创建空的 host object 并设为属性（子属性通过运行期 GetProp 链式访问，已有属性如 `Math.PI` 在编译时已折叠为常量，不涉及此路径）
  - **函数型**（parseInt、parseFloat、isNaN、isFinite 等）：创建对应的 `NativeCallable` 并设为属性
  - **`globalThis`**：指向自身（self-reference）
  - **存根型**（$262、Temporal、Intl 等）：仍返回 `undefined`（保持 stub 行为，不影响当前功能）

注意：`eval` 不走此路径。语义层对 `eval` 标识符有特殊路径（`Constant::NativeCallableEval`），保持不动。

`get_builtin_global` 自身在迁移完成后删除。
#### 语义层：$global 与 $this 分离

**引入两个作用域变量：**

| 变量 | scope | script 模式 | module 模式 | 用途 |
|---|---|---|---|---|
| `$0.$global` | 0 | 全局对象 | 全局对象 | builtin 查询、globalThis |
| `$0.$this` | 0 | 全局对象（同 $global） | `undefined` | this 表达式 |

在 `lower_module()` 的 entry block 中：

```rust
// 1. 创建全局对象
let go_val = ...;
CallBuiltin(CreateGlobalObject) → StoreVar("$0.$global", go_val)

// 2. 设置 $this
if script_mode {
    StoreVar("$0.$this", go_val)     // this = 全局对象
} else {
    StoreVar("$0.$this", undefined)  // module 模式：this = undefined
}
```

**`globalThis` 引用解析：**

当前：`CallBuiltin(GetBuiltinGlobal, "globalThis")` → `StubGlobal`

改为：
```
LoadVar("$0.$global") + GetProp($global, "globalThis")
```

全局对象的 `globalThis` 属性在 `CreateGlobalObject` 时已设为 `self`，因此返回正确的引用。

**builtin 引用解析（lowerer_assignments.rs）：**

当前：undeclared 全局标识符匹配 `is_builtin_global` → `CallBuiltin(GetBuiltinGlobal, name)`

改为：
```
let global_obj = lower_load_global(block)?;  // LoadVar $0.$global
let key = intern_string(name);
GetProp { dest, object: global_obj, key }
```

语义层新增 `lower_load_global()` 方法，等价于 `LoadVar { name: "$0.$global" }`。

**现有 `lower_this()` 不动：**
- 非箭头函数：`LoadVar { name: "$this" }` — 现在 scope 0 中始终有值
- 箭头函数：从 env 对象读取 — 不动

#### WASM 后端：main 签名

`main` 的 WASM 签名从 Type 1 `() -> ()` 改为 Type N `() -> i64`：

- 正常返回值：JS value encoded as i64（通常是 encode_undefined()）
- 异常返回值：TAG_EXCEPTION encoded as i64（与异常传播设计联动）

`CompileMode::Script` 和 `CompileMode::Normal` 都使用 `() -> i64`。
`CompileMode::Eval` 保持 `(i64) -> i64`。

`Builtin::CreateGlobalObject` 编译为：`Call import #316`，结果存到 dest local。

#### 运行期：main 返回值检查

```rust
// 当前
let main = instance.get_typed_func::<(), ()>(&mut store, "main")?;
let main_result = main.call(&mut store, ());

// 改为
let main = instance.get_typed_func::<(), i64>(&mut store, "main")?;
let main_result = main.call(&mut store, ());
match main_result {
    Ok(return_val) => {
        if value::is_exception(return_val) {
            // 检查 runtime_error，输出未捕获异常信息
            // 跳过 microtasks/timers
        } else {
            // 正常流程：microtasks + timers
        }
    }
    Err(trap) => {
        // 真实的 WASM trap — 保持现有错误处理
    }
}
```

#### 删除的代码

- `get_builtin_global` 宿主函数（import 312）
- `Builtin::GetBuiltinGlobal` IR 变体
- `StubGlobal` NativeCallable 变体（如无其他用途）

---

## 问题 2：跨函数异常传播

### 现状

- `Terminator::Throw { value }` 编译为 `call import_env_throw; unreachable`
- `Instruction::Call` / `Instruction::CallIndirect` 返回后无异常检查
- 同一函数内 try/catch 正常（IR 层 TryContext 重定向 throw 到 catch_entry）
- 跨函数调用时，被调函数的 throw → WASM trap → 无法捕获

### 设计

#### Terminator::Throw 编译变更

`Terminator::Throw { value }` 的输入仍然是用户实际抛出的 JS 值（可以是 object、string、number、undefined 等任意值）。跨函数传播不能直接把该值改 tag：`TAG_EXCEPTION` 目前是 handle tag，直接复用低 32 位会丢失 primitive thrown value。

因此新增一组异常封装/解封装 host helper：

| helper | 签名 | 作用 |
|---|---|---|
| `create_exception` | `(i64 thrown_value) -> i64` | 在运行时异常表中保存原始 thrown value，返回 `TAG_EXCEPTION` handle |
| `exception_value` | `(i64 exception_handle) -> i64` | 取回原始 thrown value，供 catch 绑定使用 |

IR 层对应新增 `Builtin::CreateException` / `Builtin::ExceptionValue`。现有 `EncodeException` / `ExceptionToObject` 的低 32 位重打 tag 实现不能继续使用；实现时直接删除或改名迁移，避免两套异常语义并存。

**JS 函数（Type 12：`(i64 env, i64 this, i32 args_base, i32 args_count) -> i64`）：**

```
// 当前
call import_env_throw
unreachable

// 改为
local.get thrown_value
call create_exception      // 返回 TAG_EXCEPTION handle
return
```

**main 函数（Type N：`() -> i64`）：**

```
local.get thrown_value
call import_env_throw      // 输出未捕获异常，设置 runtime_error
local.get thrown_value
call create_exception
return                     // 运行期看到 TAG_EXCEPTION 后跳过 microtasks/timers
```

这样 main 的返回值也符合统一约定：正常完成返回普通 JS 值，异常完成返回 `TAG_EXCEPTION`。

#### Call 后异常检查必须进入 IR/语义层

不能只在 WASM 后端给 `Instruction::Call` 加检查。原因：

- try/catch 的 catch_entry、exception_var 在 `Lowerer.try_contexts` 中，WASM 后端看不到
- throw 经过 finally 或 iterator cleanup 时，必须走 `emit_throw_value()` 中已有的清理逻辑
- 如果后端直接 `return TAG_EXCEPTION`，会绕过当前函数内包裹该 call 的 try/catch

正确做法：语义层在生成普通 `Instruction::Call` 后，立即把“返回值是否为异常”的分支显式写入 IR。

概念 IR：

```
%ret = call %callee(%args)
%is_exc = is_exception %ret
branch %is_exc, bb_exception, bb_continue

bb_exception:
  %thrown = exception_value %ret
  emit_throw_value(bb_exception, %thrown)

bb_continue:
  ... 正常使用 %ret ...
```

`emit_throw_value()` 已经知道当前 TryContext/finally/iterator-cleanup，因此：

- 若当前 call 在 try 块内：异常路径存入 catch exception var 并跳到 catch_entry
- 若当前 call 在 finally 保护范围内：先执行 pending finalizers，再继续传播
- 若当前 call 不在任何 handler 内：生成 `Terminator::Throw`，由当前函数返回 `TAG_EXCEPTION`

采用显式 block 形态：语义层创建 `bb_exception` / `bb_continue`，用现有 `IsException`、新的 `ExceptionValue` 和 `Terminator::Branch` 表达控制流。不得使用 Call metadata 形态；该方案会把 TryContext/cleanup 知识泄漏到后端，边界错误。

#### 表达式 lowering 的配套调整

当前 `lower_call_expr()` 返回 `ValueId`，调用者仍把后续指令追加到传入的原 block。插入异常分支后，call 的正常后继不再是原 block，而是新建的 `bb_continue`。

因此实现计划需要先引入一个小型表达式 lowering 辅助：

```rust
struct ExprResult {
    value: ValueId,
    block: BasicBlockId, // 值所在的可继续追加指令的 block
}
```

优先改造 call expression 及其直接调用点；已有短路表达式也存在类似“返回值在 merge block”的模式，后续可以逐步收敛到同一结构。

#### native_call 与 CallBuiltin

`Instruction::Call` 当前同时覆盖两条运行路径：

- `TAG_NATIVE_CALLABLE` → import `native_call`
- 普通 JS 函数/closure → `call_indirect`

两条路径都产生一个 i64 返回值，因此语义层的 post-call `IsException` 检查覆盖二者。

`Instruction::CallBuiltin` 是直接宿主调用，不走 JS 函数调用约定。除 `Builtin::Eval` 这类已经返回 `TAG_EXCEPTION` 的内建外，普通 host builtin 仍由 host 侧设置 runtime_error。实现时必须逐个确认会返回 `TAG_EXCEPTION` 的 CallBuiltin，并在语义层给这些调用同样包 post-call 检查，不能简单假设全部不需要。

#### import_env_throw 简化

`import_env_throw` 不再在普通 JS 函数 throw 路径中调用。它只用于 main 入口的未捕获异常展示。渲染仍基于原始 thrown value，而不是 `TAG_EXCEPTION` handle。

---

## 受影响文件清单

### wjsm-ir
| 文件 | 改动 |
|---|---|
| `src/builtin.rs` | 删除 `GetBuiltinGlobal`；新增 `CreateGlobalObject`、`CreateException`、`ExceptionValue` |
| `src/lib.rs` | `Program` 新增 `script_mode: bool`；`EncodeException` / `ExceptionToObject` 重命名或替换为保留 thrown value 的 `CreateException` / `ExceptionValue` |

### wjsm-parser
| 文件 | 改动 |
|---|---|
| — | 无（`parse_script_as_module` 保持） |

### wjsm-semantic
| 文件 | 改动 |
|---|---|
| `src/lib.rs` | `lower_module()` 新增 `script: bool` 参数 |
| `src/lowerer_core.rs` | 新增 `script_mode` 字段；`lower_module` 中创建全局对象、设置 `$this`/`$global`；main 正常完成返回 `undefined` |
| `src/lowerer_assignments.rs` | builtin 引用解析改为 `LoadVar($global) + GetProp` |
| `src/lowerer_calls_eval.rs` | 普通 `Instruction::Call` 后插入显式异常检查 block；改造 call expression 的返回 block 传播 |
| `src/lowerer_async_eval.rs` | 新增 `lower_load_global()` 方法；TLA/eval 模式下正确处理 `$global` / `$this` |
| `src/builtins.rs` | 保留 `is_builtin_global` 作为全局对象属性解析触发器；删除 `GetBuiltinGlobal` 映射 |

### wjsm-backend-wasm
| 文件 | 改动 |
|---|---|
| `src/lib.rs` | `compile()` 读取 `Program.script_mode`；import list 更新 |
| `src/compiler_module.rs` | main 签名 `Type1`→`TypeN`；`CompileMode::Script` 处理 |
| `src/compiler_control.rs` | `Terminator::Throw` 改为 `create_exception + return`，main 路径额外调用 `throw_fn` |
| `src/compiler_instructions.rs` | 编译语义层生成的 `IsException` / exception dispatch blocks；不在后端私自决定 catch 路径 |
| `src/compiler_builtins.rs` | 新增 `CreateGlobalObject`、异常 helper 编译；删除 `GetBuiltinGlobal` |
| `src/compiler_core.rs` | 新 import 注册：`create_global_object`、`create_exception`、`exception_value` |

### wjsm-runtime
| 文件 | 改动 |
|---|---|
| `src/lib.rs` | main 调用改为 `TypedFunc<(), i64>`；返回值异常检查 |
| `src/host_imports/collections_buffers.rs` | 新增 `create_global_object_fn`；删除 `get_builtin_global_fn` |
| `src/host_imports/core.rs` | `throw_fn` 仅处理 main 未捕获异常；新增/接入异常表 helpers |

### wjsm-cli
| 文件 | 改动 |
|---|---|
| `src/lib.rs` | `compile_source` 传递 script 标志；`run_pipeline` 传递到 semantic/backend |

### wjsm-test262
| 文件 | 改动 |
|---|---|
| — | 无（已传递 `--script`） |
