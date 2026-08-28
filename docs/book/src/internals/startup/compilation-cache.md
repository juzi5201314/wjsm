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
| 缓存目录不可解析（`WJSM_CACHE_DIR` 为空且无 XDG/HOME） | 只走进程内 Weak 池；miss 后编译但不落盘 |
| 磁盘命中 | 从 `${cache_dir}/<digest>.wnat` 加载 image，跳过编译 |
| 磁盘 Miss | 由 `NativeCompiler::compile` 从 IR 编译，写入磁盘 |
| 并发同 key | in-flight gate 合并，只编译一次 |

磁盘 cache 校验失败（header/object/hash/权限）时计为 invalidated 并重新编译。

## builtin IR 段缓存

多文件项目每次冷启动都要把入口依赖的 Node builtin 模块重新 lower 成 IR。`wjsm-module/src/builtin_cache.rs` 把这部分产物按依赖闭包序列化到磁盘。

| 条件 | 行为 |
| --- | --- |
| 缓存目录可解析且 `WJSM_NO_BUILTIN_CACHE` 未设 | 走缓存路径并落盘 |
| `WJSM_NO_BUILTIN_CACHE` 非空 | 整体跳过缓存 |
| 缓存目录不可解析（`WJSM_CACHE_DIR` 为空且无 XDG/HOME） | 构建段但不落盘 |

缓存键是 `sha256(BUILTIN_CACHE_ABI_HASH ‖ emit_debug_checks ‖ 每个 builtin canonical 名)`。`BUILTIN_CACHE_ABI_HASH` 由构建期对 builtin 源码与 module/parser/semantic/IR 输入做摘要生成，源码变化自动换命名空间。

## 输入寻址 artifact 缓存

文件入口的 parse + lower 产物按 `sha256(源码闭包读集 ‖ 编译选项 ‖ 语义 ABI 指纹)` 落盘为 `${cache_dir}/artifact/<content_key>.wjsm`，入口寻址的 `.dep` 索引记录读集事实供回放校验。命中时同源二次运行跳过 parse/lower，语义 ABI bump 自动作废旧条目。详见[缓存实现](../tooling/cache.md)。

## 与编译缓存的区别

| 机制 | 缓存对象 | 触发时机 |
| --- | --- | --- |
| 输入寻址 artifact 缓存 | 文件入口的 portable artifact | 文件入口编译 |
| Native image cache | 用户代码的 native image | 每次编译用户代码 |
| Builtin IR 段缓存 | builtin 模块的 IR 段 | 首次冷启动 |
| 启动种子 | 嵌入的 global/EvalIndirect 种子 | 进程启动时始终恢复 |

四者对象不同。artifact 缓存跳过 parse + lower，native image cache 跳过 Cranelift 编译，builtin IR 段缓存跳过 builtin 模块 lower。启动种子不包含 builtin JS。

## 深入了解

- [缓存实现](../tooling/cache.md)
- [Engine 配置与实例池](engine-pool.md)
- [实例化与执行生命周期](../host-runtime/instantiation-and-lifecycle.md)
