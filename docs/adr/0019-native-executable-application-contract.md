# ADR 0019: native-executable 的 packed 应用合同

## Status

Accepted（2026-08-16）

Amends ADR 0016 §5 与 ADR 0017。不改变 stub+overlay、同宿主、不改写 PE/ELF 头、不调用系统 linker、不拆共享库，或 overlay 整层 zstd。

## Context

ADR 0016–0018 已经能产出可搬走的单文件 ELF/PE：预链 `wjsm-exec`、预编译 `NativeObject`、canonical `.wjsm` 与源码快照。主程序启动走 `execute_precompiled`，不从 IR 再 codegen。

第一刀仍把 packed exe 当成「能跑主图的启动器」：特化关闭、`process.argv` 重复 exe 名、`cluster.fork` / `child_process.fork` 仍 `spawn(execPath, ['run', script])`、`node:fs` 只读真盘、stdout 缓冲到进程退出、packed 无 inspector。这些使单文件无法当作 Node 式应用分发。

用户锁定：禁止 `libwjsm.so`、禁止合 guest `.text` 进 `PT_LOAD`、禁止自解压 trampoline；stub 保留 Cranelift；overlay 继续同时携带 `.wjsm` 与预编译 object。

## Decision

### 1. 进程身份

packed 的 `process.argv` 为：

```text
[execPath, /wjsm-exec/<entry>, ...用户参数]
```

与 `node app.js args` 同形。`wjsm-exec` 只把 `args().skip(1)` 交给 `configure_process_arguments`。`process.execPath` 是当前 exe。快照模式下 `process.__wjsm_packed === true`，供 fork/cluster 判断；不是用户 API。

### 2. fork / cluster 再执行同一 packed 文件

`cluster.fork` 与 `child_process.fork` 在 packed 下 `spawn(execPath, userArgs, { ipc })`，禁止拼 `run` 子命令。

内部环境变量 `WJSM_EXEC_ENTRY`（仅 parent 设置，不作文档承诺）：

- 未设：执行预编译主图。
- 设为 logical URL：从快照 lowering+codegen 该入口后执行；未命中快照则 fail-closed。
- `cluster.fork` 同主入口时不设此变量，只传 `NODE_UNIQUE_ID` / IPC。

### 3. 快照是虚拟路径的 fs owner

`/wjsm-exec/...` 与 `file:///wjsm-exec/...` 的 `fs` 读（`readFile` / `stat` / `exists` / `access` / `realpath` / `readdir`）只走 `ModuleSourceStore`。写、chmod、unlink、mkdir 等返回 EROFS。`cwd`、`/tmp` 与其它绝对路径仍走真盘。相对 `cwd` 的 `'data.json'` 不是快照。

### 4. 直出 I/O

`NativeRuntimeConfig` 增加 `OutputMode::{Capture, Inherit}`。`Capture` 是默认，供进程内 fixture。`wjsm-exec` 与 `wjsm run` / `wjsm eval` 用 `Inherit`：`console` 与 `process.stdout.write` 立即写 OS 流。

### 5. 特化默认开

废止 ADR 0016 §5「第一刀关闭特化」。`wjsm-exec` 与 `wjsm run` 一样读 `WJSM_DISABLE_SPECIALIZATION`。

### 6. inspector 从环境启用

packed exe 无 clap。`WJSM_INSPECT` / `WJSM_INSPECT_BRK` 与 `NODE_OPTIONS` 里的 `--inspect` / `--inspect-brk` 启用 CDP。此时忽略预编译 object，从快照源码按 `debug_codegen` 重新 lowering（DebugCheck 是 lowering 产物，既有 `.wjsm` 没有插桩）。

### 7. 启动只读 overlay；settings 参与身份校验

`wjsm-exec` 读 footer 后只读 payload，不把 stub 再读进第二份缓冲。`verify_stub_identity` 校验 codegen settings。release stub 使用 thin LTO 与 strip。

### 8. 静态 Worker / fork 补入快照

不打包整棵 `--root`。打包扫描已记录源码里的静态 `new Worker('./x')`、`fork('./x')`、`cluster.setupMaster({ exec: './x' })` / `setupPrimary`，根内相对路径自动纳入；缺文件则打包失败。`--include` 仍用于计算路径。

### 9. Owner

| 职责 | Owner |
| --- | --- |
| packed 进程身份 / 子进程入口 | `wjsm-exec` |
| 输出模式、inspect 环境、快照入口编译 | `wjsm-host-native` |
| 虚拟路径 fs | `wjsm-host-native` + `wjsm-module` store |
| fork/cluster JS 协议 | `wjsm-module` builtin JS |
| 静态补入与原子打包 | `wjsm-cli` |
| footer / 按路径 unpack / settings 校验 | `wjsm-exec-format` |

## Consequences

- packed exe 可在同 OS/arch 上当作 Node 式单文件应用分发。
- stub 仍含 compiler；eval / worker / 动态 import / 特化 / inspect 重编保持完整。
- 发行物仍需同时提供 `wjsm` 与 `wjsm-exec`（工具链）。用户制品仍是一个 exe。

## Non-goals

- 把 guest `.text` 合进 stub `PT_LOAD`
- stub 自解压 / UPX
- `libwjsm.so` 或任何旁路共享库
- 交叉编译、macOS
- 把 N-API `.node` 打进单文件
- 默认打包整个 `--root`
- 从 stub 拿掉 Cranelift / 从 overlay 拿掉 `.wjsm`
- Windows 代码签名、图标、version resource

## Verification

- 搬走 exe 并删除源码树后：argv 身份、fork/cluster、快照 `fs.readFileSync(__dirname + '/x')`、写虚拟路径 EROFS、cwd 诱饵不泄漏。
- 直出：子进程先打印再读 stdin，父进程在写入 stdin 之前读到 stdout。
- settings 失配拒绝；`WJSM_DISABLE_SPECIALIZATION=1` 仍可关特化。

## References

- ADR 0016 — 同宿主 native executable 为 stub + overlay
- ADR 0017 — 制品内源码快照
- ADR 0018 — overlay 整层 zstd
- ADR 0007 — Inspector / CDP
