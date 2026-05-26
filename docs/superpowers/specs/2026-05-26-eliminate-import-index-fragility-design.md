# 消除宿主导入索引脆弱性 — 设计文档

## 问题

ES 运行时通过 WASM 宿主函数（imports）实现语言内建能力（console.log、Promise、Array 方法等）。当前系统中**同一份"函数→WASM索引"映射被四处独立维护**，任一更改都会导致静默错位：

### 脆弱点

| 位置 | 作用 | 脆弱原因 |
|---|---|---|
| `HOST_IMPORT_NAMES[384]`（`compiler_core.rs`） | WASM import section 的顺序定义 | 手动维护顺序 |
| `builtin_func_indices`（`compiler_core.rs`，~300 行） | `Builtin` 枚举变体 → WASM 索引 | 每个映射手写硬编码数字 |
| `imports: Vec<Extern>` 组装（`lib.rs:211-794`） | 运行时宿主函数的顺序 Vec | `include!` + `extend` + `push` 顺序 |
| `register_all_imports`（`misc.rs:235-264`） | 将多个子模块 Vec 穿插成正确顺序 | `drain(0..6)`/`remove(0)` 按精确数量拆散重组 |
| 各子模块 Vec 返回值（16 个文件） | 带 `// 116` 索引注释的函数列表 | 注释与实际索引脱钩无检测 |

### 后果

- 新增导入容易插错位置，运行时静默调用错误函数
- 子模块内部调序（使代码更清晰）会导致索引偏移
- `register_all_imports` 的 `drain`/`remove` 依赖对所有子模块内部顺序的精确了解
- 不存在编译期或运行期验证

## 设计目标

1. **一劳永逸**：新增导入时不需要在任何地方手写数字索引
2. **编译期/初始化期检测**：不一致立即 panic，而非静默错位
3. **高性能**：WASM `call` 指令路径零开销
4. **优雅**：消除 `register_all_imports` 的 drain/remove 交错逻辑，消除 `include!` 裸块

## 设计

### 核心原则

`HOST_IMPORT_NAMES` 为**唯一真相来源（SSOT）**。运行时和后端的所有映射都从它派生。

### 数据流

```
HOST_IMPORT_NAMES[384]
    │
    ├──→ WASM import section 生成（已有的迭代代码不变）
    ├──→ builtin_func_indices: HashMap<Builtin, u32>（自动生成，replaces 300行手写）
    │        │
    │        └──→ WASM call 指令（编译时查表，路径不变）
    │
    └──→ 运行时 Linker: 按 ("env", name) 注册宿主函数
              （replaces 按位置 Vec<Extern>）
```

### 运行时侧：wasmtime Linker

**当前**：`Instance::new(&mut store, &module, &imports)` — 按位置链接。

**变更后**：使用 wasmtime `Linker` API，按 `(module, name)` 注册：

```rust
let mut linker = Linker::new(engine);
define_promise(&mut linker)?;           // 注册 "promise_create" 等
define_promise_combinators(&mut linker)?; // 注册 "promise_all" 等
define_async_fn(&mut linker)?;
// ... 每个功能域一个 define_xxx 函数
let instance = linker.instantiate(&mut store, &module)?;
```

每个 `define_xxx` 函数签名：

```rust
fn define_promise(linker: &mut Linker<RuntimeState>) -> anyhow::Result<()> {
    linker.func_wrap("env", "promise_create",
        |mut caller: Caller<'_, RuntimeState>, _arg: i64| -> i64 {
            // 函数体直接从当前 promise.rs 搬过来，完全不变
        },
    )?;
    linker.func_wrap("env", "promise_instance_resolve",
        |mut caller: Caller<'_, RuntimeState>, promise: i64, value: i64| {
            // 完全不变
        },
    )?;
    Ok(())
}
```

**关键点**：
- `linker.func_wrap` 的闭包类型与 `Func::wrap` 完全相同 — 无需修改函数体
- 注册顺序无关 — Linker 按名字匹配 WASM 模块的 `(import "env" "xxx")`
- 删除 `register_all_imports` 及其 drain/remove 逻辑
- 将 `include!` 裸块文件改为正常 `pub(crate) fn`

### 后端侧：自动生成 builtin_func_indices

**当前**：`compiler_core.rs` 中约 300 行手写映射：

```rust
builtin_func_indices.insert(Builtin::ConsoleLog, 0);
builtin_func_indices.insert(Builtin::ConsoleError, 23);
// ... ~300 行
builtin_func_indices.insert(Builtin::ObjGetByIndex, 383);
```

**变更后**：在 `Compiler::new()` 时自动生成：

