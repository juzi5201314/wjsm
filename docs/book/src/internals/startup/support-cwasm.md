# 预编译 Support cwasm

这一章说明 support cwasm 在运行时的加载与注入机制。

## 运行时注入

三个函数控制 embedded support cwasm 的运行时访问：

- `install_embedded_support_cwasm(bytes)`：显式注入，进程内只需调用一次（`OnceLock`，重复 set 静默忽略）。
- `embedded_support_cwasm()`：返回已安装的 cwasm；未显式注入时使用 zgc 默认 artifact（`LazyLock<DEFAULT_ZGC_SUPPORT_CWASM>`）。
- `embedded_support_cwasm_for(kind)`：按 GC 算法（`MarkSweep` / `G1` / `Zgc`）返回对应 flavor 的 cwasm。

`INSTALLED_SUPPORT_CWASM` 是 `OnceLock<&'static [u8]>`。`DEFAULT_ZGC_SUPPORT_CWASM` 是 `LazyLock<Option<&'static [u8]>>`，在没有显式注入时提供 zgc 默认 artifact。返回 `None` 仅当 `embedded` feature 未启用。

## 为什么 per-flavor

不同 GC flavor 的 support module 有不同的 barrier、alloc、scan 函数：

| GC | 需要的 support 函数 |
| --- | --- |
| Mark-Sweep | 分配、标记、清除 |
| G1 | 分配、SATB 写屏障、标记、remset |
| ZGC | 分配、着色指针读屏障、代际写屏障、并发标记、转发 |

per-flavor cwasm 让运行时只加载当前 GC 对应的实现，减小内存占用。

## 加载时机

support module 在启动路径的早期加载，先于用户模块实例化。它把 `__alloc_ptr`、`__gc_phase`、`__good_color` 等 env global 初始化为默认值，user module 的 fast path 直接使用。

## 与 startup snapshot 的关系

startup snapshot 的 ABI hash 包含 `support_abi_union_hash()`（三种 flavor 的合并 hash），而不是单个 flavor 的 hash。这让一个 embedded snapshot 能匹配任意 GC 选择——快照恢复后根据当前 GC 加载对应 support cwasm。

## 深入了解

- [构建期嵌入工件](embedded-artifacts.md)
- [Support 模块与辅助函数](../backend/support-module.md)
- [GC 选择、配置与不变量](../gc/configuration-and-invariants.md)
