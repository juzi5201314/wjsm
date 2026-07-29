# 项目结构与配置文件

wjsm 不强制目录约定。一个项目只需要入口文件；`package.json` 决定 `.js` 的模块格式，
`wjsm.toml` / `wjsm.json` 提供命令行默认值。

## 最小项目

```text
myapp/
  package.json
  main.js
```

`wjsm init myapp` 生成的就是这个结构，`package.json` 里的 `"type": "module"` 让 `.js` 按 ES 模块解析。

## 常见布局

```text
myapp/
  package.json        模块格式、依赖、scripts
  wjsm.toml           wjsm 命令行默认值（可选）
  node_modules/       依赖
  src/
    main.ts           入口
    util/…            被导入的模块
  tests/
    math.test.ts      wjsm test 会发现 *.test.ts
```

```bash
wjsm run src/main.ts --root .
wjsm test tests
wjsm build src/main.ts --root . -o dist/app.wasm
```

## `package.json`

wjsm 读取其中四类信息：

| 字段 | 用途 |
| --- | --- |
| `type` | `.js` 按 ESM 还是 CommonJS 解析 |
| `exports` / `imports` / `main` / `module` / `browser` | 作为依赖包被导入时的入口解析 |
| `dependencies` | `wjsm install` 写入；解析本身只看 `node_modules` 实际内容 |
| `scripts` | `wjsm run <name>` 与 `wjsm test` 可执行的命令 |

## `wjsm.toml` / `wjsm.json`

在当前工作目录查找，`wjsm.toml` 优先于 `wjsm.json`；`--config <PATH>` 可以指定任意路径。
两种格式都支持把配置放在顶层或 `cli` 表下：

```toml
root = "."
stats = true
```

```json
{ "cli": { "verbose": 1, "root": "." } }
```

命令行显式给出的选项优先于配置文件。完整键名、取值和优先级规则见
[配置来源与优先级](../configuration/sources-and-precedence.md)。

## 输出目录

wjsm 不预设 `dist/`，`build -o` 指哪写哪，父目录需要已存在。编译缓存在
`$HOME/.cache/wjsm`（可用 `WJSM_CACHE_DIR` 改），不落在项目目录里。

## 深入了解

- [CLI 参数模型与配置合并](../../internals/tooling/cli-and-config.md)
- [模块图与解析器如何使用 package.json](../../internals/modules/graph-and-resolution.md)
