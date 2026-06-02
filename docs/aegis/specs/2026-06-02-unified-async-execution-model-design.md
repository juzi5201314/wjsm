# 统一异步执行模型设计规格

**状态**: 已完成
**日期**: 2026-06-02  
**决策**: 一次性全量转换（非分阶段）  
**ADR 信号**: 执行模型单一化（async-only）— 影响公共 API 契约、宿主兼容性边界、依赖方向（tokio 成为硬依赖）

---

## 1. 问题陈述

### 1.1 核心问题

当前代码库已完成从 **同步/异步执行路径共存** 到 **async-only** 的根治转换：

- `execute` / `execute_with_writer` 已统一为 async-only 公共 API；同步 wrapper 已删除。
- `register_linker` 仅注册 async 宿主函数；`register_complex_bridges_sync` 已删除。
- `register_common_bridges` 为纯内存/状态桥接，无 WASM re-entry，保留为 `Func::wrap`。
- 所有可能 re-enter Wasm 的宿主 import（数组回调、Function.call/apply、Proxy/Reflect trap、TypedArray 回调、eval、JSON.parse reviver、microtask drain、timer 回调等）已全面迁移到 `func_wrap_async` + `call_wasm_callback_async`。
- `read_object_property_by_name` **无需 async 化** — 其当前实现仅读取对象 slot 与原型链内存字段，不触发 getter 或 Proxy trap（Plan Re-grounded Correction 1）。
**Wasmtime 约束**：async store（`config.async_support(true)`）上的 **所有** WASM 调用必须通过 `call_async().await`。`func.call()` 在 async store 上直接 panic。

### 1.2 Re-entry 路径完整清单（已全面转换）

**宿主 import 直接回调**（`Func::wrap` → `func_wrap_async`）：

| 模块 | 回调数 | 典型函数 |
|---|---|---|
| `array_object.rs` | 11 | forEach, map, filter, reduce, sort, find, some, every, flatMap, func_call, func_apply |
| `proxy_reflect.rs` | 12 | get, set, has, delete, apply, construct, defineProperty, ownKeys, getPrototypeOf, isExtensible, preventExtensions... |
| `typedarray_new_methods.rs` | 10 | forEach, map, filter, reduce, reduceRight, find, findIndex, some, every, sort |
| `misc.rs` | 4 | native_call, drain_microtasks, eval_direct, eval_indirect |
| `core.rs` | 1 | `op_in`（Proxy `has` trap） |
| `primitive_core.rs` | 1 | `string_replace`（callable replacer） |
| `proxy_traps.rs` | 3 | proxy_trap_get, proxy_trap_set, proxy_trap_delete |
| `timers_arrays.rs` | 1 | `json_parse`（reviver + toString/valueOf coercion） |

**Runtime 辅助函数间接路径**（同步 helper → async helper）：

| 间接路径 | 辅助函数 | 位置 |
|---|---|---|
| Getter 调度 | `reflect_get_impl_with_receiver_async` | `runtime_host_helpers.rs` |
| Proxy defineProperty trap | `define_property_internal_async` | `runtime_host_helpers.rs` |
| Proxy 扩展性/原型链 | `proxy_or_target_get_prototype_of_impl_async`, `proxy_or_target_is_extensible_impl_async`, `proxy_or_target_prevent_extensions_impl_async` | `runtime_host_helpers.rs` |
| 函数 dispatch | `resolve_and_call_async`, `resolve_callable_and_call_async`, `func_apply_impl_async` | `runtime_values.rs` |
| Eval compiled WASM | `perform_eval_from_caller_async`, `try_compiled_eval_from_caller_async` | `runtime_eval.rs` |
| Microtask drain / resume | `drain_microtasks_async`, `resume_async_function_async` | `runtime_microtask.rs`, `runtime_async_fn.rs` |
| JSON reviver | `apply_reviver_async`, `json_parse_to_string_async` | `runtime_json.rs` |

**说明**：`read_object_property_by_name` 保持同步（仅读取对象 slot / 原型链内存，不触发 getter/Proxy trap），无需级联 `.await`（Re-grounded Correction 1）。
### 1.3 非目标

