# wjsm-bench — 跨运行时基准

与 Node.js 的同机性能对比基准（hyperfine 驱动）：同源码场景目录 + Rust harness，量化 wjsm 与 Node 在稳态与冷启动两档下的差距，作为回归跟踪基线。

## 目录

```
bench/scenarios/   十个自包含场景（node 与 wjsm 双跑同一份源码）
crates/wjsm-bench/ Rust harness：CLI、环境快照、hyperfine 调用、RSS、JSON 报告
```

场景自包含约束：无 `import`/`export`、无 fs/net/`Intl`/`Date`/随机，只用核心语言能力 + 全局 `performance` 与 `console.log`，输出 `ns_per_op=… iterations=…`。

## 依赖

- Rust toolchain
- `cargo install hyperfine`（≥1.13）

## 构建与跑法

```bash
cargo build --release -p wjsm-bench -p wjsm-cli
WJSM=target/release/wjsm-cli target/release/wjsm-bench            # 全场景 default 档
WJSM=target/release/wjsm-cli target/release/wjsm-bench --quick    # 冒烟：runs=3 warmup=1 window=200ms
WJSM=target/release/wjsm-cli target/release/wjsm-bench --cold     # 追加无热磁盘缓存档（启动快照仍开）
WJSM=target/release/wjsm-cli target/release/wjsm-bench --scenarios fib   # 子串过滤场景
WJSM=target/release/wjsm-cli target/release/wjsm-bench --runtimes node   # 只测 node
```

- `WJSM` / `NODE` 覆盖运行时二进制，默认用 PATH 中的 `wjsm` / `node`。
- 报告默认 `/tmp/wjsm-bench-<unix秒>.json`，`--output` 可指定。

## 方法论

见 [跨运行时基准](https://book.wjsm.dev/internals/testing/cross-runtime-benchmarks.html)。
