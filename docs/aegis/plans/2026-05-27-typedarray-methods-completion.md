# Plan: TypedArray 方法补全 + BigInt64Array/BigUint64Array 启用

**Date:** 2026-05-27
**Goal:** 补全 TypedArray 缺失的 ~20 个原型方法，并启用 BigInt64Array / BigUint64Array 构造器。
**Baseline/Authority Refs:**
- Design Spec: `docs/superpowers/specs/2026-05-19-typedarray-methods-design.md`
- Prior Plan: `docs/superpowers/plans/2026-05-19-typedarray-methods.md`
- Phase 8 Plan: `docs/superpowers/plans/2026-05-13-es-builtins-phase8-arraybuffer-dataview-typedarray.md`

---

## 1. Problem & Scope

### Current State
- TypedArray 构造器（Int8Array ~ Float64Array）已可用
- 仅实现了 6 个原型方法：`length`/`byteLength`/`byteOffset`/`set`/`slice`/`subarray`
- 缺失 ~20 个方法：`fill`, `reverse`, `sort`, `indexOf`, `includes`, `join`, `map`, `filter`, `reduce`, `find`, `findIndex`, `some`, `every`, `forEach`, `copyWithin`, `at`, `entries`, `keys`, `values`, `toString`, `lastIndexOf`, `reduceRight`
- `BigInt64Array` / `BigUint64Array` 在 `ALL_BUILTINS` 中已注册，但在 `compiler_core.rs:1024-1029` 被显式跳过（panic 豁免）
- `TypedArrayEntry.element_kind` 当前仅支持 0=Int, 1=Uint, 2=Clamped, 3=Float；需要扩展以支持 BigInt（64-bit signed/unsigned）

### Target State
- 所有 ~20 个 TypedArray 原型方法在 IR / 语义层 / 后端 / 运行时全部贯通
- `BigInt64Array` / `BigUint64Array` 构造器可用，支持通过 `new BigInt64Array(buffer)` 创建
- 运行时 `ta_read`/`ta_write`/`sab_read`/`sab_write` 支持 64-bit BigInt 元素读写
- 语义层 `builtin_from_typedarray_proto_method` 和 `is_typedarray_constructor_expr` 已覆盖全部方法名
- 后端 `compiler_builtins.rs` 和 `compiler_core.rs` 的 import 映射包含全部新 builtin
- 全局对象 `create_global_object` 暴露 `BigInt64Array` / `BigUint64Array`

---

## 2. Architecture & Tech Stack

| Layer | Crate | Key Files |
|-------|-------|-----------|
| IR | `wjsm-ir` | `src/builtin.rs` |
| Semantic | `wjsm-semantic` | `src/builtins.rs`, `src/lib.rs`, `src/lowerer_*.rs` |
| Backend (WASM) | `wjsm-backend-wasm` | `src/compiler_core.rs`, `src/compiler_builtins.rs`, `src/lib.rs` |
| Runtime | `wjsm-runtime` | `src/host_imports/typedarray_new_methods.rs`, `src/host_imports/collections_buffers.rs`, `src/lib.rs` |

### Data Flow
```
JS Source → swc AST
    → Semantic Lowerer (builtin_from_typedarray_proto_method → CallBuiltin)
    → IR (Builtin enum)
    → Backend WASM (compiler_builtins.rs → emit Call import)
    → Runtime (host_imports/typedarray_new_methods.rs → ta_read/ta_write)
    → Memory (ArrayBuffer / SharedArrayBuffer)
```

---

## 3. Compatibility Boundary

- **Must NOT break** 现有 9 个 TypedArray 构造器（Int8Array ~ Float64Array）
- **Must NOT break** 已有 6 个原型方法（length/byteLength/byteOffset/set/slice/subarray）
- **Must NOT change** 现有 TypedArray 构造器调用约定：`(buffer, byteOffset, length)` → TypedArray 对象
- **Must NOT change** 现有 `HOST_IMPORT_NAMES` 数组中已有 import 的索引顺序（只能追加到末尾）
- **Must NOT change** 现有 `element_kind` 0-3 的语义（向后兼容）
- BigInt64Array / BigUint64Array 的 `element_kind` 使用新值 `4` / `5`
- `TypedArray.from` / `TypedArray.of` 不在本计划范围内（需要 %TypedArray% 抽象构造函数，当前架构不支持）