```rust
fn build_builtin_func_indices() -> HashMap<Builtin, u32> {
    // 先构建 name → index 反向查表
    let name_to_idx: HashMap<&str, u32> = HOST_IMPORT_NAMES.iter()
        .enumerate()
        .map(|(i, name)| (*name, i as u32))
        .collect();

    let mut map = HashMap::new();
    // 需要遍历所有 Builtin 变体，用 Display 名字查反向表
    for (variant, name) in BUILTIN_VARIANTS {
        let idx = name_to_idx.get(name)
            .expect("Builtin 变体的 Display 名字必须在 HOST_IMPORT_NAMES 中存在");
        map.insert(variant, *idx);
    }
    map
}
```

**如何遍历所有 Builtin 变体**：使用一个常量数组 `ALL_BUILTINS: &[Builtin]`。这是唯一需要手动维护的地方 — 但它只列变体，不写数字。

### 各模块变更清单

#### 运行时文件

| 文件 | 变更 |
|---|---|
| `host_imports/promise.rs` | `fn register_promise_imports(...) -> Vec<Extern>` → `fn define_promise(linker: &mut Linker<RuntimeState>) -> Result<()>` |
| `host_imports/promise_combinators.rs` | 同上模式 |
| `host_imports/async_fn.rs` | 同上 |
| `host_imports/async_generator.rs` | 同上 |
| `host_imports/proxy_reflect.rs` | 同上 |
| `host_imports/misc.rs` | 拆分出 `define_misc`；删除 `register_all_imports` 和 `register_misc_imports` |
| `host_imports/promise_async.rs` | 删除（当前仅为重新导出 `misc.rs`） |
| `host_imports/core.rs` | `include!` 裸块 → `pub(crate) fn define_core(...)` |
| `host_imports/timers_arrays.rs` | 同上 |
| `host_imports/array_object.rs` | 同上 |
| `host_imports/primitive_core.rs` | 同上 |
| `host_imports/string_methods.rs` | 同上 |
| `host_imports/math_number_error.rs` | 同上 |
| `host_imports/collections_buffers.rs` | 同上 |
| `host_imports/proxy_traps.rs` | 同上 |
| `host_imports/typedarray_new_methods.rs` | 同上 |
| `host_imports/weakref_finalization.rs` | 同上 |
| `host_imports/atomics.rs` | 同上 |
| `host_imports/get_builtin_global_entry.rs` | 同上 |
| `lib.rs` | `Vec<Extern>` 组装 → `Linker` 注册；删除 `Instance::new` |
| `host_imports/mod.rs` | 更新模块声明 |

#### 后端文件

| 文件 | 变更 |
|---|---|
| `compiler_core.rs` | 添加 `ALL_BUILTINS` 常量数组 + `build_builtin_func_indices()`；删除 300 行手写映射 |

#### IR 文件

| 文件 | 变更 |
|---|---|
| `builtin.rs` | 可能需要添加 `ALL_BUILTINS` 数组（如果在 IR crate 定义 Builtin 枚举） |

### 不变的部分

- `HOST_IMPORT_NAMES` 数组内容不变（仍是生成 WASM import section 的权威顺序）
- WASM `call` 指令生成路径不变（仍通过 `builtin_func_indices` 查表）
- WASM 模块的 import/export section 格式不变
- 所有宿主函数的闭包体不变

### 性能

| 阶段 | 当前 | 变更后 | 差异 |
|---|---|---|---|
| WASM `call` 指令 | `Call(数字索引)` | `Call(数字索引)` | 零差异 |
| 模块实例化 | `Instance::new()` 按位置数组 | `Linker::instantiate()` 按名字哈希 | 一次性，微秒级差异 |
| 编译器初始化 | 300 行 HashMap insert | 遍历 ALL_BUILTINS + name_to_idx 查表 | 等同 |

## 验证策略

1. **编译期检测**：`build_builtin_func_indices` 中 `expect` — 如果 `Builtin::Display` 名字不在 `HOST_IMPORT_NAMES` 中，在编译器初始化时 panic（而非运行时静默调用错误函数）
2. **运行时检测**：`Linker::func_wrap` 返回 `Result` — 如果注册了 WASM 模块不导入的函数，会报错
3. **现有测试**：全部 372 个 E2E fixture 测试保持不变，作为回归防线
4. **快照测试**：semantic snapshot 测试不受影响（不涉及运行时/后端层）

## 迁移顺序

将每个 `host_imports/*.rs` 的改造作为独立 Phase，每个 Phase 可独立编译验证。最后统一汇入 `lib.rs` 主组装点，一次性切换到 Linker。
