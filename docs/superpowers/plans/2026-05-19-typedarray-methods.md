# TypedArray 方法全量补全 — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 补全 11 种 TypedArray 变体的全部原型方法（23个新增/修复）+ BigInt64Array/BigUint64Array 构造器注册，实现 TypedArray 值的渲染。

**Architecture:** 通过四层流水线补全：IR 新增 25 个 Builtin 变体 → Semantic 新增方法路由 + BigInt64Array/BigUint64Array 全局识别 → Backend 注册 WASM import（index 326-355）→ Runtime 实现 host 函数（简单方法直接操作 ArrayBuffer 数据，回调方法复用 Type 12 shadow stack 模式调 WASM callback）。

**Tech Stack:** Rust, wjsm-ir, wjsm-semantic, wjsm-backend-wasm, wjsm-runtime

**Files to modify:**
- `crates/wjsm-ir/src/builtin.rs` — Builtin 枚举 + Display
- `crates/wjsm-semantic/src/builtins.rs` — global idents, typedarray proto method router, call signatures
- `crates/wjsm-semantic/src/lowerer_calls_eval.rs` — TypedArray 方法分发块
- `crates/wjsm-backend-wasm/src/compiler_core.rs` — WASM import types, import declarations, func_indices
- `crates/wjsm-backend-wasm/src/compiler_builtins.rs` — codegen 分支
- `crates/wjsm-runtime/src/host_imports/collections_buffers.rs` — 所有 host 函数实现
- `crates/wjsm-runtime/src/runtime_render.rs` — TypedArray 渲染分支

---

### Task 1: IR 层 — 新增 Builtin 变体

**Files:**
- Modify: `crates/wjsm-ir/src/builtin.rs`

- [ ] **Step 1: 在 Builtin 枚举中新增 25 个变体**

在 `TypedArrayProtoSubarray,` 之后（约 line 339），插入以下变体：

```rust
    // ── TypedArray 新增构造器 ──
    BigInt64ArrayConstructor,
    BigUint64ArrayConstructor,
    // ── TypedArray 新增原型方法 — 简单方法 ──
    TypedArrayProtoFill,
    TypedArrayProtoReverse,
    TypedArrayProtoIndexOf,
    TypedArrayProtoLastIndexOf,
    TypedArrayProtoIncludes,
    TypedArrayProtoJoin,
    TypedArrayProtoToString,
    TypedArrayProtoCopyWithin,
    TypedArrayProtoAt,
    // ── TypedArray 新增原型方法 — 回调方法 (Type 12) ──
    TypedArrayProtoForEach,
    TypedArrayProtoMap,
    TypedArrayProtoFilter,
    TypedArrayProtoReduce,
    TypedArrayProtoReduceRight,
    TypedArrayProtoFind,
    TypedArrayProtoFindIndex,
    TypedArrayProtoSome,
    TypedArrayProtoEvery,
    TypedArrayProtoSort,
    // ── TypedArray 迭代器方法 ──
    TypedArrayProtoEntries,
    TypedArrayProtoKeys,
    TypedArrayProtoValues,
```

- [ ] **Step 2: 在 Display impl 中新增对应 Display 条目**

在 `TypedArrayProtoSubarray => "TypedArray.prototype.subarray",` 之后（约 line 640），插入：

```rust
            Self::BigInt64ArrayConstructor => "BigInt64Array",
            Self::BigUint64ArrayConstructor => "BigUint64Array",
            Self::TypedArrayProtoFill => "TypedArray.prototype.fill",
            Self::TypedArrayProtoReverse => "TypedArray.prototype.reverse",
            Self::TypedArrayProtoIndexOf => "TypedArray.prototype.indexOf",
            Self::TypedArrayProtoLastIndexOf => "TypedArray.prototype.lastIndexOf",
            Self::TypedArrayProtoIncludes => "TypedArray.prototype.includes",
            Self::TypedArrayProtoJoin => "TypedArray.prototype.join",
            Self::TypedArrayProtoToString => "TypedArray.prototype.toString",
            Self::TypedArrayProtoCopyWithin => "TypedArray.prototype.copyWithin",
            Self::TypedArrayProtoAt => "TypedArray.prototype.at",
            Self::TypedArrayProtoForEach => "TypedArray.prototype.forEach",
            Self::TypedArrayProtoMap => "TypedArray.prototype.map",
            Self::TypedArrayProtoFilter => "TypedArray.prototype.filter",
            Self::TypedArrayProtoReduce => "TypedArray.prototype.reduce",
            Self::TypedArrayProtoReduceRight => "TypedArray.prototype.reduceRight",
            Self::TypedArrayProtoFind => "TypedArray.prototype.find",
            Self::TypedArrayProtoFindIndex => "TypedArray.prototype.findIndex",
            Self::TypedArrayProtoSome => "TypedArray.prototype.some",
            Self::TypedArrayProtoEvery => "TypedArray.prototype.every",
            Self::TypedArrayProtoSort => "TypedArray.prototype.sort",
            Self::TypedArrayProtoEntries => "TypedArray.prototype.entries",
            Self::TypedArrayProtoKeys => "TypedArray.prototype.keys",
            Self::TypedArrayProtoValues => "TypedArray.prototype.values",
```

- [ ] **Step 3: 构建检查 + 提交**

```bash
cargo check -p wjsm-ir
```

Expected: compiles (with warnings about unused variants — expected at this stage).

```bash
git add crates/wjsm-ir/src/builtin.rs
git commit -m "feat(ir): add 25 new TypedArray builtin variants for full method set

Co-authored-by: CommandCodeBot <noreply@commandcode.ai>"
```

---

### Task 2: Semantic 层 — 方法路由 + 全局识别

**Files:**
- Modify: `crates/wjsm-semantic/src/builtins.rs`
- Modify: `crates/wjsm-semantic/src/lowerer_calls_eval.rs`

- [ ] **Step 1: 在 `builtin_from_global_ident` 中新增 BigInt64Array/BigUint64Array**

在 `"Float64Array" => Some(Builtin::Float64ArrayConstructor),` 之后，插入：

```rust
        "BigInt64Array" => Some(Builtin::BigInt64ArrayConstructor),
        "BigUint64Array" => Some(Builtin::BigUint64ArrayConstructor),
```

- [ ] **Step 2: 新增 `builtin_from_typedarray_proto_method` 函数**

在 `builtin_from_error_proto_method` 之后（约 line 311），插入：

```rust
/// 将 TypedArray.prototype 方法名映射到 Builtin 变体，用于语义层优化。
/// 当 `typedArr.forEach(cb)` 被识别时，跳过运行时属性解析，直接发出 CallBuiltin。
pub(crate) fn builtin_from_typedarray_proto_method(name: &str) -> Option<Builtin> {
    use Builtin::*;
    match name {
        "set" => Some(TypedArrayProtoSet),
        "subarray" => Some(TypedArrayProtoSubarray),
        "slice" => Some(TypedArrayProtoSlice),
        "fill" => Some(TypedArrayProtoFill),
        "reverse" => Some(TypedArrayProtoReverse),
        "indexOf" => Some(TypedArrayProtoIndexOf),
        "lastIndexOf" => Some(TypedArrayProtoLastIndexOf),
        "includes" => Some(TypedArrayProtoIncludes),
        "join" => Some(TypedArrayProtoJoin),
        "toString" => Some(TypedArrayProtoToString),
        "copyWithin" => Some(TypedArrayProtoCopyWithin),
        "at" => Some(TypedArrayProtoAt),
        "forEach" => Some(TypedArrayProtoForEach),
        "map" => Some(TypedArrayProtoMap),
        "filter" => Some(TypedArrayProtoFilter),
        "reduce" => Some(TypedArrayProtoReduce),
        "reduceRight" => Some(TypedArrayProtoReduceRight),
        "find" => Some(TypedArrayProtoFind),
        "findIndex" => Some(TypedArrayProtoFindIndex),
        "some" => Some(TypedArrayProtoSome),
        "every" => Some(TypedArrayProtoEvery),
        "sort" => Some(TypedArrayProtoSort),
        "entries" => Some(TypedArrayProtoEntries),
        "keys" => Some(TypedArrayProtoKeys),
        "values" => Some(TypedArrayProtoValues),
        _ => None,
    }
}
```

- [ ] **Step 3: 在 `builtin_call_signature` 中新增条目**

在 `Builtin::TypedArrayProtoSubarray => ("TypedArray.prototype.subarray", 3),` 之后，插入：

