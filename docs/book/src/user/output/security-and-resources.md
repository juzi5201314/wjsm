# 文件系统权限与资源边界

wjsm 默认不给程序完整的机器访问权。本章列出默认边界和放开方式。

## 文件系统

读写根都是同一组路径：当前工作目录、入口文件所在目录（或 `--root` 指定的目录）、系统临时目录。根之外的读写被拒绝。

| 需求 | 做法 |
| --- | --- |
| 额外读取目录 | `WJSM_FS_ALLOW_READ`，多个路径用平台分隔符（Linux `:`） |
| 写入任意位置 | `WJSM_FS_ALLOW_WRITE=1` |

放开写权限等于取消该层保护，只在明确需要时使用。

## 子进程

`child_process` 默认全部拒绝，错误消息会指出解决方式：

```text
child_process execution is disabled for 'echo'; set WJSM_CHILD_PROCESS_ALLOW to an allowlisted command or '*'
```

`WJSM_CHILD_PROCESS_ALLOW` 接受命令名列表，或 `*` 放开全部。

## 内存与并发上限

| 资源 | 默认 | 覆盖方式 |
| --- | --- | --- |
| JavaScript 堆 | 不限制 | `--max-heap-size` |
| 影子栈 | 16 MiB | `--shadow-stack-max` / `WJSM_SHADOW_STACK_MAX` |
| Wasmtime 线性内存预留 | Wasmtime 默认 | `--wasmtime-memory-reservation` |
| Worker 线程数 | 32 | `WJSM_WORKER_THREADS_MAX` |
| `node:vm` 活跃 Realm 数 | 1024 | `WJSM_VM_MAX_REALMS` |

堆超限报 `JavaScript heap budget exhausted`，影子栈超限报 `RangeError: Maximum call stack size exceeded`。

## 环境变量与网络

`process.env` 暴露启动时的完整进程环境，wjsm 不做过滤。需要隔离时在启动前清理环境。

网络访问（`fetch`、`node:net`、`node:http`）没有独立的开关，能力等同于宿主进程的网络权限。

## 沙箱定位

WebAssembly 的内存隔离防止程序越界访问宿主内存，但不限制通过宿主 import 发起的系统调用。上面几项是实际的能力边界，别把「运行在 Wasm 里」当成完整沙箱。

## 深入了解

- [文件系统与子进程宿主实现](../../internals/runtime-features/fs-process-and-child-process.md)
- [Worker 线程模型与上限](../../internals/runtime-features/worker-threads.md)
- [`node:vm` 的 Realm 管理](../../internals/runtime-features/node-vm.md)
- [堆预算与 GC 配置不变量](../../internals/gc/configuration-and-invariants.md)
