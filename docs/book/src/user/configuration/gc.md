# 垃圾回收器

wjsm 内置三种垃圾回收器，运行时选择哪一种由 `--gc` 或环境变量决定。三者共用同一套托管堆，切换不需要重新编译程序。

| 值 | 说明 |
| --- | --- |
| `mark-sweep` | 标记-清除，实现最简单，行为最容易预测 |
| `g1` | 分区回收，面向大堆与吞吐 |
| `zgc` | 分代并发回收，**默认值** |

## 选择方式

```bash
wjsm --gc mark-sweep run app.js
WJSM_GC=g1 wjsm run app.js
```

优先级从高到低：`--gc` → `WJSM_TEST_GC` → `WJSM_GC` → 默认 `zgc`。`WJSM_TEST_GC` 排在 `WJSM_GC` 之前，便于测试环境临时覆盖项目设定。

非法值会在启动阶段被拒绝，并列出合法名称：

```text
Error: unknown GC algorithm `bogus`; expected one of: mark-sweep, g1, zgc
```

`--gc` 不能写在 `wjsm.toml` 里，它只接受命令行或环境变量。

> <details><summary>三种 GC 的实际差异在哪里？</summary>
>
> 用最朴素的话说，三种 GC 都在做「找出没人引用的对象，回收它们」这件事，但找的方式不同：
>
> - **mark-sweep**：从根出发标记所有可达对象，然后回收未标记的。STW（stop-the-world），全程阻塞程序。简单可靠但暂停时间随堆增长。
> - **G1**：把堆切成固定大小的 region，回收时按「回收价值」挑 region。暂停时间可控（默认目标 200ms），但仍然 STW，只是分批做。
> - **ZGC**（默认）：用「着色指针」+「读屏障」做到大部分工作并发执行。暂停时间与堆大小基本无关——大堆和小堆的暂停时间差不多。
>
> 经验上：
>
> - 小程序（< 64 MiB）随便选，差异不大。
> - 中等堆（64-512 MiB）用 ZGC 暂停最短。
> - 大堆（> 1 GiB）必须用 ZGC，否则暂停会非常明显。
> - 调试 GC 自身 bug 时用 mark-sweep，行为最可预测。
>
> </details>

## 观察 GC 行为

当前没有面向用户的 `WJSM_GC_LOG`。比较回收器时用 `--time`、`--stats` 和 `wjsm-gc-bench`；调试 GC 语义时用 `--gc mark-sweep`。

## 与堆预算的关系

`--max-heap-size` 限制的是 JavaScript 堆的分配预算，与选哪个回收器无关。预算耗尽时程序以运行时错误终止，而不是继续增长内存。

## 深入了解

- [三种回收器共用的 ManagedHeap 架构](../../internals/gc/managed-heap.md)
- [Mark-Sweep 的标记与清除阶段](../../internals/gc/mark-sweep.md)
- [G1 的分区与回收集选择](../../internals/gc/g1.md)
- [Generational ZGC 的并发阶段与着色指针](../../internals/gc/zgc.md)
- [GC 选择逻辑与必须维持的不变量](../../internals/gc/configuration-and-invariants.md)