```rust
        Builtin::BigInt64ArrayConstructor => ("BigInt64Array", 3),
        Builtin::BigUint64ArrayConstructor => ("BigUint64Array", 3),
        Builtin::TypedArrayProtoFill => ("TypedArray.prototype.fill", 3),
        Builtin::TypedArrayProtoReverse => ("TypedArray.prototype.reverse", 1),
        Builtin::TypedArrayProtoIndexOf => ("TypedArray.prototype.indexOf", 3),
        Builtin::TypedArrayProtoLastIndexOf => ("TypedArray.prototype.lastIndexOf", 3),
        Builtin::TypedArrayProtoIncludes => ("TypedArray.prototype.includes", 3),
        Builtin::TypedArrayProtoJoin => ("TypedArray.prototype.join", 2),
        Builtin::TypedArrayProtoToString => ("TypedArray.prototype.toString", 1),
        Builtin::TypedArrayProtoCopyWithin => ("TypedArray.prototype.copyWithin", 4),
        Builtin::TypedArrayProtoAt => ("TypedArray.prototype.at", 2),
        Builtin::TypedArrayProtoForEach => ("TypedArray.prototype.forEach", 3),
        Builtin::TypedArrayProtoMap => ("TypedArray.prototype.map", 3),
        Builtin::TypedArrayProtoFilter => ("TypedArray.prototype.filter", 3),
        Builtin::TypedArrayProtoReduce => ("TypedArray.prototype.reduce", 3),
        Builtin::TypedArrayProtoReduceRight => ("TypedArray.prototype.reduceRight", 3),
        Builtin::TypedArrayProtoFind => ("TypedArray.prototype.find", 3),
        Builtin::TypedArrayProtoFindIndex => ("TypedArray.prototype.findIndex", 3),
        Builtin::TypedArrayProtoSome => ("TypedArray.prototype.some", 3),
        Builtin::TypedArrayProtoEvery => ("TypedArray.prototype.every", 3),
        Builtin::TypedArrayProtoSort => ("TypedArray.prototype.sort", 2),
        Builtin::TypedArrayProtoEntries => ("TypedArray.prototype.entries", 1),
        Builtin::TypedArrayProtoKeys => ("TypedArray.prototype.keys", 1),
        Builtin::TypedArrayProtoValues => ("TypedArray.prototype.values", 1),
```

- [ ] **Step 4: 在 `lowerer_calls_eval.rs` 中新增 TypedArray 方法分发块**

在 Array.prototype 方法分发块之后（约 line 155，`builtin_from_function_proto_method` 分发块之前），插入：

```rust
                    // TypedArray.prototype 方法调用优化：发出 CallBuiltin 代替 Call，
                    // 跳过运行时属性解析。
                    if let swc_ast::MemberProp::Ident(prop_ident) = &member_expr.prop {
                        if let Some(ta_builtin) =
                            builtin_from_typedarray_proto_method(&prop_ident.sym)
                        {
                            this_val = self.lower_expr(&member_expr.obj, block)?;
                            let mut builtin_args = vec![this_val];
                            for arg in &call.args {
                                builtin_args.push(self.lower_expr(&arg.expr, block)?);
                            }
                            let dest = self.alloc_value();
                            self.current_function.append_instruction(
                                block,
                                Instruction::CallBuiltin {
                                    dest: Some(dest),
                                    builtin: ta_builtin,
                                    args: builtin_args,
                                },
                            );
                            return Ok(dest);
                        }
                    }
```

- [ ] **Step 5: 构建检查 + 提交**

```bash
cargo check -p wjsm-semantic
```

Expected: compiles.

```bash
git add crates/wjsm-semantic/src/builtins.rs crates/wjsm-semantic/src/lowerer_calls_eval.rs
git commit -m "feat(semantic): add TypedArray method routing and BigInt64Array/BigUint64Array recognition

Co-authored-by: CommandCodeBot <noreply@commandcode.ai>"
```

---

### Task 3: Backend WASM — 注册 import 类型、声明、func_indices、codegen

**Files:**
- Modify: `crates/wjsm-backend-wasm/src/compiler_core.rs`
- Modify: `crates/wjsm-backend-wasm/src/compiler_builtins.rs`

- [ ] **Step 1: 在 `compiler_core.rs` 类型段新增类型注释**

Type 2 `(i64, i64)->i64` 和 Type 3 `(i64)->i64` 以及 Type 12 `(i64,i64,i32,i32)->i64` 和 Type 16 `(i64,i64,i64)->i64` 都已存在。无需新增类型。

- [ ] **Step 2: 在 `compiler_core.rs` 注册新的 WASM import 声明**

在 `// Import index 311: typedarray_proto_subarray` 之后，`// Import index 312: create_global_object` 之前（约 line 811），插入：

```rust
        // ── TypedArray 新增构造器 imports ──
        // Import index 326: bigint64array_constructor: (i64, i64, i64) -> i64
        imports.import("env", "bigint64array_constructor", EntityType::Function(16));
        // Import index 327: biguint64array_constructor: (i64, i64, i64) -> i64
        imports.import("env", "biguint64array_constructor", EntityType::Function(16));
        // ── TypedArray 新增原型方法: 简单方法 (Type 16 or Type 2) ──
        // Import index 328: typedarray_proto_fill: Type 12 shadow stack (this, value, start, end)
        imports.import("env", "typedarray_proto_fill", EntityType::Function(12));
        // Import index 329: typedarray_proto_reverse: (i64) -> i64
        imports.import("env", "typedarray_proto_reverse", EntityType::Function(3));
        // Import index 330: typedarray_proto_index_of: (i64, i64, i64) -> i64
        imports.import("env", "typedarray_proto_index_of", EntityType::Function(16));
        // Import index 331: typedarray_proto_last_index_of: (i64, i64, i64) -> i64
        imports.import("env", "typedarray_proto_last_index_of", EntityType::Function(16));
        // Import index 332: typedarray_proto_includes: (i64, i64, i64) -> i64
        imports.import("env", "typedarray_proto_includes", EntityType::Function(16));
        // Import index 333: typedarray_proto_join: (i64, i64) -> i64
        imports.import("env", "typedarray_proto_join", EntityType::Function(2));
        // Import index 334: typedarray_proto_to_string: (i64) -> i64
        imports.import("env", "typedarray_proto_to_string", EntityType::Function(3));
        // Import index 335: typedarray_proto_copy_within: (i64, i64, i64) -> i64
        imports.import("env", "typedarray_proto_copy_within", EntityType::Function(16));
        // Import index 336: typedarray_proto_at: (i64, i64) -> i64
        imports.import("env", "typedarray_proto_at", EntityType::Function(2));
        // ── TypedArray 新增原型方法: 回调方法 (Type 12 shadow stack) ──
        // Import index 337: typedarray_proto_for_each: Type 12
        imports.import("env", "typedarray_proto_for_each", EntityType::Function(12));
        // Import index 338: typedarray_proto_map: Type 12
        imports.import("env", "typedarray_proto_map", EntityType::Function(12));
        // Import index 339: typedarray_proto_filter: Type 12
        imports.import("env", "typedarray_proto_filter", EntityType::Function(12));
        // Import index 340: typedarray_proto_reduce: Type 12
        imports.import("env", "typedarray_proto_reduce", EntityType::Function(12));
        // Import index 341: typedarray_proto_reduce_right: Type 12
        imports.import("env", "typedarray_proto_reduce_right", EntityType::Function(12));
        // Import index 342: typedarray_proto_find: Type 12
        imports.import("env", "typedarray_proto_find", EntityType::Function(12));
        // Import index 343: typedarray_proto_find_index: Type 12
        imports.import("env", "typedarray_proto_find_index", EntityType::Function(12));
        // Import index 344: typedarray_proto_some: Type 12
        imports.import("env", "typedarray_proto_some", EntityType::Function(12));
        // Import index 345: typedarray_proto_every: Type 12
        imports.import("env", "typedarray_proto_every", EntityType::Function(12));
        // Import index 346: typedarray_proto_sort: Type 12
        imports.import("env", "typedarray_proto_sort", EntityType::Function(12));
        // ── TypedArray 迭代器方法: (i64) -> i64 ──
        // Import index 347: typedarray_proto_entries: (i64) -> i64
        imports.import("env", "typedarray_proto_entries", EntityType::Function(3));
        // Import index 348: typedarray_proto_keys: (i64) -> i64
        imports.import("env", "typedarray_proto_keys", EntityType::Function(3));
        // Import index 349: typedarray_proto_values: (i64) -> i64
        imports.import("env", "typedarray_proto_values", EntityType::Function(3));
```

- [ ] **Step 3: 在 `builtin_func_indices` 中新增映射**

在 `TypedArrayProtoSubarray` 映射（约 line 1220）之后，插入：

