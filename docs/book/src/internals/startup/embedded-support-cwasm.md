# 嵌入式 support cwasm（补充）

这一章是 `support-cwasm.md` 的补充细节，说明运行时如何注入和管理 per-flavor 的预编译 support 模块。

## 运行时注入

| 函数 | 行为 |
| --- | --- |
| `install_embedded_support_cwasm(bytes)` | 显式注入，进程内只需调用一次（`OnceLock`，重复 set 静默忽略） |
| `embedded_support_cwasm()` | 返回已安装的 cwasm；未显式注入时使用 zgc 默认 artifact（`LazyLock<DEFAULT_ZGC_SUPPORT_CWASM>`） |
| `embedded_support_cwasm_for(kind)` | 按 GC 算法（`MarkSweep` / `G1` / `Zgc`）返回对应 flavor 的 cwasm |

## 为什么 per-flavor

不同 GC flavor 的 support module 有不同的 barrier、alloc、scan 函数：

| GC | 需要哪些 support 函数 |
| --- | --- |
| Mark-Sweep | 分配、标记、清除 |
| G1 | 分配、SATB 写屏障、标记、remset |
| ZGC | 分配、着色指针读屏障、代际写屏障、并发标记、转发 |

per-flavor cwasm 让运行时只加载当前 GC 对应的实现，减小内存占用。

## 与 startup snapshot 的关系

startup snapshot 的 ABI hash 包含 `support_abi_union_hash()`（三种 flavor 的合并 hash），而不是单个 flavor 的 hash。这让一个 embedded snapshot 能匹配任意 GC 选择。

## 深入了解

- [构建期嵌入工件](embedded-artifacts.md)
- [预编译 Support cwasm](support-cwasm.md)
- [Support 模块与辅助函数](../backend/support-module.md)
