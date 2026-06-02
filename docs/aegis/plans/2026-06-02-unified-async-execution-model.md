# 统一异步执行模型 — 实现计划

## Goal

消除 `wjsm-runtime` 中 async store 上的 sync WASM re-entry panic 风险。将所有直接执行 WASM re-entry（`call_wasm_callback` / `func.call()`）的 `Func::wrap` 回调转为 `func_wrap_async`，并为间接 re-entry 的 runtime helpers 创建 async 版本。

## Architecture

**Scope 修正**（vs spec 2026-06-02）：

Spec 认为 `read_object_property_by_name` 需要 async 化并级联 ~130 个 `.await`。**经验证这是错误的**：`read_object_property_by_name_with_env`（`runtime_values.rs:375`）是纯 slot 查找 + 原型链遍历，不调用 `call_wasm_callback`，不做 getter/proxy dispatch。Getter dispatch 在 `reflect_get_impl_with_receiver`（`runtime_host_helpers.rs:1639`），Proxy trap dispatch 在 `proxy_or_target_*_impl` — 这些是独立的 helper，不是 property read 的一部分。

**实际 scope**：

| 类别 | 数量 | 说明 |
|---|---|---|
| 直接 `call_wasm_callback` 的 `Func::wrap` 回调 | ~48 | 需转 `func_wrap_async` |
| 含 `call_wasm_callback`/`func.call()` 的 runtime helpers | 8 | 需创建 `_async` 版本 |
| 纯操作回调（无 re-entry） | ~450 | **不变**，保持 `Func::wrap` |
| `read_object_property_by_name` 调用点 | 0 变更 | 纯 slot 查找，无需 async |

**关键约束**：Wasmtime 允许在同一 async store/linker 上混合注册 `Func::wrap`（sync）和 `func_wrap_async`（async）宿主函数。仅当 sync 回调尝试 WASM re-entry（`func.call()`）时才 panic。不做 re-entry 的 sync 回调完全安全。

## Tech Stack

- **wasmtime** `func_wrap_async` + `Box::new(async move { ... })` — 已有模式（`register_complex_bridges_async`）
- **tokio** — CLI 层 `block_on` 桥接（`wjsm-runtime` 已依赖 tokio）

## Baseline / Authority Refs

- Spec: `docs/aegis/specs/2026-06-02-unified-async-execution-model-design.md`
- 已有 async 模式: `register_complex_bridges_async`（`lib.rs:774`）— `func_wrap_async` + `call_wasm_callback_async`
- 已有 async helpers: `call_wasm_callback_async`（`runtime_host_helpers.rs:187`）、`drain_microtasks_async`、`call_host_function_with_args_async`

## Compatibility Boundary

- **不变**：所有 fixture `.expected` 文件、WASM codegen、IR、语义分析、模块系统
- **公共 API 变更**：`execute_with_writer` sync → async（唯一公共入口）
- **内部变更**：`register_linker_async` → `register_linker`（重命名），sync 路径删除

## Verification

- `cargo build --workspace` — 编译通过
- `cargo nextest run --workspace` — 全测试通过
- `Func::wrap` 在 `host_imports/` 中匹配数 < 原始数（仅 re-entry 回调被转换）
- `call_wasm_callback(` 在 `Func::wrap` 回调闭包体内零匹配
- `func.call(` 在 async 路径零匹配（仅 `func.call_async(`）
- 所有 `.expected` fixture 输出不变

---

## Plan Pressure Test

```
- Owner / contract / retirement:
  - wjsm-runtime 是唯一 owner
  - 旧 sync 路径（execute_with_writer sync、register_linker sync）被删除
  - 新 async-only 路径取代
- Verification scope:
  - cargo build + nextest 全通过
  - fixture 输出不变
  - grep 验证无遗漏 re-entry
- Task executability:
  - 每个 define_* 模块独立可验证（cargo check）
  - Runtime helpers 先建后转
- Pressure result: proceed
```

## Plan-Time Complexity Check

```
- Target files: 18 host_imports/*.rs + ~6 runtime_*.rs + lib.rs + wjsm-cli
- Existing size / shape signals:
  - collections_buffers.rs: ~2400 行（最大模块）
  - proxy_reflect.rs: ~1400 行
  - array_object.rs: ~1800 行
- Owner fit: 变更在现有模块内，不新增文件
- Add-in-place risk: 低 — 机械模式替换，不改变逻辑
- Better file boundary: N/A（in-place edit）
- Recommendation: edit-in-place
```

