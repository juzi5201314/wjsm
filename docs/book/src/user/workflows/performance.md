# 性能与内存调优

wjsm 是 AOT 运行时：启动时一次编译，执行路径平坦，没有 JIT warmup 曲线。性能调优的两个杠杆是堆预算和编译观察。生产 GC 固定为并发分代 ZGC，暂停时间与堆大小基本无关。

## 堆上限

`--max-heap-size <SIZE>` 限制 JavaScript ManagedHeap 的分配预算。预算耗尽时程序以运行时错误终止，而不是继续增长内存：

```bash
wjsm --max-heap-size 256M run app.ts
```

堆上限与回收器无关——它们是正交的配置。

## 观察编译与执行开销

`--time` 打印各阶段耗时，`--stats` 打印 IR 规模：

```bash
wjsm run --time -e 'console.log(1)'
wjsm run --stats app.ts
wjsm run -v --time app.ts     # -v 切换到微秒精度
```

```text
Timing: parse=6ms, lower=10ms, compile=6ms, execute=67ms
```

`-v` 让计时用微秒单位，同时打印阶段进入信息。`execute` 只在实际执行的命令里出现；`build --stage compile --time` 只会看到前三段。

`--stats` 输出常量数、函数数、基本块数和指令数。对已编码的 `.wjsm` 还会打印 artifact 字节数。磁盘缓存可用时，执行后还会打印 native cache 的 entries / bytes / hits / misses / invalidated。

## 磁盘缓存

磁盘缓存默认可用：`WJSM_CACHE_DIR` 未设置时回落 `${XDG_CACHE_HOME}/wjsm`，再回落 `${HOME}/.cache/wjsm`。重复执行同一文件入口时，输入寻址 artifact 缓存跳过 parse/lower，native image 缓存跳过 Cranelift 编译，builtin IR 段缓存跳过 builtin lower：

```bash
wjsm run --time app.ts     # 冷路径：parse/lower/compile 全额支付
wjsm run --time app.ts     # 命中：parse=0 lower=0，读盘量级
wjsm cache stats
```

`WJSM_CACHE_DIR` 设为空串显式禁用磁盘缓存。测试套件把缓存目录重定向到进程隔离的 `/tmp` 路径。默认 256 MiB LRU，可用 `WJSM_CACHE_MAX_BYTES` 调整。

对比特化前后用 `WJSM_DISABLE_SPECIALIZATION=1`。对比 Cranelift 优化档位用 `WJSM_OPT_LEVEL=none`。

## 临时文件

wjsm 的临时文件（artifact、bench 报告、冷缓存目录）写到 `/tmp`：

```bash
wjsm build app.ts -o /tmp/app.wjsm
wjsm validate /tmp/app.wjsm
wjsm run /tmp/app.wjsm
```

bench 报告默认写到 `/tmp/wjsm-bench-<unix秒>.json`，`--cold` 把磁盘缓存固定在 `/tmp/wjsm-bench-cold-cache` 并每轮清空。启动快照仍恢复。不要并发跑 `--cold`。

## 深入了解

- [垃圾回收器](../configuration/gc.md)
- [堆、root 帧与内存预留](../configuration/memory.md)
- [性能分析与回归证据](../../internals/testing/performance.md)
- [跨运行时基准](../../internals/testing/cross-runtime-benchmarks.md)
