# 命令行配置项

全局选项放在子命令之前，对所有子命令生效。完整语法以 `wjsm --help` 为准。

## 全局选项

| 选项 | 取值 | 作用 |
| --- | --- | --- |
| `--config <PATH>` | 路径 | 指定配置文件（默认从当前目录找 `wjsm.toml`/`wjsm.json`） |
| `-q` / `--quiet` | flag | 抑制非必要诊断 |
| `-v` / `--verbose` | 可重复 | 增加阶段诊断（`-v` 进度，`-vv` 细节） |
| `--time` | flag | 输出各 pipeline 阶段耗时 |
| `--stats` | flag | 输出 IR 与 artifact 统计（常量、函数、块、指令数、产物大小） |
| `--verify-ir` | flag | 继续到 codegen 前验证 IR 不变量 |
| `--color <WHEN>` | `auto`/`always`/`never` | 控制输出颜色；也响应 `NO_COLOR` 环境变量 |
| `--no-color` | flag | 等价 `--color never`，与 `--color` 互斥 |
| `--browser` | flag | 启用 `browser` 解析条件和 `package.json` 的 `browser` 字段 |
| `--condition <NAME>` | 字符串，可重复 | 追加自定义模块解析条件 |
| `--max-heap-size <SIZE>` | 整数+后缀 | JavaScript 对象堆上限，默认 `64M`；支持 `K`/`M`/`G` 后缀 |
| `--inspect[=HOST:PORT]` | 可选地址 | 启动 CDP inspector，默认 `127.0.0.1:9229` |
| `--inspect-brk[=HOST:PORT]` | 可选地址 | 启动 inspector 并在入口暂停 |

## 与子命令的组合

全局选项放在子命令之前：

```bash
wjsm --verify-ir run app.ts
wjsm --inspect=9229 run app.js
wjsm -v --time build app.ts -o out.wjsm
```

`--inspect` 和 `--inspect-brk` 必须用 `=` 传参（`--inspect=9229`），否则会把后续子命令名吞掉当地址解析。

## 地址解析规则

`--inspect` / `--inspect-brk` 的地址参数支持三种写法：

| 写法 | 解析结果 |
| --- | --- |
| `9229` | `127.0.0.1:9229` |
| `127.0.0.1:9229` | 原样 |
| `0` | `127.0.0.1:0`（系统分配端口） |

## 堆大小解析

`--max-heap-size` 接受带后缀的整数：

| 写法 | 字节数 |
| --- | --- |
| `--max-heap-size 128M` | 128 × 1024² = 134217728 |
| `--max-heap-size 1G` | 1024³ = 1073741824 |
| `--max-heap-size 512K` | 512 × 1024 = 524288 |
| `--max-heap-size 67108864` | 67108864（无后缀=字节） |

`0` 会被拒绝（`heap size must be greater than zero`）。

## 退出码

| 码 | 含义 |
| --- | --- |
| 0 | 成功 |
| 1 | 编译错误 |
| 2 | 运行时错误 |
| 3 | 用法错误 |

## 深入了解

- [全局选项参考](../cli/global-options.md)
- [环境变量](environment-variables.md)
- [配置来源与优先级](sources-and-precedence.md)
- [CLI 参数模型与配置合并](../../internals/tooling/cli-and-config.md)
