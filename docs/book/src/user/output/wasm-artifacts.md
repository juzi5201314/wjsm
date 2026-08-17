# 制品与宿主要求

`.wjsm` 携带 verified semantic IR，不携带机器码。编译到 native image 发生在运行时，绑定当前宿主。`--format native-executable` 则把预编译 object 打进同宿主 ELF/PE，不能跨平台携带。

## Portable `.wjsm`

| 组成 | 说明 |
| --- | --- |
| verified semantic IR | 经过 `Program::verify()` 校验的语义 IR |
| canonical module manifest | 模块图、required builtins、导出表 |
| semantic ABI / hash | 用于跨宿主兼容性检查 |
| 可选 source map 与 source text | 供 inspector 堆栈映射和错误定位 |

同一 `.wjsm` 可以在支持平台间携带。`validate` 不生成机器码。`run` / `disasm` / `size` 需要受支持的宿主：当前生产 capability 是 **x86_64 Linux** 与 **x86_64 Windows**。不支持的宿主在 native compiler 初始化时 fail-closed：

```text
Error: native backend capability error: unsupported host ...
```

```bash
wjsm validate /tmp/app.wjsm
wjsm run /tmp/app.wjsm
```

设置了 `WJSM_CACHE_DIR` 时，`run` 才按 artifact digest、native ABI hash、codegen hash、target、Cranelift 版本和 settings 查找或写入 `.wnat`。

## 同宿主 `native-executable`

```bash
wjsm build app.ts --format native-executable -o /tmp/app
```

打包内容是预链 `wjsm-exec` stub、portable `.wjsm`、预编译 `NativeObject` 与制品内源码快照。overlay 正文整层 zstd。它不是 portable 制品，也不能把 runtime-private object 改后缀冒充。只支持 `--stage compile`。失败时不创建或覆盖输出文件。

拷走 exe 后只读快照，不依赖构建机源码树：

- 解析、JSON、动态 `import` / `require`、静态 `new Worker` / `fork` 与 `__dirname` 下的 `fs` 读走快照；
- `import.meta` 与 `process.argv[1]` 使用虚拟身份 `/wjsm-exec/...`；
- `cluster.fork` / `child_process.fork` 再执行同一个 exe；
- 快照外模块与虚拟路径写操作明确失败，不回退读盘；
- 计算出来的入口仍须 `--include`；相对 `cwd` 的文件名不是快照。

`--include` 只属于 native-executable；越界或缺文件则打包失败且不写输出。发行物需要同时带 `wjsm` 与 `wjsm-exec`（或设置 `WJSM_EXEC_STUB`）。packed exe 没有 clap；用 `WJSM_INSPECT` / `WJSM_INSPECT_BRK` 或 `NODE_OPTIONS` 里的 `--inspect` / `--inspect-brk` 启用 CDP。此时忽略预编译 object，从快照源码按 debug lowering 重新编译。

`wjsm run` 与 portable `.wjsm` 仍读真实磁盘。

## 深入了解

- [Portable `.wjsm` 制品](portable-artifacts.md)
- [`build`](../cli/build.md)
- [`validate`](../cli/validate.md)
- [预编译执行与磁盘缓存](../../internals/tooling/precompiled-execution.md)
- [Direct Cranelift 后端](../../internals/backend/index.html)