---

## 4. Plan Pressure Test

- **Owner / contract / retirement:** TypedArray 方法由 `typedarray_new_methods.rs` 统一拥有；BigInt 变体由 `collections_buffers.rs` 的宏和 `typedarray_new_methods.rs` 的读写辅助函数共同拥有。无退役计划。
- **Verification scope:** 单元测试覆盖每个新方法至少一个正向 case；BigInt64Array / BigUint64Array 覆盖创建、读写、set、slice。
- **Task executability:** 每个任务都是单一文件修改，边界清晰，可独立验证编译。
- **Pressure result:** proceed

## 5. Plan-Time Complexity Check

- **Target files:** `compiler_core.rs` (~1140 lines), `typedarray_new_methods.rs` (~945 lines), `collections_buffers.rs` (~1500 lines), `builtin.rs` (~650 lines), `builtins.rs` (~700 lines)
- **Existing size / shape signals:** `compiler_core.rs` 已超 1000 行，但 import 注册是机械追加；`typedarray_new_methods.rs` 方法体模式高度重复。
- **Owner fit:** 新代码放入已有文件，遵循现有模式。
- **Add-in-place risk:** 低 — 所有修改都是追加或模式扩展，不影响现有逻辑路径。
- **Recommendation:** edit-in-place

---

## 6. Task Breakdown

### Task 1: 扩展 TypedArrayEntry.element_kind 支持 BigInt
**Files:** `crates/wjsm-runtime/src/lib.rs`
**Why:** 现有 element_kind 仅 0-3（Int/Uint/Clamped/Float），BigInt64Array 需要 64-bit signed integer，BigUint64Array 需要 64-bit unsigned integer。
**Impact/Compatibility:** 仅扩展枚举语义，不影响现有 0-3 的行为。
**Verification:** `cargo check -p wjsm-runtime`

- [ ] **Step 1:** 修改 `TypedArrayEntry` 注释，扩展 element_kind 语义：
  ```rust
  /// 0=Int, 1=Uint, 2=Clamped, 3=Float, 4=BigInt, 5=BigUint
  element_kind: u8,
  ```
- [ ] **Step 2:** 运行 `cargo check -p wjsm-runtime`，确认通过

---

### Task 2: 运行时 ta_read / ta_write / sab_read / sab_write 支持 element_kind 4/5
**Files:** `crates/wjsm-runtime/src/host_imports/typedarray_new_methods.rs`
**Why:** BigInt64Array / BigUint64Array 的读写需要 64-bit 整数转换，不能复用现有的 32-bit 浮点/整数路径。
**Impact/Compatibility:** 新增 match arm，不影响现有分支。
**Verification:** `cargo check -p wjsm-runtime`

- [ ] **Step 1:** 在 `ta_read` 函数的 `match (elem_size, element_kind)` 中，在 `(8, 3)`（Float64）分支之后，添加：
  ```rust
  (8, 4) => {
      // BigInt64Array: read 8 bytes as i64, encode as bigint
      let bytes = &data[off..off + 8];
      let val = i64::from_le_bytes([
          bytes[0], bytes[1], bytes[2], bytes[3],
          bytes[4], bytes[5], bytes[6], bytes[7],
      ]);
      // Store in bigint_table and return handle
      let mut table = caller.data().bigint_table.lock().expect("bigint table mutex");
      let handle = table.len() as u32;
      table.push(crate::BigIntEntry { value: val.into() });
      value::encode_handle(value::TAG_BIGINT, handle)
  }
  (8, 5) => {
      // BigUint64Array: read 8 bytes as u64, encode as bigint
      let bytes = &data[off..off + 8];
      let val = u64::from_le_bytes([
          bytes[0], bytes[1], bytes[2], bytes[3],
          bytes[4], bytes[5], bytes[6], bytes[7],
      ]);
      let mut table = caller.data().bigint_table.lock().expect("bigint table mutex");
      let handle = table.len() as u32;
      table.push(crate::BigIntEntry { value: val.into() });
      value::encode_handle(value::TAG_BIGINT, handle)
  }
  ```