---

## 转换模式参考

### Pattern A: `Func::wrap` → `func_wrap_async`（单个回调）

**Before**（sync 回调，含 WASM re-entry）：
```rust
let f = Func::wrap(
    &mut store,
    |mut caller: Caller<'_, RuntimeState>, _env_obj: i64, this_val: i64, args_base: i32, args_count: i32| -> i64 {
        // ... 纯 Rust 逻辑 ...
        call_wasm_callback(&mut caller, cb, this_arg, &[elem, idx_val, this_val])
            .unwrap_or(value::encode_undefined());
        // ...
        value::encode_undefined()
    },
);
linker.define(&mut store, "env", "arr_proto_for_each", f)?;
```

**After**（async 回调）：
```rust
linker.func_wrap_async(
    "env", "arr_proto_for_each",
    |mut caller: Caller<'_, RuntimeState>, (_env_obj, this_val, args_base, args_count): (i64, i64, i32, i32)| -> i64 {
        Box::new(async move {
            // ... 纯 Rust 逻辑（不变）...
            call_wasm_callback_async(&mut caller, cb, this_arg, &[elem, idx_val, this_val]).await
                .unwrap_or(value::encode_undefined());
            // ...
            value::encode_undefined()
        })
    },
)?;
```

**关键变化**：
1. `Func::wrap(&mut store, |caller, p1, p2, ...| { ... })` → `linker.func_wrap_async("env", "name", |caller, (p1, p2, ...): (T1, T2, ...)| { Box::new(async move { ... }) })`
2. 参数从独立参数变为 tuple
3. `call_wasm_callback(...)` → `call_wasm_callback_async(...).await`
4. 删除 `let f = ...; linker.define(...)` 中间变量
5. 如果回调内调用 `define_property_internal` → `define_property_internal_async(...).await`
6. 如果回调内调用 `reflect_get_impl_with_receiver` → `reflect_get_impl_with_receiver_async(...).await`

### Pattern B: Runtime helper → async 版本

**Before**（sync helper，含 `call_wasm_callback`）：
```rust
pub(crate) fn reflect_get_impl_with_receiver(
    caller: &mut Caller<'_, RuntimeState>, target: i64, prop: i64, receiver: i64,
) -> i64 {
    // ... slot 查找 ...
    // getter dispatch:
    return call_wasm_callback(caller, getter, receiver, &[])
        .unwrap_or_else(|_| value::encode_undefined());
}
```

**After**（新增 async 版本，sync 版本保留）：
```rust
pub(crate) async fn reflect_get_impl_with_receiver_async(
    caller: &mut Caller<'_, RuntimeState>, target: i64, prop: i64, receiver: i64,
) -> i64 {
    // ... slot 查找（不变）...
    // getter dispatch:
    return call_wasm_callback_async(caller, getter, receiver, &[]).await
        .unwrap_or_else(|_| value::encode_undefined());
}
// sync 版本保留（给不 re-enter 的 Func::wrap 回调用）
```

### Pattern C: `call_wasm_callback_async` bound 函数修复

**Before**（line 286，async 版本中仍用 sync 调用）：
```rust
return call_wasm_callback(&mut *caller, bound_func, bound_this, &combined_args);
```

**After**：
```rust
return call_wasm_callback_async(&mut *caller, bound_func, bound_this, &combined_args).await;
```

---

## Tasks

### Task 1: Runtime helpers — 创建 async 版本

**Files**:
- Modify: `crates/wjsm-runtime/src/runtime_host_helpers.rs`
- Modify: `crates/wjsm-runtime/src/runtime_values.rs`

**Why**: 建立 async re-entry 基础设施。所有后续 `define_*` 转换依赖这些 async helpers。

**Impact**: 新增 ~7 个 async 函数。sync 版本保留（给 non-re-entry 回调用）。

**Steps**:

- [ ] **Step 1**: 修复 `call_wasm_callback_async` 中 bound 函数的 sync re-entry（line 286）

将 `call_wasm_callback_async` 函数体内 line 286 的：
```rust
return call_wasm_callback(&mut *caller, bound_func, bound_this, &combined_args);
```
替换为：
```rust
return call_wasm_callback_async(&mut *caller, bound_func, bound_this, &combined_args).await;
```

- [ ] **Step 2**: 创建 `resolve_callable_and_call_async` in `runtime_values.rs`

