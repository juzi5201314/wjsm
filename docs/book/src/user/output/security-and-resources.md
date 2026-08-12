# 安全与资源边界

## Direct native 安全边界

wjsm 直接把 verified semantic IR 编译为当前宿主机器码。它不提供进程内 memory/control-flow sandbox；artifact verifier、checked lowering、strict relocation、symbol allowlist、unwind validation 与 W^X 是受信编译/加载 TCB，不能把 compiler/runtime bug 隔离在宿主进程之外。

不要把不受信任代码和宿主秘密放进同一个 runtime process。多租户或恶意代码场景必须增加独立 OS process、低权限账户、filesystem/network policy、cgroup/job object、seccomp 或容器等外部隔离。

## 资源边界

| 资源 | 配置入口 |
| --- | --- |
| JavaScript ManagedHeap | `--max-heap-size <SIZE>` |
| Collector | `--gc <mark-sweep\|g1\|zgc>`、`WJSM_GC` |
| Native image cache | `WJSM_CACHE_DIR`、`wjsm cache --dir ...` |
| Worker / realm / I/O 限额 | 对应 runtime 环境变量与 API contract |

每个 agent 拥有独立 ManagedHeap、handle table、collector、scheduler 与 mutable runtime tables。跨 agent 只允许 structured clone、SAB/Atomics 和显式 IPC；不共享 GC handle 或 raw address。

## 制品与 cache

Portable `.wjsm` 在执行前做 bounded decode、hash/ABI、manifest 与 IR verification。native cache 是可重建派生数据；损坏、stale 或权限不安全的 cache entry 会被 invalidated，而不是执行其 bytes。

这些检查提升输入与加载完整性，但不改变“需要 OS process 隔离不受信任代码”的结论。