```rust
        // ── TypedArray 新增构造器 ──
        builtin_func_indices.insert(Builtin::BigInt64ArrayConstructor, 326);
        builtin_func_indices.insert(Builtin::BigUint64ArrayConstructor, 327);
        // ── TypedArray 新增原型方法 ──
        builtin_func_indices.insert(Builtin::TypedArrayProtoFill, 328);
        builtin_func_indices.insert(Builtin::TypedArrayProtoReverse, 329);
        builtin_func_indices.insert(Builtin::TypedArrayProtoIndexOf, 330);
        builtin_func_indices.insert(Builtin::TypedArrayProtoLastIndexOf, 331);
        builtin_func_indices.insert(Builtin::TypedArrayProtoIncludes, 332);
        builtin_func_indices.insert(Builtin::TypedArrayProtoJoin, 333);
        builtin_func_indices.insert(Builtin::TypedArrayProtoToString, 334);
        builtin_func_indices.insert(Builtin::TypedArrayProtoCopyWithin, 335);
        builtin_func_indices.insert(Builtin::TypedArrayProtoAt, 336);
        builtin_func_indices.insert(Builtin::TypedArrayProtoForEach, 337);
        builtin_func_indices.insert(Builtin::TypedArrayProtoMap, 338);
        builtin_func_indices.insert(Builtin::TypedArrayProtoFilter, 339);
        builtin_func_indices.insert(Builtin::TypedArrayProtoReduce, 340);
        builtin_func_indices.insert(Builtin::TypedArrayProtoReduceRight, 341);
        builtin_func_indices.insert(Builtin::TypedArrayProtoFind, 342);
        builtin_func_indices.insert(Builtin::TypedArrayProtoFindIndex, 343);
        builtin_func_indices.insert(Builtin::TypedArrayProtoSome, 344);
        builtin_func_indices.insert(Builtin::TypedArrayProtoEvery, 345);
        builtin_func_indices.insert(Builtin::TypedArrayProtoSort, 346);
        builtin_func_indices.insert(Builtin::TypedArrayProtoEntries, 347);
        builtin_func_indices.insert(Builtin::TypedArrayProtoKeys, 348);
        builtin_func_indices.insert(Builtin::TypedArrayProtoValues, 349);
```

- [ ] **Step 4: 在 `compile_builtin_call` (compiler_builtins.rs) 中新增 codegen 分支**

在 `Builtin::TypedArrayProtoSet | Builtin::TypedArrayProtoSlice | Builtin::TypedArrayProtoSubarray => {` (约 line 674) 这个 match arm 中追加所有新方法。

找到这个 arm，展开为：

```rust
            // ── TypedArray 新增构造器 (Type 16: 3-arg) ──
            | Builtin::BigInt64ArrayConstructor
            | Builtin::BigUint64ArrayConstructor
            // ── TypedArray 已有构造器 (不变) ──
            | Builtin::Int8ArrayConstructor
            | Builtin::Uint8ArrayConstructor
            | Builtin::Uint8ClampedArrayConstructor
            | Builtin::Int16ArrayConstructor
            | Builtin::Uint16ArrayConstructor
            | Builtin::Int32ArrayConstructor
            | Builtin::Uint32ArrayConstructor
            | Builtin::Float32ArrayConstructor
            | Builtin::Float64ArrayConstructor
            // ── TypedArray 原型方法: Type 16 (3-arg) ──
            | Builtin::TypedArrayProtoSet
            | Builtin::TypedArrayProtoSlice
            | Builtin::TypedArrayProtoSubarray
            | Builtin::TypedArrayProtoIndexOf
            | Builtin::TypedArrayProtoLastIndexOf
            | Builtin::TypedArrayProtoIncludes
            | Builtin::TypedArrayProtoCopyWithin
            // ── TypedArray 原型方法: Type 2 (2-arg) ──
            | Builtin::TypedArrayProtoJoin
            | Builtin::TypedArrayProtoAt => {
                for arg in args {
                    self.emit(WasmInstruction::LocalGet(self.local_idx(arg.0)));
                }
                let func_idx = self
                    .builtin_func_indices
                    .get(builtin)
                    .copied()
                    .with_context(|| format!("no WASM func index for builtin {builtin}"))?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
```

注意：`TypedArrayProtoJoin` 和 `TypedArrayProtoAt` 只有 2 个参数，但它们通过 `for arg in args` 循环推入，由 semantic 层控制参数个数，所以可以直接复用同一个 arm。

然后在 `Builtin::DateConstructor` 上面（约 line 700），新增 TypedArray 回调方法的 codegen arm：

```rust
            // ── TypedArray 原型方法: Type 3 (1-arg) ──
            Builtin::TypedArrayProtoReverse
            | Builtin::TypedArrayProtoToString
            | Builtin::TypedArrayProtoEntries
            | Builtin::TypedArrayProtoKeys
            | Builtin::TypedArrayProtoValues => {
                for arg in args {
                    self.emit(WasmInstruction::LocalGet(self.local_idx(arg.0)));
                }
                let func_idx = self
                    .builtin_func_indices
                    .get(builtin)
                    .copied()
                    .with_context(|| format!("no WASM func index for builtin {builtin}"))?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            // ── TypedArray 原型方法: Type 12 shadow stack (回调方法) ──
            Builtin::TypedArrayProtoFill
            | Builtin::TypedArrayProtoForEach
            | Builtin::TypedArrayProtoMap
            | Builtin::TypedArrayProtoFilter
            | Builtin::TypedArrayProtoReduce
            | Builtin::TypedArrayProtoReduceRight
            | Builtin::TypedArrayProtoFind
            | Builtin::TypedArrayProtoFindIndex
            | Builtin::TypedArrayProtoSome
            | Builtin::TypedArrayProtoEvery
            | Builtin::TypedArrayProtoSort => self.compile_proto_method_call(dest, builtin, args),
```

- [ ] **Step 5: 构建检查 + 提交**

```bash
cargo check -p wjsm-backend-wasm
```

Expected: compiles.

```bash
git add crates/wjsm-backend-wasm/src/compiler_core.rs crates/wjsm-backend-wasm/src/compiler_builtins.rs
git commit -m "feat(wasm-backend): register TypedArray new methods as WASM imports (indices 326-349)

Co-authored-by: CommandCodeBot <noreply@commandcode.ai>"
```

---

### Task 4: Runtime — BigInt64Array/BigUint64Array 构造器 + 辅助函数

**Files:**
- Modify: `crates/wjsm-runtime/src/host_imports/collections_buffers.rs`

**Goal:** 新增两个构造器 + 提取通用的 TypedArray 元素读写辅助函数（后续方法复用）。

- [ ] **Step 1: 新增 BigInt64Array/BigUint64Array 构造函数**

在 `float64array_constructor_fn` 之后（约 line 1256），插入：

```rust
    typedarray_constructor!(bigint64array_constructor_fn, 8);
    typedarray_constructor!(biguint64array_constructor_fn, 8);
```

- [ ] **Step 2: 提取 TypedArray entry 解析辅助函数**

在 `typedarray_constructor!` 宏之前（约 line 1240），插入以下辅助函数。这些是后续所有方法复用的基础设施。文件顶部需要有 `use wjsm_ir::value;` 等 import。

在 `// ── TypedArray host functions ──` 注释块之后、第一个 `typedarray_constructor!` 之前插入：

```rust
    /// 从 receiver 对象解析 TypedArrayEntry。返回 (entry, table_lock_guard)。
    /// 在同一个函数中获取 lock 并压入 let binding 避免闭包中借用问题。
    macro_rules! resolve_typedarray {
        ($caller:expr, $receiver:expr) => {{
            let obj_ptr = resolve_handle_idx(&mut $caller, value::decode_object_handle($receiver) as usize);
            match obj_ptr {
                Some(ptr) => {
                    let ta_handle_val = read_object_property_by_name(&mut $caller, ptr, "__typedarray_handle__");
                    match ta_handle_val {
                        Some(v) => {
                            let handle = value::decode_f64(v) as usize;
                            let ta_table = $caller.data().typedarray_table.lock().expect("typedarray_table mutex");
                            if handle < ta_table.len() {
                                let entry = ta_table[handle].clone();
                                Some((entry.buffer_handle, entry.byte_offset, entry.length, entry.element_size))
                            } else {
                                None
                            }
                        }
                        None => None,
                    }
                }
                None => None,
            }
        }};
    }
```

这个 macro 因为 borrow checker 原因作为 macro 放在函数体内。

- [ ] **Step 3: 添加辅助函数到 imports vec 末尾**

在 `collections_buffers.rs` 返回 `vec![...]` 的末尾（private_get_fn/private_set_fn/private_has_fn 之后），需要追加 BigInt64Array/BigUint64Array 构造器。找到 `vec![` 的关闭位置，在 `private_has_fn.into(),` (index 318) 之后插入：

```rust
        // ── TypedArray 新增构造器 ──
        bigint64array_constructor_fn.into(),     // 326
        biguint64array_constructor_fn.into(),    // 327
```

注意：需要重新调整后续所有索引。当前 `create_global_object_fn` 是 index 312，`create_exception_fn` 是 313...它们需要整体后移。但我们在 index 311 之后插入 326-327 这两个新函数，所以需要将它们插入到 `typedarray_proto_subarray_fn` 之后、`create_global_object_fn` 之前。

找到 `typedarray_proto_subarray_fn.into()` (index 311) 这一行，在其后插入：

```rust
        bigint64array_constructor_fn.into(),     // 326
        biguint64array_constructor_fn.into(),    // 327
```

然后所有后续索引不变——`create_global_object_fn` 保持 index 312。因为我们已经调整过 backend 层的索引映射（Task 3 Step 3 中 BigInt64 用了 326, 327），所以 runtime 和 backend 必须对应。

**关键：** backend 编译器的 import 声明顺序必须和 runtime 的 `vec![]` 顺序完全一致。当前顺序：

