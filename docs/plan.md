# Host Builtins 完整解耦执行方案

## 目标

将 `wjsm-host-wasm/src/host_imports/` 的 43 个文件（24,113 行）从 wasmtime 强耦合中彻底解耦，使其成为后端无关的 builtin 实现，为多后端（wasmtime / native / cranelift）做准备。

**核心约束**：零性能损失（泛型单态化 `<E: ExecContext>` 代替 `dyn ExecContext`）。

## 现状

```
wjsm-host (trait 层, ~15 方法)          wjsm-host-wasm (24K 行, 43 文件)
├── HeapContext                         ├── WasmHeapContext (impl HeapContext) ← 仅此处实现 trait
├── ConsoleHost/ObjectHost/GcHost       ├── host_imports/  ← 全部直接用 Caller<RuntimeState>
├── AsyncHost                           │   ├── 43 个文件直接引用 Caller<'_, RuntimeState>
└── HostRuntime (组合 marker)           │   ├── ~34 处 call_wasm_callback_async (再入回调)
                                        │   ├── ~50 处 caller.data().xxx (RuntimeState 字段)
                                        │   └── 18 个文件使用 NativeCallable (内部调度枚举)
                                        └── runtime_host_helpers/ (回调调度, shadow stack)
```

**问题**：`HeapContext` 只覆盖了基本堆读写（15 方法），但 builtins 还需要：
- 再入回调（`call_wasm_callback_async`）— 核心瓶颈
- RuntimeState 状态访问（proxy_table / closure_table / bound_table / enumerators / error_table 等 30+ 字段）
- 字符串存储与读取（`store_runtime_string` / `read_value_string_bytes`）
- 属性键系统（`canonicalize_v2_name_id` / `intern_runtime_property_key`）
- 属性读写助手（`define_host_data_property` / `get_by_name_id_sync`）
- 异常/错误处理（`set_runtime_error` / `make_type_error_exception`）
- Promise 操作（`alloc_promise` / `settle_promise` / `resolve_promise`）
- 对象分配（`alloc_host_object`）
- NativeCallable 调度（`create_native_callable` / `push_native_callable`）

**关键发现**：43 个 host_imports 文件中，**34 个**（79%）使用共享 helper（alloc_host_object / define_host_data_property / store_runtime_string / settle_promise 等），**18 个**（42%）使用 NativeCallable。只有 11 个文件完全不使用共享 helper。这意味着解耦后需要移动的不只是 builtin 逻辑，还有一整套共享 helper 函数。

## 方案

### 架构

```
wjsm-ir (零依赖, NaN-boxing 编解码, wk_symbol, 常量)
  ↑
wjsm-host (trait 层: ExecContext + 共享类型, 不依赖 wasmtime)
  ↑
wjsm-builtins (后端无关 builtin 实现, ~24K 行, 依赖 wjsm-host + wjsm-ir)
  ↑
wjsm-host-wasm (wasmtime 后端: WasmExecContext + 薄注册层, 依赖 wjsm-builtins)
```

**新建 `wjsm-builtins` crate**：存放从 host-wasm 迁出的 builtin 实现逻辑。保持 `wjsm-host` 纯净（只有 trait 定义和共享类型）。

### 零性能损失原理：泛型单态化

```rust
// wjsm-builtins (共享, 后端无关)
pub async fn array_map_impl<E: ExecContext>(ctx: &mut E, arr: Handle, cb: Value, this: Value) -> Value {
    let len = ctx.array_read_length(arr);
    let result = ctx.alloc_array(len);
    for i in 0..len {
        let elem = ctx.array_elem(arr, i);                              // 编译期内联
        let mapped = ctx.call_js_async(cb, this, &[elem, encode_f64(i as f64)]).await;
        ctx.array_write_elem(result, i, mapped);                       // 编译期内联
    }
    result
}

// wjsm-host-wasm (wasmtime 注册层 — 薄包装)
linker.func_wrap_async("env", "array_map", |mut caller: Caller<RuntimeState>, ...| {
    Box::pin(async move {
        let mut ctx = WasmExecContext::new(&mut caller);               // 零成本：仅包裹 &mut Caller
        array_map_impl(&mut ctx, arr, cb, this).await                  // 单态化, 全内联
    })
});
```

- wasmtime 后端用 `WasmExecContext` 实例化 → 编译器内联所有 trait 方法 → **零 vtable 开销**
- `Box::pin` 分配成本与当前一致（现有代码已经在用 `Box::pin`）
- `WasmExecContext::new` 是零成本构造（仅存一个 `&mut Caller` 引用 + 提取一次 `WasmEnv`）
- 未来 native 后端用 `NativeExecContext` 实例化，同样零 vtable

### 性能对比

| 操作 | 当前 | 解耦后 (泛型) | 解耦后 (dyn) |
|------|------|-------------|-------------|
| 堆读写 | 直接方法调用 | **0%** (内联) | +2-3ns/次 (vtable) |
| 再入回调 | ~300ns (wasmtime 主导) | **0%** (内联) | +2-3ns/次 |
| WasmEnv 提取 | 每操作一次 | **减少** (每 context 一次) | 同当前 |
| Box::pin | 已存在 | 不变 | 不变 |

