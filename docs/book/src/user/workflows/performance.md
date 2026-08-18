# 性能与内存调优

wjsm 是 AOT 运行时：启动时一次编译，执行路径平坦，没有 JIT warmup 曲线。性能调优的三个杠杆是 GC 算法、堆预算和编译观察。

## GC 算法选择

三种回收器共用同一套托管堆，切换不需要重新编译程序：

| 值 | 适用场景 | 特点 |
| --- | --- | --- |
| `zgc` | **默认**，大堆（> 1 GiB） | 分代并发回收，暂停时间与堆大小基本无关 |
| `g1` | 中等堆（64–512 MiB），需要控制暂停 | 分区回收，暂停可控（默认目标 200ms），仍然 STW |
| `mark-sweep` | 小程序（< 64 MiB），调试 GC | 标记-清除，行为最可预测，暂停随堆增长 |

```bash
wjsm --gc zgc run app.ts           # 默认
wjsm --gc g1 --max-heap-size 512M run app.ts
WJSM_GC=mark-sweep wjsm run app.ts
```

小程序三种 GC 差异不大。大堆必须用 ZGC，否则暂停会非常明显。调 GC 自身 bug 时用 mark-sweep。

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

`--stats` 输出常量数、函数数、基本块数和指令数。对已编码的 `.wjsm` 还会打印 artifact 字节数。若设置了 `WJSM_CACHE_DIR`，执行后还会打印 native cache 的 entries / bytes / hits / misses / invalidated。

## 打开磁盘缓存

`wjsm run` 默认不读写硬盘缓存，每次都从 IR 重新 codegen。重复执行同一份源码或 `.wjsm` 时，显式设置目录即可复用 native image 和 builtin IR 段：

```bash
export WJSM_CACHE_DIR=~/.cache/wjsm
wjsm run app.ts
wjsm cache stats
```

测试套件默认也不设这个变量。空值不会回落到 `$HOME/.cache/wjsm`。打开后默认 256 MiB LRU，可用 `WJSM_CACHE_MAX_BYTES` 调整。

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
