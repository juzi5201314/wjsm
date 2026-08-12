# `run`

编译并立即执行一个 JS/TS 入口。这是日常使用 wjsm 的主要方式：编译产物不落盘，直接在同一进程内执行。

```text
wjsm run [OPTIONS] [INPUT] [-- <ARGS>...]
```

## 输入来源

| 写法 | 行为 |
| --- | --- |
| `wjsm run app.ts` | 编译并运行文件 |
| `wjsm run -e '<源码>'` | 运行内联源码，脚本名显示为 `[run-eval]` |
| `wjsm run -` | 从标准输入读取源码 |
| `wjsm run <script-name>` | 若该路径不存在，且 `package.json` 的 `scripts` 里有同名条目，则执行包脚本 |

入口文件与 `-e` 都不给会直接报错：`Either an input file or -e <code> is required`。

## 传递参数给脚本

`--` 之后的内容原样进入 `process.argv`：

```bash
wjsm run app.js -- --port 8080 input.txt
```

`process.argv[0]` 是 wjsm 可执行文件路径，`argv[1]` 是脚本路径（`-e` 模式下为 `[run-eval]`），其后是你的参数。

## 监听改动

```bash
wjsm run --watch app.ts
```

`--watch` 监听入口所在目录的文件改动并重新编译执行。它不能与包脚本一起用：对包脚本传 `--watch` 会报 `watch mode is not supported for package scripts`。

> <details><summary>`--watch` 的实现方式</summary>
>
> 内部并不是「单进程里热重载」——那需要 IR 重用、状态迁移、文件句柄回收等一系列问题，远比「重启一遍」复杂。wjsm 的选择是 fork：父进程监听文件，子进程跑代码；文件变化就杀掉子进程、起一个新的。
>
> 带来的几个可观察行为：
>
> - 改文件后大概 200ms 内看到新输出（合并窗口）。
> - 子进程的状态完全丢失——全局变量、`Map` 里塞的数据全没了。每次重启都像第一次跑。
> - `setInterval` 之类的持续任务在每次重启里都会再开一份（如果你在主进程代码里写的话）。通常做法是把 watcher 模式的代码结构设计成「每次跑都幂等」。
>
> </details>

## 模块与脚本解析

默认按 ES 模块解析。`--script` 切换为脚本解析，此时 `await` 可以当普通标识符用：

```bash
wjsm run --script -e 'var await = 1; console.log(await)'
```

`--root <DIR>` 指定模块解析根目录，多文件项目会从该根做 bundling。

## 退出码

`run` 的退出码来自被执行的程序：`process.exit(7)` 会让 wjsm 也以 7 退出。未捕获异常记为运行时错误，退出码 2。

## 深入了解

- [编译到执行的完整编排路径](../../internals/pipeline/orchestration.md)
- [编译缓存](../../internals/startup/compilation-cache.md)
- [实例化与执行生命周期](../../internals/host-runtime/instantiation-and-lifecycle.md)