以 `Array(1000).map(x => x+1)` 为例：wasmtime 调用 ~332μs，泛型解耦额外 0ns，dyn 解耦额外 ~9μs（2.7%）。

**选泛型方案**。

## ExecContext Trait 设计

`ExecContext` 是 `HeapContext` 的超集，覆盖 builtins 所需的全部操作。方法按能力域分组，注释标注来源。

```rust
// wjsm-host/src/exec_context.rs

use crate::{Handle, Value};
use std::future::Future;
use std::pin::Pin;

/// 异步回调返回类型（BoxFuture）。
pub type ExecFuture<'a> = Pin<Box<dyn Future<Output = anyhow::Result<Value>> + Send + 'a>>;

// ── 共享类型（后端无关）──

/// Proxy 条目。
pub struct ProxyEntry {
    pub target: Value,
    pub handler: Value,
}

/// Closure 条目。
pub struct ClosureEntry {
    pub func_idx: u32,
    pub env_obj: Value,
}

/// Bound function 条目。
pub struct BoundEntry {
    pub target_func: Value,
    pub bound_this: Value,
    pub bound_args: Vec<Value>,
}

/// Promise 结算状态。
pub enum PromiseSettlement {
    Fulfill(Value),
    Reject(Value),
}

// ── ExecContext trait ──

/// 后端无关执行上下文：覆盖 host builtins 所需的全部操作。
///
/// 各后端用自身运行时上下文实现本 trait。wasmtime 后端用 `WasmExecContext`，
/// native 后端用 `NativeExecContext`。builtins 代码以 `<E: ExecContext>` 泛型实例化，
/// 编译期单态化，零 vtable 开销。
pub trait ExecContext: crate::heap_context::HeapContext {
    // ═══ 对象分配 ═══

    /// 分配一个空对象，返回 NaN-boxed 值。
    fn alloc_object(&mut self, capacity: u32) -> Value;
    /// 分配一个数组，返回 NaN-boxed 值。
    fn alloc_array(&mut self, capacity: u32) -> Value;

    // ═══ 字符串存储与读取 ═══

    /// 存储字符串，返回 NaN-boxed 值。
    fn store_string(&mut self, s: &str) -> Value;
    /// 存储拥有所有权的字符串。
    fn store_string_owned(&mut self, s: String) -> Value;
    /// 读取字符串原始字节。
    fn read_string_bytes(&mut self, val: Value) -> Option<Vec<u8>>;
    /// 读取字符串为 lossy UTF-8。
    fn read_string_utf8_lossy(&mut self, val: Value) -> String;

    // ═══ 属性键系统 ═══

    /// 将 name_id 规范化为 V2 property key（可能需读线性内存）。
    fn canonicalize_name_id(&mut self, name_id: u32) -> Option<u32>;
    /// Intern 字符串为 property key，返回 name_id。
    fn intern_property_key(&mut self, s: &str) -> u32;
    /// 从 name_id 反查字符串（仅 RuntimeString 路径）。
    fn property_key_string(&mut self, name_id: u32) -> Option<String>;
    /// 判断 name_id 是否匹配期望字符串。
    fn name_id_matches(&mut self, name_id: u32, expected: &str) -> bool;
    /// 将属性值转为 name_id（可能需读内存/查表）。
    fn property_value_to_name_id(&mut self, prop: Value, allow_symbol: bool) -> Option<u32>;

    // ═══ 再入回调 ═══

    /// 同步调用 JS 函数（后端自行决定桥接策略）。
    fn call_js(&mut self, func: Value, this: Value, args: &[Value]) -> anyhow::Result<Value>;
    /// 异步调用 JS 函数。
    fn call_js_async<'a>(&'a mut self, func: Value, this: Value, args: &'a [Value]) -> ExecFuture<'a>;
    /// 判断值是否可调用（含 Proxy apply trap 链）。
    fn is_callable(&mut self, val: Value) -> bool;

    // ═══ 状态表访问 ═══

    /// 查询 Proxy 条目。
    fn proxy_entry(&mut self, proxy: Handle) -> Option<ProxyEntry>;
    /// 查询 Closure 条目。
    fn closure_entry(&mut self, handle: Handle) -> Option<ClosureEntry>;
    /// 查询 Bound function 条目。
    fn bound_entry(&mut self, handle: Handle) -> Option<BoundEntry>;
    /// 分派 NativeCallable（Promise resolving / stream 等）。
    fn dispatch_native_callable(&mut self, idx: u32, this: Value, args: &[Value]) -> Option<Value>;

    // ═══ 枚举器 ═══

    /// 从值创建枚举器，返回 NaN-boxed 枚举器值。
    fn create_enumerator(&mut self, val: Value) -> Value;
    /// 推进枚举器到下一个。
    fn enumerator_advance(&mut self, handle: Handle);
    /// 获取当前键。
    fn enumerator_key(&mut self, handle: Handle) -> Value;
    /// 是否已完成。
    fn enumerator_done(&mut self, handle: Handle) -> bool;

    // ═══ 异常与错误 ═══

    /// 抛出异常值（存入 error_table + 输出 + 设置 runtime_error）。
    fn throw_exception(&mut self, val: Value);
    /// 设置最近错误消息。
    fn set_last_error(&mut self, msg: String);
    /// 取出最近错误消息。
    fn take_last_error(&mut self) -> Option<String>;
    /// 创建 TypeError 异常值。
    fn make_type_error(&mut self, msg: &str) -> Value;

    // ═══ 属性助手（高层）═══

    /// 定义数据属性（按字符串键）。
    fn define_data_property(&mut self, obj: Value, key: &str, value: Value);
    /// 按 name_id 读取属性值。
    fn get_property_by_name_id(&mut self, obj: Value, name_id: u32) -> Value;
    /// 按 name_id 获取方法（含 callable 检查）。
    fn get_method_by_name_id(&mut self, obj: Value, name_id: u32) -> anyhow::Result<Option<Value>>;

    // ═══ 数组助手 ═══

    /// 写数组元素（含 handle 解析）。
    fn array_write_elem(&mut self, arr: Value, index: u32, value: Value);
    /// 读数组长度（值级，含 handle 解析）。
    fn array_read_length(&mut self, arr: Value) -> Option<u32>;
    /// 写数组长度。
    fn array_write_length(&mut self, arr: Value, len: u32);

    // ═══ 值渲染 ═══

    /// 将值渲染为显示字符串。
    fn render_value(&mut self, val: Value) -> String;

    // ═══ Promise ═══

    /// 分配新 Promise。
    fn alloc_promise(&mut self) -> Value;
    /// 结算 Promise。
    fn settle_promise(&mut self, promise: Value, settlement: PromiseSettlement);
    /// 解析 Promise（可能触发 thenable 链）。
    fn resolve_promise(&mut self, promise: Value, value: Value);

    // ═══ GC ═══

    /// GC safepoint poll（分配计数器检查 + 可能触发回收）。
    fn gc_safepoint_poll(&mut self);
    /// GC 屏障缓冲区 flush。
    fn gc_barrier_flush(&mut self);

    // ═══ 原型链 ═══

    /// 获取对象原型 handle。
    fn prototype_of(&mut self, handle: Handle) -> Option<Handle>;
    /// 获取 RegExp 原型值（惰性初始化）。
    fn regexp_prototype(&mut self) -> Value;

    // ═══ Handle 解析 ═══

    /// 解析值为 handle 索引（含 function_props_base 重定位）。
    fn resolve_handle_idx(&mut self, val: Value) -> Option<usize>;

    // ═══ 进程 ═══

    /// 检查是否有待处理的进程退出信号。
    fn pending_exit_signal(&mut self) -> Option<i32>;
}
```

