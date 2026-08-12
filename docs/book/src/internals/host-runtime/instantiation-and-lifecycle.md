# 实例化与执行生命周期

这一章说明 `NativeRuntime::execute` 的完整生命周期，从 artifact 加载到退出码返回。

## 执行步骤

```text
NativeRuntime::execute(artifact, options)
  1. 校验 owner thread
  2. 重置输出缓冲，恢复 startup snapshot
  3. 由 repository 对 artifact 执行 cache hit/load 或 direct compile
  4. 配置 module manifest、program/image registry 与 source metadata
  5. 发布当前 image 与 call/root/source frame
  6. 调用 typed native entry
  7. drain Promise/microtask/external event loop
  8. materialize/传播 JS exception，关闭 child resources
  9. 返回 stdout/stderr/exit code/cache stats
```

## Owner-thread 约束

`NativeRuntime` 持有 pinned `NativeVmContext` 和 mutable runtime state，不可跨线程共享。每次 `execute` 调用先校验 owner thread，非 owner 线程调用直接 panic。

每个 worker / test262 agent 创建独立 runtime，拥有独立 heap、scheduler 和 mutable side tables。跨 agent 只通过 structured clone、SAB/Atomics 和显式 IPC 传递。

## Image 加载

`NativeImageRepository` 是 image 与磁盘 cache 的唯一 owner。`NativeCacheKey` 绑定：

- portable artifact digest；
- native ABI hash；
- native codegen source hash；
- 当前 target；
- Cranelift 版本；
- codegen/ISA settings。

命中时直接加载已有 image；miss 时由 `NativeCompiler::compile` 从 IR 编译。同 key 的并发 prepare 由 in-flight gate 合并。

## 事件循环 drain

入口函数返回后，runtime 继续 drain：

- Promise reaction（`then`/`catch`/`finally`）；
- `queueMicrotask` 回调；
- `FinalizationRegistry` 回调；
- timer 回调；
- I/O completion。

drain 在所有待处理任务耗尽后结束。进程退出不需要显式保持存活。

## 退出码

| 情况 | 退出码 |
| --- | --- |
| 正常完成 | 0 |
| `process.exit(n)` | n |
| 未捕获异常 | 2 |
| 编译期错误 | 1（在执行前失败） |

`process.exit` 通过错误通道回传，不直接终止进程，让 diagnostics 缓冲区有机会刷出。

## 深入了解

- [Direct Cranelift 后端概览](../backend/README.md)
- [Promise、微任务与异步调度器](../runtime-features/async-scheduler.md)
- [Portable `.wjsm` 制品](../../user/output/portable-artifacts.md)
- [标准输出、标准错误与退出码](../../user/output/process-io.md)