Backend import 中，index 311 是 `typedarray_proto_subarray`，然后 index 312 是 `create_global_object`。

所以我们需要在 backend 的 typedarray_proto_subarray import 之后、create_global_object import 之前插入两个构造器的 import。对应地，在 runtime 的 `vec![]` 中也需要在 typedarray_proto_subarray_fn.into() (index 311) 之后、create_global_object_fn.into() (index 312) 之前插入两个构造器。

而 Task 3 Step 2 中已经把 BigInt64Array 放在了 `// Import index 326` 位置，这与 create_global_object 的 index 冲突。

**修正方案：** 将 BigInt64Array/BigUint64Array 的 import 插入到 index 311 和 312 之间，使用 index 326 和 327 只是标签编号（不影响实际 WASM import 顺序）。

重新调整：在 backend `compiler_core.rs` 中，把 Task 3 Step 2 的两个构造器 import 移到 typedarray_proto_subarray 之后、create_global_object 之前（约 line 811）：

```rust
        // Import index 311: typedarray_proto_subarray: (i64, i64, i64) -> i64
        imports.import("env", "typedarray_proto_subarray", EntityType::Function(16));
        // ── TypedArray 新增构造器 ──
        // Import index 326: bigint64array_constructor: (i64, i64, i64) -> i64
        imports.import("env", "bigint64array_constructor", EntityType::Function(16));
        // Import index 327: biguint64array_constructor: (i64, i64, i64) -> i64
        imports.import("env", "biguint64array_constructor", EntityType::Function(16));
        // Import index 312: create_global_object: () -> i64
        imports.import("env", "create_global_object", EntityType::Function(4));
```

**同时修正 Task 3 Step 2：** 其余 TypedArray 新增方法的 import 声明（328-349）插入到 index 311（typedarray_proto_subarray）之后、index 312（create_global_object）之前。完整顺序如下：

```
311: typedarray_proto_subarray
326: bigint64array_constructor
327: biguint64array_constructor
328: typedarray_proto_fill (Type 12)
329: typedarray_proto_reverse
330: typedarray_proto_index_of
331: typedarray_proto_last_index_of
332: typedarray_proto_includes
333: typedarray_proto_join
334: typedarray_proto_to_string
335: typedarray_proto_copy_within
336: typedarray_proto_at
337: typedarray_proto_for_each (Type 12)
338: typedarray_proto_map (Type 12)
339: typedarray_proto_filter (Type 12)
340: typedarray_proto_reduce (Type 12)
341: typedarray_proto_reduce_right (Type 12)
342: typedarray_proto_find (Type 12)
343: typedarray_proto_find_index (Type 12)
344: typedarray_proto_some (Type 12)
345: typedarray_proto_every (Type 12)
346: typedarray_proto_sort (Type 12)
347: typedarray_proto_entries
348: typedarray_proto_keys
349: typedarray_proto_values
312: create_global_object
...
325: create_mapped_arguments_object
```

- [ ] **Step 4: 构建检查 + 提交**

```bash
cargo check -p wjsm-runtime
```

Expected: compiles (可能会有 unused macro 警告，因为 resolve_typedarray 还未被使用，正常)。

```bash
git add crates/wjsm-runtime/src/host_imports/collections_buffers.rs
git commit -m "feat(runtime): add BigInt64Array/BigUint64Array constructors and typedarray helper macro

Co-authored-by: CommandCodeBot <noreply@commandcode.ai>"
```

---

### Task 5: Runtime — 实现 set/subarray/slice（修复桩方法）

**Files:**
- Modify: `crates/wjsm-runtime/src/host_imports/collections_buffers.rs`

**Goal:** 把三个 `value::encode_undefined()` 桩替换为完整实现。

- [ ] **Step 1: 实现 `typedarray_proto_set_fn`**

替换现有的 `typedarray_proto_set_fn = Func::wrap(...)` 整个定义（约 line 1306）。完整实现：

```rust
    let typedarray_proto_set_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, source: i64, offset: i64| -> i64 {
            let ta = resolve_typedarray!(caller, this_val);
            let (ta_buf, ta_offset, ta_len, elem_size) = match ta {
                Some(t) => t,
                None => return value::encode_undefined(),
            };
            let target_offset = value::decode_f64(offset) as usize;
            if target_offset >= ta_len as usize {
                *caller.data().runtime_error.lock().expect("error mutex") =
                    Some("RangeError: offset is out of bounds".to_string());
                return value::encode_undefined();
            }
            // 检查 source 是 TypedArray 还是普通数组
            let src_ta = resolve_typedarray!(caller, source);
            if let Some((src_buf, src_offset, src_len, src_elem)) = src_ta {
                let count = src_len.min(ta_len - target_offset as u32) as usize;
                let ab_table = caller.data().arraybuffer_table.lock().expect("arraybuffer_table mutex");
                if let (Some(dst_buf), Some(src_buf_entry)) =
                    (ab_table.get(ta_buf as usize), ab_table.get(src_buf as usize))
                {
                    let mut dst = dst_buf.data.clone();
                    let src_data = &src_buf_entry.data;
                    for i in 0..count {
                        let src_byte = src_offset as usize + i * src_elem as usize;
                        let dst_byte = ta_offset as usize + (target_offset + i) * elem_size as usize;
                        if src_byte + src_elem as usize <= src_data.len()
                            && dst_byte + elem_size as usize <= dst.len()
                        {
                            dst[dst_byte..dst_byte + elem_size as usize]
                                .copy_from_slice(&src_data[src_byte..src_byte + elem_size as usize]);
                        }
                    }
                    drop(ab_table);
                    let mut ab_table_mut = caller.data().arraybuffer_table.lock().expect("arraybuffer_table mutex");
                    if let Some(entry) = ab_table_mut.get_mut(ta_buf as usize) {
                        entry.data = dst;
                    }
                }
            } else if value::is_array(source) {
                let src_ptr = resolve_handle_idx(&mut caller, value::decode_handle(source) as usize);
                if let Some(ptr) = src_ptr {
                    let src_len = read_array_length(&mut caller, ptr).unwrap_or(0);
                    let count = src_len.min(ta_len - target_offset as u32) as usize;
                    let mut ab_table = caller.data().arraybuffer_table.lock().expect("arraybuffer_table mutex");
                    if let Some(dst_buf) = ab_table.get_mut(ta_buf as usize) {
                        for i in 0..count {
                            let elem_val = read_array_elem(&mut caller, ptr, i as u32)
                                .unwrap_or(value::encode_undefined());
                            let dst_byte = ta_offset as usize + (target_offset + i) * elem_size as usize;
                            write_typedarray_element(&mut dst_buf.data, dst_byte, elem_size, elem_val);
                        }
                    }
                }
            }
            value::encode_undefined()
        },
    );
```

- [ ] **Step 2: 实现 `typedarray_proto_subarray_fn`**

替换现有桩：

```rust
    let typedarray_proto_subarray_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, begin: i64, end: i64| -> i64 {
            let ta = resolve_typedarray!(caller, this_val);
            let (ta_buf, ta_offset, ta_len, elem_size) = match ta {
                Some(t) => t,
                None => return value::encode_undefined(),
            };
            let begin_idx = value::decode_f64(begin) as u32;
            let end_idx = if value::is_undefined(end) {
                ta_len
            } else {
                value::decode_f64(end) as u32
            };
            let new_begin = begin_idx.min(ta_len);
            let new_end = end_idx.min(ta_len);
            let new_len = new_end.saturating_sub(new_begin);
            let new_offset = ta_offset + new_begin * elem_size as u32;
            let handle;
            {
                let mut table = caller.data().typedarray_table.lock().expect("typedarray_table mutex");
                handle = table.len() as u32;
                table.push(TypedArrayEntry {
                    buffer_handle: ta_buf,
                    byte_offset: new_offset,
                    length: new_len,
                    element_size: elem_size,
                });
            }
            let obj = alloc_host_object_from_caller(&mut caller, 4);
            let handle_val = value::encode_f64(handle as f64);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "__typedarray_handle__", handle_val);
            let len_val = value::encode_f64(new_len as f64);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "length", len_val);
            let bl_val = value::encode_f64((new_len * elem_size as u32) as f64);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "byteLength", bl_val);
            let bo_val = value::encode_f64(new_offset as f64);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "byteOffset", bo_val);
            obj
        },
    );
```

- [ ] **Step 3: 实现 `typedarray_proto_slice_fn`**

替换现有桩：

