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

## 观察 GC 行为

设置 `WJSM_GC_LOG=1` 输出回收日志。该变量只认字面值 `1`，`true` 或 `on` 都不会启用。

```bash
WJSM_GC_LOG=1 wjsm --gc g1 run app.js
```

## 与堆预算的关系

`--max-heap-size` 限制的是 JavaScript 堆的分配预算，与选哪个回收器无关。预算耗尽时程序以运行时错误终止，而不是继续增长内存。

## 深入了解

- [三种回收器共用的 ManagedHeap 架构](../../internals/gc/managed-heap.md)
- [Mark-Sweep 的标记与清除阶段](../../internals/gc/mark-sweep.md)
- [G1 的分区与回收集选择](../../internals/gc/g1.md)
- [Generational ZGC 的并发阶段与着色指针](../../internals/gc/zgc.md)
- [GC 选择逻辑与必须维持的不变量](../../internals/gc/configuration-and-invariants.md)