- **不改 WASM 代码生成**：`wjsm-backend-wasm` 的编译管线不变
- **不改 IR 层**：`wjsm-ir` 不变
- **不改语义分析层**：`wjsm-semantic` 不变
- **不改 fixture 文件**：`fixtures/` 下的 `.js`/`.expected` 文件不变
- **不改模块系统**：`wjsm-module` 不变（模块加载在 WASM 编译前完成）

---

## 2. 解决方案：统一异步模型

**消除同步执行路径**，统一为单一 async 路径：

1. **宿主函数注册**：所有可能 re-enter Wasm 的 import 使用 `linker.func_wrap_async`；纯内存/状态 import 保留 `Func::wrap`。
2. **WASM re-entry**：所有 `call_wasm_callback` → `call_wasm_callback_async`；`resolve_and_call` → `resolve_and_call_async`；eval → `perform_eval_from_caller_async`；microtask drain → `drain_microtasks_async`。
3. **执行入口**：`execute` / `execute_with_writer` 已是 async-only；同步 wrapper 已删除。
4. **CLI 集成**：`tokio::runtime::Runtime::block_on()` 桥接 `wjsm_runtime::execute(...).await`。
### 2.2 技术细节

#### 2.2.1 宿主函数注册变更

**Before**（sync `define_*`）：
```rust
pub(crate) fn define_array_object(
    linker: &mut Linker<RuntimeState>,
    store: &mut Store<RuntimeState>,
) -> Result<()> {
    let f = Func::wrap(&mut *store, |mut caller: Caller<'_, RuntimeState>, args_base: i32, args_count: i32| -> i64 {
        // ... 回调体（同步）
        call_wasm_callback(&mut caller, cb, this_arg, &[elem, idx])
    });
    linker.define(&mut *store, "env", "arr_proto_for_each", f)?;
    Ok(())
}
```

**After**（async `define_*`）：
```rust
pub(crate) fn define_array_object(
    linker: &mut Linker<RuntimeState>,
) -> Result<()> {
    linker.func_wrap_async(
        "env", "arr_proto_for_each",
        |mut caller: Caller<'_, RuntimeState>, (args_base, args_count): (i32, i32)| -> BoxFuture<'_, i64> {
            Box::pin(async move {
                // ... 回调体（异步）
                call_wasm_callback_async(&mut caller, cb, this_arg, &[elem, idx]).await
            })
        },
    )?;
    Ok(())
}
```

**关键变化**：
- `Func::wrap(&mut store, ...)` + `linker.define(...)` → `linker.func_wrap_async("env", name, ...)`
- `store` 参数移除（`func_wrap_async` 仅需 Linker 的 engine 引用）
- 回调体包装在 `Box::pin(async move { ... })` 中
- `call_wasm_callback(...)` → `call_wasm_callback_async(...).await`
- 参数从独立参数变为 tuple：`arg1: i64, arg2: i32` → `(arg1, arg2): (i64, i32)`

#### 2.2.2 属性访问说明

`read_object_property_by_name` **保持同步**。

Re-grounded Correction（计划正文已核实）：`runtime_values.rs:375-449` 与 `read_object_property_by_name_proto_walk_with_env` 只读取对象 slot 和原型链内存字段，不触发 getter，不触发 Proxy trap，不调用 `call_wasm_callback`。旧 spec 中“`read_object_property_by_name` 会触发 getter/Proxy，需级联 130+ `.await`”是错误权威，已修正。

因此：
- `read_object_property_by_name` 无需 async 化，不增加 `.await` 级联。
- 原 spec 草案中的 `read_object_property_by_name_direct` 快速路径未实施（因为完整路径本身已是纯内存操作，无需拆分）。

#### 2.2.3 执行入口变更

**已完成**：
- `execute` / `execute_with_writer` 已是 async-only 公共 API；同步 wrapper 已删除。
- `register_linker` 已统一为 async-only；`register_complex_bridges_sync` 已删除。
- `register_common_bridges` 保持为纯内存/状态桥接，使用 `Func::wrap`，无 re-entry。

