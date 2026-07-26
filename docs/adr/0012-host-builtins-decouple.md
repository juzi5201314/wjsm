# ADR 0012: Host Builtins 后端解耦（wjsm-builtins crate）

**状态**: Accepted
**日期**: 2026-07-26

## 背景

原 `wjsm-host-wasm/src/host_imports/` 下的 43 个 host builtin 文件全部直接依赖
`wasmtime::Caller<RuntimeState>`，算法逻辑（Array.prototype.map、Proxy trap、
Reflect.set、Promise.then 等）与 wasmtime 注册代码混编在同一个 `Func::wrap`
闭包中。这使得：

1. **无法支持非 wasmtime 后端**：所有算法逻辑绑定到 wasmtime API，未来 native
   后端（cranelift JIT）无法复用。
2. **测试困难**：算法逻辑只能在 wasmtime 运行时上下文中执行，无法独立单测。
3. **文件臃肿**：`proxy_reflect.rs`（1731 行）、`core.rs`（1827 行）、
   `array_object.rs`（2668 行）等文件混合注册 + 算法，难以维护。

## 决策

### 新建 `wjsm-builtins` crate

```
wjsm-ir (零依赖)
  ↑
wjsm-host (ExecContext trait + 共享类型 + 纯函数)
  ↑
wjsm-builtins (builtin 实现, 泛型 `<E: ExecContext>`)
  ↑                    ↑
wjsm-host-wasm    wjsm-module
  ↑
wjsm-cli
```

`wjsm-builtins` 以 `<E: wjsm_host::ExecContext>` 泛型单态化，零 vtable 开销。
wasmtime 后端用 `WasmExecContext` 实现 trait，builtins 代码编译期内联。

### ExecContext trait 设计

`ExecContext: HeapContext` 定义 builtins 所需的全部操作，约 260+ 方法：

- 对象分配 / 字符串存储 / 属性键系统
- 再入回调（`call_js` / `call_js_async`）
- Proxy / Closure / Bound 状态表访问
- 枚举器 / 异常 / 属性助手 / 数组助手
- Promise 原语（alloc/settle/resolve + entry 操作 + species + combinator）
- GC safepoint / 原型链 / Handle 解析

共享类型（`ProxyEntry`、`PromiseEntry`、`PromiseState`、`PromiseReaction`、
`ReactionType`、`CapturedScope`、`NativeCallableRef` 等）定义在 `wjsm-host` 中，
后端无关。`NativeCallable` 完整枚举留在 `wjsm-host-wasm` types.rs（~150 变体，
被 32 个 runtime_* 文件使用），builtins 通过 `NativeCallableRef` 子集 + trait
方法 `create_native_callable` 间接创建。

### host-wasm 保留薄注册层

`host_imports/` 下的文件改为薄注册层：

```rust
linker.func_wrap_async("env", "promise_then",
    |mut caller: Caller<RuntimeState>, (p, on_f, on_r): (i64, i64, i64)| {
        Box::new(async move {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::promise::promise_then_impl(&mut ctx, p, on_f, on_r)
        })
    })?;
```

共享 helper（`alloc_host_object` / `define_host_data_property` /
`store_runtime_string` / `settle_promise`）留在 host-wasm，`WasmExecContext`
内部委托，32 个 `runtime_*.rs` 文件不受影响。

## 迁移进度

| Phase | 文件 | 状态 |
|-------|------|------|
| P0 | 基础设施（crate + trait + WasmExecContext） | ✅ |
| P1 | 低耦合 builtin（math/number/error/primitive_core 等 8 文件） | ✅ |
| P2 | 含回调 builtin（string_methods/object_builtins 等 8 文件） | ✅ |
| P3 | 再入型 builtin（array/typedarray/collections 5 文件） | ✅ |
| P4 | Proxy/Reflect（proxy_traps/proxy_reflect/async 5 文件） | ✅ |
| P5 | Promise/QueuingStrategy（promise + streams_queuing） | ✅ |
| P5+ | Streams/Fetch/Modules/Atomics/Core/gc 剩余（~12K 行） | 待续 |
| P6 | 收尾（mod.rs re-export / AGENTS.md / 本 ADR） | ✅ |

### P4 关键修复

P4 迁移中发现并修复了 V2 堆 property key canonicalize 缺失问题：
`get_own_property_slot` / `set_property_by_name_id` / `delete_property_by_name_id` /
`define_data_property_with_flags` / `define_accessor_property_with_flags` 直接用
编译期 `MemoryString` name_id 查 V2 堆，但 V2 堆存的是 canonicalize 后的
`RuntimeString` key。统一在每个方法中加 `canonicalize_v2_name_id` 转换。

`reflect_get_impl_with_receiver_async` 原调用 `ctx.reflect_get_sync()`（用
`block_in_place`），在 current-thread tokio runtime 上 panic。用纯异步的
`ordinary_get_async`（沿原型链查属性槽 + 异步调 getter）替换。

## 影响

### 正面

- 算法逻辑后端无关，未来 native 后端可直接复用
- builtins 可独立单测（mock ExecContext）
- 文件职责清晰：builtins = 算法，host_imports = 注册
- 泛型单态化，零运行时开销

### 风险

- `ExecContext` trait 方法多（260+），接口稳定性需关注
- `NativeCallable` 跨 crate 可见性通过 `NativeCallableRef` 子集解决，
  新增 NativeCallable 变体需同步更新 trait 方法
- async 泛型 Future 对每个 E 生成不同 Future 类型，编译时间略增（<10%）

## 参考

- `docs/plan.md` — 完整迁移计划
- ADR 0011 — Runtime 按后端无关性拆分（wjsm-host / wjsm-host-wasm / wjsm-gc / wjsm-dyncode）
