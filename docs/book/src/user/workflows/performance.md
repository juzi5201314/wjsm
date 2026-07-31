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

> <details><summary>Winch vs Cranelift：怎么选？</summary>
>
> 两者都是 wasmtime 的编译器，但定位不同：
>
> - **Winch**（基线编译器）：编译快、生成代码质量一般。类似解释执行的 V8（baseline JIT）。
> - **Cranelift**：编译慢、生成代码经过优化。类似 V8 的 TurboFan（优化 JIT）。
>
> 决策树：
>
> - 程序启动频繁、跑得快（脚本、CI 任务）→ 用 Winch，省下启动开销。
> - 程序跑得久（long-running service、批处理）→ 用 Cranelift，省下的是执行时的 CPU。
> - 需要 inspector 调试 → 必须 Cranelift，Winch 没调试信息。
>
> 经验数据：Cranelift 比 Winch 慢 30-100%（编译时间），但执行速度快 10-30%。拐点大概在「程序执行时间 / 启动时间 > 5」时 Cranelift 占优。
>
> </details>

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

## 与 Node.js 对比

想量化 wjsm 与 Node.js 在同机上的性能差距，用 `wjsm-bench`（hyperfine 驱动，同源码场景双跑）：

```bash
cargo build --release -p wjsm-bench -p wjsm-cli
WJSM=target/release/wjsm-cli target/release/wjsm-bench --quick
```

`WJSM` / `NODE` 环境变量可覆盖对比的二进制（默认 PATH 中的 `wjsm` / `node`）。输出 JSON 报告与终端对比表（wall median / ns_per_op / RSS）。方法论与报告字段见[跨运行时基准](../../internals/testing/cross-runtime-benchmarks.md)。

## 深入了解

- [并发阶段、工作线程与 GC Pacing 如何决定暂停时间](../../internals/gc/concurrency-and-pacing.md)
- [启动快照边界：哪些状态能被固化](../../internals/startup/startup-snapshot.md)
- [编译缓存的键设计与落盘格式](../../internals/tooling/cache.md)
- [性能分析方法与回归证据要求](../../internals/testing/performance.md)
- [跨运行时基准](../../internals/testing/cross-runtime-benchmarks.md)
