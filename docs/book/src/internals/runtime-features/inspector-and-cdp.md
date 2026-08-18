# Inspector 与 CDP

这一章说明 Chrome DevTools Protocol (CDP) 调试器的实现。

## 模块组织

`crates/wjsm-host-native/src/inspector/` 实现 CDP 调试器：

| 文件 | 内容 |
| --- | --- |
| `mod.rs` | Inspector 入口，管理会话 |
| `server.rs` | WebSocket 服务器 |
| `cdp.rs` | CDP 协议处理 |
| `state.rs` | 调试器状态（断点、暂停等） |
| `pause.rs` / `pause_ops.rs` | 暂停/恢复执行 |
| `remote_object.rs` | 远程对象表示 |
| `debug_info.rs` | 调试信息（源码映射、变量名） |

## 启用方式

`wjsm run` 用 `--inspect` / `--inspect-brk` 启用 inspector。packed exe 没有 clap，改读 `WJSM_INSPECT` / `WJSM_INSPECT_BRK` 或 `NODE_OPTIONS`。启用后走 debug lowering（插入 `DebugCheck`），packed 路径会忽略预编译 object、从制品内源码快照重新编译。

## CDP 协议

CDP 是 Chrome DevTools 使用的协议，wjsm 实现了其中与 JavaScript 调试相关的子集：

- `Debugger.enable` / `Debugger.disable`
- `Debugger.setBreakpoint` / `Debugger.setBreakpointByUrl`
- `Debugger.pause` / `Debugger.resume`
- `Debugger.stepOver` / `Debugger.stepInto` / `Debugger.stepOut`
- `Runtime.evaluate` / `Runtime.getProperties`
- `Console.messageAdded`

## 暂停机制

wjsm 通过 epoch interruption 实现暂停。`--inspect` 启用时，epoch interruption 开启，调试器在断点处 `increment_epoch` 触发中断。generated code 在 safepoint 检查 epoch，如果过期则进入暂停处理。

## 远程对象

`remote_object.rs` 把 JavaScript 对象表示为 CDP 的 RemoteObject。调试器通过 CDP 查询对象属性、调用方法等。这需要 `ExecContext` 的方法访问对象，与正常运行路径共用。

## 深入了解

- [Engine 配置与 epoch interruption](../startup/engine-pool.md)
- [变量活跃性、槽位与 GC Spill（safepoint）](../backend/liveness-slots-and-spills.md)
- [用户侧的 Inspector 调试](../../user/configuration/inspector.md)
