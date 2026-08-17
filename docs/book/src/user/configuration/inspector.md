# Inspector 调试

wjsm 内置 Chrome DevTools Protocol (CDP) 调试器，通过 `--inspect` / `--inspect-brk` 启用。

## 启用

```bash
# 启动 inspector，默认 127.0.0.1:9229
wjsm --inspect run app.js

# 指定端口
wjsm --inspect=9230 run app.js

# 指定地址和端口
wjsm --inspect=0.0.0.0:9229 run app.js

# 启动并在入口暂停
wjsm --inspect-brk run app.js
```

启动后 stderr 输出：

```text
Debugger listening on ws://127.0.0.1:9229/...
```

`--inspect` 必须用 `=` 传参（`--inspect=9229`），否则会把后续子命令名当地址解析。packed exe 改用 `WJSM_INSPECT` / `WJSM_INSPECT_BRK` 或 `NODE_OPTIONS`。

## 连接方式

### Chrome DevTools

打开 `chrome://inspect`，点击 "Configure" 添加 `HOST:PORT`（默认 `localhost:9229`），目标会出现在 remote target 列表中。

### 直接连接

用 WebSocket 客户端连接 inspector 输出的 `ws://` 地址，按 CDP 协议发消息。

## 功能

wjsm 实现了 CDP 的 JavaScript 调试子集：

| CDP 命令 | 作用 |
| --- | --- |
| `Debugger.enable` / `disable` | 启用/停用调试 |
| `Debugger.setBreakpointByUrl` | 按源码位置设断点 |
| `Debugger.setBreakpoint` | 按 location 设断点 |
| `Debugger.pause` / `resume` | 暂停/恢复执行 |
| `Debugger.stepOver` / `stepInto` / `stepOut` | 单步 |
| `Runtime.evaluate` | 在暂停上下文执行表达式 |
| `Runtime.getProperties` | 查看对象属性 |

## 工作方式

wjsm 通过 epoch interruption 实现暂停：

1. `--inspect` 启用时，epoch interruption 开启。
2. 调试器在断点处 `increment_epoch` 触发中断。
3. 执行中的代码在下一个 safepoint 检查 epoch，过期则进入暂停处理。
4. 暂停期间调试器通过 CDP 响应前端请求。

## 限制

- **safepoint 依赖**：暂停只在 safepoint 生效。纯计算密集的代码路径如果长时间不到达 safepoint，断点不会立即触发。
- **非所有路径支持**：部分 native 代码路径不检查 epoch，在这些路径上无法暂停。
- **断点精度**：源码映射依赖 debug info，优化等级降低映射质量。
- **性能开销**：启用 inspector 会走 debug lowering（插入 `DebugCheck`），packed exe 还会忽略预编译 object、从源码快照重编译。

## 深入了解

- [Inspector 与 CDP 实现](../../internals/runtime-features/inspector-and-cdp.md)
- [命令行配置项](cli-options.md)
- [变量活跃性、槽位与 GC Spill（safepoint）](../../internals/backend/liveness-slots-and-spills.md)