**方法统计**：约 37 个新方法 + HeapContext 现有 15 个 = 共 ~52 个。纯函数（`encode_symbol_name_id` 等）不移入 trait，保留在 `wjsm-host` 或 `wjsm-ir` 作为自由函数。

### WasmExecContext 实现

```rust
// wjsm-host-wasm/src/exec_context_impl.rs

/// wasmtime 后端的 ExecContext 实现。
///
/// 零成本构造：仅持有 &mut Caller 引用 + 一次 WasmEnv 提取。
/// 所有方法直接委托到现有 host-wasm 代码（heap_access_v2, runtime_host_helpers 等）。
pub(crate) struct WasmExecContext<'a, 'b> {
    caller: &'a mut Caller<'b, RuntimeState>,
    env: WasmEnv,  // 构造时提取一次, 后续方法复用
}

impl<'a, 'b> WasmExecContext<'a, 'b> {
    pub(crate) fn new(caller: &'a mut Caller<'b, RuntimeState>) -> Self {
        let env = WasmEnv::from_caller(caller).unwrap_or_default();
        Self { caller, env }
    }
}

impl<'a, 'b> HeapContext for WasmExecContext<'a, 'b> {
    // 委托到现有 WasmHeapContext 逻辑（可直接复用或内联）
    // ...
}

impl<'a, 'b> ExecContext for WasmExecContext<'a, 'b> {
    fn store_string(&mut self, s: &str) -> Value {
        crate::runtime_render::store_runtime_string(self.caller, s.to_string())
    }

    fn call_js_async<'c>(&'c mut self, func: Value, this: Value, args: &'c [Value]) -> ExecFuture<'c> {
        Box::pin(async move {
            crate::call_wasm_callback_async(self.caller, func, this, args).await
        })
    }

    // ... 其余方法同理委托
}
```

### 注册层模式（host-wasm 保留）

每个 `define_*` 函数保留在 host-wasm，但闭包体缩减为一行委托：

