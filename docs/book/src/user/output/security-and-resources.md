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

> <details><summary>「Wasm 沙箱」是个被滥用得厉害的概念</summary>
>
> WebAssembly 的「沙箱」只有一层语义：WASM 模块的线性内存和宿主进程是隔离的，模块不能直接读写宿主内存（除了通过 import 显式提供的部分）。
>
> 这是**内存安全**层面的隔离，不是**系统调用安全**层面的隔离。一个 WASM 模块可以：
>
> - 通过 import 调用 `fetch` 发任意网络请求
> - 通过 import 调用文件操作读任意文件（如果宿主允许）
> - 通过 import 派生任意子进程
> - 占用任意多内存（直到 `--max-heap-size` 限制）
>
> 想要「完全沙箱」必须在这些 import 上加更严格的限制：网络黑名单、文件系统 chroot、cgroups 限制、seccomp 系统调用过滤……这些是容器/虚拟化层做的事，不是 WASM 本身。
>
> 写代码时记住：「我的 wjsm 程序能跑成功 = 它走完了 wjsm 沙箱允许的路径」——但「走完 wjsm 沙箱」不等于「不会访问你不想让它访问的资源」。
>
> </details>

## 深入了解

- [文件系统与子进程宿主实现](../../internals/runtime-features/fs-process-and-child-process.md)
- [Worker 线程模型与上限](../../internals/runtime-features/worker-threads.md)
- [`node:vm` 的 Realm 管理](../../internals/runtime-features/node-vm.md)
- [堆预算与 GC 配置不变量](../../internals/gc/configuration-and-invariants.md)