- [ ] **Step 2:** 在 `ta_write` 函数的 `match (elem_size, element_kind)` 中，在 `(8, 3)` 分支之后，添加对应写入逻辑：
  ```rust
  (8, 4) => {
      // BigInt64Array: decode bigint handle, write i64
      let handle = value::decode_handle(value) as usize;
      let table = caller.data().bigint_table.lock().expect("bigint table mutex");
      let val = table.get(handle).map(|e| e.value.clone()).unwrap_or_default();
      drop(table);
      let val_i64: i64 = val.try_into().unwrap_or(0);
      data[off..off + 8].copy_from_slice(&val_i64.to_le_bytes());
  }
  (8, 5) => {
      // BigUint64Array: decode bigint handle, write u64
      let handle = value::decode_handle(value) as usize;
      let table = caller.data().bigint_table.lock().expect("bigint table mutex");
      let val = table.get(handle).map(|e| e.value.clone()).unwrap_or_default();
      drop(table);
      let val_u64: u64 = val.try_into().unwrap_or(0);
      data[off..off + 8].copy_from_slice(&val_u64.to_le_bytes());
  }
  ```
- [ ] **Step 3:** 对 `sab_read` 和 `sab_write` 做同样的修改（SharedArrayBuffer 路径）
- [ ] **Step 4:** 运行 `cargo check -p wjsm-runtime`，确认通过

---

### Task 3: 运行时添加 BigInt64Array / BigUint64Array 构造器
**Files:** `crates/wjsm-runtime/src/host_imports/collections_buffers.rs`
**Why:** 当前 `typedarray_constructor!` 宏仅支持 element_kind 0-3，需要扩展以支持 4/5。
**Impact/Compatibility:** 新增两个构造器宏调用，不影响现有 9 个构造器。
**Verification:** `cargo check -p wjsm-runtime`

- [ ] **Step 1:** 在现有 9 个 `typedarray_constructor!` 宏调用之后，追加：
  ```rust
  typedarray_constructor!(bigint64array_constructor_fn, "bigint64array_constructor", 8, 4);
  typedarray_constructor!(biguint64array_constructor_fn, "biguint64array_constructor", 8, 5);
  ```
- [ ] **Step 2:** 运行 `cargo check -p wjsm-runtime`，确认通过

---

### Task 4: 运行时全局对象暴露 BigInt64Array / BigUint64Array
**Files:** `crates/wjsm-runtime/src/host_imports/collections_buffers.rs`
**Why:** `create_global_object_fn` 中的 `builtin_pairs` 数组未包含 BigInt64Array / BigUint64Array，导致全局作用域中无法访问这两个构造器。
**Impact/Compatibility:** 纯新增暴露，不影响现有全局对象。
**Verification:** `cargo check -p wjsm-runtime`

- [ ] **Step 1:** 在 `builtin_pairs` 数组中，在 `("DataView", NativeCallable::DataViewConstructorGlobal)` 之后，追加：
  ```rust
  ("BigInt64Array", NativeCallable::BigInt64ArrayConstructor),
  ("BigUint64Array", NativeCallable::BigUint64ArrayConstructor),
  ```
  **注意：** 需要先确认 `NativeCallable` 枚举是否已有这两个变体。如果没有，见 Task 5。
- [ ] **Step 2:** 运行 `cargo check -p wjsm-runtime`，确认通过

---

### Task 5: 运行时 NativeCallable 枚举添加 BigInt64Array / BigUint64Array 变体
**Files:** `crates/wjsm-runtime/src/lib.rs`
**Why:** `NativeCallable` 枚举当前没有 BigInt64Array / BigUint64Array 的变体。
**Impact/Compatibility:** 纯新增枚举变体，不影响现有变体。
**Verification:** `cargo check -p wjsm-runtime`

- [ ] **Step 1:** 在 `NativeCallable` 枚举中，在 `TypedArrayConstructor(())` 之后，追加：
  ```rust
  BigInt64ArrayConstructor,
  BigUint64ArrayConstructor,
  ```
