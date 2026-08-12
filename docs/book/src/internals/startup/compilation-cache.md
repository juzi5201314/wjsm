# 编译缓存

这一章说明 native image cache 的键计算和失效机制。

## NativeCacheKey

native image cache 由 `NativeImageRepository` 管理。cache key 绑定六个维度：

| 维度 | 来源 | 变化时 |
| --- | --- | --- |
| portable artifact digest | `.wjsm` 内容 SHA-256 | 重新编译 |
| native ABI hash | `NATIVE_ABI_HASH` | cache 全失效 |
| native codegen source hash | 编译器源码 hash | cache 全失效 |
| 当前 target | ISA / CPU | 跨平台不共享 |
| Cranelift 版本 | `cranelift_native` | 版本升级时失效 |
| codegen / ISA settings | `isa_config.rs` | 配置变化时失效 |

任一维度变化都使 cache miss，保证编译产物与运行环境严格匹配。

## 命中与 miss

| 情况 | 行为 |
| --- | --- |
| 未设置 `WJSM_CACHE_DIR` | 只走进程内 Weak 池；miss 后编译但不落盘 |
| 磁盘命中 | 从 `${WJSM_CACHE_DIR}/<digest>.wnat` 加载 image，跳过编译 |
| 磁盘 Miss | 由 `NativeCompiler::compile` 从 IR 编译，写入磁盘 |
| 并发同 key | in-flight gate 合并，只编译一次 |

磁盘 cache 校验失败（header/object/hash/权限）时计为 invalidated 并重新编译。

## builtin IR 段缓存

多文件项目每次冷启动都要把入口依赖的 Node builtin 模块重新 lower 成 IR。`wjsm-module/src/builtin_cache.rs` 把这部分产物按依赖闭包序列化到磁盘。

| 条件 | 行为 |
| --- | --- |
| `WJSM_CACHE_DIR` 已设置且 `WJSM_NO_BUILTIN_CACHE` 未设 | 走缓存路径并落盘 |
| `WJSM_NO_BUILTIN_CACHE` 非空 | 整体跳过缓存 |
| `WJSM_CACHE_DIR` 未设置或为空 | 构建段但不落盘 |

缓存键是 `sha256(BUILTIN_CACHE_VERSION ‖ emit_debug_checks ‖ 每个 builtin canonical 与其源码 SHA-256)`。`BUILTIN_CACHE_VERSION` 在 builtin 源码、lowerer 或 IR 布局变化时必须手动 bump。

## 与编译缓存的区别

| 机制 | 缓存对象 | 触发时机 |
| --- | --- | --- |
| Native image cache | 用户代码的 native image | 每次编译用户代码 |
| Builtin IR 段缓存 | builtin 模块的 IR 段 | 首次冷启动 |
| Startup snapshot | bootstrap 后的堆状态 | 进程启动时 |

三者都加速启动，但对象不同。native image cache 跳过编译，builtin IR 段缓存跳过 builtin 模块 lower，startup snapshot 跳过 builtin JS 执行。

## 深入了解

- [缓存实现](../tooling/cache.md)
- [Engine 配置与实例池](engine-pool.md)
- [实例化与执行生命周期](../host-runtime/instantiation-and-lifecycle.md)