复制 `resolve_callable_and_call`（line 1088-1220）为 `resolve_callable_and_call_async`，将：
  - `call_wasm_callback(caller, ...)` → `call_wasm_callback_async(caller, ...).await`
  - `func.call(&mut *caller, ...)` (line 1205) → `func.call_async(&mut *caller, ...).await`

- [ ] **Step 3**: 创建 `reflect_get_impl_with_receiver_async` in `runtime_host_helpers.rs`

复制 `reflect_get_impl_with_receiver`（line 1639-1748）为 `_async` 版本，将：
  - `call_wasm_callback(caller, getter, receiver, &[])` (line 1738) → `call_wasm_callback_async(caller, getter, receiver, &[]).await`

- [ ] **Step 4**: 创建 `define_property_internal_async` in `runtime_host_helpers.rs`

复制 `define_property_internal`（line 1211-1240）+ 其内部 `define_property_on_target` 为 `_async` 版本，将：
  - `call_wasm_callback(caller, trap, ...)` (line 1272) → `call_wasm_callback_async(caller, trap, ...).await`

- [ ] **Step 5**: 创建 `proxy_or_target_get_prototype_of_impl_async` in `runtime_host_helpers.rs`

复制 line 796-870 为 `_async`，将 line 812 的 `call_wasm_callback` → `call_wasm_callback_async(...).await`。

- [ ] **Step 6**: 创建 `proxy_or_target_is_extensible_impl_async` in `runtime_host_helpers.rs`

复制 line 872-920 为 `_async`，将 line 889 的 `call_wasm_callback` → `call_wasm_callback_async(...).await`。

- [ ] **Step 7**: 创建 `proxy_or_target_prevent_extensions_impl_async` in `runtime_host_helpers.rs`

复制 line 922-960 为 `_async`，将 line 940 的 `call_wasm_callback` → `call_wasm_callback_async(...).await`。

- [ ] **Step 8**: 验证编译

```bash
cargo check -p wjsm-runtime
```

**Verification**: `cargo check -p wjsm-runtime` passes. 新增的 `_async` 函数存在但尚未被调用（dead code warnings 可暂时 `#[allow(dead_code)]`）。

- [ ] **Step 9**: Commit

```bash
git add -A && git commit -m "feat: add async versions of re-entry runtime helpers

- resolve_callable_and_call_async
- reflect_get_impl_with_receiver_async
- define_property_internal_async (+ define_property_on_target_async)
- proxy_or_target_get_prototype_of_impl_async
- proxy_or_target_is_extensible_impl_async
- proxy_or_target_prevent_extensions_impl_async
- Fix call_wasm_callback_async bound function dispatch to use async

Co-authored-by: CommandCodeBot <noreply@commandcode.ai>"
```

---

### Task 2: Convert `host_imports/array_object.rs` re-entry callbacks

**Files**:
- Modify: `crates/wjsm-runtime/src/host_imports/array_object.rs`

**Why**: 11 个回调直接调用 `call_wasm_callback`（forEach, map, filter, reduce, reduceRight, sort, find, findIndex, some, every, flatMap）。

**Impact**: 仅转换含 `call_wasm_callback` 的回调。其余 ~30 个回调（push, pop, includes, indexOf, join, concat, slice, fill, reverse, flat, shift, unshift, at, copyWithin, splice, isArray, ...）保持 `Func::wrap`。

**Steps**:

- [ ] **Step 1**: 识别需转换的 11 个回调

搜索 `call_wasm_callback` 在文件中的位置。每个匹配位于一个 `Func::wrap` 回调内。需转换的回调名：
1. `arr_proto_sort_fn` (line ~509)
2. `arr_proto_for_each_fn` (line ~683)
3. `arr_proto_map_fn` (line ~730)
4. `arr_proto_filter_fn` (line ~771)
5. `arr_proto_reduce_fn` (line ~834)
6. `arr_proto_reduce_right_fn` (line ~891)
7. `arr_proto_find_fn` (line ~932)
8. `arr_proto_find_index_fn` (line ~968)
9. `arr_proto_some_fn` (line ~1009)
10. `arr_proto_every_fn` (line ~1045)
11. `arr_proto_flat_map_fn` (line ~1092)

- [ ] **Step 2**: 对每个回调应用 Pattern A 转换

对每个回调，执行以下变换：
1. `let VAR = Func::wrap(&mut store, |CALLER, PARAMS| -> RET { BODY });`
   → `linker.func_wrap_async("env", "NAME", |CALLER, (PARAMS): (TYPES)| { Box::new(async move { BODY_ASYNC }) })?;`