- [ ] **Step 2:** 在 `runtime_builtins.rs` 的 `dispatch_native_callable` match 中（或对应的调用点），为这两个新变体添加处理逻辑。由于 TypedArray 构造器是通过 host import 直接分派的（不是通过 `call_indirect`），`NativeCallable` 变体仅用于全局对象属性注册，实际调用由 `int8array_constructor` 等 host import 处理。因此 dispatch 逻辑可以返回 `undefined` 或空对象（与 `TypedArrayConstructor` 保持一致）：
  ```rust
  NativeCallable::BigInt64ArrayConstructor | NativeCallable::BigUint64ArrayConstructor => {
      Some(value::encode_undefined())
  }
  ```
- [ ] **Step 3:** 运行 `cargo check -p wjsm-runtime`，确认通过

---

### Task 6: 后端 compiler_core.rs 添加 BigInt64Array / BigUint64Array import
**Files:** `crates/wjsm-backend-wasm/src/compiler_core.rs`
**Why:** `compiler_core.rs` 中手动注册了所有 TypedArray 构造器的 import，但缺少 BigInt64Array / BigUint64Array。
**Impact/Compatibility:** 新增两个 import 注册，索引递增。注意 `HOST_IMPORT_NAMES` 数组长度和 `lib.rs` 中的断言需要同步更新。
**Verification:** `cargo check -p wjsm-backend-wasm`

- [ ] **Step 1:** 在 `compiler_core.rs` 的 TypedArray constructor imports 区域（当前 float64array_constructor 之后，typedarray_proto_length 之前），追加：
  ```rust
  imports.import("env", "bigint64array_constructor", EntityType::Function(16));
  imports.import("env", "biguint64array_constructor", EntityType::Function(16));
  ```
- [ ] **Step 2:** 同步更新 `lib.rs` 中的 `HOST_IMPORT_NAMES` 数组：
  - 在 `"float64array_constructor"` 之后，追加 `"bigint64array_constructor"` 和 `"biguint64array_constructor"`
  - 数组长度从 `[&str; 384]` 改为 `[&str; 386]`
  - `debug_assert_eq!` 中的 `384` 改为 `386`
- [ ] **Step 3:** 在 `compiler_core.rs:1024-1029` 的跳过逻辑中，移除 `BigInt64ArrayConstructor` 和 `BigUint64ArrayConstructor` 的豁免：
  ```rust
  // 修改前：
  if matches!(
      builtin,
      Builtin::Debugger
          | Builtin::BigInt64ArrayConstructor
          | Builtin::BigUint64ArrayConstructor
  ) {
      continue;
  }
  // 修改后：
  if matches!(builtin, Builtin::Debugger) {
      continue;
  }
  ```
- [ ] **Step 4:** 运行 `cargo check -p wjsm-backend-wasm`，确认通过

---

### Task 7: 后端 compiler_builtins.rs 确认 BigInt64Array / BigUint64Array 分派
**Files:** `crates/wjsm-backend-wasm/src/compiler_builtins.rs`
**Why:** 确认 BigInt64ArrayConstructor / BigUint64ArrayConstructor 已被包含在正确的 match arm 中。
**Impact/Compatibility:** 当前代码中这两个 variant 已被包含在 `// ── TypedArray 新增构造器 ──` 的 match arm 中（见 line 849-850），无需修改。本任务为验证任务。
**Verification:** `cargo check -p wjsm-backend-wasm`

- [ ] **Step 1:** 确认 `compiler_builtins.rs` 中 `Builtin::BigInt64ArrayConstructor | Builtin::BigUint64ArrayConstructor` 已存在于 `// ── DataView set methods ──` 之后的 match arm 中，且该 arm 使用 Type 16（3-arg）调用约定。
- [ ] **Step 2:** 运行 `cargo check -p wjsm-backend-wasm`，确认通过

---

