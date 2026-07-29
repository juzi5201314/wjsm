# 模块解析条件

两个全局选项控制 `package.json` 的 `exports` / `imports` 条件匹配和 `browser` 字段处理：`--browser` 和 `--condition`。

## 条件顺序

解析器按固定顺序匹配条件键，第一个命中的分支胜出：

1. `wjsm`
2. `browser`（仅在传了 `--browser` 时）
3. `--condition` 给出的自定义条件，按命令行顺序
4. `node`
5. 边类型：`import` 或 `require`
6. `default`

`wjsm` 永远排在最前，包里存在 `"wjsm"` 键时它会遮蔽其余所有分支。

`--condition` 里的保留名会被忽略：`wjsm`、`browser`、`node`、`import`、`require`、`default`。想启用 `browser` 条件要用 `--browser`，而不是 `--condition browser`。

## 示例

```json
{
  "exports": {
    ".": {
      "browser": "./browser.js",
      "development": "./dev.js",
      "node": "./node.js",
      "default": "./default.js"
    }
  }
}
```

```bash
wjsm run app.mjs                            # → node.js
wjsm --browser run app.mjs                  # → browser.js
wjsm --condition development run app.mjs    # → dev.js
wjsm --browser --condition development run app.mjs  # → browser.js
```

最后一条命中 `browser`，因为 `browser` 排在自定义条件之前。

## browser 字段

`--browser` 同时启用 `package.json` 的 `browser` 字段。该字段有两种形式：字符串表示替换包入口，对象表示按路径重映射，值为 `false` 时该模块解析为空模块。不传 `--browser` 时字段被完全忽略。

## 配置文件

两项都能写进 `wjsm.toml`：

```toml
browser = true
condition = ["development"]
```

命令行显式给出 `--browser` 或 `--condition` 时，配置文件里的对应项不生效。

## 深入了解

- [条件解析、`exports` 匹配与 browser 映射的实现](../../internals/modules/package-conditions.md)
- [模块图构建与解析器如何缓存 package.json](../../internals/modules/graph-and-resolution.md)
