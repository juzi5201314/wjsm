# 运行时错误

运行时失败退出码是 `2`，`process.exit(n)` 的自定义码原样传出。

## 未捕获异常

```text
Uncaught exception: Error: boom
Runtime error: Uncaught exception: Error: boom
```

第一行来自运行时的诊断输出（stdout），第二行是 CLI 的错误摘要（stderr）。

## 未处理的 Promise 拒绝

```text
UnhandledPromiseRejectionWarning: Error: boom
```

这是警告而非致命错误，进程仍以 `0` 退出。给 Promise 补 `.catch()`。

## 堆预算耗尽

```text
Runtime error: JavaScript heap budget exhausted: requested 144 bytes with 1048576/1048576 bytes used
```

`--max-heap-size` 设得太小，或程序确实持有过多存活对象。提高上限，或减少常驻数据。

## 调用栈溢出

```text
Runtime error: RangeError: Maximum call stack size exceeded
  (shadow stack: sp=131048 + 48 > limit=131072)
```

递归过深或 `--shadow-stack-max` 太小。默认上限 16MiB，正常递归不会触及。

## indirect call type mismatch

```text
Caused by: wasm trap: indirect call type mismatch
```

出现在 locale 敏感方法（`toLocaleString`、`localeCompare`）等未实现的路径上。改用非 locale 版本，详见 [限制与已知差异](../runtime/limitations.md)。

## 子进程被拒绝

```text
child_process execution is disabled for 'echo';
set WJSM_CHILD_PROCESS_ALLOW to an allowlisted command or '*'
```

按提示设置允许列表。

## 文件读写被拒绝

默认只允许访问 cwd、入口目录（或 `--root`）和系统临时目录。用 `WJSM_FS_ALLOW_READ` 追加读根，`WJSM_FS_ALLOW_WRITE=1` 解除写限制。

## 深入了解

- [实例化与执行生命周期](../../internals/host-runtime/instantiation-and-lifecycle.md)
- [GC 选择、配置与不变量](../../internals/gc/configuration-and-invariants.md)