2. 回调体内 `call_wasm_callback(` → `call_wasm_callback_async(`，并在调用处加 `.await`
3. 删除 `linker.define(&mut store, "env", "NAME", VAR)?;`（被 `func_wrap_async` 替代）

示例 — `arr_proto_for_each_fn`（完整转换）：

**Before**:
```rust
let arr_proto_for_each_fn = Func::wrap(
    &mut store,
    |mut caller: Caller<'_, RuntimeState>,
     _env_obj: i64, this_val: i64, args_base: i32, args_count: i32| -> i64 {
        let cb = read_shadow_arg(&mut caller, args_base, 0);
        // ...
        for i in 0..len {
            // ...
            if call_wasm_callback(&mut caller, cb, this_arg, &[elem, idx_val, this_val]).is_err() {
                return value::encode_undefined();
            }
        }
        value::encode_undefined()
    },
);
linker.define(&mut store, "env", "arr_proto_for_each", arr_proto_for_each_fn)?;
```

**After**:
```rust
linker.func_wrap_async(
    "env", "arr_proto_for_each",
    |mut caller: Caller<'_, RuntimeState>,
     (_env_obj, this_val, args_base, args_count): (i64, i64, i32, i32)| -> i64 {
        Box::new(async move {
            let cb = read_shadow_arg(&mut caller, args_base, 0);
            // ...
            for i in 0..len {
                // ...
                if call_wasm_callback_async(&mut caller, cb, this_arg, &[elem, idx_val, this_val]).await.is_err() {
                    return value::encode_undefined();
                }
            }
            value::encode_undefined()
        })
    },
)?;
```

对其余 10 个回调重复相同模式。

- [ ] **Step 3**: 移除 `store` 参数（如果函数内所有回调都已转换）

由于 `array_object.rs` 中混合 sync/async 回调，`store` 参数仍需保留给 sync `Func::wrap` 回调。**函数签名不变**。

- [ ] **Step 4**: 验证编译

```bash
cargo check -p wjsm-runtime 2>&1 | head -50
```

**Verification**: `cargo check -p wjsm-runtime` passes. 文件中 `call_wasm_callback(` 在 `Func::wrap` 闭包内零匹配（已全部转为 `_async`）。

- [ ] **Step 5**: Commit

```bash
git add -A && git commit -m "feat: convert array_object.rs re-entry callbacks to func_wrap_async

Convert 11 callbacks (sort, forEach, map, filter, reduce, reduceRight,
find, findIndex, some, every, flatMap) from Func::wrap to func_wrap_async.
Remaining ~30 non-re-entry callbacks stay as Func::wrap.

Co-authored-by: CommandCodeBot <noreply@commandcode.ai>"
```

---

### Task 3: Convert `host_imports/proxy_reflect.rs` re-entry callbacks

**Files**:
- Modify: `crates/wjsm-runtime/src/host_imports/proxy_reflect.rs`

**Why**: 12 个回调直接调用 `call_wasm_callback`（proxy get/set/has/deleteProperty/apply/construct, reflect get/set/has/delete/apply/construct）。此外 `reflect_get` 回调调用 `reflect_get_impl_with_receiver`，`reflect_defineProperty` 调用 `define_property_internal` — 需改用 `_async` 版本。

**Impact**: 12 回调转 async。其余 ~5 个回调（`reflect_get_prototype_of`, `reflect_set_prototype_of`, `reflect_is_extensible`, `reflect_prevent_extensions`, `reflect_get_own_property_descriptor`）保持 sync（它们不直接调用 `call_wasm_callback`，但它们调用的 `proxy_or_target_*_impl` helpers 有 `_async` 版本 — 但这些回调本身不做 WASM re-entry，仅在 proxy 路径通过 `call_wasm_callback` re-enter — 需验证）。

**⚠️ 注意**：`reflect_get_prototype_of` 等 5 个回调虽然自身不含 `call_wasm_callback`，但它们调用 `proxy_or_target_get_prototype_of_impl` 等 helper，这些 helper **内部** 对 proxy 做 `call_wasm_callback`。如果这些回调在 async store 上执行且目标是 proxy，sync helper 内的 `call_wasm_callback` 会 panic。

**决策**：为安全起见，将这 5 个回调也转为 async，调用 `_async` helpers。总计 **17 个回调**转 async。

**Steps**:

- [ ] **Step 1**: 转换 12 个直接 `call_wasm_callback` 回调（Pattern A）

