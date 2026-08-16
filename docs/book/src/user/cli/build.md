# `build`

把 JS/TS 构建为 portable semantic-IR artifact：

```bash
wjsm build app.ts -o /tmp/app.wjsm
wjsm build -e 'console.log(1)' -o /tmp/one.wjsm
wjsm build app.ts --format native-executable -o /tmp/app
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

`--format wjsm` 是默认且可跨平台携带的制品格式。`--format native-executable` 在当前宿主上打包预链 `wjsm-exec` stub、预编译 native object 与制品内源码快照，得到可直接运行的 ELF/PE。只支持 `--stage compile`；失败时不创建或覆盖输出文件。runtime 私有 object 本身不是 executable。

packed exe 离开构建机源码树后仍可在同 OS/arch 上运行：解析、JSON、动态 `import` / `require` 与 `new Worker` 只读快照。`import.meta` 使用虚拟身份 `file:///wjsm-exec/...`，不含构建机路径。快照 = 打包期实际读过的文件加上 `--include`；未命中则 fail-closed，不回退读盘。`--include` 只属于 native-executable，越界或缺文件则打包失败且不写输出。portable `.wjsm` 不带快照。`wjsm run` 仍读真实磁盘。

## 验证

```bash
wjsm validate /tmp/app.wjsm
wjsm size /tmp/app.wjsm
wjsm run /tmp/app.wjsm
```

构建产物不含机器码；目标/CPU、Cranelift 版本、native ABI 与 codegen settings 只进入当前宿主的 native cache key。