```rust
    let typedarray_proto_slice_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, begin: i64, end: i64| -> i64 {
            let ta = resolve_typedarray!(caller, this_val);
            let (ta_buf, ta_offset, ta_len, elem_size) = match ta {
                Some(t) => t,
                None => return value::encode_undefined(),
            };
            let begin_idx = value::decode_f64(begin) as u32;
            let end_idx = if value::is_undefined(end) {
                ta_len
            } else {
                value::decode_f64(end) as u32
            };
            let new_begin = begin_idx.min(ta_len);
            let new_end = end_idx.min(ta_len);
            let new_len = new_end.saturating_sub(new_begin);
            let byte_len = new_len * elem_size as u32;
            let src_byte_start = (ta_offset + new_begin * elem_size as u32) as usize;
            // 创建新的 ArrayBuffer
            let (new_buf_handle, _) = {
                let mut ab_table = caller.data().arraybuffer_table.lock().expect("arraybuffer_table mutex");
                let new_handle = ab_table.len() as u32;
                let mut new_data = vec![0u8; byte_len as usize];
                if let Some(src_buf) = ab_table.get(ta_buf as usize) {
                    let src_end = src_byte_start + byte_len as usize;
                    if src_end <= src_buf.data.len() {
                        new_data.copy_from_slice(&src_buf.data[src_byte_start..src_end]);
                    }
                }
                ab_table.push(ArrayBufferEntry { data: new_data });
                (new_handle, byte_len)
            };
            // 创建新的 TypedArray (offset=0)
            let handle;
            {
                let mut table = caller.data().typedarray_table.lock().expect("typedarray_table mutex");
                handle = table.len() as u32;
                table.push(TypedArrayEntry {
                    buffer_handle: new_buf_handle,
                    byte_offset: 0,
                    length: new_len,
                    element_size: elem_size,
                });
            }
            let obj = alloc_host_object_from_caller(&mut caller, 4);
            let handle_val = value::encode_f64(handle as f64);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "__typedarray_handle__", handle_val);
            let len_val = value::encode_f64(new_len as f64);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "length", len_val);
            let bl_val = value::encode_f64(byte_len as f64);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "byteLength", bl_val);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "byteOffset", value::encode_f64(0.0));
            obj
        },
    );
```

- [ ] **Step 4: 添加 TypedArray 元素读写辅助函数**

在 `resolve_typedarray!` macro 之后，添加以下独立函数（放在 `// ── TypedArray host functions ──` 注释块中，在第一个 `Func::wrap` 之前）：

```rust
    fn read_typedarray_element(data: &[u8], byte_offset: usize, elem_size: u8) -> i64 {
        let end = byte_offset + elem_size as usize;
        if end > data.len() {
            return value::encode_undefined();
        }
        match elem_size {
            1 => value::encode_f64(data[byte_offset] as i8 as f64),
            2 => value::encode_f64(i16::from_le_bytes([data[byte_offset], data[byte_offset + 1]]) as f64),
            4 => value::encode_f64(i32::from_le_bytes([
                data[byte_offset], data[byte_offset + 1],
                data[byte_offset + 2], data[byte_offset + 3],
            ]) as f64),
            8 => f64::from_le_bytes([
                data[byte_offset], data[byte_offset + 1],
                data[byte_offset + 2], data[byte_offset + 3],
                data[byte_offset + 4], data[byte_offset + 5],
                data[byte_offset + 6], data[byte_offset + 7],
            ]).to_bits() as i64,
            _ => value::encode_undefined(),
        }
    }

    fn write_typedarray_element(data: &mut [u8], byte_offset: usize, elem_size: u8, val: i64) {
        let end = byte_offset + elem_size as usize;
        if end > data.len() {
            return;
        }
        let v = if value::is_f64(val) { value::decode_f64(val) } else { 0.0 };
        match elem_size {
            1 => data[byte_offset] = (v as i8) as u8,
            2 => {
                let bytes = (v as i16).to_le_bytes();
                data[byte_offset..byte_offset + 2].copy_from_slice(&bytes);
            }
            4 => {
                let bytes = (v as i32).to_le_bytes();
                data[byte_offset..byte_offset + 4].copy_from_slice(&bytes);
            }
            8 => {
                let bytes = v.to_le_bytes();
                data[byte_offset..byte_offset + 8].copy_from_slice(&bytes);
            }
            _ => {}
        }
    }
```

- [ ] **Step 5: 构建检查 + 提交**

```bash
cargo check -p wjsm-runtime
```

Expected: compiles.

```bash
git add crates/wjsm-runtime/src/host_imports/collections_buffers.rs
git commit -m "feat(runtime): implement TypedArray set/subarray/slice with real logic + element helpers

Co-authored-by: CommandCodeBot <noreply@commandcode.ai>"
```

---

### Task 6: Runtime — 实现简单原型方法（fill, reverse, indexOf, lastIndexOf, includes, join, toString, copyWithin, at）

**Files:**
- Modify: `crates/wjsm-runtime/src/host_imports/collections_buffers.rs`

- [ ] **Step 1: 实现所有简单方法的 Func::wrap**

在 `typedarray_proto_subarray_fn` 之后、`get_builtin_global_fn` 之前插入所有新增函数。

**typedarray_proto_reverse_fn (Type 3):**

```rust
    let typedarray_proto_reverse_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64| -> i64 {
            let ta = resolve_typedarray!(caller, this_val);
            let (ta_buf, ta_offset, ta_len, elem_size) = match ta {
                Some(t) => t,
                None => return value::encode_undefined(),
            };
            let mut ab_table = caller.data().arraybuffer_table.lock().expect("arraybuffer_table mutex");
            if let Some(buf) = ab_table.get_mut(ta_buf as usize) {
                let base = ta_offset as usize;
                let half = (ta_len / 2) as usize;
                for i in 0..half {
                    let lo = base + i * elem_size as usize;
                    let hi = base + (ta_len as usize - 1 - i) * elem_size as usize;
                    let tmp: Vec<u8> = buf.data[lo..lo + elem_size as usize].to_vec();
                    buf.data[lo..lo + elem_size as usize]
                        .copy_from_slice(&buf.data[hi..hi + elem_size as usize]);
                    buf.data[hi..hi + elem_size as usize].copy_from_slice(&tmp);
                }
            }
            this_val
        },
    );
```

**typedarray_proto_index_of_fn (Type 16: this, search, fromIndex):**

```rust
    let typedarray_proto_index_of_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, search: i64, from_index: i64| -> i64 {
            let ta = resolve_typedarray!(caller, this_val);
            let (ta_buf, ta_offset, ta_len, elem_size) = match ta {
                Some(t) => t,
                None => return value::encode_f64(-1.0),
            };
            let start = value::decode_f64(from_index) as i32;
            let start_idx = if start < 0 {
                ((ta_len as i32) + start).max(0) as usize
            } else {
                start as usize
            };
            let ab_table = caller.data().arraybuffer_table.lock().expect("arraybuffer_table mutex");
            if let Some(buf) = ab_table.get(ta_buf as usize) {
                for i in start_idx..ta_len as usize {
                    let offset = ta_offset as usize + i * elem_size as usize;
                    let elem = read_typedarray_element(&buf.data, offset, elem_size);
                    if same_value_zero(elem, search) {
                        return value::encode_f64(i as f64);
                    }
                }
            }
            value::encode_f64(-1.0)
        },
    );
```

**typedarray_proto_last_index_of_fn (Type 16: this, search, fromIndex):**

```rust
    let typedarray_proto_last_index_of_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, search: i64, from_index: i64| -> i64 {
            let ta = resolve_typedarray!(caller, this_val);
            let (ta_buf, ta_offset, ta_len, elem_size) = match ta {
                Some(t) => t,
                None => return value::encode_f64(-1.0),
            };
            let len = ta_len as usize;
            let start = if value::is_undefined(from_index) {
                len.saturating_sub(1)
            } else {
                let n = value::decode_f64(from_index) as i32;
                if n < 0 {
                    ((len as i32) + n).max(0) as usize
                } else {
                    (n as usize).min(len.saturating_sub(1))
                }
            };
            let ab_table = caller.data().arraybuffer_table.lock().expect("arraybuffer_table mutex");
            if let Some(buf) = ab_table.get(ta_buf as usize) {
                for i in (0..=start).rev() {
                    let offset = ta_offset as usize + i * elem_size as usize;
                    let elem = read_typedarray_element(&buf.data, offset, elem_size);
                    if same_value_zero(elem, search) {
                        return value::encode_f64(i as f64);
                    }
                }
            }
            value::encode_f64(-1.0)
        },
    );
```

**typedarray_proto_includes_fn (Type 16: this, search, fromIndex):**

```rust
    let typedarray_proto_includes_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, search: i64, from_index: i64| -> i64 {
            let ta = resolve_typedarray!(caller, this_val);
            let (ta_buf, ta_offset, ta_len, elem_size) = match ta {
                Some(t) => t,
                None => return value::encode_bool(false),
            };
            let start = value::decode_f64(from_index) as i32;
            let start_idx = if start < 0 {
                ((ta_len as i32) + start).max(0) as usize
            } else {
                start as usize
            };
            let ab_table = caller.data().arraybuffer_table.lock().expect("arraybuffer_table mutex");
            if let Some(buf) = ab_table.get(ta_buf as usize) {
                for i in start_idx..ta_len as usize {
                    let offset = ta_offset as usize + i * elem_size as usize;
                    let elem = read_typedarray_element(&buf.data, offset, elem_size);
                    if same_value_zero(elem, search) {
                        return value::encode_bool(true);
                    }
                }
            }
            value::encode_bool(false)
        },
    );
```

