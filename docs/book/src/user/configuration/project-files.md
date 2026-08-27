# `wjsm.toml` 与 `wjsm.json`

项目级配置文件，为常用 CLI 选项提供默认值，省得每次敲命令。

## 查找规则

1. `--config <PATH>` 显式指定 → 用该文件。
2. 否则从**当前工作目录**查找 `wjsm.toml`，找不到再找 `wjsm.json`。
3. 两个都不存在 → 不加载配置文件，所有值走环境变量或默认值。

查找只在当前目录进行，不向上递归到父目录。

## 支持的字段

所有字段可选，缺省字段等价于不设该选项：

| 字段 | 类型 | 对应 CLI 选项 |
| --- | --- | --- |
| `quiet` | bool | `-q` / `--quiet` |
| `verbose` | u8 | `-v`（可重复，0/1/2） |
| `time` | bool | `--time` |
| `stats` | bool | `--stats` |
| `verify-ir` | bool | `--verify-ir` |
| `color` | `"auto"` / `"always"` / `"never"` | `--color` |
| `no-color` | bool | `--no-color` |
| `browser` | bool | `--browser` |
| `condition` | string[] | `--condition`（可重复） |
| `root` | string | `--root` |
| `script` | bool | `--script` |

字段名在 TOML 中用 kebab-case（`verify-ir`），在 JSON 中同理。

## 示例

`wjsm.toml`：

```toml
verify-ir = true
browser = true
condition = ["development"]
verbose = 1
```

`wjsm.json`：

```json
{
  "verify-ir": true,
  "browser": true,
  "condition": ["development"],
  "verbose": 1
}
```

也支持把选项放在 `cli` 键下（两种格式都支持），等价于顶层写法：

```toml
[cli]
verify-ir = true
browser = true
```

## 覆盖规则

命令行显式传了的选项覆盖配置文件，没传的才用配置文件的值。判断方式是检查该参数是否来自命令行（`ValueSource::CommandLine`），不是检查值是否与默认值不同。

```bash
# 配置文件: verify-ir = true
wjsm run app.js            # verify-ir 生效（来自配置文件）
```

`--inspect`、`--max-heap-size` 等运行时选项不能写进配置文件，只能用 CLI 或环境变量。

## 深入了解

- [配置来源与优先级](sources-and-precedence.md)
- [CLI 参数模型与配置合并](../../internals/tooling/cli-and-config.md)
