# 统一异步执行模型设计规格

**状态**: 草案  
**日期**: 2026-06-02  
**决策**: 一次性全量转换（非分阶段）  
**ADR 信号**: 执行模型单一化（async-only）— 影响公共 API 契约、宿主兼容性边界、依赖方向（tokio 成为硬依赖）

---

## 1. 问题陈述

### 1.1 核心问题

当前代码库中存在 **同步/异步执行路径共存** 的架构缺陷，导致 async store 上存在 sync WASM re-entry panic 风险：

- `register_linker_async` 复用 18 个 `define_*` 模块，这些模块使用 `Func::wrap`（同步闭包）注册宿主函数
- `register_common_bridges` 也使用 `Func::wrap`，且 sync/async 路径共享
- 异步执行路径（`execute_with_writer_async`）在 async store 上运行
- 同步闭包内调用 `call_wasm_callback`（同步函数），其内部执行 `func.call()`（同步 WASM re-entry）
- 间接路径：`read_object_property_by_name` 对 getter/Proxy 对象的属性读取会触发 `call_wasm_callback`

**Wasmtime 约束**：async store（`config.async_support(true)`）上的 **所有** WASM 调用必须通过 `call_async().await`。`func.call()` 在 async store 上直接 panic。

### 1.2 Re-entry 路径完整清单

**直接路径**（~48 个 `Func::wrap` 回调直接调用 `call_wasm_callback`）：

| 模块 | 直接调用数 | 典型函数 |
|---|---|---|
| `array_object.rs` | 11 | forEach, map, filter, reduce, sort, find, some, every, flatMap... |
| `proxy_reflect.rs` | 12 | get, set, has, delete, apply, construct, defineProperty, ownKeys... |
| `typedarray_new_methods.rs` | 10 | forEach, map, filter, reduce, sort... |
| `misc.rs` | 2 | native_call, queue_microtask |
| `core.rs` | 1 | create_error_object |

**间接路径**（通过 getter dispatch / Proxy trap dispatch 触发 `call_wasm_callback`）：

| 间接路径 | 入口函数 | 位置 |
|---|---|---|
| Getter 调度 | `reflect_get_impl_with_receiver` → accessor flag → `call_wasm_callback(getter)` | `runtime_host_helpers.rs` |
| Proxy get trap | proxy_table 查找 → `call_wasm_callback(trap)` | `runtime_host_helpers.rs` |
| Proxy set/has/delete/apply/construct | `proxy_or_target_*_impl` | `runtime_host_helpers.rs` |
| `define_property_internal` | Proxy defineProperty trap | `runtime_host_helpers.rs` |
| `resolve_callable_and_call` | 直接 `func.call()`（同一问题） | `runtime_values.rs` |

**级联影响**：`read_object_property_by_name` 被 50+ 文件 ~565 处调用。虽然仅 Proxy/getter 场景触发 re-entry，但 Rust 类型系统不允许条件 async — 函数必须整体 async。

### 1.3 非目标

- **不改 WASM 代码生成**：`wjsm-backend-wasm` 的编译管线不变
- **不改 IR 层**：`wjsm-ir` 不变
- **不改语义分析层**：`wjsm-semantic` 不变
- **不改 fixture 文件**：`fixtures/` 下的 `.js`/`.expected` 文件不变
- **不改模块系统**：`wjsm-module` 不变（模块加载在 WASM 编译前完成）

---

## 2. 解决方案：统一异步模型

### 2.1 核心原则

**消除同步执行路径**，统一为单一 async 路径：

1. **宿主函数注册**：所有 `Func::wrap` → `linker.func_wrap_async`
2. **WASM re-entry**：所有 `call_wasm_callback` → `call_wasm_callback_async`（已有）
3. **属性访问**：`read_object_property_by_name` → async（getter/Proxy 路径用 `.await`）
4. **执行入口**：仅保留 `execute_with_writer_async`，重命名为 `execute_with_writer`
5. **CLI 集成**：`tokio::runtime::Runtime::block_on()` 桥接

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

#### 2.2.2 属性访问函数双层设计

