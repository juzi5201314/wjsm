# ADR 0007：Inspector / CDP 与 wasmtime guest_debug

- 状态：Accepted
- 日期：2026-07-09
- 关联：issue #313（Inspector / Debugger 节）

## 背景

wjsm 需要 Node 风格的 `--inspect` / `--inspect-brk` 与 Chrome DevTools Protocol（CDP），
以便在源码行上断点、查看栈/变量并步进。既有能力仅有函数级 `wjsm_sourcemap` 与
`debugger;` 编译期 no-op，不足以支撑调试器。

## 决策

采用**混合暂停模型**：

1. **语句级宿主 safepoint（主路径）**  
   inspect 编译路径 lowering 发射 `Instruction::DebugCheck`；backend 在
   `CompileOptions.debug = true` 时生成对 `env.debug_break(line, col, flags)` 的
   异步 host call。`flags&1` 表示 `debugger;` 无条件暂停。断点表与步进状态在
   host 侧匹配。

2. **wasmtime `guest_debug`（观测路径）**  
   inspect 启用时 `Config::guest_debug(true)`（强制 Cranelift，拒绝 Winch），
   在 `debug_break` 内通过 `debug_exit_frames` / `FrameHandle::local` 读取
   帧局部（NaN-box `i64`），映射到 `wjsm_debug` 局部名表。

3. **CDP 传输**  
   loopback TCP：HTTP discovery（`/json`、`/json/list`、`/json/version`）+
   WebSocket（`tokio-tungstenite`）。最小域：Debugger + Runtime。

4. **元数据**  
   仅 debug 编译发射 custom section `wjsm_debug`（version=1：行表、locals、
   debugger PC）。不改动既有 `wjsm_sourcemap` 语义。

5. **CLI / Node API**  
   `--inspect[=host:port]`、`--inspect-brk[=host:port]`；builtin `node:inspector`
   提供 `open`/`close`/`url`（URL 由 runtime 写入 `__wjsm_inspector_url`）。

## 后果

- 默认执行路径无语句级插桩、无 guest_debug 性能税。
- 新增 `env.debug_break` host import（registry 末尾追加）；所有模块均 import，
  无 inspector 时立即返回。
- 启动 snapshot **不**包含 debugger 会话状态。
- CDP 未实现 Profiler/Network/DOM 等域；未知 method 返回 `-32601`。
- 步进采用简化模型：step 模式下下一次 `debug_break` 即停。

## 备选方案（未采纳）

| 方案 | 原因 |
|---|---|
| 纯 wasmtime PC 断点 | AOT 行→WASM PC 映射脆弱；config 注释曾称 breakpoints 未支持 |
| 纯 GC safepoint 复用 | 频率/语义与用户断点不符，且为 sync host |
| 升级 wasmtime 大版本再开 guest_debug | 43.x 已具备所需 API，无需大升级 |

## 验证

- `cargo nextest run -p wjsm-runtime --test inspector_cdp`
- `cargo nextest run -E 'test(inspect) | test(node_builtin_inspector)'`
- 手工：`wjsm run --inspect-brk -e 'let x=1; debugger; console.log(x)'` 后用 DevTools 连接
