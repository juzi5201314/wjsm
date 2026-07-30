# `wjsm.toml` 与 `wjsm.json`

项目配置文件用来固定一组全局默认值，省去每次在命令行重复输入。

## 查找规则

未传 `--config` 时，wjsm 在**当前工作目录**依次查找 `wjsm.toml`、`wjsm.json`，使用第一个存在的文件。查找不向上递归，也不跟随入口文件所在目录。

传 `--config <PATH>` 时只读该路径：扩展名为 `.json` 按 JSON 解析，其余一律按 TOML 解析。文件存在但内容非法会直接报错退出，不会静默忽略。

## 结构

两种格式都接受两种写法：键放在顶层，或放在 `cli` 表/对象下。存在 `cli` 时只读 `cli` 里的内容。

```toml
# wjsm.toml，顶层写法
stats = true
condition = ["development"]
root = "src"
```

```json
{
  "cli": {
    "stats": true,
    "condition": ["development"],
    "root": "src"
  }
}
```

## 可用键

键名使用 kebab-case。未列出的键会被忽略，不报错。

| 键 | 类型 | 对应选项 |
| --- | --- | --- |
| `quiet` | 布尔 | `-q` |
| `verbose` | 整数 | `-v` 的重复次数 |
| `time` | 布尔 | `--time` |
| `stats` | 布尔 | `--stats` |
| `verify-ir` | 布尔 | `--verify-ir` |
| `color` | `auto` / `always` / `never` | `--color` |
| `no-color` | 布尔 | `--no-color` |
| `target` | `wasm` / `jit` | `--target` |
| `browser` | 布尔 | `--browser` |
| `condition` | 字符串数组 | `--condition`（可重复） |
| `max-heap-size` | 整数（字节） | `--max-heap-size` |
| `root` | 路径 | 子命令的 `--root` |
| `script` | 布尔 | 子命令的 `--script` |

`max-heap-size` 在文件里必须写成字节整数，`K` / `M` / `G` 后缀只有命令行接受。

`root` 与 `script` 是子命令级选项，只对 `build`、`run`、`test`、`check`、`lint`、`dump-ir`、`dump-ast`、`dump-wat` 生效；`script` 额外作用于 `repl`。

## 不能通过文件配置的选项

`--gc`、`--inspect`、`--inspect-brk`、`--shadow-stack-max`、`--wasmtime-memory-reservation` 没有对应的配置键。写进文件不会报错，但完全不生效——只能用命令行或环境变量。

> <details><summary>为什么不把这几个选项也加进配置文件？</summary>
>
> 主要是「作用域」问题。这些选项都和执行环境绑定——不同的执行方式（CI 跑测试、本地开发、临时调试）可能需要不同的 GC、不同的 Inspector 端口、不同的影子栈上限。把它们放配置文件里会让「这个项目用什么配置」和「我打算怎么跑」混在一起：
>
> - 同一个 `wjsm.toml`，在 CI 上用 `--gc zgc`、本地开发用 `--gc mark-sweep`、调试时用 `--inspect`。
> - 把 GC 写进 `wjsm.toml` 后，CI 脚本想覆盖就得显式传 `--gc zgc`——这反而是「优先级反转」，应该 CI 不传就沿用项目默认。
>
> 设计上把「项目级风格配置」（`stats`、`time` 这类纯观察选项）和「执行环境配置」（`--gc`、`--inspect` 这类影响行为的选项）分开，是为了让配置文件能安全 commit 到 git 而不必担心 CI 行为被本地配置影响。
>
> </details>

## 深入了解

- [配置合并实现与键的单一 owner](../../internals/tooling/cli-and-config.md)