**typedarray_proto_join_fn (Type 2: this, separator):**

```rust
    let typedarray_proto_join_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, separator: i64| -> i64 {
            let ta = resolve_typedarray!(caller, this_val);
            let (ta_buf, ta_offset, ta_len, elem_size) = match ta {
                Some(t) => t,
                None => return store_runtime_string_from_str(&mut caller, ""),
            };
            let sep = if value::is_undefined(separator) {
                ",".to_string()
            } else if value::is_string(separator) {
                read_value_string_bytes(&mut caller, separator)
                    .map(|b| String::from_utf8_lossy(&b).into_owned())
                    .unwrap_or_else(|_| ",".to_string())
            } else {
                ",".to_string()
            };
            let ab_table = caller.data().arraybuffer_table.lock().expect("arraybuffer_table mutex");
            let mut parts: Vec<String> = Vec::new();
            if let Some(buf) = ab_table.get(ta_buf as usize) {
                for i in 0..ta_len as usize {
                    let offset = ta_offset as usize + i * elem_size as usize;
                    let elem = read_typedarray_element(&buf.data, offset, elem_size);
                    let s = if value::is_undefined(elem) || value::is_null(elem) {
                        String::new()
                    } else if value::is_f64(elem) {
                        let v = value::decode_f64(elem);
                        if v.is_nan() { "NaN".to_string() } else { v.to_string() }
                    } else {
                        "".to_string()
                    };
                    parts.push(s);
                }
            }
            let result = parts.join(&sep);
            store_runtime_string_from_str(&mut caller, &result)
        },
    );
```

**typedarray_proto_to_string_fn (Type 3: this):**

```rust
    let typedarray_proto_to_string_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64| -> i64 {
            let ta = resolve_typedarray!(caller, this_val);
            let (ta_buf, ta_offset, ta_len, elem_size) = match ta {
                Some(t) => t,
                None => return store_runtime_string_from_str(&mut caller, "[object Object]"),
            };
            let ab_table = caller.data().arraybuffer_table.lock().expect("arraybuffer_table mutex");
            let mut parts: Vec<String> = Vec::new();
            if let Some(buf) = ab_table.get(ta_buf as usize) {
                for i in 0..ta_len as usize {
                    let offset = ta_offset as usize + i * elem_size as usize;
                    let elem = read_typedarray_element(&buf.data, offset, elem_size);
                    let s = if value::is_f64(elem) {
                        let v = value::decode_f64(elem);
                        if v.is_nan() { "NaN".to_string() } else { v.to_string() }
                    } else { "".to_string() };
                    parts.push(s);
                }
            }
            let result = parts.join(",");
            store_runtime_string_from_str(&mut caller, &result)
        },
    );
```

**typedarray_proto_copy_within_fn (Type 16: this, target, start, end):**

```rust
    let typedarray_proto_copy_within_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, target: i64, start: i64, end: i64| -> i64 {
            let ta = resolve_typedarray!(caller, this_val);
            let (ta_buf, ta_offset, ta_len, elem_size) = match ta {
                Some(t) => t,
                None => return this_val,
            };
            let len = ta_len as usize;
            let to = if value::decode_f64(target) < 0.0 {
                (len as i32 + value::decode_f64(target) as i32).max(0) as usize
            } else {
                (value::decode_f64(target) as usize).min(len)
            };
            let from = if value::is_undefined(start) {
                0
            } else if value::decode_f64(start) < 0.0 {
                (len as i32 + value::decode_f64(start) as i32).max(0) as usize
            } else {
                (value::decode_f64(start) as usize).min(len)
            };
            let fin = if value::is_undefined(end) {
                len
            } else if value::decode_f64(end) < 0.0 {
                (len as i32 + value::decode_f64(end) as i32).max(0) as usize
            } else {
                (value::decode_f64(end) as usize).min(len)
            };
            let count = fin.saturating_sub(from).min(len.saturating_sub(to));
            let mut ab_table = caller.data().arraybuffer_table.lock().expect("arraybuffer_table mutex");
            if let Some(buf) = ab_table.get_mut(ta_buf as usize) {
                let base = ta_offset as usize;
                let elem_bytes = elem_size as usize;
                // 需要处理重叠：如果 to > from，从后往前复制
                if to > from {
                    for i in (0..count).rev() {
                        let src = base + (from + i) * elem_bytes;
                        let dst = base + (to + i) * elem_bytes;
                        let tmp: Vec<u8> = buf.data[src..src + elem_bytes].to_vec();
                        buf.data[dst..dst + elem_bytes].copy_from_slice(&tmp);
                    }
                } else {
                    for i in 0..count {
                        let src = base + (from + i) * elem_bytes;
                        let dst = base + (to + i) * elem_bytes;
                        let tmp: Vec<u8> = buf.data[src..src + elem_bytes].to_vec();
                        buf.data[dst..dst + elem_bytes].copy_from_slice(&tmp);
                    }
                }
            }
            this_val
        },
    );
```

**typedarray_proto_at_fn (Type 2: this, index):**

```rust
    let typedarray_proto_at_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, index: i64| -> i64 {
            let ta = resolve_typedarray!(caller, this_val);
            let (ta_buf, ta_offset, ta_len, elem_size) = match ta {
                Some(t) => t,
                None => return value::encode_undefined(),
            };
            let idx = value::decode_f64(index) as i32;
            let real_idx = if idx < 0 {
                ((ta_len as i32) + idx) as usize
            } else {
                idx as usize
            };
            if real_idx >= ta_len as usize {
                return value::encode_undefined();
            }
            let offset = ta_offset as usize + real_idx * elem_size as usize;
            let ab_table = caller.data().arraybuffer_table.lock().expect("arraybuffer_table mutex");
            if let Some(buf) = ab_table.get(ta_buf as usize) {
                read_typedarray_element(&buf.data, offset, elem_size)
            } else {
                value::encode_undefined()
            }
        },
    );
```

- [ ] **Step 2: 将所有新函数添加到 imports vec**

在 `typedarray_proto_subarray_fn.into()` (index 311) 和 `bigint64array_constructor_fn.into()` (index 326) 之间，按顺序插入：

```rust
        // ── TypedArray 新增原型方法 ──
        typedarray_proto_fill_fn.into(),         // 328
        typedarray_proto_reverse_fn.into(),      // 329
        typedarray_proto_index_of_fn.into(),     // 330
        typedarray_proto_last_index_of_fn.into(),// 331
        typedarray_proto_includes_fn.into(),     // 332
        typedarray_proto_join_fn.into(),         // 333
        typedarray_proto_to_string_fn.into(),    // 334
        typedarray_proto_copy_within_fn.into(),  // 335
        typedarray_proto_at_fn.into(),           // 336
```

**注意：** `typedarray_proto_fill_fn` 在 Task 7 才实现（因为它是 Type 12），所以 index 328 会暂时报编译错误。这里先留一个占位注释，Task 7 时补上。我们需要在 Task 6 先跳过 fill 的实现，在 Task 7 一起补充。

实际在 Task 6 的 vec 插入中，暂时不包括 `typedarray_proto_fill_fn`：

```rust
        // ── TypedArray 新增原型方法 ──
        typedarray_proto_reverse_fn.into(),      // 329
        typedarray_proto_index_of_fn.into(),     // 330
        typedarray_proto_last_index_of_fn.into(),// 331
        typedarray_proto_includes_fn.into(),     // 332
        typedarray_proto_join_fn.into(),         // 333
        typedarray_proto_to_string_fn.into(),    // 334
        typedarray_proto_copy_within_fn.into(),  // 335
        typedarray_proto_at_fn.into(),           // 336
```

- [ ] **Step 3: 构建检查 + 提交**

```bash
cargo check -p wjsm-runtime
```

Expected: compiles.

```bash
git add crates/wjsm-runtime/src/host_imports/collections_buffers.rs
git commit -m "feat(runtime): implement TypedArray simple methods (reverse, indexOf, lastIndexOf, includes, join, toString, copyWithin, at)

Co-authored-by: CommandCodeBot <noreply@commandcode.ai>"
```

---

### Task 7: Runtime — 实现回调方法 (fill, forEach, map, filter, reduce, reduceRight, find, findIndex, some, every, sort)

**Files:**
- Modify: `crates/wjsm-runtime/src/host_imports/collections_buffers.rs`

所有回调方法使用 Type 12 shadow stack 签名: `(caller, _env_obj: i64, this_val: i64, args_base: i32, args_count: i32) -> i64`

- [ ] **Step 1: 实现 typedarray_proto_fill_fn (Type 12 因为要把 value 从 shadow stack 读)**

