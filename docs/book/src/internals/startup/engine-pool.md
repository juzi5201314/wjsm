# Engine 配置与池化

这一章说明 `NativeRuntime` 的 engine 初始化和 image repository 池化策略。

## Engine 初始化

`NativeCompiler::new()` 用当前宿主 ISA 初始化。`wjsm-backend-native/src/isa_config.rs` 是唯一构造 Cranelift ISA/flags 的地方。当前 production capability：

| 平台 | 状态 |
| --- | --- |
| x86_64 Linux | 支持 |
| x86_64 Windows | 支持 |
| 其他 | `UnsupportedTargetCapability`，fail-closed |

不支持的平台直接返回 capability error，不回退到其他 backend、解释器或 test heap。

## NativeImageRepository

`NativeImageRepository` 是进程内 image 与磁盘 cache 的唯一 owner。它持有：

- `Weak<CompiledImage>` 列表——调用方持 `Arc` 决定 image 生命周期；
- 磁盘 cache 目录——由 `WJSM_CACHE_DIR` 或 `$HOME/.cache/wjsm` 决定；
- in-flight gate——同 key 的并发 prepare 只编译一次。

repository 只保存 `Weak` 引用，image 的生命周期由调用方持有的 `Arc` 决定。这意味着没有活跃引用的 image 可以被回收，下次使用时重新从磁盘 cache 加载或重新编译。

## CompiledImage 生命周期

`CompiledImage` 拥有：

- executable mappings（W^X）；
- entry table（函数入口偏移）；
- source metadata（源码映射）；
- unwind registration。

drop 顺序**必须先注销 unwind，再释放 mapping**。function table 中不得永久缓存裸 code pointer——image 被 drop 后 pointer 失效。

## epoch interruption

`--inspect` / `--inspect-brk` 启用时，engine 开启 epoch interruption。调试器在断点处 `increment_epoch` 触发中断，generated code 在 safepoint 检查 epoch，过期则进入暂停处理。

epoch interruption 只在 inspector 启用时生效，不影响正常运行路径的性能。

## 深入了解

- [编译缓存](compilation-cache.md)
- [Engine 配置与实例池](../host-runtime/engine-configuration.md)
- [实例化与执行生命周期](../host-runtime/instantiation-and-lifecycle.md)
- [Inspector 与 CDP](../runtime-features/inspector-and-cdp.md)