同 Task 2 模式。对每个回调：
1. `Func::wrap` → `func_wrap_async` + `Box::new(async move { ... })`
2. `call_wasm_callback(` → `call_wasm_callback_async(...).await`
3. 删除 `linker.define(...)` + 中间变量

对 `reflect_get` 回调（line ~39-82）：
- `reflect_get_impl_with_receiver(...)` → `reflect_get_impl_with_receiver_async(...).await`

对 `reflect_defineProperty` 回调（line ~1100-1120）：
- `define_property_internal(...)` → `define_property_internal_async(...).await`

- [ ] **Step 2**: 转换 5 个间接 re-entry 回调

转换 `reflect_get_prototype_of`, `reflect_set_prototype_of`, `reflect_is_extensible`, `reflect_prevent_extensions`, `reflect_get_own_property_descriptor` 为 `func_wrap_async`。

在回调体内，将：
- `proxy_or_target_get_prototype_of_impl(...)` → `proxy_or_target_get_prototype_of_impl_async(...).await`
- `proxy_or_target_is_extensible_impl(...)` → `proxy_or_target_is_extensible_impl_async(...).await`
- `proxy_or_target_prevent_extensions_impl(...)` → `proxy_or_target_prevent_extensions_impl_async(...).await`

- [ ] **Step 3**: 验证编译

```bash
cargo check -p wjsm-runtime 2>&1 | head -50
```

- [ ] **Step 4**: Commit

```bash
git add -A && git commit -m "feat: convert proxy_reflect.rs callbacks to func_wrap_async

Convert all 17 Proxy/Reflect callbacks to func_wrap_async.
Uses async versions of reflect_get_impl_with_receiver,
define_property_internal, and proxy_or_target_*_impl helpers.

Co-authored-by: CommandCodeBot <noreply@commandcode.ai>"
```

---

### Task 4: Convert `host_imports/typedarray_new_methods.rs` re-entry callbacks

**Files**:
- Modify: `crates/wjsm-runtime/src/host_imports/typedarray_new_methods.rs`

**Why**: 10 个回调直接调用 `call_wasm_callback`（forEach, map, filter, reduce, reduceRight, find, findIndex, some, every, sort）。

**Impact**: 仅转换这 10 个回调。其余 ~12 个回调（includes, indexOf, lastIndexOf, join, toString, copyWithin, at, fill, reverse, entries, keys, values）保持 `Func::wrap`。

**Steps**:

- [ ] **Step 1**: 对 10 个回调应用 Pattern A 转换

同 Task 2 模式。每个回调：`Func::wrap` → `func_wrap_async`，`call_wasm_callback(` → `call_wasm_callback_async(...).await`。

- [ ] **Step 2**: 验证编译

```bash
cargo check -p wjsm-runtime
```

- [ ] **Step 3**: Commit

```bash
git add -A && git commit -m "feat: convert typedarray_new_methods.rs re-entry callbacks to func_wrap_async

Convert 10 callbacks (forEach, map, filter, reduce, reduceRight, find,
findIndex, some, every, sort) from Func::wrap to func_wrap_async.

Co-authored-by: CommandCodeBot <noreply@commandcode.ai>"
```

---

### Task 5: Convert `host_imports/misc.rs` re-entry callbacks

**Files**:
- Modify: `crates/wjsm-runtime/src/host_imports/misc.rs`

**Why**: 2 个回调直接调用 `call_wasm_callback`（`native_call` line 92, `queueMicrotask` line 155）。

**Steps**:

- [ ] **Step 1**: 对 2 个回调应用 Pattern A 转换

`native_call_fn` (line ~43-200): `Func::wrap` → `func_wrap_async`，`call_wasm_callback(` → `call_wasm_callback_async(...).await`。

`queue_microtask_fn` (line ~22-200): 同上。注意此回调内有 `call_wasm_callback` 在 microtask drain 路径。

- [ ] **Step 2**: 验证编译

```bash
cargo check -p wjsm-runtime
```

- [ ] **Step 3**: Commit

```bash
git add -A && git commit -m "feat: convert misc.rs re-entry callbacks to func_wrap_async

Convert native_call and queueMicrotask callbacks to func_wrap_async.

Co-authored-by: CommandCodeBot <noreply@commandcode.ai>"
```

---

### Task 6: Convert `host_imports/core.rs` re-entry callbacks

**Files**:
- Modify: `crates/wjsm-runtime/src/host_imports/core.rs`

**Why**: 1 个回调直接调用 `call_wasm_callback`（line 716，位于某个宿主函数内）。

