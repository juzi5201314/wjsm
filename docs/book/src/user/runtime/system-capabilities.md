# 文件系统、网络与进程能力

wjsm 默认给程序一组受限的系统能力。这一章说明默认边界在哪、怎么放开。

## 文件系统

读写都限制在一组根目录内，默认包含：

- 当前工作目录
- 入口文件所在目录（用 `--root` 时改为该目录）
- 系统临时目录

根目录之外的访问会抛错：

```js
// fs-demo.mjs
import fs from "node:fs";

fs.writeFileSync("/tmp/demo.txt", "ok");        // 临时目录在允许范围内
try {
  fs.writeFileSync("/etc/demo.txt", "x");        // 越界
} catch (error) {
  console.log("denied");
}
```

放开边界的两个环境变量：

| 变量 | 作用 |
| --- | --- |
| `WJSM_FS_ALLOW_READ` | 追加额外读根，用平台路径分隔符分隔多个路径 |
| `WJSM_FS_ALLOW_WRITE=1` | 取消写入路径限制 |

```bash
WJSM_FS_ALLOW_READ=/etc wjsm run fs-demo.mjs
WJSM_FS_ALLOW_WRITE=1 wjsm run fs-demo.mjs
```

> <details><summary>「沙箱」到底是什么层级？</summary>
>
> wjsm 的文件系统沙箱**不是**基于 OS 级别的强制隔离（如 chroot、容器）。它是 wjsm 宿主里的一段 Rust 代码，在 `fs.writeFileSync` 之类的函数被调用时做路径前缀检查：
>
> - 拒绝访问：抛 JS 错误，进程继续。
> - 允许访问：调用对应的 Rust std::fs 函数。
>
> 这意味着如果 wjsm 宿主有 bug 跳过检查，或者代码用其他机制（直接 syscall、原生模块）绕过，**沙箱就失效了**。WebAssembly 提供的内存隔离只保护「不越界访问线性内存」，不保护「不走 host import 自己想办法」。
>
> 在多租户、不可信代码场景下，应该用容器或 OS 级别的隔离（namespaces、cgroups），不要依赖 wjsm 自己的沙箱。wjsm 的沙箱是「让普通代码不能轻易乱写文件」，不是「让恶意代码没法搞破坏」。
>
> </details>

## 网络

`fetch` 可直接调用，支持 `data:` URL 和真实 HTTP 请求：

```bash
wjsm run -e 'fetch("data:text/plain,hello").then(r => r.text()).then(t => console.log(t))'
```

网络访问没有单独的开关，能否连通取决于宿主环境。`node:http`、`node:net`、`node:tls`、`node:https` 提供了各自已覆盖的能力子集。

## 子进程

子进程默认完全禁用。未配置 allowlist 时调用 `node:child_process` 会抛错：

```text
child_process execution is disabled for 'echo'; set WJSM_CHILD_PROCESS_ALLOW to an allowlisted command or '*'
```

用 `WJSM_CHILD_PROCESS_ALLOW` 放开，值是命令名列表，`*` 表示全部允许：

```bash
WJSM_CHILD_PROCESS_ALLOW=echo wjsm run spawn-demo.mjs
WJSM_CHILD_PROCESS_ALLOW='*' wjsm run spawn-demo.mjs
```

允许任意命令等于放弃这层隔离，只在明确需要时开启。

## 进程信息

`process` 是普通全局对象，`process.argv`、`process.env`、`process.platform`、`process.nextTick` 均可用：

```bash
WJSM_DEMO=1 wjsm run -e 'console.log(process.env.WJSM_DEMO, process.platform)'
```

## 深入了解

- [文件系统、进程与子进程的宿主实现与沙箱判定](../../internals/runtime-features/fs-process-and-child-process.md)
- [网络、HTTP 与 TLS 的宿主能力组织](../../internals/runtime-features/network-http-and-tls.md)
- [Host 能力 Trait 如何划分可授予的运行时能力](../../internals/host-runtime/host-traits.md)
