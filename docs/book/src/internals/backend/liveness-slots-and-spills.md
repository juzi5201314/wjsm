# 变量活跃性、槽位与 GC Spill

这一章说明局部变量如何分配 WASM local、影子栈槽位如何管理，以及 GC safepoint 如何决定哪些值需要溢出。

## 变量到 local 的映射

`control_locals.rs` 的 `allocate_var_locals` 遍历函数体，为每个 IR 变量分配 WASM local。普通函数的变量映射到 `var_locals: HashMap<String, u32>`（local index），eval 模式映射到 `var_memory_offsets`（eval 帧内存偏移）。

Phi 节点通过 `allocate_phi_locals` 分配独立 local，前驱块尾写入，目标块开头读取。

## 值类型推断

`analysis_value_ty.rs` 做轻量值类型推断，判断一个 `ValueId` 在某个点是否确定是 int32、double 或句柄。推断结果用于省掉部分 NaN-box 解包：如果两侧都是 int32，`Binary::Add` 可以直接 `i32.add` 而不 decode→re-encode。

`builtin_returns_scalar` 白名单维护在这里：返回纯标量（int32 / f64 / bool）的 builtin 不触发 GC。GC 分析通过 `!builtin_returns_scalar` 判断 builtin 是否可能触发 GC，保证两层一致性。

## GC 分析

`compiler_gc_analysis.rs` 的 `GcAnalysis::analyze` 做模块级 GC 分析，不动点迭代求传递闭包：

1. 含分配指令（`NewObject` / `NewArray` / `StringConcatVa` 等）的函数直接 `may_gc = true`。
2. `CallBuiltin` 查 `builtin_may_trigger_gc` 白名单反面。
3. `Call` 若 callee 是 `LoadVar` 且 name ∈ `known_callee_vars` → 追溯 callee 函数的 may-GC 状态；否则保守 `may_gc = true`。

**GC 正确性红线**：unknown callee 一律保守 may-GC，只对单次赋值的函数声明变量建映射。

> <details><summary>为什么 unknown callee 一定要保守 may-GC？</summary>
>
> GC spill 是在「可能触发 GC 的调用点」前后，把活跃的句柄值溢出到影子栈，让 GC 标记阶段能找到。漏掉溢出 = GC 标记不到活跃对象 = 对象被错误回收 = 内存安全 bug。
>
> 对于 unknown callee，我们不知道它会不会触发 GC——可能它内部是 `() => 1`（无 GC），也可能它内部有 `new Map()`（有 GC）。如果 codegen 假定它无 GC、跳过 spill，而运行时它实际有 GC，结果是灾难性的 Use-After-Free。
>
> 「保守 may-GC」的代价：多写一次 spill 序列，多读一次影子栈。这是性能成本。「激进 may-GC 推断」的成本：偶发的内存安全 bug，调试几周。
>
> 显然选前者。这就是为什么 `known_callee_vars` 是 conservative hint，不是 aggressive optimization。
>
> </details>

## Safepoint spill

在可能触发 GC 的调用点，活跃的句柄值需要溢出到影子栈（GC spill）。后端在每个 may-GC 调用前插入 spill 序列：

1. `GlobalGet __shadow_sp` 保存当前栈指针。
2. 对每个活跃的 `TAG_OBJECT_HANDLE` / `TAG_STRING` / `TAG_FUNCTION` 值，写入影子栈并推进 `__shadow_sp`。
3. 调用 may-GC 函数。
4. `GlobalSet __shadow_sp` 恢复栈指针。

`GcAnalysis` 的 `call_targets` 表记录每个 `Call` 的 callee 函数 id，若 callee `may_gc == false` 则省略 spill。这是 wjsm 最重要的优化之一——纯计算函数的调用不产生 spill 代码。

## 影子栈溢出检查

`emit_shadow_stack_overflow_check` 在写入影子栈前检查剩余空间，不足时调用 `env.throw` 抛出 `RangeError: Maximum call stack size exceeded`。错误消息包含实际的 sp 和 limit 数值，用于调试。

## 深入了解

- [NaN-boxed 值表示与标签](value-representation.md)
- [IR Function 的 known_callee_vars 字段](../ir/program-module-function.md)
- [影子栈的用户侧说明](../../user/configuration/memory.md)