```rust
// BEFORE (24K 行逻辑内联在闭包中):
pub(crate) fn define_core(linker, store) -> Result<()> {
    linker.define(&mut store, "env", "console_log", Func::wrap(&mut store,
        |mut caller: Caller<RuntimeState>, args_base: i32, args_count: i32| {
            write_console_values(&mut caller, args_base, args_count, None);  // ← 100 行逻辑
        }))?;
    // ... 200+ 行
}

// AFTER (闭包变成一行):
pub(crate) fn define_core(linker, store) -> Result<()> {
    linker.define(&mut store, "env", "console_log", Func::wrap(&mut store,
        |mut caller: Caller<RuntimeState>, args_base: i32, args_count: i32| {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::core::console_log(&mut ctx, args_base, args_count);
        }))?;
    // ... 同理
}
```

## 纯函数迁移

以下函数不依赖 `Caller` / `RuntimeState`，可直接移到 `wjsm-host` 或 `wjsm-builtins` 作为自由函数：

| 函数 | 当前位置 | 迁移目标 | 说明 |
|------|---------|---------|------|
| `encode_symbol_name_id` | property_key.rs | wjsm-host | 纯位运算 |
| `encode_runtime_string_name_id` | property_key.rs | wjsm-host | 纯位运算 |
| `name_id_to_property_key_value` | property_key.rs | wjsm-host | 纯位运算 |
| `decode_name_id` | property_key.rs | wjsm-host | 纯枚举 |
| `symbol_value_to_name_id` | property_key.rs | wjsm-host | 纯查表 |
| `is_symbol_name_id` | property_key.rs | wjsm-host | 纯判断 |
| `RuntimeString::from_utf8_str` | runtime_string.rs | wjsm-host | 纯构造 |
| `string_iter_advance_unit_pos` | core.rs | wjsm-builtins | 纯计算 |
| `format_number_js` | math_number_error.rs | wjsm-builtins | 纯格式化 |
| `format_number_to_fixed_js` | math_number_error.rs | wjsm-builtins | 纯格式化 |
| `format_number_to_exponential_js` | math_number_error.rs | wjsm-builtins | 纯格式化 |
| `format_number_to_precision_js` | math_number_error.rs | wjsm-builtins | 纯格式化 |
| `number_proto_to_string_radix` | math_number_error.rs | wjsm-builtins | 纯计算 |
| `js_string_content_to_f64` | runtime_string_to_number.rs | wjsm-builtins | 纯解析 |
| `parse_date_string` | collections_buffers.rs | wjsm-builtins | 纯解析 |
| `ms_to_datetime_local` | collections_buffers.rs | wjsm-builtins | 纯格式化 |

## 迁移顺序

按依赖深度和复杂度排序，从简到难。每个文件迁移后立即验证对应 fixtures 通过。

### Phase 0：基础设施（1-2 天）

- [ ] 创建 `wjsm-builtins` crate（Cargo.toml, 依赖 `wjsm-host` + `wjsm-ir`）
- [ ] 在 `wjsm-host` 中定义 `ExecContext` trait + 共享类型（ProxyEntry, ClosureEntry, BoundEntry, PromiseSettlement）
- [ ] 在 `wjsm-host-wasm` 中实现 `WasmExecContext`（委托到现有代码）
- [ ] 验证：`cargo build` 通过，现有测试全绿（此阶段无逻辑迁移）
- [ ] 迁移纯函数到 `wjsm-host` / `wjsm-builtins`
- [ ] 验证：`cargo nextest run --workspace` 全绿

### Phase 1：低耦合 builtin（2-3 天）

**特征**：不使用共享 helper 函数（alloc_host_object / define_host_data_property / store_runtime_string / settle_promise 等），不需要 ExecContext 扩展方法。

| 文件 | 行数 | 回调 | NativeCallable | 共享 helper | 实际耦合点 |
|------|------|------|---------------|------------|-----------|
| `math_number_error.rs` | 1303 | 0 | 2 | 13 | store_runtime_string, NativeCallable |
| `primitive_core.rs` | 598 | 0 | 0 | 1 | store_runtime_string |
| `get_builtin_global_entry.rs` | 105 | 0 | 39 | 0 | NativeCallable (全局对象构造) |
| `inspector_host.rs` | 132 | 0 | 0 | 0 | caller.data().inspector |
| `generator.rs` | 75 | 0 | 0 | 0 | caller.data().generator_prototype |
| `async_fn.rs` | 363 | 0 | 0 | 0 | WasmEnv::from_caller |
| `async_generator.rs` | 278 | 0 | 0 | 0 | 无（同 generator） |
| `object_builtins_async.rs` | 125 | 0 | 0 | 0 | 无 |

- [ ] 逐文件迁移：提取闭包体为 `pub fn xxx<E: ExecContext>(ctx: &mut E, ...)` / `pub async fn xxx<E>(ctx, ...)`
- [ ] 每文件迁移后：`cargo build` + 对应 `happy` / `errors` fixtures 测试
- [ ] Phase 1 结束验证：`cargo nextest run --workspace` 全绿

### Phase 2：含回调 builtin（3-4 天）

**特征**：调用 `call_wasm_callback_async` 或 `block_in_place` + `rt.block_on`，但不涉及 Proxy trap。

