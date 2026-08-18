# 跨运行时基准

这一章说明 wjsm 与 Node.js 的同机性能对比基准：目标、方法论来源、目录结构、跑法与报告字段含义。

## 目标与为什么

wjsm 是 AOT 运行时（SWC AST → 语义 IR → portable `.wjsm` → Cranelift CLIF → native image → `NativeRuntime`），与 Node.js（V8 JIT）的执行模型不同：

- **Node.js**：解释执行起步，热点函数被 JIT 优化到稳态；短生命周期程序测到的主要是 JIT warmup 前的性能。
- **wjsm**：启动时一次编译到当前宿主机器码，执行路径平坦，没有 warmup 曲线。启动快照始终从嵌入的 `startup_snapshot.bin` 恢复。

因此单测"一次运行"的墙钟时间会把 V8 的 warmup 成本与 wjsm 的启动成本混在一起，无法区分稳态性能与启动性能。基准必须分两档：

- **default 档（稳态）**：不强制设置 `WJSM_CACHE_DIR`（沿用进程环境）；node 由 hyperfine `--warmup` 预热。
- **wjsm_cold 档（`--cold`）**：把 `WJSM_CACHE_DIR` 指到 `/tmp/wjsm-bench-cold-cache`，每轮 `--prepare` 清空 —— 量化无热 native/builtin 磁盘缓存的启动+编译成本。启动快照始终恢复。

两档对比能回答：wjsm 的差距来自磁盘缓存 miss 的编译（冷档恶化、稳态缩小），还是执行核心本身（两档同比例差距）。

## 方法论来源

- **Bun**：同源码场景目录（同一份 `.js` 在多个运行时下跑），工具分层（mitata 做微基准、hyperfine 做端到端、bombardier 做 HTTP）。
- **Deno**：hyperfine 驱动端到端对比，`--reload` 区分冷热档，并补充非时间维度（RSS、编译时间）。

wjsm 采用：**同源码场景目录 + Rust harness（hyperfine 驱动）+ 环境快照**。场景文件自包含、确定性、无 I/O，两个运行时跑同一份源码。

## 目录结构

```
bench/scenarios/*.js       十个自包含场景（双运行时同源）
crates/wjsm-bench/          Rust harness：CLI、环境快照、hyperfine 调用、RSS、JSON 报告
docs/book/src/internals/testing/cross-runtime-benchmarks.md   本页
```

场景一览：

| 场景 | 负载 |
| --- | --- |
| `fib30.js` | 递归斐波那契（函数调用栈） |
| `arithmetic.js` | 浮点标量循环 |
| `json-heavy.js` | JSON.parse / stringify 往返 |
| `regex.js` | 三种正则的 test + exec 混合 |
| `array-ops.js` | map / filter / reduce / sort |
| `object-props.js` | 类构造、getter、方法调用 |
| `string-ops.js` | 拼接、slice、split、模板插值 |
| `map-set.js` | Map / Set 插入查找删除 |
| `closures.js` | 闭包创建与调用 |
| `alloc-churn.js` | 临时对象分配 + 有界缓存淘汰 |

## 跑法

构建（正式测量务必用 release）：

```bash
cargo build --release -p wjsm-bench -p wjsm-cli
WJSM=target/release/wjsm-cli target/release/wjsm-bench            # 全场景 default 档
WJSM=target/release/wjsm-cli target/release/wjsm-bench --quick    # 冒烟快捷档
WJSM=target/release/wjsm-cli target/release/wjsm-bench --cold     # 追加 wjsm 冷启动档
WJSM=target/release/wjsm-cli target/release/wjsm-bench --scenarios fib   # 只跑 fib30
WJSM=target/release/wjsm-cli target/release/wjsm-bench --runtimes node   # 只测 node
```

- `WJSM` / `NODE` 环境变量覆盖运行时二进制；不设置时用 PATH 中的 `wjsm` / `node`。
- `--quick` = `--runs 3 --warmup 1 --window-ms 200`，且优先于显式 `--runs/--warmup/--window-ms`。
- `--scenarios` 对 `.js` 文件名做子串过滤；`--runtimes` 为逗号分隔的 `node,wjsm`。
- 报告默认写到 `/tmp/wjsm-bench-<unix秒>.json`，可用 `--output` 指定。
- 依赖：`cargo install hyperfine`（≥1.13，冷档按命令 `--prepare` 需要 ≥1.13）。

## 报告字段

| 字段 | 含义 |
| --- | --- |
| `regimes.default` | 稳态档：热磁盘缓存 |
| `regimes.wjsm_cold` | 冷档：每轮清空 `WJSM_CACHE_DIR`；启动快照仍恢复 |
| `wall` | hyperfine 壁钟分布（mean / stddev / median / min / max / runs） |
| `ns_per_op` | 场景内稳态单次 work 耗时（仅 default 档；场景 stdout 的 `ns_per_op=` 解析） |
| `max_rss_kb` | `/usr/bin/time -v` 的最大驻留集（仅 Linux 且有 GNU time 时） |
| `environment` | node / wjsm / Cranelift / hyperfine 版本、CPU、内存、GC、git rev |

`regimes` 与 `scenarios` 用 BTreeMap 保证 JSON 键序稳定；`schema_version` 字段演进时递增。

## 公平性注意事项

- **同机、固定版本**：报告嵌入所有版本与硬件信息，跨机器结果不可直接比较。
- **warmup + median**：hyperfine 预热丢弃前几次运行，报告用中位数抗噪声。
- **冷档只针对 wjsm**：node 没有等效的"每进程编译缓存关闭"开关，其冷热由 OS 页缓存决定，hyperfine `--warmup` 已覆盖。报告如实记录，不假装公平。
- **GC 只比用户可观察结果**：wjsm 的 host GC 与 V8 的 JIT 内 GC 机制不同，只比较端到端时间与 RSS，不比内部指标。
- **排除 HTTP / fs / npm**：场景只用核心语言能力 + `performance` + `console.log`，不含 I/O，避免系统调用与页面缓存干扰。
- **首跑数字即基线**：本套件定位是量化基线 + 回归跟踪，不承诺持平 Node；后续改动对比同报告结构即可。
- **不要并发跑 `--cold`**：固定冷缓存目录 `/tmp/wjsm-bench-cold-cache` 会被两个进程互相清空。

## 深入了解

- [性能分析与回归证据](performance.md)
- [GC Benchmark](gc-benchmarks.md)
- [用户侧的性能与内存调优](../../user/workflows/performance.md)
