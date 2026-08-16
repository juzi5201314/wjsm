# 标准输出、标准错误与退出码

## stdout

程序主动产生的输出写 stdout，例如 `console.log`、`process.stdout.write` 和 `wjsm eval` 的求值结果。`build -o -` 也写 stdout，但只允许重定向到非终端目标，避免 portable artifact 二进制污染终端。

## stderr

CLI 与 runtime 诊断写 stderr：

- parse/lower/artifact/native compile 错误；
- 未捕获 JavaScript exception 摘要；
- `--verbose`、`--time`、`--stats` 输出；
- `out.wjsm` 覆盖警告与 watch 状态。

因此可独立重定向程序输出与诊断：

```bash
wjsm run app.ts >program.out 2>diagnostics.log
```

## 退出码

| 码 | 含义 |
| --- | --- |
| `0` | 成功 |
| `1` | parse/lower/build/artifact validation 等编译侧失败 |
| `2` | 未捕获 runtime 错误 |
| `3` | CLI 参数用法错误 |
| 其他 | `process.exit(n)` 请求的状态 |

`--format native-executable` 属于编译侧打包：成功写出同宿主 ELF/PE；失败返回 1，且不会创建或覆盖目标文件。