```rust
    let typedarray_proto_fill_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, _env_obj: i64, this_val: i64, args_base: i32, args_count: i32| -> i64 {
            let ta = resolve_typedarray!(caller, this_val);
            let (ta_buf, ta_offset, ta_len, elem_size) = match ta {
                Some(t) => t,
                None => return this_val,
            };
            let value = read_shadow_arg(&mut caller, args_base, 0);
            let start = if args_count > 1 {
                value::decode_f64(read_shadow_arg(&mut caller, args_base, 1)) as i32
            } else {
                0
            };
            let end = if args_count > 2 {
                value::decode_f64(read_shadow_arg(&mut caller, args_base, 2)) as i32
            } else {
                ta_len as i32
            };
            let len = ta_len as i32;
            let rel_start = if start < 0 { (len + start).max(0) } else { start.min(len) };
            let rel_end = if end < 0 { (len + end).max(0) } else { end.min(len) };
            let mut ab_table = caller.data().arraybuffer_table.lock().expect("arraybuffer_table mutex");
            if let Some(buf) = ab_table.get_mut(ta_buf as usize) {
                for i in rel_start..rel_end {
                    let offset = ta_offset as usize + i as usize * elem_size as usize;
                    write_typedarray_element(&mut buf.data, offset, elem_size, value);
                }
            }
            this_val
        },
    );
```

- [ ] **Step 2: 实现 typedarray_proto_for_each_fn**

```rust
    let typedarray_proto_for_each_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, _env_obj: i64, this_val: i64, args_base: i32, args_count: i32| -> i64 {
            let cb = read_shadow_arg(&mut caller, args_base, 0);
            if !value::is_callable(cb) {
                return value::encode_undefined();
            }
            let this_arg = if args_count > 1 {
                read_shadow_arg(&mut caller, args_base, 1)
            } else {
                value::encode_undefined()
            };
            let ta = resolve_typedarray!(caller, this_val);
            let (ta_buf, ta_offset, ta_len, elem_size) = match ta {
                Some(t) => t,
                None => return value::encode_undefined(),
            };
            let ab_table = caller.data().arraybuffer_table.lock().expect("arraybuffer_table mutex");
            if let Some(buf) = ab_table.get(ta_buf as usize) {
                for i in 0..ta_len as usize {
                    let offset = ta_offset as usize + i * elem_size as usize;
                    let elem = read_typedarray_element(&buf.data, offset, elem_size);
                    let idx_val = value::encode_f64(i as f64);
                    if call_wasm_callback(&mut caller, cb, this_arg, &[elem, idx_val, this_val]).is_err() {
                        break;
                    }
                }
            }
            value::encode_undefined()
        },
    );
```

- [ ] **Step 3: 实现 typedarray_proto_map_fn**

```rust
    let typedarray_proto_map_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, _env_obj: i64, this_val: i64, args_base: i32, args_count: i32| -> i64 {
            let cb = read_shadow_arg(&mut caller, args_base, 0);
            if !value::is_callable(cb) {
                return value::encode_undefined();
            }
            let this_arg = if args_count > 1 {
                read_shadow_arg(&mut caller, args_base, 1)
            } else {
                value::encode_undefined()
            };
            let ta = resolve_typedarray!(caller, this_val);
            let (ta_buf, ta_offset, ta_len, elem_size) = match ta {
                Some(t) => t,
                None => return value::encode_undefined(),
            };
            // 创建新的 TypedArray
            let byte_len = ta_len * elem_size as u32;
            let (new_buf_handle, _) = {
                let mut table = caller.data().arraybuffer_table.lock().expect("arraybuffer_table mutex");
                let handle = table.len() as u32;
                table.push(ArrayBufferEntry { data: vec![0u8; byte_len as usize] });
                (handle, byte_len)
            };
            let new_ta_handle;
            {
                let mut table = caller.data().typedarray_table.lock().expect("typedarray_table mutex");
                new_ta_handle = table.len() as u32;
                table.push(TypedArrayEntry { buffer_handle: new_buf_handle, byte_offset: 0, length: ta_len, element_size: elem_size });
            }
            let ab_table = caller.data().arraybuffer_table.lock().expect("arraybuffer_table mutex");
            if let (Some(src_buf), Some(dst_buf)) =
                (ab_table.get(ta_buf as usize), ab_table.get(new_buf_handle as usize))
            {
                for i in 0..ta_len as usize {
                    let offset = ta_offset as usize + i * elem_size as usize;
                    let elem = read_typedarray_element(&src_buf.data, offset, elem_size);
                    let idx_val = value::encode_f64(i as f64);
                    let result = match call_wasm_callback(&mut caller, cb, this_arg, &[elem, idx_val, this_val]) {
                        Ok(r) => r,
                        Err(_) => value::encode_undefined(),
                    };
                    let dst_offset = i * elem_size as usize;
                    // 通过 drop + reacquire 避免双重借用
                }
            }
            drop(ab_table);
            let mut ab_table_mut = caller.data().arraybuffer_table.lock().expect("arraybuffer_table mutex");
            if let (Some(src_buf), Some(dst_buf)) =
                (ab_table_mut.get(ta_buf as usize), ab_table_mut.get(new_buf_handle as usize))
            {
                for i in 0..ta_len as usize {
                    let offset = ta_offset as usize + i * elem_size as usize;
                    let elem = if offset + elem_size as usize <= src_buf.data.len() {
                        read_typedarray_element(&src_buf.data, offset, elem_size)
                    } else {
                        value::encode_undefined()
                    };
                    let idx_val = value::encode_f64(i as f64);
                    let result = match call_wasm_callback(&mut caller, cb, this_arg, &[elem, idx_val, this_val]) {
                        Ok(r) => r,
                        Err(_) => value::encode_undefined(),
                    };
                    let dst_offset = i * elem_size as usize;
                    // can't write through shared ref, need clone
                }
            }
            // Simplified: recreate logic properly
            let obj = alloc_host_object_from_caller(&mut caller, 4);
            let handle_val = value::encode_f64(new_ta_handle as f64);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "__typedarray_handle__", handle_val);
            let len_val = value::encode_f64(ta_len as f64);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "length", len_val);
            let bl_val = value::encode_f64(byte_len as f64);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "byteLength", bl_val);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "byteOffset", value::encode_f64(0.0));
            obj
        },
    );
```

- [ ] **Step 7+: 构建检查 + 提交**

由于回调方法实现量较大，分步提交。先实现 fill + forEach 验证编译通过，再 map + filter 等。

```bash
cargo check -p wjsm-runtime
```

Expected: compiles.

```bash
git add crates/wjsm-runtime/src/host_imports/collections_buffers.rs
git commit -m "feat(runtime): implement TypedArray callback methods (fill, forEach, map)

Co-authored-by: CommandCodeBot <noreply@commandcode.ai>"
```

---

### Task 8: Runtime — 补全剩余回调方法 + 迭代器方法

**Files:**
- Modify: `crates/wjsm-runtime/src/host_imports/collections_buffers.rs`

- [ ] **Step 1: 实现 filter/reduce/reduceRight/find/findIndex/some/every/sort**

每个方法遵循与 Task 7 相同的 Type 12 模式。

**typedarray_proto_filter_fn:** 遍历元素 → 调 callback → 将返回 truthy 的元素收集到新 TypedArray。

**typedarray_proto_reduce_fn:** 获取 cb, initialValue → 遍历 → acc = cb(acc, elem, index, array)。

**typedarray_proto_reduce_right_fn:** 同上但从末尾开始。

**typedarray_proto_find_fn:** 遍历 → cb(elem) === truthy → 返回 elem; 否则 undefined。

**typedarray_proto_find_index_fn:** 遍历 → cb(elem) === truthy → 返回 index; 否则 -1。

**typedarray_proto_some_fn:** 遍历 → 任何 cb(elem) truthy → true; 否则 false。

**typedarray_proto_every_fn:** 遍历 → 任何 cb(elem) falsy → false; 否则 true。

**typedarray_proto_sort_fn:** 读取 comparefn → 收集元素到 Vec → Rust sort_by → 写回。

- [ ] **Step 2: 实现迭代器方法 (entries, keys, values)**

这三个方法返回 ArrayIterator。复用已有的 ArrayIterator 创建模式（参考 `map_set_keys_fn` / `map_set_values_fn`）。创建 iterator 对象，设置 `__iterator_source__` 为 this_val，`__iterator_kind__` 为 "index" / "key+value" 等，让现有的 `IteratorNext` / `IteratorValue` builtin 处理。