| 文件 | 行数 | 回调 | NativeCallable | 共享 helper | 实际耦合点 |
|------|------|------|---------------|------------|-----------|
| `string_methods.rs` | 842 | 1 | 1 | 24 | replace callback, store_runtime_string, NativeCallable |
| `object_builtins.rs` | 695 | 0 | 0 | 2 | defineProperty getter/setter |
| `timers_arrays.rs` | 253 | 0 | 0 | 1 | store_runtime_string |
| `weakref_finalization.rs` | 326 | 0 | 3 | 5 | FinalizationRegistry, NativeCallable |
| `core_async.rs` | 712 | 1 | 1 | 0 | op_in / instanceof async, NativeCallable |
| `misc.rs` | 142 | 0 | 0 | 1 | store_runtime_string |
| `private_fields.rs` | 247 | 4 | 0 | 0 | block_in_place + rt.block_on |
| `reentrant/string.rs` | ~170 | 1 | 0 | 11 | replace callback, store_runtime_string |

- [x] 同 Phase 1 模式迁移，回调改用 `ctx.call_js_async()` / `ctx.call_js()`
- [x] `block_in_place` + `rt.block_on` 路径改用 `ctx.call_js()`（后端自行桥接）
- [x] Phase 2 真迁移：算法在 `wjsm-builtins`，host_imports 仅薄注册；ExecContext 不含 Phase2 高阶 API
- [x] Phase 2 结束验证：全绿

### Phase 3：再入型 builtin — 数组/TypedArray（3-4 天）

**特征**：高频再入回调（map/filter/reduce/sort/find 等），部分使用 NativeCallable。

| 文件 | 行数 | 回调 | NativeCallable | 共享 helper | 实际耦合点 |
|------|------|------|---------------|------------|-----------|
| `reentrant/array.rs` | ~750 | 13 | 0 | 0 | call_wasm_callback_async |
| `reentrant/typedarray.rs` | ~530 | 10 | 0 | 0 | call_wasm_callback_async |
| `typedarray_new_methods.rs` | 974 | 0 | 0 | 4 | store_runtime_string |
| `array_object.rs` | 2668 | 3 | 2 | 8 | Array.from / concat / flatMap, NativeCallable |
| `collections_buffers.rs` | 2513 | 0 | 59 | 15 | Map/Set/WeakMap + Date, NativeCallable |

- [ ] 先迁移 `reentrant/array.rs`（最密集回调，验证 `call_js_async` 泛型路径正确）
- [ ] 跑 `Array.prototype` 相关 fixtures：`cargo nextest run -E 'test(happy__array)'`
- [ ] 逐文件迁移其余
- [ ] Phase 3 结束验证：全绿 + benchmark 对比（见下）

### Phase 4：Proxy / Reflect（2-3 天）

**特征**：Proxy trap 全链，最深耦合。

| 文件 | 行数 | 回调 | NativeCallable | proxy_table | 实际耦合点 |
|------|------|------|---------------|-------------|-----------|
| `proxy_traps.rs` | 113 | 0 | 0 | 2 | proxy_table |
| `proxy_reflect.rs` | 1731 | 4 | 1 | 6 | call_wasm_callback_async, proxy_table, NativeCallable |
| `proxy_reflect_async.rs` | 656 | 9 | 0 | 10 | call_wasm_callback_async, proxy_table |
| `reentrant/proxy.rs` | ~300 | 0 | 0 | 0 | resolve_handle, WasmEnv::from_caller |
| `reentrant/mod.rs` | ~500 | 2 | 0 | 1 | call_wasm_callback_async, proxy_table |

- [ ] 先迁移 `proxy_traps.rs`（基础 trap 解析）
- [ ] 再迁移 `proxy_reflect.rs`（核心 Reflect 实现）
- [ ] 最后迁移 async overrides
- [ ] Phase 4 结束验证：`cargo nextest run -E 'test(happy__proxy)'` + `test(happy__reflect)` 全绿

### Phase 5：Promise / Streams / Fetch / Modules / Atomics / Core（3-4 天）

**特征**：混合耦合度，部分使用 NativeCallable 和共享 helper。

