# 活跃性、槽位与 GC Spill

这一章说明 generated code 如何在 may-GC 点保护活跃句柄，以及 shadow stack 的角色。

## safepoint

safepoint 是 native code 执行可以安全暂停的点。后端在以下位置插入 safepoint 检查：

- 分配 fast path（bump pointer 达到阈值时触发 GC）；
- 循环回边（防止长时间不触发 safepoint）。

safepoint 时，当前线程把活跃句柄 spill 到 shadow stack，GC 工作线程可以安全扫描。

## shadow stack

shadow stack 是一段由 generated code 管理的内存区域，用于在 safepoint 时存储活跃的 boxed 根值。它的布局由 `wjsm-native-abi` 定义，`NATIVE_ABI_HASH` 覆盖该布局。

| 时机 | 操作 |
| --- | --- |
| 函数入口 | 在 shadow stack 上为本地变量预留槽位 |
| 句柄活跃开始 | 把 NaN-box 值写入对应槽位 |
| safepoint | GC 扫描 shadow stack，标记活跃句柄 |
| 句柄不再活跃 | 槽位可被覆盖（不需要显式清除） |
| 函数返回 | 释放预留槽位 |

## NativeRootFrame

`NativeRootFrame` 是编译器在 may-GC 调用前发布的 root frame 描述。它告诉 collector「当前有哪些活跃句柄需要扫描」。runtime collector 的 strong closure 合并以下来源：

- native root frames（generated code 发布的）；
- active call arena / activation / continuation；
- variables、intrinsics 与 host roots；
- object side-table internal slots；
- WeakMap ephemeron fixed point。

## 槽位分配

后端在编译期分析每个函数的值活跃性，决定哪些值在 may-GC 点需要 spill。分析在 `analysis_value_ty.rs` 中做值类型推断，用于省掉部分 NaN-box 解包——如果值已知是 number，不需要当 boxed root 处理。

`known_callee_vars` 帮助做 callee 的 no-GC 分析：如果一个函数声明的所有 callee 都在 `known_callee_vars` 里，调用这些 callee 不触发 GC，可以省掉 root frame 发布。

## INV-C2 约束

raw pointer 不跨潜在 moving/collect GC 点。这是 GC 的硬不变量。generated code 持有 raw pointer 的时间窗口必须在两个 safepoint 之间，跨越 safepoint 时必须先 spill 为 handle。

## 深入了解

- [并发阶段、工作线程与 Pacing](../gc/concurrency-and-pacing.md)
- [根、弱引用与 Finalizer](../gc/roots-weak-and-finalizers.md)
- [编译器内部结构](compiler-architecture.md)
- [Function 的元数据](../ir/program-module-function.md)