### Task 8: 语义层 builtin_from_typedarray_proto_method 已完整
**Files:** `crates/wjsm-semantic/src/builtins.rs`
**Why:** 确认所有 ~20 个新方法都已在映射表中。根据代码审查，该函数已包含全部方法。本任务为验证任务。
**Impact/Compatibility:** 无代码变更。
**Verification:** `cargo check -p wjsm-semantic`

- [ ] **Step 1:** 确认 `builtin_from_typedarray_proto_method` 包含以下全部方法：
  `set`, `subarray`, `slice`, `fill`, `reverse`, `indexOf`, `lastIndexOf`, `includes`, `join`, `toString`, `copyWithin`, `at`, `forEach`, `map`, `filter`, `reduce`, `reduceRight`, `find`, `findIndex`, `some`, `every`, `sort`, `entries`, `keys`, `values`
- [ ] **Step 2:** 确认 `builtin_call_signature` 中每个新方法都有对应的签名定义
- [ ] **Step 3:** 运行 `cargo check -p wjsm-semantic`，确认通过

---

### Task 9: 语义层 is_typedarray_constructor_expr 包含 BigInt64Array / BigUint64Array
**Files:** `crates/wjsm-semantic/src/lib.rs`
**Why:** 该函数用于识别 TypedArray 构造函数调用，以启用 `typedarray_bindings` 优化。当前已包含 BigInt64Array / BigUint64Array（line 1471-1472）。本任务为验证任务。
**Impact/Compatibility:** 无代码变更。
**Verification:** `cargo check -p wjsm-semantic`

- [ ] **Step 1:** 确认 `is_typedarray_constructor_expr` 的 matches 列表包含 `"BigInt64Array"` 和 `"BigUint64Array"`
- [ ] **Step 2:** 运行 `cargo check -p wjsm-semantic`，确认通过

---

### Task 10: 语义层 lowerer_async_eval.rs 包含 BigInt64Array / BigUint64Array
**Files:** `crates/wjsm-semantic/src/lowerer_async_eval.rs`
**Why:** async/eval lowerer 中有一个硬编码的 builtin 列表用于直接构造器调用优化。当前列表包含 Int8Array ~ Float64Array，但**不包含** BigInt64Array / BigUint64Array。
**Impact/Compatibility:** 新增两个 match arm，使 async/eval 场景下的 BigInt64Array/BigUint64Array 构造器调用也能走直接优化路径。
**Verification:** `cargo check -p wjsm-semantic`

- [ ] **Step 1:** 在 `lowerer_async_eval.rs` 的 `Builtin::Float64ArrayConstructor` 之后，追加：
  ```rust
  | Builtin::BigInt64ArrayConstructor
  | Builtin::BigUint64ArrayConstructor
  ```
- [ ] **Step 2:** 运行 `cargo check -p wjsm-semantic`，确认通过

---

### Task 11: 语义层 builtins.rs 全局构造器注册包含 BigInt64Array / BigUint64Array
**Files:** `crates/wjsm-semantic/src/builtins.rs`
**Why:** `builtin_from_global_constructor` 函数将全局名称映射到 Builtin 变体。当前已包含 BigInt64Array / BigUint64Array（line 109-110）。本任务为验证任务。
**Impact/Compatibility:** 无代码变更。
**Verification:** `cargo check -p wjsm-semantic`

- [ ] **Step 1:** 确认 `builtin_from_global_constructor` 包含 `"BigInt64Array" => Some(Builtin::BigInt64ArrayConstructor)` 和 `"BigUint64Array" => Some(Builtin::BigUint64ArrayConstructor)`
- [ ] **Step 2:** 运行 `cargo check -p wjsm-semantic`，确认通过

---

### Task 12: 语义层 lowerer_declarations.rs / lowerer_assignments.rs 的 typedarray_bindings 支持 BigInt64Array / BigUint64Array
**Files:** `crates/wjsm-semantic/src/lowerer_declarations.rs`, `crates/wjsm-semantic/src/lowerer_assignments.rs`
**Why:** `is_typedarray_constructor_expr` 已包含 BigInt64Array / BigUint64Array，因此 `typedarray_bindings` 的插入逻辑会自动覆盖。本任务为验证任务。
**Impact/Compatibility:** 无代码变更。
**Verification:** `cargo check -p wjsm-semantic`

