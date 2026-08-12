# Engine 配置与实例池

这一章说明 `NativeRuntime` 的 engine 配置和 image repository 管理。

## NativeImageRepository

`NativeImageRepository` 是进程内 image 与磁盘 cache 的唯一 owner。它管理：

| 职责 | 说明 |
| --- | --- |
| 内存 image | `Weak<CompiledImage>`，调用方持 `Arc` 决定生命周期 |
| 磁盘 cache | 按 `NativeCacheKey` 查找 |
| 并发合并 | 同 key 的并发 prepare 由 in-flight gate 合并 |
| 失效 | 损坏 / stale / 权限不安全的条目 invalidated，重新编译 |

`CompiledImage` 拥有 executable mappings、entry table、source metadata 与 unwind registration。drop 顺序必须先注销 unwind，再释放 mapping。

## ISA 配置

`wjsm-backend-native/src/isa_config.rs` 是唯一构造和 mutation Cranelift ISA/flags 的地方。所有 codegen 路径使用 `cranelift_native::builder()` 初始化当前宿主 target。

当前 production capability 是 x86_64 Linux 与 x86_64 Windows。不支持的 target 立即返回 `UnsupportedTargetCapability`，不回退到其他 backend、解释器或 test heap。

## Cache key

`NativeCacheKey` 绑定六个维度：

1. portable artifact digest（`.wjsm` 内容 hash）；
2. native ABI hash（vmctx/CallArgs/frame/symbol 布局）；
3. native codegen source hash（编译器源码 hash）；
4. 当前 target（ISA/CPU）；
5. Cranelift 版本；
6. codegen/ISA settings。

任一维度变化都使 cache miss，保证编译产物与运行环境严格匹配。

## 磁盘 cache 校验

cache header/object/hash 或权限校验失败时计为 invalidated 并重新编译。runtime 绝不执行未通过校验的 bytes。

## 深入了解

- [实例化与执行生命周期](instantiation-and-lifecycle.md)
- [缓存实现](../tooling/cache.md)
- [Direct Cranelift 后端概览](../backend/README.md)
- [Core 不变量](../reference/invariants.md)