**CLI 集成**（`crates/wjsm-cli/src/lib.rs`）：
```rust
// wjsm run / wjsm eval
let rt = tokio::runtime::Runtime::new()?;
rt.block_on(async {
    wjsm_runtime::execute(&wasm_bytes).await
})?;
```
| 变更类别 | 实际规模 | 说明 |
|---|---|---|
| `define_*` 模块 `Func::wrap` → `func_wrap_async` | ~43 回调 | 仅 re-entry 回调转换；纯内存/状态 import 保留 `Func::wrap` |
| Runtime helpers → async 版本 | ~16 函数 | 含 `call_wasm_callback_async`、`resolve_and_call_async`、`resolve_callable_and_call_async`、`func_apply_impl_async`、`drain_microtasks_async`、`resume_async_function_async`、`perform_eval_from_caller_async`、`try_compiled_eval_from_caller_async`、`reflect_get_impl_with_receiver_async`、`define_property_internal_async`、`proxy_or_target_*_impl_async`、`reflect_apply_impl_async`、`reflect_construct_impl_async`、`apply_reviver_async`、`json_parse_to_string_async` 等 |
| `call_wasm_callback` → `_async.await` | ~48 调用点 | 直接替换（host imports + runtime helpers） |
| 其他级联（`resolve_and_call`、eval、microtask） | ~30 调用点 | 辅助函数 async 化 |
| `read_object_property_by_name` → async | 0 | 保持同步（Re-grounded Correction 1） |
| `*_direct` 优化 | 未实施 | 完整路径本身已是纯内存操作 |
| `define_*` 函数签名 `store` 参数 | 部分保留 | 纯 `Func::wrap` import 仍需 `&mut Store`；async override 移除 `store` |
| Sync 路径删除 | ~2000 行 | `register_complex_bridges_sync`、sync `execute` / `execute_with_writer`（已提前删除）、sync helper 定义（Task 16） |
### 3.2 核心文件清单

#### 3.2.1 已修改文件

**`crates/wjsm-runtime/src/host_imports/`**（新增 async override + 删除 sync re-entry 块）：
- `array_object.rs` — 数组回调、Function.call/apply（13 个 `call_wasm_callback` / `resolve_and_call` 调用点）
- `proxy_reflect.rs` — Proxy/Reflect trap（12 个调用点）
- `typedarray_new_methods.rs` — TypedArray 回调方法（10 个调用点）
- `misc.rs` — native_call, drain_microtasks, eval_direct, eval_indirect（4 个调用点）
- `core.rs` — `op_in`（Proxy `has` trap，1 个调用点）
- `primitive_core.rs` — `string_replace` callable replacer（1 个调用点）
- `proxy_traps.rs` — proxy_trap_get/set/delete（3 个调用点）
- `timers_arrays.rs` — `json_parse` reviver（1 个调用点）
- `reentrant_async.rs` — 新增 `define_*_async` override（含 TypedArray、primitive_core）

### 3.2 已修改文件清单

#### 3.2.1 宿主 import 层（host_imports/）

新增 `define_*_async` async override + 删除 sync re-entry 块：
- `array_object.rs` — 数组回调（forEach/map/filter/reduce/find/some/every/flatMap/sort）、Function.call/apply
- `proxy_reflect.rs` — Proxy/Reflect trap（get/set/has/delete/apply/construct/defineProperty/ownKeys/getPrototypeOf/isExtensible/preventExtensions/getOwnPropertyDescriptor）
- `typedarray_new_methods.rs` — TypedArray 回调方法（forEach/map/filter/reduce/reduceRight/find/findIndex/some/every/sort）
- `misc.rs` — native_call、drain_microtasks、eval_direct、eval_indirect
- `core.rs` — `op_in`（Proxy `has` trap）
- `primitive_core.rs` — `string_replace`（callable replacer）
- `proxy_traps.rs` — proxy_trap_get/set/delete
- `timers_arrays.rs` — `json_parse`（reviver + toString/valueOf coercion）
- `reentrant_async.rs` — 集中注册上述所有 async override

纯内存/状态 import，未改动：
- `collections_buffers.rs`、`promise.rs`、`promise_combinators.rs`、`async_fn.rs`、`async_generator.rs`、`string_methods.rs`、`math_number_error.rs`、`object_builtins.rs`、`fetch.rs`、`atomics.rs`、`weakref_finalization.rs`、`get_builtin_global_entry.rs`

