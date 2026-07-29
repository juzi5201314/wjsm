# 文件系统、进程与子进程

这一章说明 `node:fs`、`node:child_process` 和进程管理的实现。

## node:fs

`crates/wjsm-host-wasm/src/runtime_node_fs.rs` 实现 `node:fs` 模块。提供同步和异步 API：

| API | 行为 |
| --- | --- |
| `readFileSync` | 同步读文件，返回 Buffer/字符串 |
| `writeFileSync` | 同步写文件 |
| `readdirSync` | 同步列目录 |
| `statSync` | 同步获取文件信息 |
| `readFile` | 异步读文件，返回 Promise |
| `writeFile` | 异步写文件 |

异步 API 通过 `AsyncHostCompletion` channel 调度，在 scheduler owner 上执行 I/O。

## node:child_process

`crates/wjsm-host-wasm/src/runtime_node_child_process/` 实现子进程：

| 文件 | 内容 |
| --- | --- |
| `spawn_async.rs` | 异步 spawn |
| `ipc.rs` | IPC 通道 |
| `child_message_callbacks.rs` | 子进程消息回调 |

`spawn` 启动子进程，通过 stdout/stderr/stdio 管道与子进程交互。`exec` 和 `execFile` 是 spawn 的 Promise 包装。`fork` 是 spawn 的特殊形式，启动另一个 wjsm 进程。

## 进程对象

`runtime_process.rs` 实现 `process` 全局对象：

- `process.argv`：命令行参数数组。
- `process.env`：环境变量对象。
- `process.exit(code)`：退出进程，`process_exit_code` 返回退出码。
- `process.cwd()`：当前工作目录。
- `process.platform`：平台字符串。

## 退出码

`process_exit_code` 和 `process_exit_diagnostics` 是公开导出的函数。退出码：0 成功，1 编译错误，2 运行时错误，3 用法错误。

## 深入了解

- [Timer、Event 与 Stream](timers-events-and-streams.md)
- [Worker Threads](worker-threads.md)
- [用户侧的文件系统、网络与进程能力](../../user/runtime/system-capabilities.md)