| 文件 | 行数 | 回调 | NativeCallable | 共享 helper | 实际耦合点 |
|------|------|------|---------------|------------|-----------|
| `promise.rs` | 524 | 0 | 0 | 2 | alloc_promise, settle_promise |
| `promise_combinators.rs` | 497 | 0 | 0 | 0 | alloc_promise, settle_promise |
| `streams/mod.rs` | ~300 | 0 | 13 | 4 | NativeCallable, alloc_host_object |
| `streams/readable.rs` | ~100 | 1 | 0 | 0 | call_wasm_callback_async |
| `streams/readable_dispatch.rs` | ~200 | 0 | 5 | 3 | NativeCallable, alloc_host_object |
| `streams/readable_pipe.rs` | ~300 | 0 | 3 | 3 | NativeCallable, alloc_host_object |
| `streams/writable.rs` | 1262 | 1 | 14 | 4 | call_wasm_callback_async, NativeCallable, alloc_host_object |
| `streams/transform.rs` | 606 | 0 | 8 | 2 | NativeCallable, alloc_host_object |
| `streams/queuing.rs` | 85 | 0 | 1 | 1 | NativeCallable, store_runtime_string |
| `streams/fetch_body.rs` | 586 | 0 | 0 | 1 | store_runtime_string |
| `fetch/mod.rs` | 283 | 0 | 0 | 0 | alloc_promise, settle_promise |
| `fetch/http.rs` | 261 | 0 | 0 | 0 | alloc_promise, settle_promise |
| `fetch/core/mod.rs` | ~400 | 0 | 3 | 13 | NativeCallable, alloc_host_object, store_runtime_string |
| `fetch/core/impl.rs` | ~800 | 0 | 0 | 14 | alloc_host_object, store_runtime_string |
| `fetch/core/resource_timing.rs` | ~100 | 0 | 0 | 0 | 无 |
| `modules.rs` | 729 | 0 | 5 | 6 | NativeCallable, store_runtime_string |
| `atomics.rs` | 1295 | 0 | 0 | 9 | alloc_host_object, store_runtime_string |
| `gc.rs` | 724 | 4 | 3 | 0 | call_wasm_callback_async, NativeCallable |
| `get_method.rs` | 437 | 8 | 3 | 2 | block_in_place + rt.block_on, NativeCallable |
| `core.rs` | 1841 | 1 | 0 | 10 | console, typeof, instanceof, eq, store_runtime_string |
| `reentrant/string.rs` | ~170 | 1 | 0 | 11 | replace callback, store_runtime_string |

- [ ] `get_method.rs` 被 Phase 1-3 文件依赖（`get_by_name_id_sync` 被 array_object.rs / core.rs 使用），需提前迁移
- [ ] `core.rs` 是最关键文件（console + typeof + instanceof + eq + 枚举器），最后迁移
- [ ] Phase 5 结束验证：全绿

### Phase 6：收尾与验证（1-2 天）

- [ ] 迁移 `host_imports/mod.rs` 的 re-export
- [ ] 清理 `wjsm-host-wasm/src/host_imports/` 中已迁移的文件（删除或保留薄注册层）
- [ ] 验证 `WasmHeapContext`（旧 HeapContext impl）是否可合并到 `WasmExecContext`
- [ ] 更新 workspace Cargo.toml 依赖关系
- [ ] 更新 AGENTS.md crate 表格
- [ ] 编写 ADR 0012

## 测试策略

### 每文件迁移后

```bash
# 对应 fixtures
cargo nextest run -E 'test(happy__<name>)'
cargo nextest run -E 'test(errors__<name>)'
```

### 每 Phase 结束

```bash
# 全量测试
cargo nextest run --workspace

# 语义快照
cargo nextest run -p wjsm-semantic
```

### Phase 3 结束后（性能验证）

```bash
# Benchmark：对比迁移前后 steady-state 执行时间
cargo run -p wjsm-gc-bench -- run --scenario iteration --gc mark-sweep
cargo run -p wjsm-gc-bench -- run --scenario iteration --gc zgc

# 手工对比：高频回调场景
cargo run -- run -e 'const a = Array(10000).fill(0).map((_, i) => i * 2); console.log(a.reduce((s, x) => s + x, 0));'
# 迁移前后用 time 对比
```

### Phase 6 最终验证

```bash
# 全量
cargo nextest run --workspace

# 启动快照兼容性
WJSM_STARTUP_SNAPSHOT=1 cargo nextest run -E 'test(startup_snapshot)'

# GC 矩阵
WJSM_TEST_GC=mark-sweep cargo nextest run --workspace
WJSM_TEST_GC=g1 cargo nextest run --workspace
WJSM_TEST_GC=zgc cargo nextest run --workspace
```

## 风险与缓解

### 风险 1：ExecContext trait 方法遗漏

**影响**：迁移某文件时发现 trait 缺方法，需回头补 trait + 重新实现。

**缓解**：Phase 0 实现后，先做 2-3 个文件的试迁移（math_number_error + string_methods），暴露 trait 缺口后再批量迁移。trait 设计是迭代的，不是一次定稿。

### 风险 2：async 泛型 Future 类型膨胀

**影响**：`async fn xxx<E: ExecContext>` 对每个 E 生成不同 Future 类型，可能增加编译时间。

**缓解**：
- 只有 wasmtime 一个后端，只生成一份
- `Func::wrap_async` 闭包返回 `Box<dyn Future>`，Future 类型不逃逸到函数签名
- 编译时间增加预计 < 10%，可接受

### 风险 3：sync→async 桥接语义差异

**影响**：当前 `block_in_place` + `rt.block_on` 是 tokio 专属。抽象为 `ctx.call_js()` 后，后端实现可能语义不一致。

**缓解**：
- `WasmExecContext::call_js` 保持完全相同的 `block_in_place` + `block_on` 路径
- native 后端不需要桥接（直接函数指针调用），语义天然一致
- 在 trait 文档中明确约定：`call_js` = `call_js_async` 的同步阻塞等价物

