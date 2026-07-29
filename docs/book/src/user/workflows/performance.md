# 性能与内存调优

wjsm 的耗时分成两块：编译（parse / lower / compile）和执行。先用 `--time` 判断瓶颈在哪一块，再决定调什么。

```bash
wjsm run app.ts --time
```

```text
Timing: parse=6ms, lower=10ms, compile=6ms, execute=67ms
```

## 编译侧

编译结果按 WASM 字节内容哈希缓存，同一份程序第二次运行直接反序列化已编译模块，跳过 Cranelift。缓存目录由 `WJSM_CACHE_DIR` 决定，默认 `$HOME/.cache/wjsm`。

```bash
wjsm cache stats
```

编译器可以换：

```bash
WJSM_COMPILER=winch wjsm run app.ts   # 编译更快，生成代码更慢
WJSM_OPT_LEVEL=none wjsm run app.ts   # 关闭优化，进一步缩短编译时间
```

Winch 适合频繁改代码、每次只跑一遍的场景；长时间运行的程序用默认的 Cranelift。启用 inspector 时会强制 Cranelift，Winch 设置被忽略。

## 执行侧

垃圾回收器选择直接影响暂停时间和吞吐：

```bash
wjsm --gc zgc run app.ts        # 默认，并发回收，暂停短
wjsm --gc g1 run app.ts         # 分区回收
wjsm --gc mark-sweep run app.ts # 最简单，暂停最长
```

`WJSM_GC_LOG=1` 打印回收事件，用于确认回收频率是否异常。

## 内存边界

```bash
wjsm --max-heap-size 512M run app.ts
wjsm --shadow-stack-max 32M run app.ts
```

`--max-heap-size` 限制 JavaScript 堆预算，超出时抛出 `JavaScript heap budget exhausted`。`--shadow-stack-max` 控制调用深度上限，默认 16M；深递归程序超限会得到 `RangeError: Maximum call stack size exceeded`，并在消息里带上实际的 sp 与 limit 数值。

## 启动开销

启动快照默认开启，它把 builtin 初始化后的堆状态直接恢复，省掉每次启动的 bootstrap。除排查快照本身的问题外，不要用 `WJSM_STARTUP_SNAPSHOT=0` 关闭它。

## 深入了解

- [并发阶段、工作线程与 GC Pacing 如何决定暂停时间](../../internals/gc/concurrency-and-pacing.md)
- [启动快照边界：哪些状态能被固化](../../internals/startup/startup-snapshot.md)
- [编译缓存的键设计与落盘格式](../../internals/tooling/cache.md)
- [性能分析方法与回归证据要求](../../internals/testing/performance.md)
