# 启动快照与嵌入工件

`wjsm` / `wjsm-exec` 在构建期嵌入一份启动种子（`startup_snapshot.bin`）。`NativeRuntime` 启动时**始终**恢复它。没有环境变量可以关闭。

这份种子不是 builtin JS，也不是引导完的 primordial 堆。它只提供 global object（handle `0`）、空 shape table 和 `EvalIndirect`。`Object.prototype` 等在恢复后当场分配。`node:` 模块仍在编译期 lower。

冷启动对比请关磁盘缓存（`WJSM_CACHE_DIR=` 设为空串，或使用 `wjsm-bench --cold`）。`--cold` 每轮清空编译缓存，不关闭启动快照。

## 校验

恢复前会核对容器与 host-state 版本、`bootstrap_hash`、`NATIVE_CODEGEN_HASH`、semantic/native ABI、`{ARCH}-{OS}` 与 endian，以及对象堆基址是否落在本次布局内。

不匹配时启动失败，不会静默换另一套堆实现。重新构建 `wjsm` 会按当前源码指纹重写嵌入种子。

## 恢复流程

1. 按当前 GC 与 `--max-heap-size` 分配 `NativeHeapMemory`。
2. 解码嵌入字节并校验指纹。
3. 恢复对象区、句柄、shape table 与 host state。
4. `ensure_intrinsic_prototypes` 补齐原型后进入用户代码。

连续创建多个 Realm 时，每次都从原始嵌入字节开始恢复。

## 深入了解

- [嵌入工件与启动快照](../../internals/startup/embedded-artifacts.md)
- [修改快照与嵌入工件](../../internals/development/changing-snapshots.md)
- [ADR 0003: Startup Snapshot Boundary](../../../../adr/0003-startup-snapshot-boundary.md)