### 风险 4：property_key 系统深度耦合

**影响**：property_key 模块同时有纯函数和需上下文的函数，迁移边界不清晰。

**缓解**：
- 纯函数（encode/decode/name_id_to_property_key_value）移到 `wjsm-host` 作自由函数
- 需上下文的（canonicalize / intern / lookup）作为 `ExecContext` 方法
- 共享类型（`DecodedNameId`）移到 `wjsm-host`

### 风险 5：RuntimeString 类型耦合

**影响**：`RuntimeString` 是 wasmtime 后端的字符串表示，被 `property_key` 和 `core.rs` 大量使用。

**缓解**：
- `RuntimeString::from_utf8_str` 是纯构造，移到 `wjsm-host`
- 需读内存的 `RuntimeString::code_unit_at` 等改为 `ExecContext` 方法
- builtins 中尽量用 `&str` / `String` 代替 `RuntimeString`，通过 `ctx.store_string(s)` / `ctx.read_string_utf8_lossy(val)` 桥接

### 风险 6：共享 helper 函数被 runtime_* 文件广泛使用

**影响**：`alloc_host_object` / `define_host_data_property` / `store_runtime_string` 等 helper 被 32 个 `runtime_*.rs` 文件使用（runtime_heap.rs / runtime_builtins.rs / runtime_promises.rs / runtime_startup.rs 等）。如果将这些 helper 移到 wjsm-builtins，会导致循环依赖。

**缓解**：
- 共享 helper **留在 host-wasm**，`WasmExecContext` 内部委托到现有实现
- builtins 通过 `ctx.alloc_object()` / `ctx.define_data_property()` / `ctx.store_string()` 等 trait 方法间接调用
- 这样既避免循环依赖，又保持泛型单态化的内联优势
- 32 个 runtime_* 文件不受影响，继续使用现有 helper

### 风险 7：NativeCallable 类型系统跨 crate 可见性

**影响**：`NativeCallable` 枚举有 ~150 个变体，被 18 个 host_imports 文件和 15 个 runtime_* 文件使用。将其移到 wjsm-builtins 会导致 runtime_* 文件也需要依赖 wjsm-builtins，同样形成循环依赖。

**缓解**：
- `NativeCallable` **留在 host-wasm**（types.rs 不动）
- ExecContext 新增 `fn create_native_callable(&mut self, callable: &NativeCallable) -> Value` 方法
- `NativeCallable` 类型通过 wjsm-host re-export 或在 wjsm-host 中定义
- builtins 通过 trait 方法创建 NativeCallable，不直接操作 types.rs

### 风险 8：get_method.rs 被 Phase 1-3 文件依赖

**影响**：`get_by_name_id_sync` 被 array_object.rs (Phase 3) 和 core.rs (Phase 5) 使用。如果 Phase 3 先迁移 array_object.rs，而 get_method.rs 还在 host-wasm，会产生跨 crate 调用。

**缓解**：
- `get_method.rs` **提前到 Phase 1**（与 math_number_error.rs 同级）
- 或者：ExecContext 新增 `fn get_property_by_name_id(&mut self, obj: Value, name_id: u32) -> Value` 方法，array_object.rs 通过 trait 调用而非直接引用

### 风险 9：proxy_table 被 10 个 host_imports 文件使用

**影响**：`proxy_table` 是 RuntimeState 中的 `Mutex<Vec<ProxyEntry>>`，被 core.rs / core_async.rs / gc.rs / get_method.rs / modules.rs / proxy_reflect.rs / proxy_reflect_async.rs / proxy_traps.rs / reentrant_async/mod.rs 共 10 个文件直接访问。这些文件需要 ExecContext 提供 proxy 查询方法。

**缓解**：
- ExecContext 已有 `fn proxy_entry(&mut self, proxy: Handle) -> Option<ProxyEntry>` 方法
- `ProxyEntry` 类型已定义为共享类型（wjsm-host）
- 所有 proxy_table 访问改为 `ctx.proxy_entry(handle)`
- 新增 proxy 条目（`table.push(...)`）通过 `fn create_proxy(&mut self, target: Value, handler: Value) -> Value` 方法

## 依赖关系调整

### Cargo.toml 变更

```toml
# 新建 crates/wjsm-builtins/Cargo.toml
[package]
name = "wjsm-builtins"
version.workspace = true
edition.workspace = true

[dependencies]
wjsm-ir = { path = "../wjsm-ir" }
wjsm-host = { path = "../wjsm-host" }
anyhow = { workspace = true }

# wjsm-host/Cargo.toml — 新增 ExecContext 相关
[dependencies]
wjsm-ir = { path = "../wjsm-ir" }
anyhow = { workspace = true }

# wjsm-host-wasm/Cargo.toml — 新增依赖
[dependencies]
wjsm-builtins = { path = "../wjsm-builtins" }  # 新增
# 其余不变

# Cargo.toml (workspace)
[workspace.dependencies]
# 已有不变
```

### 依赖图（迁移后）

