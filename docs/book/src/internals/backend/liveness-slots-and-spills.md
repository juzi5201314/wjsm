# 活跃性、槽位与 GC Spill

这一章说明 generated code 如何在 may-GC 点保护活跃句柄。现行机制是 `NativeRootFrame`，不是独立的影子栈线性内存。

## safepoint

safepoint 是 native mutator 可以安全暂停的点。后端在以下位置插入检查：

- 分配 fast path（NLAB / bump 达到阈值）；
- 循环回边（`stack_budget_bytes` 耗尽后走 `CooperativePoll`）。

暂停前 generated code 发布当前 `NativeRootFrame`。GC worker 只扫描 bitmap 置位的槽。

## NativeRootFrame

`NativeRootFrame` 定义在 `wjsm-native-abi`，由 `native_abi_hash()` 覆盖：

| 字段 | 用途 |
| --- | --- |
| `previous` | 串起调用链上的 root 帧 |
| `slots` | boxed 根值数组（`i64`） |
| `bitmap_words` / `bitmap_word_count` | 哪些槽真正存活 |

collector 只读 bitmap 置位且下标 `< root_count` 的槽。更大下标的槽不会被读，generated code 不必清零。

| 时机 | 操作 |
| --- | --- |
| 函数入口 | 为本地 boxed 值预留 slot |
| 句柄活跃开始 | 写入 NaN-box 并置位 bitmap |
| safepoint | collector 扫描置位槽 |
| 句柄不再活跃 | 清掉对应 bit；槽可被覆盖 |
| 函数返回 | 弹出这一帧 |

runtime collector 的 strong closure 还合并：call arena / activation / continuation、variables 与 host roots、object side-table internal slots、WeakMap ephemeron 不动点。

## 槽位分配

后端在编译期分析每个函数的值活跃性，决定哪些值在 may-GC 点需要进入 root 帧。`f64_analysis` 等推断若能证明值是 number，就不必当 boxed root。

`known_callee_vars` 帮助做 callee 的 no-GC 分析：若所有 callee 都已知不分配，可以省掉这一次 root 帧发布。

## INV-C2 约束

raw pointer 不跨潜在 moving/collect GC 点。generated code 持有 raw pointer 的窗口必须落在两个 safepoint 之间；跨越时先变成 handle。

## 深入了解

- [并发阶段、工作线程与 Pacing](../gc/concurrency-and-pacing.md)
- [根、弱引用与 Finalizer](../gc/roots-weak-and-finalizers.md)
- [编译器内部结构](compiler-architecture.md)
- [Native ABI 索引](../reference/abi-index.md)