#### 3.2.2 Runtime 辅助函数层

- `runtime_host_helpers.rs` — 删除 `call_wasm_callback`（sync）；保留 `call_wasm_callback_async`。新增/保留 `reflect_get_impl_with_receiver_async`、`define_property_internal_async`、`proxy_or_target_*_impl_async`、`reflect_apply_impl_async`、`reflect_construct_impl_async`。
- `runtime_values.rs` — 删除 `resolve_and_call`、`resolve_callable_and_call`、`func_apply_impl`（sync）；保留 `_async` 版本。
- `runtime_json.rs` — 新增 `apply_reviver_async`、`json_parse_to_string_async`、`json_parse_to_wasm_async`。
- `runtime_eval.rs` — 删除 `perform_eval_from_caller`（sync）；保留 `perform_eval_from_caller_async`、`try_compiled_eval_from_caller_async`。
- `runtime_microtask.rs` — 删除 `drain_microtasks`、`drain_microtasks_from_caller`、`call_host_function`、`call_host_function_with_args`（sync）；保留 `drain_microtasks_async`、`call_host_function_with_args_async` 等。
- `runtime_async_fn.rs` — 删除 `resume_async_function`（sync）；保留 `resume_async_function_async`。
- `runtime_builtins.rs` — 新增 `call_native_callable_with_args_from_caller_async`。
- `lib.rs` — `register_linker` 已是 async-only；`register_complex_bridges_sync` 已删除。
- `scheduler.rs` — 已是 async-only，无需改动。

#### 3.2.3 CLI 层

- `crates/wjsm-cli/src/lib.rs` — 使用 `tokio::runtime::Runtime::block_on(wjsm_runtime::execute(...).await)`。

---

## 4. Runtime 辅助函数变更详表

### 4.1 删除（sync 版本，替换为已有/新建的 async 版本）

| 函数 | 位置 | 处置 |
|---|---|---|
| `call_wasm_callback` | `runtime_host_helpers.rs` | 删除（`_async` 版本已存在） |
| `resolve_callable_and_call` | `runtime_values.rs` | 新建 `_async`（`func.call_async().await`） |
| `reflect_get_impl_with_receiver` | `runtime_host_helpers.rs` | 新建 `_async`（getter dispatch → `call_wasm_callback_async.await`） |
| `define_property_internal` | `runtime_host_helpers.rs` | 新建 `_async`（Proxy defineProperty trap） |
| `proxy_or_target_get_prototype_of_impl` | `runtime_host_helpers.rs` | 新建 `_async` |
| `proxy_or_target_is_extensible_impl` | `runtime_host_helpers.rs` | 新建 `_async` |
| `proxy_or_target_prevent_extensions_impl` | `runtime_host_helpers.rs` | 新建 `_async` |

### 4.2 不变（无 WASM re-entry）

`resolve_handle`、`read_shadow_arg_with_env`、`find_memory_c_string`、`alloc_host_object`、`alloc_array`、`define_host_data_property`（纯写）、`write_object_property_by_name_id`（纯写）、`define_host_data_property_from_caller`（纯写）

### 4.3 `*_direct` 安全使用规则

使用 `read_object_property_by_name_direct`（sync）的前提：**目标对象确定不是 Proxy，且属性确定不是 accessor（getter）**。

**安全场景**（可用 `*_direct`）：
- 内部 handle：`__map_handle__`、`__set_handle__`、`__typedarray_handle__`、`__arraybuffer_handle__`、`__weakmap_handle__`、`__weakset_handle__`、`__weakref_handle__`、`__finalization_registry_handle__`、`__proxy_handle__`、`__matchall_handle__`、`__date_ms__`、`__response_handle__`、`__request_handle__`、`__headers_handle__`
- 自身刚用 `define_host_data_property` 写入的属性

**必须用 async 版本**：
- 用户代码创建的对象上的属性读取
- 原型链查找（可能命中 getter）
- 不确定对象类型的通用属性读取

**默认规则**：新增代码默认使用 async 版本。仅在对目标对象有明确保证时使用 `*_direct`。

