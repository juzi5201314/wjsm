# `build`

把 JS/TS 构建为 portable semantic-IR artifact：

```bash
wjsm build app.ts -o /tmp/app.wjsm
wjsm build -e 'console.log(1)' -o /tmp/one.wjsm
```

`-o/--output` 默认是 `out.wjsm`，`-o -` 把二进制 artifact 写到非终端 stdout。多文件入口使用 `--root` 设置 module resolution root；`--script` 按 script 而非 module 解析。

## 流水线阶段

| `--stage` | 行为 |
| --- | --- |
| `parse` | 输出 SWC AST JSON |
| `lower` | 输出 semantic IR |
| `compile` | 编码并写出 portable `.wjsm` |
| `execute` | 构建后立即由当前宿主执行 |

`parse`/`lower` 的文本写 stdout，不能与文件输出混用。

## 输出格式

`--format wjsm` 是默认且唯一已实现的用户制品格式。`--format native-executable` 当前返回稳定的未实现错误、退出码 1，并且不创建或覆盖输出文件。runtime 私有 native object/image 不是平台 executable。

## 验证

```bash
wjsm validate /tmp/app.wjsm
wjsm size /tmp/app.wjsm
wjsm run /tmp/app.wjsm
```

构建产物不含机器码；目标/CPU、Cranelift 版本、native ABI 与 codegen settings 只进入当前宿主的 native cache key。
