# 嵌入式 support cwasm（补充）

这一章是 `support-cwasm.md` 的补充细节。主章见[预编译 Support cwasm](support-cwasm.md)。

## 运行时注入

三个函数控制 embedded support cwasm 的运行时访问：

- `install_embedded_support_cwasm(bytes)`：显式注入，进程内只需调用一次（`OnceLock`，重复 set 静默忽略）。
- `embedded_support_cwasm()`：返回已安装的 cwasm；未显式注入时使用 zgc 默认 artifact（`LazyLock<DEFAULT_ZGC_SUPPORT_CWASM>`）。
- `embedded_support_cwasm_for(kind)`：按 GC 算法（`MarkSweep` / `G1` / `Zgc`）返回对应 flavor 的 cwasm。

## 为什么 per-flavor

不同 GC flavor 的 support module 有不同的 barrier、alloc、scan 函数。mark-sweep 不需要读屏障，ZGC 需要。per-flavor cwasm 让运行时只加载当前 GC 对应的实现，减小内存占用。

## 与 startup snapshot 的关系

startup snapshot 的 ABI hash 包含 `support_abi_union_hash()`（三种 flavor 的合并 hash），而不是单个 flavor 的 hash。这让一个 embedded snapshot 能匹配任意 GC 选择。

## 深入了解

- [构建期嵌入工件](embedded-artifacts.md)
- [预编译 Support cwasm](support-cwasm.md)
- [Support 模块与辅助函数](../backend/support-module.md)