```
read_object_property_by_name (async — 完整路径)
  ├─ 普通 data property → 直接 slot 读取（fast path，无 .await）
  └─ Proxy/getter → call_wasm_callback_async.await（slow path）

read_object_property_by_name_direct (sync — 快速路径)
  └─ 直接 slot 读取，跳过 Proxy/getter 检测
     仅用于确定是纯 data property 的内部属性
```

**async 版本**（替代现有 `read_object_property_by_name`）：
```rust
pub(crate) async fn read_object_property_by_name<C: AsContextMut<Data = RuntimeState>>(
    caller: &mut C, obj: i64, prop_name: &str,
) -> i64 {
    // 1. Proxy 检测 → call_wasm_callback_async.await
    // 2. 属性 slot 查找
    // 3. Accessor (getter) → call_wasm_callback_async.await
    // 4. Data property → 直接返回值
    // 5. 原型链递归 → self.await
}
```

**sync direct 版本**（新增，用于内部 handle 读取优化）：
```rust
pub(crate) fn read_object_property_by_name_direct<C: AsContextMut<Data = RuntimeState>>(
    caller: &mut C, obj: i64, prop_name: &str,
) -> i64 {
    // 直接 slot 查找，无 Proxy/getter 检测
    // 如属性不存在返回 undefined（不遍历原型链）
}
```

#### 2.2.3 执行入口变更

**删除**（sync 路径）：
- `pub fn execute_with_writer(...)` — sync 执行入口
- `fn register_linker(...)` — sync linker 注册
- `fn register_common_bridges(...)` — sync/async 共享桥接（sync 版本）
- `fn register_complex_bridges_sync(...)` — sync 复杂桥接

**保留 + 重命名**（async 路径成为唯一路径）：
- `execute_with_writer_async` → `execute_with_writer`
- `execute_async` → `execute`
- `register_linker_async` → `register_linker`
- `register_complex_bridges_async` → `register_complex_bridges`

**CLI 集成**（`crates/wjsm-cli/src/lib.rs`）：
```rust
// wjsm run / wjsm eval
let rt = tokio::runtime::Runtime::new()?;
rt.block_on(async {
    wjsm_runtime::execute(&wasm_bytes).await
})?;
```

---

## 3. 变更范围

### 3.1 文件影响统计

| 变更类别 | 估算 | 说明 |
|---|---|---|
| `define_*` 模块 `Func::wrap` → `func_wrap_async` | ~500 回调 | 机械转换 |
| Runtime helpers → async 版本 | ~8 函数 | 新建/转换 |
| `read_object_property_by_name` → async + `.await` | ~130 调用点 | 级联 |
| `call_wasm_callback` → `_async.await` | ~48 调用点 | 直接替换 |
| 其他级联（`resolve_callable_and_call` 等） | ~20 调用点 | |
| `*_direct` 优化替换（内部 handle 读取） | ~80 调用点 | 可选后续优化 |
| `define_*` 函数签名 `store` 参数移除 | 18 函数 | |
| Sync 路径删除 | ~2000 行 | `execute_with_writer` + `register_linker` + `register_common_bridges` + `register_complex_bridges_sync` |

### 3.2 核心文件清单

#### 3.2.1 必须修改的文件

**`crates/wjsm-runtime/src/host_imports/`**（18 个 define_* 文件）：
- `array_object.rs` — 数组方法（11 个 `call_wasm_callback` 调用点）
- `proxy_reflect.rs` — Proxy/Reflect（12 个调用点）
- `typedarray_new_methods.rs` — TypedArray 方法（10 个调用点）
- `misc.rs` — native_call, queue_microtask, eval, jsx（2 个调用点）
- `core.rs` — 核心操作（obj_get, obj_set, typeof, instance_of...）（1 个调用点）
- `collections_buffers.rs` — Map/Set/WeakMap/WeakSet/ArrayBuffer/DataView/TypedArray/Date
- `promise.rs` — Promise create/then/catch/finally/resolve/reject
- `promise_combinators.rs` — Promise.all/race/allSettled/any
- `async_fn.rs` — async function start/resume/suspend
- `async_generator.rs` — async generator start/next/return/throw
- `primitive_core.rs` — BigInt/Symbol/RegExp/String match/replace/search/split
- `string_methods.rs` — String 方法
- `math_number_error.rs` — Math/Number/Error/Error 子类构造函数
- `object_builtins.rs` — Object.keys/values/entries/assign/create/defineProperty...
- `fetch.rs` — fetch/Headers/Request/Response
- `proxy_traps.rs` — Proxy trap get/set/delete
- `atomics.rs` — Atomics 方法
- `weakref_finalization.rs` — WeakRef/FinalizationRegistry
- `get_builtin_global_entry.rs` — get_builtin_global

