# 全局选项与通用规则

全局选项对所有子命令可用，位置不限：写在子命令前后效果相同。它们控制诊断输出、模块解析条件、内存上限、垃圾回收器和调试器。

## 诊断与输出控制

| 选项 | 作用 |
| --- | --- |
| `-q`, `--quiet` | 抑制非必要诊断。同时把 `--verbose` 的有效级别压回 0 |
| `-v`, `--verbose` | 可重复。`-v` 打印阶段进度，`-vv` 追加每阶段的规模统计 |
| `--time` | 打印各阶段耗时。无 `-v` 时以毫秒计，有 `-v` 时以微秒计 |
| `--stats` | 打印常量数、函数数、基本块数、指令数和 WASM 字节数 |
| `--verify-ir` | 在 lower 之后校验 IR 不变量，失败即中止 |
| `--color <auto\|always\|never>` | 颜色策略，默认 `auto` |
| `--no-color` | 关闭颜色。与 `--color` 互斥 |

`auto` 的判定顺序是：`CLICOLOR_FORCE` 非空且不为 `0` 则强制着色；否则 `NO_COLOR` 非空则关闭；否则检测 stdout 或 stderr 是否为终端。

`--time` 与 `--stats` 输出到 stderr，因此不会污染 `dump-*` 或 `build -o -` 的 stdout 数据流：

```bash
wjsm run --time --stats -e 'console.log(1)'
```

```text
=== Statistics ===
  Constants: 21
  Functions: 1
  Basic Blocks: 3
  Instructions: 46
  WASM Size: 25686 bytes
1
Timing: parse=6ms, lower=10ms, compile=6ms, execute=67ms
```

## 编译与解析

- `--target <wasm|jit>`，默认 `wasm`。`jit` 目前会在编译阶段报错退出，不是可用后端。
- `--browser` 启用 `browser` 包解析条件以及 `package.json` 的 `browser` 字段映射。
- `--condition <NAME>` 追加自定义包解析条件，可重复。
- `--config <PATH>` 指定配置文件；未指定时在当前目录查找 `wjsm.toml`、`wjsm.json`。

## 内存与垃圾回收

- `--max-heap-size <SIZE>` 限制 JavaScript 堆。
- `--shadow-stack-max <SIZE>` 设置影子栈软上限，默认 16M。
- `--wasmtime-memory-reservation <SIZE>` 覆盖 Wasmtime 线性内存的虚拟地址预留。
- `--gc <mark-sweep|g1|zgc>` 选择垃圾回收器，覆盖 `WJSM_GC` 与 `WJSM_TEST_GC`。

`SIZE` 接受纯字节数或 `K`/`KB`/`KiB`、`M`/`MB`/`MiB`、`G`/`GB`/`GiB` 后缀（大小写不敏感），必须大于零：

```bash
wjsm --max-heap-size 512M --shadow-stack-max 32M run app.js
```

## 调试器

`--inspect[=HOST:PORT]` 与 `--inspect-brk[=HOST:PORT]` 启用 Chrome DevTools Protocol 调试端点，默认地址 `127.0.0.1:9229`。

传值必须用 `=`，否则 clap 会把后面的子命令名当成地址：

```bash
wjsm --inspect=9229 run app.js     # 正确
wjsm --inspect 9229 run app.js     # 错误：9229 被当作地址后 run 无法解析
```

地址写法：纯数字为端口（`9229` → `127.0.0.1:9229`）；`:PORT` 同样绑定回环地址；`HOST:PORT` 原样使用；端口 `0` 表示由系统分配临时端口。两个选项同时出现时 `--inspect-brk` 生效，并在入口暂停。

> <details><summary>为什么 `--inspect 9229` 不工作？这是 clap 解析器的取舍</summary>
>
> clap 看到 `--inspect` 后，下一个 token 有两种合理解释：要么是 `--inspect` 的值，要么是另一个参数/子命令。它采用「短参数加空格分隔值」的常见约定：碰到一个看起来像参数的东西（数字也算）就停止接收 `--inspect` 的值。
>
> `--inspect=9229` 用 `=` 显式标记「这是参数值」，clap 就能可靠地切分。
>
> 这种约定在大多数 CLI 工具里通用（npm、cargo、git 都这样）。代价是偶尔要写等号——`--inspect=9229` 多按两次键，换来的是解析逻辑永远清晰。
>
> </details>

## 通用输入规则

- 接收源码的子命令都支持 `-e/--eval <SOURCE>` 直接传入源码。
- 文件参数写 `-` 表示从标准输入读取。
- `build` 的 `-o -` 表示写 stdout；当 stdout 是终端时会拒绝写入二进制。

## 深入了解

- [CLI 参数模型与配置合并](../../internals/tooling/cli-and-config.md)
- [源码输入与编译编排](../../internals/tooling/source-input.md)
- [阶段隔离与诊断输出](../../internals/pipeline/stage-isolation.md)