- [ ] **Step 1:** 确认 `lowerer_declarations.rs:51` 和 `lowerer_assignments.rs:456` 调用的 `is_typedarray_constructor_expr` 已包含 BigInt64Array / BigUint64Array
- [ ] **Step 2:** 运行 `cargo check -p wjsm-semantic`，确认通过

---

### Task 13: IR 层 builtin.rs 确认所有 TypedArray 方法 variant 已定义
**Files:** `crates/wjsm-ir/src/builtin.rs`
**Why:** 确认 IR 层的 `Builtin` 枚举包含全部 ~20 个新方法 variant。根据代码审查，已全部包含。本任务为验证任务。
**Impact/Compatibility:** 无代码变更。
**Verification:** `cargo check -p wjsm-ir`

- [ ] **Step 1:** 确认 `Builtin` 枚举包含：
  `TypedArrayProtoFill`, `TypedArrayProtoReverse`, `TypedArrayProtoIndexOf`, `TypedArrayProtoLastIndexOf`, `TypedArrayProtoIncludes`, `TypedArrayProtoJoin`, `TypedArrayProtoToString`, `TypedArrayProtoCopyWithin`, `TypedArrayProtoAt`, `TypedArrayProtoForEach`, `TypedArrayProtoMap`, `TypedArrayProtoFilter`, `TypedArrayProtoReduce`, `TypedArrayProtoReduceRight`, `TypedArrayProtoFind`, `TypedArrayProtoFindIndex`, `TypedArrayProtoSome`, `TypedArrayProtoEvery`, `TypedArrayProtoSort`, `TypedArrayProtoEntries`, `TypedArrayProtoKeys`, `TypedArrayProtoValues`
- [ ] **Step 2:** 确认 `ALL_BUILTINS` 数组包含上述全部 variant（以及 `BigInt64ArrayConstructor`, `BigUint64ArrayConstructor`）
- [ ] **Step 3:** 确认 `import_name()` 方法为每个新方法返回正确的字符串
- [ ] **Step 4:** 运行 `cargo check -p wjsm-ir`，确认通过

---

### Task 14: 运行时 typedarray_new_methods.rs 确认所有新方法已实现
**Files:** `crates/wjsm-runtime/src/host_imports/typedarray_new_methods.rs`
**Why:** 确认运行时 host import 函数已包含全部 ~20 个新方法。根据代码审查，已全部实现。本任务为验证任务。
**Impact/Compatibility:** 无代码变更。
**Verification:** `cargo check -p wjsm-runtime`

- [ ] **Step 1:** 确认文件中包含以下全部函数的 `Func::wrap` 定义和 `linker.define` 调用：
  `typedarray_proto_fill`, `typedarray_proto_reverse`, `typedarray_proto_index_of`, `typedarray_proto_last_index_of`, `typedarray_proto_includes`, `typedarray_proto_join`, `typedarray_proto_to_string`, `typedarray_proto_copy_within`, `typedarray_proto_at`, `typedarray_proto_for_each`, `typedarray_proto_map`, `typedarray_proto_filter`, `typedarray_proto_reduce`, `typedarray_proto_reduce_right`, `typedarray_proto_find`, `typedarray_proto_find_index`, `typedarray_proto_some`, `typedarray_proto_every`, `typedarray_proto_sort`, `typedarray_proto_entries`, `typedarray_proto_keys`, `typedarray_proto_values`
- [ ] **Step 2:** 运行 `cargo check -p wjsm-runtime`，确认通过

---

### Task 15: 运行时 runtime_render.rs 支持 element_kind 4/5
**Files:** `crates/wjsm-runtime/src/runtime_render.rs`
**Why:** 调试/渲染 TypedArray 内容时，`runtime_render.rs` 根据 `element_kind` 决定渲染格式。需要支持 4/5。
**Impact/Compatibility:** 新增 match arm，不影响现有渲染。
**Verification:** `cargo check -p wjsm-runtime`