**Steps**:

- [ ] **Step 1**: 定位 line 716 的 `call_wasm_callback` 所在回调

确认该回调的宿主函数名（`obj_get`、`typeof`、或其他）。对该回调应用 Pattern A。

- [ ] **Step 2**: 验证编译

```bash
cargo check -p wjsm-runtime
```

- [ ] **Step 3**: Commit

```bash
git add -A && git commit -m "feat: convert core.rs re-entry callback to func_wrap_async

Co-authored-by: CommandCodeBot <noreply@commandcode.ai>"
```

---

### Task 7: Convert `runtime_json.rs` re-entry callbacks

**Files**:
- Modify: `crates/wjsm-runtime/src/runtime_json.rs`

**Why**: 2 处 `call_wasm_callback` 调用（JSON.stringify `toJSON` dispatch line 645, JSON.parse `reviver` dispatch line 785）。

**Impact**: 这些调用位于 runtime 内部函数中（非 `Func::wrap` 回调），被 async 回调间接调用。需将包含它们的函数转为 async。

**Steps**:

- [ ] **Step 1**: 定位调用链

`call_wasm_callback` at line 645 位于 `json_stringify_impl` 或类似函数内。
`call_wasm_callback` at line 785 位于 `json_parse_impl` 或类似函数内。

将这些函数转为 async，`call_wasm_callback(` → `call_wasm_callback_async(...).await`。

- [ ] **Step 2**: 级联更新调用者

如果 `json_stringify_impl` 被 `Func::wrap` 回调调用，该回调也需转 `func_wrap_async`。
定位所有调用 `json_stringify_impl` / `json_parse_impl` 的 `Func::wrap` 回调并转换。

- [ ] **Step 3**: 验证编译

```bash
cargo check -p wjsm-runtime
```

- [ ] **Step 4**: Commit

```bash
git add -A && git commit -m "feat: convert runtime_json.rs re-entry paths to async

Convert JSON.stringify toJSON and JSON.parse reviver dispatch to
call_wasm_callback_async.

Co-authored-by: CommandCodeBot <noreply@commandcode.ai>"
```

---

### Task 8: lib.rs — 转换 `register_common_bridges` 中 re-entry 回调

**Files**:
- Modify: `crates/wjsm-runtime/src/lib.rs`

**Why**: `register_common_bridges`（sync/async 共享）中可能有 `Func::wrap` 回调包含 `call_wasm_callback`。`register_complex_bridges_sync` 已被 `register_complex_bridges_async` 覆盖（后者用 `func_wrap_async` + `call_wasm_callback_async`）。

**Steps**:

- [ ] **Step 1**: 审计 `register_common_bridges` 中的 re-entry

```bash
grep -n 'call_wasm_callback\b' crates/wjsm-runtime/src/lib.rs
```

检查 `register_common_bridges` 内的 `Func::wrap` 回调是否包含 `call_wasm_callback`。如果有，对该回调应用 Pattern A。

- [ ] **Step 2**: 删除 `register_complex_bridges_sync`

`register_complex_bridges_sync`（line ~402）已被 `register_complex_bridges_async`（line ~774）完全替代。删除 sync 版本。

- [ ] **Step 3**: 重命名 `register_complex_bridges_async` → `register_complex_bridges`

- [ ] **Step 4**: 验证编译

```bash
cargo check -p wjsm-runtime
```

- [ ] **Step 5**: Commit

```bash
git add -A && git commit -m "feat: clean up lib.rs bridges — delete sync complex bridges, audit common bridges

Co-authored-by: CommandCodeBot <noreply@commandcode.ai>"
```

---

### Task 9: lib.rs — 删除 sync 执行路径，重命名 async 路径

**Files**:
- Modify: `crates/wjsm-runtime/src/lib.rs`

**Why**: 消除 sync 路径，async 成为唯一路径。

**Steps**:

- [ ] **Step 1**: 删除 `register_linker`（sync 版本）

删除 `fn register_linker(linker, store)`（line ~63-82）。保留 `register_linker_async`。

- [ ] **Step 2**: 重命名 `register_linker_async` → `register_linker`

全局替换 `register_linker_async` → `register_linker`。

- [ ] **Step 3**: 删除 `execute_with_writer`（sync 版本）

删除 `pub fn execute_with_writer(...)` 整个函数体（sync 路径，包含 `Config::new()` + `Store::new()` + sync linker + sync instantiation + sync main.call + sync timer loop）。