```
wjsm-ir (零依赖)
  ↑
wjsm-host (ExecContext trait + 共享类型 + 纯函数)
  ↑
wjsm-builtins (builtin 实现, ~24K 行)
  ↑                    ↑
wjsm-host-wasm    wjsm-module
  ↑
wjsm-cli
```

## 预期最终状态

```
wjsm-host/src/
├── exec_context.rs       # ExecContext trait (~50 方法)
├── heap_context.rs       # HeapContext trait (现有, 不变)
├── console_host.rs       # ConsoleHost (现有, 不变)
├── object_host.rs        # ObjectHost (现有, 不变)
├── gc_host.rs            # GcHost (现有, 不变)
├── async_host.rs         # AsyncHost (现有, 不变)
├── runtime_trait.rs      # HostRuntime (现有, 不变)
├── property_key.rs       # 纯属性键函数 (从 host-wasm 迁入)
├── runtime_string.rs     # RuntimeString 纯部分 (从 host-wasm 迁入)
└── lib.rs

wjsm-builtins/src/
├── core.rs               # console + typeof + eq + instanceof + 枚举器 + GC safepoint
├── math_number_error.rs  # Math + Number + Error
├── string_methods.rs     # String.prototype
├── array_object.rs       # Array.from / concat / flatMap 等
├── object_builtins.rs    # Object.keys / values / entries / defineProperty 等
├── object_builtins_async.rs
├── proxy_reflect.rs      # Reflect + Proxy trap 逻辑
├── proxy_reflect_async.rs
├── proxy_traps.rs        # Proxy trap 解析
├── promise.rs            # Promise 逻辑
├── promise_combinators.rs
├── streams/              # WHATWG Streams
│   ├── mod.rs
│   ├── readable.rs       # ReadableStream (含 ctrl / dispatch / pipe)
│   ├── writable.rs       # WritableStream
│   ├── transform.rs      # TransformStream
│   ├── queuing.rs        # QueuingStrategy
│   └── fetch_body.rs     # fetch body stream
├── fetch/                # Fetch API
│   ├── mod.rs            # fetch 入口
│   ├── http.rs           # HTTP 客户端
│   └── core/             # Request / Response / Headers
│       ├── mod.rs
│       ├── impl.rs
│       └── resource_timing.rs
├── modules.rs            # 动态导入 / CJS require
├── atomics.rs            # SharedArrayBuffer + Atomics
├── gc.rs                 # GC 辅助函数
├── collections_buffers.rs # Map / Set / WeakMap / Date
├── primitive_core.rs     # Symbol / BigInt
├── private_fields.rs     # 私有字段
├── generator.rs          # Generator
├── async_fn.rs           # AsyncFunction
├── async_generator.rs    # AsyncGenerator
├── inspector.rs          # Inspector 暂停点
├── timers.rs             # setTimeout / setInterval
├── weakref_finalization.rs
├── get_method.rs         # GetMethod 核心
├── number_format.rs      # 纯数字格式化函数
├── reentrant/            # 再入型 builtins（高频回调）
│   ├── mod.rs            # 公共逻辑（原 reentrant_async/mod.rs）
│   ├── array.rs          # reentrant_array_async
│   ├── typedarray.rs     # reentrant_typedarray_async
│   ├── string.rs         # reentrant_string_async
│   └── proxy.rs          # reentrant_proxy_async
└── lib.rs

wjsm-host-wasm/src/
├── exec_context_impl.rs  # WasmExecContext (impl ExecContext) ← 新增
│   # 内部委托到现有 helper（alloc_host_object, store_runtime_string, settle_promise 等）
│   # 这些 helper 留在 host-wasm，避免循环依赖
├── heap_context_impl.rs  # WasmHeapContext (impl HeapContext) ← 可能合并到上者
├── host_imports/         # 薄注册层 (define_* 函数, 每个闭包一行委托)
├── runtime_host_helpers/ # 回调调度 (call_wasm_callback_async 等, 不变)
├── types.rs              # NativeCallable 枚举 (不变, 留在 host-wasm)
└── ...                   # 其余 runtime_*.rs 不变
```

## 工期估算

| Phase | 内容 | 估算 |
|-------|------|------|
| 0 | 基础设施 + trait 设计 + 纯函数迁移 | 1-2 天 |
| 1 | 低耦合 builtin (8 文件) | 2-3 天 |
| 2 | 含回调 builtin (8 文件) | 3-4 天 |
| 3 | 再入型数组/TypedArray (5 文件) | 3-4 天 |
| 4 | Proxy / Reflect (5 文件) | 2-3 天 |
| 5 | Promise/Streams/Fetch/Modules/Atomics/Core (22 文件) | 3-4 天 |
| 6 | 收尾验证 + ADR | 1-2 天 |
| **合计** | | **15-22 天** |

## 回退策略

每个 Phase 独立交付，任何 Phase 结束后仓库处于可工作状态（全绿）。如果某 Phase 遇到阻塞，已完成的 Phase 不会回退。

迁移采用文件级原子性：单个文件要么完全迁移（逻辑移到 wjsm-builtins + 注册层委托），要么不迁移。不出现半迁移状态。