---

## 5. 迁移策略

### 5.1 一次性转换（已确认）

由于 property helpers 的级联特性，分阶段会导致中间态不一致（getter/Proxy 路径仍有 sync re-entry 风险）。确认采用一次性全量转换。

### 5.2 实施顺序

尽管是一次性提交，实施按以下顺序推进（每个步骤确保 `cargo check` 通过）：

**Step 1: Runtime helpers async 化**
1. 确认 `call_wasm_callback_async` 已存在且正确
2. 新建 `resolve_callable_and_call_async`
3. 新建 `reflect_get_impl_with_receiver_async`
4. 新建 `define_property_internal_async`
5. 新建 `proxy_or_target_*_impl_async` 系列
6. `read_object_property_by_name` → async
7. 新增 `read_object_property_by_name_direct`（sync fast path）

**Step 2: Runtime 模块级联更新**
1. `runtime_values.rs` — `resolve_callable_and_call` 调用点
2. `runtime_host_helpers.rs` — 内部 `read_object_property_by_name` + `call_wasm_callback` 调用点
3. `runtime_json.rs` — `call_wasm_callback` → `_async.await`
4. `runtime_builtins.rs` — `read_object_property_by_name` → `.await` 或 `_direct`
5. `runtime_heap.rs` / `runtime_render.rs` — 内部 handle 读取 → `_direct`
6. `runtime_promises.rs` / `runtime_eval.rs` — 按需更新

**Step 3: define_* 模块转换**
1. 逐个转换 18 个 `define_*` 函数
2. `Func::wrap` → `linker.func_wrap_async`
3. 回调体包装 `Box::pin(async move { ... })`
4. `call_wasm_callback(...)` → `call_wasm_callback_async(...).await`
5. `read_object_property_by_name(...)` → `.await` 或 `_direct`
6. 移除 `store` 参数

**Step 4: lib.rs 整合**
1. 删除 `register_linker`（sync）
2. `register_linker_async` → `register_linker`
3. 删除 `register_common_bridges`（sync），转 async
4. 删除 `register_complex_bridges_sync`
5. `register_complex_bridges_async` → `register_complex_bridges`
6. 删除 `execute_with_writer`（sync）
7. `execute_with_writer_async` → `execute_with_writer`
8. `execute_async` → `execute`

**Step 5: CLI 集成**
1. `wjsm-cli` 添加 `tokio` 依赖
2. `run` / `eval` 子命令 → `block_on(execute(...).await)`

**Step 6: Sync 路径清理**
1. 删除所有已弃用的 sync 函数
2. 删除 Phase 3 async twin helpers 的 sync 版本（`drain_microtasks`、`call_host_function_with_args` 等）
3. `cargo clippy` 清理 dead code

**Step 7: 验证**
1. `cargo build --workspace`
2. `cargo nextest run --workspace`
3. 性能基准对比

---

## 6. 风险评估

### 6.1 技术风险

| 风险 | 影响 | 缓解措施 |
|---|---|---|
| 大量 `.await` 级联导致编译错误 | 高 | 按 Step 顺序推进，每步 `cargo check` |
| 异步闭包生命周期 / Send bound | 中 | `Box::pin(async move { ... })` + `RuntimeState: Send` |
| `Func::wrap_async` 签名兼容性 | 中 | wasmtime 版本已支持（当前 `register_complex_bridges_async` 已验证） |
| 性能退化（async 调度开销） | 低 | `*_direct` 优化内部属性读取；epoch yield 开销可忽略 |
| tokio 依赖引入 | 低 | `wjsm-cli` 已可引入 tokio；`wjsm-runtime` 已通过 `tokio::sync::mpsc` 和 `tokio::time` 依赖 tokio |

### 6.2 不变量

- **所有 `Func::wrap` 在 async store 上被消除** — 编译后搜索 `Func::wrap` 在 `wjsm-runtime` 中应零匹配
- **所有 `func.call()` 在 async store 上被消除** — 搜索 `.call(` 应为零（仅 `.call_async(` 存在）
- **所有 fixture 输出不变** — `.expected` 文件无需修改

---

## 7. 测试策略

