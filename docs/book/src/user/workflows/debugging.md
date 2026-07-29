# 调试与诊断

调试手段分两类：观察编译流水线的中间产物，或用 Chrome DevTools 附加到运行中的程序。

## 定位失败发生在哪个阶段

诊断信息本身会告诉你阶段。解析和语义错误带源码位置并以退出码 1 结束；运行时错误以 `Uncaught exception:` 开头并以退出码 2 结束。确认阶段后再选工具：

```bash
wjsm check app.ts                 # 只解析和检查，不执行
wjsm dump-ast -e 'const x = 1'    # 解析结果
wjsm dump-ir -e 'const x = 1'     # 语义降级结果
wjsm dump-wat -e 'const x = 1'    # 代码生成结果
```

相邻两个阶段的输出对比通常足以定位问题：AST 正确而 IR 不对，说明问题在语义降级；IR 正确而 WAT 不对，说明问题在后端。

## 流水线耗时与统计

```bash
wjsm run app.ts --time --stats
```

`--time` 打印 parse / lower / compile / execute 四段耗时，`--stats` 打印常量数、函数数、基本块数、指令数和 WASM 字节数。加 `-v` 后 `--time` 从毫秒切换到微秒。

## IR 校验

`--verify-ir` 在 lower 阶段之后检查 IR 不变量，不通过就直接报错。怀疑是语义层生成了非法 IR 时打开它。

## 附加调试器

```bash
wjsm --inspect=9229 run app.ts
wjsm --inspect-brk=9229 run app.ts
```

启动后终端打印 `Debugger listening on ws://127.0.0.1:9229/<uuid>`，在 Chrome 打开 `chrome://inspect` 连接。`--inspect-brk` 会在入口暂停等待客户端，没有客户端连接时程序不会继续执行。

支持的能力是运行时已实现的那部分 CDP，不等同于完整的 Node.js inspector。

## `debugger` 语句

`debugger` 在 wjsm 中是编译期空操作，不会触发断点，`wjsm lint` 会就此给出 `debugger-noop` 提示。请在 DevTools 里设置断点，而不是依赖 `debugger` 语句。

## 深入了解

- [分层调试流程：如何按阶段隔离故障](../../internals/testing/debugging-workflow.md)
- [Inspector 与 CDP 的实现边界和 guest_debug 依赖](../../internals/runtime-features/inspector-and-cdp.md)
- [各阶段诊断输出的 owner 与阶段隔离规则](../../internals/pipeline/stage-isolation.md)