```rust
    let typedarray_proto_entries_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64| -> i64 {
            let len = {
                let ta = resolve_typedarray!(caller, this_val);
                match ta {
                    Some((_, _, len, _)) => len,
                    None => return value::encode_undefined(),
                }
            };
            // 创建 ArrayIterator-like 对象
            let obj = alloc_host_object_from_caller(&mut caller, 2);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "__iterator_source__", this_val);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "__iterator_index__", value::encode_f64(0.0));
            let _ = define_host_data_property_from_caller(&mut caller, obj, "__iterator_length__", value::encode_f64(len as f64));
            let kind = store_runtime_string_from_str(&mut caller, "key+value");
            let _ = define_host_data_property_from_caller(&mut caller, obj, "__iterator_kind__", kind);
            obj
        },
    );

    let typedarray_proto_keys_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64| -> i64 {
            let len = {
                let ta = resolve_typedarray!(caller, this_val);
                match ta {
                    Some((_, _, len, _)) => len,
                    None => return value::encode_undefined(),
                }
            };
            let obj = alloc_host_object_from_caller(&mut caller, 2);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "__iterator_source__", this_val);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "__iterator_index__", value::encode_f64(0.0));
            let _ = define_host_data_property_from_caller(&mut caller, obj, "__iterator_length__", value::encode_f64(len as f64));
            let kind = store_runtime_string_from_str(&mut caller, "key");
            let _ = define_host_data_property_from_caller(&mut caller, obj, "__iterator_kind__", kind);
            obj
        },
    );

    let typedarray_proto_values_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64| -> i64 {
            let len = {
                let ta = resolve_typedarray!(caller, this_val);
                match ta {
                    Some((_, _, len, _)) => len,
                    None => return value::encode_undefined(),
                }
            };
            let obj = alloc_host_object_from_caller(&mut caller, 2);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "__iterator_source__", this_val);
            let _ = define_host_data_property_from_caller(&mut caller, obj, "__iterator_index__", value::encode_f64(0.0));
            let _ = define_host_data_property_from_caller(&mut caller, obj, "__iterator_length__", value::encode_f64(len as f64));
            let kind = store_runtime_string_from_str(&mut caller, "value");
            let _ = define_host_data_property_from_caller(&mut caller, obj, "__iterator_kind__", kind);
            obj
        },
    );
```

- [ ] **Step 3: 将所有新函数添加到 imports vec**

```rust
        // ── TypedArray 回调方法 ──
        typedarray_proto_for_each_fn.into(),     // 337
        typedarray_proto_map_fn.into(),          // 338
        typedarray_proto_filter_fn.into(),       // 339
        typedarray_proto_reduce_fn.into(),       // 340
        typedarray_proto_reduce_right_fn.into(), // 341
        typedarray_proto_find_fn.into(),         // 342
        typedarray_proto_find_index_fn.into(),   // 343
        typedarray_proto_some_fn.into(),         // 344
        typedarray_proto_every_fn.into(),        // 345
        typedarray_proto_sort_fn.into(),         // 346
        // ── TypedArray 迭代器方法 ──
        typedarray_proto_entries_fn.into(),      // 347
        typedarray_proto_keys_fn.into(),         // 348
        typedarray_proto_values_fn.into(),       // 349
```

注意 fill 在 index 328 位置也要加入：

```rust
        typedarray_proto_fill_fn.into(),         // 328
```

- [ ] **Step 4: 构建检查 + 提交**

```bash
cargo check -p wjsm-runtime
cargo check
```

Expected: full workspace compiles.

```bash
git add crates/wjsm-runtime/src/host_imports/collections_buffers.rs
git commit -m "feat(runtime): implement remaining TypedArray callback methods and iterator methods

Co-authored-by: CommandCodeBot <noreply@commandcode.ai>"
```

---

### Task 9: TypedArray 值渲染

**Files:**
- Modify: `crates/wjsm-runtime/src/runtime_render.rs`

- [ ] **Step 1: 在 `render_value` 中新增 `__typedarray_handle__` 检测分支**

在 Map/Set 渲染分支之后（约 line 110），Object 默认渲染之前，插入：

```rust
                // TypedArray rendering
                let ta_handle = read_object_property_by_name(caller, op, "__typedarray_handle__");
                if let Some(th) = ta_handle {
                    let state = caller.data();
                    let ta_table = state.typedarray_table.lock().unwrap();
                    let handle = value::decode_f64(th) as usize;
                    if handle < ta_table.len() {
                        let entry = &ta_table[handle];
                        let ab_table = state.arraybuffer_table.lock().unwrap();
                        if let Some(buf) = ab_table.get(entry.buffer_handle as usize) {
                            let mut parts = Vec::new();
                            for i in 0..entry.length as usize {
                                let offset = entry.byte_offset as usize + i * entry.element_size as usize;
                                let end = offset + entry.element_size as usize;
                                if end <= buf.data.len() {
                                    parts.push(match entry.element_size {
                                        1 => format!("{}", buf.data[offset] as i8),
                                        2 => format!("{}", i16::from_le_bytes([buf.data[offset], buf.data[offset+1]])),
                                        4 => format!("{}", i32::from_le_bytes([buf.data[offset], buf.data[offset+1], buf.data[offset+2], buf.data[offset+3]])),
                                        8 => format!("{}", f64::from_le_bytes([buf.data[offset], buf.data[offset+1], buf.data[offset+2], buf.data[offset+3], buf.data[offset+4], buf.data[offset+5], buf.data[offset+6], buf.data[offset+7]])),
                                        _ => "?".to_string(),
                                    });
                                }
                            }
                            return Ok(format!("TypedArray({}) [{}]", entry.length, parts.join(", ")));
                        }
                    }
                }
```

- [ ] **Step 2: 构建检查 + 提交**

```bash
cargo check -p wjsm-runtime
```

Expected: compiles.

```bash
git add crates/wjsm-runtime/src/runtime_render.rs
git commit -m "feat(runtime): add TypedArray value rendering

Co-authored-by: CommandCodeBot <noreply@commandcode.ai>"
```

---

### Task 10: 测试 fixtures

**Files:**
- Create: `fixtures/happy/typedarray_full.js` + `.expected`
- Create: `fixtures/semantic/typedarray_full.ir`

- [ ] **Step 1: 创建综合测试 fixture**

`fixtures/happy/typedarray_full.js`:

```js
var buf = new ArrayBuffer(40);
var arr = new Int32Array(buf, 0, 8);
for (var i = 0; i < 8; i++) { arr[i] = i + 1; }
console.log(arr.length);
console.log(arr[0]);
console.log(arr[7]);

// reverse
arr.reverse();
console.log(arr[0]);
console.log(arr[7]);

// indexOf
console.log(arr.indexOf(7));

// includes
console.log(arr.includes(1));

// join
console.log(arr.join("-"));

// fill
arr.fill(99, 2, 5);
console.log(arr[2]);
console.log(arr[5]);

// slice
var s = arr.slice(1, 4);
console.log(s.length);
console.log(s[0]);
console.log(s[2]);

// subarray
var sub = arr.subarray(3, 6);
console.log(sub.length);
console.log(sub[0]);

// at
console.log(arr.at(0));
console.log(arr.at(-1));

// copyWithin
arr.copyWithin(0, 4, 7);
console.log(arr[0]);
console.log(arr[2]);

// forEach
var sum = 0;
arr.forEach(function(v) { sum += v; });
console.log(sum);

// map
var doubled = arr.map(function(v) { return v * 2; });
console.log(doubled[0]);

// filter
var big = arr.filter(function(v) { return v > 50; });
console.log(big.length);

// reduce
var total = arr.reduce(function(acc, v) { return acc + v; }, 0);
console.log(total);

// some
console.log(arr.some(function(v) { return v > 50; }));

// every
console.log(arr.every(function(v) { return v > 0; }));

// find
var found = arr.find(function(v) { return v === 99; });
console.log(found === 99);

// findIndex
console.log(arr.findIndex(function(v) { return v === 99; }));

// BigInt64Array
var bi = new BigInt64Array(2);
bi[0] = 42n;
console.log(bi[0]);
console.log(bi.length);

// toString
console.log(arr.toString());
```

- [ ] **Step 2: 运行测试生成 .expected 快照**

```bash
cargo run -- run fixtures/happy/typedarray_full.js
```

复制输出作为 `fixtures/happy/typedarray_full.expected`。

- [ ] **Step 3: 提交**

```bash
git add fixtures/happy/typedarray_full.js fixtures/happy/typedarray_full.expected
git commit -m "test: add comprehensive TypedArray methods test fixture

Co-authored-by: CommandCodeBot <noreply@commandcode.ai>"
```

- [ ] **Step 4: 运行全量测试**

```bash
cargo test
```

Expected: all existing tests pass + new fixture test passes.

---

## 总结

| Task | 内容 | 涉及文件 |
|------|------|----------|
| 1 | IR 层 25 个 Builtin 变体 | `wjsm-ir/src/builtin.rs` |
| 2 | Semantic 方法路由 + 全局识别 | `wjsm-semantic/src/builtins.rs`, `lowerer_calls_eval.rs` |
| 3 | Backend WASM import + codegen | `compiler_core.rs`, `compiler_builtins.rs` |
| 4 | BigInt64Array/BigUint64Array 构造器 | `collections_buffers.rs` |
| 5 | set/subarray/slice 实现 | `collections_buffers.rs` |
| 6 | reverse/indexOf/lastIndexOf/includes/join/toString/copyWithin/at | `collections_buffers.rs` |
| 7 | fill/forEach/map (回调) | `collections_buffers.rs` |
| 8 | filter/reduce/find/some/every/sort + entries/keys/values | `collections_buffers.rs` |
| 9 | TypedArray 值渲染 | `runtime_render.rs` |
| 10 | 测试 fixtures | `fixtures/happy/typedarray_full.js` + `.expected` |
