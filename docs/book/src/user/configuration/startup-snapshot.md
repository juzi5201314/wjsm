# 启动快照与嵌入工件

wjsm 在构建可执行文件时把 primordial JS heap 状态序列化成二进制，嵌入 `wjsm` / `wjsm-exec`。`NativeRuntime` 启动时始终从这些字节恢复，跳过重复的 builtin bootstrap。

当前没有环境变量可以关闭快照。冷启动对比请测量进程墙钟时间，或只关闭磁盘缓存（不设 `WJSM_CACHE_DIR`）。

## 快照内容

快照捕获的是 seed 模块引导后的 primordial 堆状态：对象区字节、句柄表、shape table 与 host 侧字符串 / native callable 表。

不捕获 timer、microtask、promise 队列、scheduler、worker 或用户对象。这些在新实例里保持空状态。

## 校验

恢复前会核对：

- snapshot 容器与 host-state 版本；
- `bootstrap_hash`、`NATIVE_CODEGEN_HASH`、semantic ABI hash、native ABI hash；
- 当前 `{ARCH}-{OS}` 与 endian；
- 对象堆基址与容量是否落在本次 `ManagedHeapLayout` 内。

不匹配时启动失败，不会静默换另一套堆实现。重新构建 `wjsm` 会按当前 bootstrap 重新生成嵌入快照。

## 恢复流程

1. 分配当前 GC 算法与 `--max-heap-size` 对应的 `NativeHeapMemory`。
2. 解码嵌入字节并校验期望指纹。
3. 恢复对象区、句柄与 shape table，再装入 host 字符串和 native callable。
4. 补齐 intrinsic 原型后进入用户代码。

连续创建多个 Realm 时，每次都从原始嵌入字节开始恢复，而不是克隆上一个 Realm 的可变堆。

## 深入了解

- [嵌入工件与启动快照](../../internals/startup/embedded-artifacts.md)
- [修改快照与嵌入工件](../../internals/development/changing-snapshots.md)
- [ADR 0003: Startup Snapshot Boundary](../../../../adr/0003-startup-snapshot-boundary.md)