**`crates/wjsm-runtime/src/`**（核心运行时）：
- `runtime_host_helpers.rs` — `call_wasm_callback`(删除), `reflect_get_impl_with_receiver` → async, `define_property_internal` → async, `proxy_or_target_*_impl` → async
- `runtime_values.rs` — `resolve_callable_and_call` → async, `read_object_property_by_name` → async
- `runtime_json.rs` — `call_wasm_callback` → `_async.await` (JSON.stringify toJSON, JSON.parse reviver)
- `runtime_builtins.rs` — `read_object_property_by_name` 调用点 → `.await`
- `runtime_heap.rs` — `read_object_property_by_name` 调用点 → `.await` 或 `_direct`
- `runtime_promises.rs` — `read_object_property_by_name` 调用点 → `.await`
- `runtime_render.rs` — `read_object_property_by_name` 调用点 → `_direct`
- `runtime_eval.rs` — `read_object_property_by_name` 调用点 → `.await`
- `runtime_async_fn.rs` — 无直接 `call_wasm_callback`，但被 async 回调调用
- `runtime_microtask.rs` — `drain_microtasks` / `call_host_function_with_args` → 确认 async 路径
- `lib.rs` — 删除 sync 路径，重命名 async 路径，`register_common_bridges` 转 async

**`crates/wjsm-runtime/src/scheduler.rs`**：
- 已是 async，仅需确认所有依赖函数签名对齐

**`crates/wjsm-cli/src/lib.rs`**（CLI 入口）：
- `run` / `eval` 子命令 → `tokio::runtime::Runtime::block_on()`

#### 3.2.2 级联影响文件

- `crates/wjsm-runtime/src/runtime_arguments.rs` — `define_host_data_property_from_caller` 调用（纯写，不变）
- `crates/wjsm-runtime/src/runtime_host_helpers.rs` — 内部多处 `read_object_property_by_name` 调用
- `tests/integration/` — fixture runner 可能需要 tokio 支持

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

- [ ] 所有现有测试通过（`cargo nextest run --workspace`）
- [ ] `Func::wrap` 在 `wjsm-runtime` 中零匹配
- [ ] `func.call(` 在 `wjsm-runtime` 中零匹配（仅 `func.call_async(`）
- [ ] getter/Proxy trap 在 async 路径正确触发
- [ ] 无 async store panic

### 8.2 代码质量

- [ ] 无 clippy 警告
- [ ] 代码格式化通过 `cargo fmt`
- [ ] 无 dead code（sync 路径完全清除）

### 8.3 Fixture 兼容性

- [ ] 所有 `.expected` 文件无需修改
- [ ] Proxy/Reflect/Getter 相关 fixture 输出一致

---

## 9. ADR 信号

| 信号 | 说明 |
|---|---|
| **执行模型单一化** | async-only，sync store 不再支持。影响：嵌入式用例需自行引入 tokio |
| **tokio 硬依赖** | `wjsm-runtime` 已依赖 tokio（`tokio::sync`, `tokio::time`），`wjsm-cli` 新增 `tokio::runtime` |
| **公共 API 变更** | `execute_with_writer` 从 sync → async。库消费者需 `.await` 或 `block_on` |
| **Phase 3 twin helpers 清理** | `drain_microtasks` / `call_host_function_with_args` 等 sync 版本将被删除，仅保留 `_async` 版本 |

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

**文档版本**: v1.0  
**最后更新**: 2026-06-02  
**Spec 自审**: 已完成（修正文件路径、类型签名、API 名称、补充非目标/ADR/级联分析）