# 配置来源与优先级

wjsm 的行为由四类输入决定，优先级从高到低：

| 优先级 | 来源 | 说明 |
| --- | --- | --- |
| 1 | 命令行选项 | `--inspect`、`--max-heap-size` 等 |
| 2 | 环境变量 | `WJSM_CACHE_DIR` 等 |
| 3 | 配置文件 | `wjsm.toml` / `wjsm.json`，项目级默认值 |
| 4 | 默认值 | 内置在代码里的 fallback |

高优先级来源显式给出值时，低优先级来源的对应项不生效——不是合并，是覆盖。

## 合并规则

- **CLI 参数**：只有用户在命令行上显式传了的选项才算「给出值」。通过 `command_line_global()` 检查 `ValueSource::CommandLine` 来判断。
- **环境变量**：在 CLI 未覆盖时生效。
- **配置文件**：只支持一部分全局选项（`quiet`、`verbose`、`time`、`stats`、`verify-ir`、`color`、`no-color`、`browser`、`condition`、`root`、`script`）。`--inspect`、`--max-heap-size` 等运行时选项不能写进配置文件。
- **默认值**：所有来源都没给出时使用，如堆上限默认 64 MiB。

## 示例

```bash
# 配置文件给出默认值，命令行不传则生效
# wjsm.toml: verify-ir = true
wjsm run app.js                               # IR 验证开启
wjsm run app.js                               # 同上

# 命令行显式传了就覆盖配置文件
wjsm run app.js                               # 配置文件 verify-ir=true 生效
# （没有 --verify-ir 的反选项，只能改配置文件）

# 环境变量覆盖配置文件
WJSM_CACHE_DIR=/tmp/wjsm-cache wjsm run app.js  # 覆盖磁盘缓存目录
```

## 不能写进配置文件的选项

以下选项只接受 CLI 或环境变量，写在 `wjsm.toml` 里会被忽略：

- `--max-heap-size`：运行时内存配置
- `--inspect` / `--inspect-brk`：调试器
- `--config`：配置文件路径本身

## 深入了解

- [`wjsm.toml` 与 `wjsm.json`](project-files.md)
- [环境变量](environment-variables.md)
- [命令行配置项](cli-options.md)
- [CLI 参数模型与配置合并](../../internals/tooling/cli-and-config.md)