- [ ] **Step 4**: 重命名 `execute_with_writer_async` → `execute_with_writer`

全局替换 `execute_with_writer_async` → `execute_with_writer`。

- [ ] **Step 5**: 重命名 `execute_async` → `execute`

全局替换 `execute_async` → `execute`。

- [ ] **Step 6**: 清理 `Func::wrap` 相关 imports

如果 `Func` import 不再被使用（仅剩 `func_wrap_async`），移除。

- [ ] **Step 7**: 验证编译

```bash
cargo check -p wjsm-runtime 2>&1 | head -80
```

CLI 层会报编译错误（调用已删除的 sync 函数）— 这是预期的，Task 10 修复。

- [ ] **Step 8**: Commit

```bash
git add -A && git commit -m "feat: delete sync execution path, rename async as default

- Delete register_linker (sync), rename register_linker_async → register_linker
- Delete execute_with_writer (sync), rename execute_with_writer_async → execute_with_writer
- Rename execute_async → execute

Co-authored-by: CommandCodeBot <noreply@commandcode.ai>"
```

---

### Task 10: CLI 集成 — tokio block_on 桥接

**Files**:
- Modify: `crates/wjsm-cli/Cargo.toml`
- Modify: `crates/wjsm-cli/src/lib.rs`

**Why**: CLI 需桥接 sync→async，通过 `tokio::runtime::Runtime::block_on` 调用 `execute`/`execute_with_writer`。

**Steps**:

- [ ] **Step 1**: 添加 tokio 依赖

在 `crates/wjsm-cli/Cargo.toml` 的 `[dependencies]` 中确认 `tokio` 已存在（`wjsm-runtime` 已传递依赖 tokio）。如果需要直接创建 runtime，添加：
```toml
tokio = { version = "1", features = ["rt-multi-thread"] }
```

- [ ] **Step 2**: 更新 `run` 子命令

在 `crates/wjsm-cli/src/lib.rs` 中，找到调用 `runtime::execute_with_writer` 的位置，改为：
```rust
let rt = tokio::runtime::Runtime::new()
    .context("Failed to create tokio runtime")?;
rt.block_on(async {
    runtime::execute_with_writer(&wasm_bytes, writer).await
})?;
```

- [ ] **Step 3**: 更新 `eval` 子命令

同上模式，将 `runtime::execute_with_writer` / `runtime::execute` 调用改为 `block_on(async { ... .await })`。

- [ ] **Step 4**: 更新其他执行入口

搜索 `wjsm-cli/src/lib.rs` 中所有 `runtime::execute` 和 `runtime::execute_with_writer` 调用点，确保都使用 `block_on`。

- [ ] **Step 5**: 验证编译

```bash
cargo build --workspace 2>&1 | tail -20
```

- [ ] **Step 6**: Commit

```bash
git add -A && git commit -m "feat: integrate CLI with tokio runtime for async execution

Co-authored-by: CommandCodeBot <noreply@commandcode.ai>"
```

---

### Task 11: 清理 — 删除不再使用的 sync helpers

**Files**:
- Modify: `crates/wjsm-runtime/src/runtime_host_helpers.rs`
- Modify: `crates/wjsm-runtime/src/runtime_values.rs`

**Why**: sync 执行路径已删除。`call_wasm_callback`（sync）、`resolve_callable_and_call`（sync）等函数不再被任何代码路径使用。

**Steps**:

- [ ] **Step 1**: 检查 `call_wasm_callback`（sync）是否仍有调用者

```bash
grep -rn 'call_wasm_callback\b(' crates/wjsm-runtime/src/ | grep -v '_async'
```

如果所有调用者都已转为 `call_wasm_callback_async`，删除 sync `call_wasm_callback` 函数。

**注意**：sync 版本的 `call_wasm_callback` 可能被不 re-enter 的 `Func::wrap` 回调间接使用 — 但实际上不 re-enter 的回调根本不调用 `call_wasm_callback`（它们不需要 WASM re-entry）。所以如果 grep 显示零非 async 调用者，可以安全删除。

- [ ] **Step 2**: 检查 `resolve_callable_and_call`（sync）是否仍有调用者

```bash
grep -rn 'resolve_callable_and_call\b(' crates/wjsm-runtime/src/ | grep -v '_async'
```

如果仅在自身定义 + `call_wasm_callback` 内调用（两者都将被删除），可以安全删除。

- [ ] **Step 3**: 检查其他 sync helpers 是否仍有调用者