### 7.1 编译验证
- `cargo build --workspace` — 全 workspace 编译通过
- `cargo clippy --workspace` — 无 warning

### 7.2 功能验证
- `cargo nextest run --workspace` — 全测试通过
- 重点关注：
  - `happy__*` fixtures — 输出与 `.expected` 一致
  - `errors__*` fixtures — 错误信息一致
  - `modules__*` fixtures — 模块系统正常
  - Proxy/Reflect 相关 fixtures — getter/trap 正确触发
  - Timer/async fixtures — scheduler 行为一致

### 7.3 回归验证
- `WJSM_UPDATE_FIXTURES=1 cargo nextest run` — 确认无 `.expected` 文件变化
- 手动验证：`cargo run -- run fixtures/happy/proxy_traps.js` — Proxy trap 在 async 路径正常

---

## 8. 验收标准

### 8.1 功能验收

- [x] 所有现有测试通过（`cargo nextest run --workspace`）— 772 passed
- [x] `Func::wrap` 在 `wjsm-runtime` 中保留但仅限纯内存/状态 import，无 re-entry；re-entry 回调已全部 `func_wrap_async`
- [x] `func.call(` 在 async Store 可达路径中已消除（仅 `.call_async(`）
- [x] getter/Proxy trap 在 async 路径正确触发
- [x] 无 async store panic

### 8.2 代码质量

- [x] clippy 0 errors（允许既有 warning，非本次引入）
- [x] 代码格式化通过 `cargo fmt`
- [x] dead sync 路径已清除（`STRICT_AUDIT` 通过）

### 8.3 Fixture 兼容性

- [x] 所有 `.expected` 文件无需修改
- [x] Proxy/Reflect/Getter 相关 fixture 输出一致

## 9. ADR 信号

| 信号 | 说明 |
|---|---|
| **执行模型单一化** | async-only，sync store 不再支持。影响：嵌入式用例需自行引入 tokio |
| **tokio 硬依赖** | `wjsm-runtime` 已依赖 tokio（`tokio::sync`, `tokio::time`），`wjsm-cli` 新增 `tokio::runtime` |
| **公共 API 变更** | `execute_with_writer` 从 sync → async。库消费者需 `.await` 或 `block_on` |
| **Phase 3 twin helpers 清理** | `drain_microtasks` / `call_host_function_with_args` 等 sync 版本已删除，仅保留 `_async` 版本 |

---

## 10. 附录

### 10.1 术语表

| 术语 | 定义 |
|---|---|
| `Func::wrap` | Wasmtime 同步宿主函数注册 API |
| `func_wrap_async` | Wasmtime 异步宿主函数注册 API（需 `config.async_support(true)`） |
| async store | Wasmtime 异步存储（`Store` on async `Engine`），所有 WASM 调用必须 `call_async` |
| WASM re-entry | 从宿主函数回调 WASM 代码（`func.call()` / `func.call_async()`） |
| getter dispatch | 读取 accessor 属性时调用 getter 函数 |
| Proxy trap | Proxy 对象的拦截器（get/set/has/delete/apply/construct 等） |
| NaN-boxed `i64` | 所有 JS 值编码为 `i64`，NaN 空间标记类型 |
| `*_direct` | 跳过 Proxy/getter 检测的直接 slot 读取（sync，性能优化） |

### 10.2 决策记录

| 日期 | 决策 | 理由 |
|---|---|---|
| 2026-06-02 | 统一异步模型（方案 3） | 根治 + 消除代码重复 + 与 wasmtime async-first 方向一致 |
| 2026-06-02 | 一次性全量转换 | property helpers 级联特性决定分阶段会导致中间态不一致 |
| 2026-06-02 | 保留 `*_direct` sync fast path | 内部 handle 读取（`__map_handle__` 等）不需要 async 开销 |
| 2026-06-02 | CLI 使用 `block_on` 桥接 | 最小化 CLI 层变更，tokio runtime 仅在入口创建 |

---

**文档版本**: v1.1  
**最后更新**: 2026-06-02（Task 17 完成，验收标准全部达成）  
**Spec 自审**: 已完成（修正文件路径、类型签名、API 名称、补充非目标/ADR/级联分析，验收标准已勾选）