- [ ] **Step 1:** 在 `runtime_render.rs` 的 `match (entry.element_size, entry.element_kind)` 中，添加 `(8, 4)` 和 `(8, 5)` 的处理逻辑，渲染为 BigInt 值（可复用 `render_value` 对 bigint handle 的处理）
- [ ] **Step 2:** 运行 `cargo check -p wjsm-runtime`，确认通过

---

### Task 16: 端到端编译测试
**Files:** N/A（全项目编译）
**Why:** 确认所有 crate 修改后，整个项目能编译通过。
**Impact/Compatibility:** 无代码变更。
**Verification:** `cargo build`

- [ ] **Step 1:** 运行 `cargo build`，确认全项目编译通过，无错误、无警告（或警告在可接受范围内）

---

### Task 17: 编写 TypedArray 方法单元测试
**Files:** `tests/` 或 `crates/wjsm-runtime/tests/`
**Why:** 验证每个新方法的正确性。
**Impact/Compatibility:** 仅新增测试文件，不影响生产代码。
**Verification:** 运行测试命令

- [ ] **Step 1:** 创建测试文件 `tests/typedarray_methods.rs`（或追加到现有测试文件），包含以下测试：
  - `fill`: `new Uint8Array(4).fill(5)` → `[5,5,5,5]`
  - `reverse`: `new Uint8Array([1,2,3]).reverse()` → `[3,2,1]`
  - `indexOf`: `new Uint8Array([1,2,3]).indexOf(2)` → `1`
  - `lastIndexOf`: `new Uint8Array([1,2,1]).lastIndexOf(1)` → `2`
  - `includes`: `new Uint8Array([1,2,3]).includes(2)` → `true`
  - `join`: `new Uint8Array([1,2,3]).join('-')` → `"1-2-3"`
  - `copyWithin`: `new Uint8Array([1,2,3,4]).copyWithin(0,2)` → `[3,4,3,4]`
  - `at`: `new Uint8Array([1,2,3]).at(-1)` → `3`
  - `forEach`: 验证回调被正确调用
  - `map`: 验证返回 Array（非 TypedArray）
  - `filter`: 验证返回 Array
  - `reduce`: 验证累加结果
  - `reduceRight`: 验证反向累加
  - `find`: 验证找到第一个匹配元素
  - `findIndex`: 验证返回正确索引
  - `some`: 验证至少一个匹配返回 true
  - `every`: 验证全部匹配返回 true
  - `sort`: 验证默认排序和 compareFn 排序
  - `entries`/`keys`/`values`: 验证返回迭代器对象
  - `toString`: 验证返回逗号分隔字符串
- [ ] **Step 2:** 编写 BigInt64Array / BigUint64Array 测试：
  - 创建：`new BigInt64Array(2)`
  - 读写：`arr[0] = 1n; arr[0]`
  - set：`arr.set([1n, 2n])`
  - slice：`arr.slice(0, 1)`
- [ ] **Step 3:** 运行测试，确认全部通过

---

### Task 18: 编写集成测试（test262 子集）
**Files:** N/A
**Why:** 验证 TypedArray 方法在真实 JavaScript 代码中的行为符合 ECMAScript 规范。
**Impact/Compatibility:** 无代码变更。
**Verification:** 运行 test262 子集

- [ ] **Step 1:** 从 test262 中选取 TypedArray 相关测试用例，运行并记录结果
- [ ] **Step 2:** 确认没有因本计划引入的回归失败

---

## 7. Risks & Rollback

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|-----------|
| BigInt 读写逻辑有端序或符号错误 | 中 | 高 | 单元测试覆盖正负值、最大值、零值 |
| HOST_IMPORT_NAMES 索引错位 | 低 | 高 | 严格按顺序追加，编译时断言检查长度 |
| element_kind 冲突 | 低 | 高 | 使用未占用的 4/5，文档化语义 |
| 运行时迭代器状态（entries/keys/values）与 Array 迭代器冲突 | 低 | 中 | 复用现有 `IteratorState::ArrayIter` 模式，已验证 |

**Rollback:** 所有修改都是增量添加，可通过 git revert 单个 commit 回滚。

---

## 8. Retirement

- 无旧代码需要退役。所有修改都是功能补全。