对 `reflect_get_impl_with_receiver`、`define_property_internal`、`proxy_or_target_*_impl` 执行同样检查。如果所有调用者都已转 `_async`，删除 sync 版本。

**⚠️ 谨慎**：某些 sync helpers 可能被不 re-enter 的 `Func::wrap` 回调通过间接路径调用（如 `reflect_get_impl_with_receiver` 被 `reflect_get` 回调调用）。确保所有调用路径都已覆盖。

- [ ] **Step 4**: 删除确认无调用者的 sync 函数

- [ ] **Step 5**: 移除 Task 1 中的 `#[allow(dead_code)]` 标注

- [ ] **Step 6**: 验证编译

```bash
cargo build --workspace 2>&1 | tail -20
```

- [ ] **Step 7**: Commit

```bash
git add -A && git commit -m "refactor: remove unused sync re-entry helpers

Delete sync versions of call_wasm_callback, resolve_callable_and_call,
reflect_get_impl_with_receiver, define_property_internal, and
proxy_or_target_*_impl — all callers now use async versions.

Co-authored-by: CommandCodeBot <noreply@commandcode.ai>"
```

---

### Task 12: 最终验证

**Files**: None (verification only)

**Steps**:

- [ ] **Step 1**: 全 workspace 编译

```bash
cargo build --workspace 2>&1 | tail -20
```

**Expected**: 编译成功，零 error。

- [ ] **Step 2**: 全测试

```bash
cargo nextest run --workspace 2>&1 | tail -30
```

**Expected**: 全测试通过。

- [ ] **Step 3**: Fixture 输出不变

```bash
WJSM_UPDATE_FIXTURES=1 cargo nextest run 2>&1 | tail -10
git diff fixtures/ | head -50
```

**Expected**: 无 `.expected` 文件变化。如有变化，分析是否为回归。

- [ ] **Step 4**: Grep 验证 — 无遗漏 re-entry

```bash
# 验证 Func::wrap 回调内无 call_wasm_callback（sync）
grep -n 'call_wasm_callback\b(' crates/wjsm-runtime/src/host_imports/*.rs | grep -v '_async' | grep -v '//'
```

**Expected**: 零匹配（所有 `call_wasm_callback` 在 host_imports 中已转 `_async`）。

```bash
# 验证 func.call( 不在 async 路径
grep -rn '\.call(' crates/wjsm-runtime/src/ | grep -v '_async' | grep -v '//' | grep -v 'func_call_fn' | grep -v 'native_call'
```

**Expected**: 仅 sync 路径残留（应已被 Task 9/11 清除）。

- [ ] **Step 5**: Clippy

```bash
cargo clippy --workspace 2>&1 | tail -20
```

**Expected**: 零 warning（或仅预存 warning）。

- [ ] **Step 6**: 总结 commit

```bash
git log --oneline feat/async-scheduler-2026-05-31 ^master | head -20
```

---

## Risks

| 风险 | 缓解 |
|---|---|
| `Func::wrap` 回调参数 tuple 化编译错误 | 逐模块 `cargo check`，参考 `register_complex_bridges_async` 已有模式 |
| async helper 内 lifetime/borrow checker 问题 | `Caller<'_, RuntimeState>` 在 async closure 内需注意 — 参考已有 `call_wasm_callback_async` |
| 遗漏间接 re-entry 路径 | Task 12 grep 验证 + 全 fixture 测试 |
| `call_wasm_callback_async` bound 函数修复引入行为变化 | bound 函数已有测试覆盖 |
| tokio runtime 在 CLI 中初始化失败 | `Runtime::new()` 错误处理 |

## Retirement

| 旧 owner/fallback | 状态 | 处置 |
|---|---|---|
| `execute_with_writer` (sync) | 待删除 | Task 9 |
| `register_linker` (sync) | 待删除 | Task 9 |
| `register_complex_bridges_sync` | 待删除 | Task 8 |
| `call_wasm_callback` (sync) | 待删除 | Task 11 |
| `resolve_callable_and_call` (sync) | 待删除 | Task 11 |
| `reflect_get_impl_with_receiver` (sync) | 待删除 | Task 11（如无 sync 调用者） |
| `define_property_internal` (sync) | 待删除 | Task 11（如无 sync 调用者） |
| `proxy_or_target_*_impl` (sync) | 待删除 | Task 11（如无 sync 调用者） |

## ADR Signal

从 spec 继承：执行模型单一化（async-only），tokio 硬依赖，公共 API 变更（`execute_with_writer` sync → async）。
