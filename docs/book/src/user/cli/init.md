# init

创建一个新项目目录。

```bash
wjsm init myapp
wjsm init myapp --force
```

生成两个文件，目录名作为项目名：

- `main.js` — 一行 `console.log`。
- `package.json` — 含 `name`、`version`、`"type": "module"`。

```text
Created project at myapp

To run:
  cd myapp
  wjsm run main.js
```

任一目标文件已存在时命令中止，提示 `Use --force to overwrite.`；`--force` 会覆盖这两个文件。目录本身
按需递归创建，已有目录不会被清空。

模板不包含 `wjsm.toml`。需要项目级默认选项时自己新建，键名见[配置文件](../configuration/project-files.md)。

## 深入了解

- [项目初始化模板的实现](../../internals/tooling/project-tools.md)
