# 全局选项

完整语法以 `wjsm --help` 为准。常用全局选项：

| 选项 | 作用 |
| --- | --- |
| `--config <PATH>` | 指定 `wjsm.toml` / `wjsm.json` |
| `-q/--quiet` | 抑制非必要诊断 |
| `-v/--verbose` | 增加阶段诊断，可重复 |
| `--time` | 输出 pipeline timing |
| `--stats` | 输出 IR 与 artifact 统计 |
| `--verify-ir` | 在继续 codegen 前验证 IR |
| `--color <auto\|always\|never>` / `--no-color` | 控制颜色 |
| `--browser` / `--condition <NAME>` | 模块解析条件 |
| `--gc <mark-sweep\|g1\|zgc>` | 选择 collector |
| `--max-heap-size <SIZE>` | 设置 JavaScript ManagedHeap 上限 |
| `--inspect[=HOST:PORT]` | 启动 CDP inspector |
| `--inspect-brk[=HOST:PORT]` | 启动并在入口暂停 |

项目只有一个 production execution backend，因此没有 target/compiler selector。不支持的宿主由 native compiler capability gate 拒绝。`--inspect` / `--inspect-brk` 必须用 `=` 传参（例如 `--inspect=9229`），避免把后续子命令名当成地址。

## 深入了解

- [CLI 参数模型与配置合并](../../internals/tooling/cli-and-config.md)
- [命令行索引](../reference/cli-index.md)
