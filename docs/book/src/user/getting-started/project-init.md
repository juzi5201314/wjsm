# 初始化项目

`wjsm init <dir>` 生成一个可以直接运行的目录，避免手写 `package.json`。

```bash
wjsm init hello-wjsm
cd hello-wjsm
wjsm run main.js
```

生成的文件只有两个：

`package.json`

```json
{
  "name": "hello-wjsm",
  "version": "0.1.0",
  "type": "module"
}
```

`main.js`

```js
// hello-wjsm - wjsm project
console.log("Hello from hello-wjsm!");
```

项目名取自目录名。`"type": "module"` 意味着 `.js` 按 ES 模块解析；若要写 CommonJS，改成 `"type": "commonjs"` 或把文件后缀改为 `.cjs`。

目标目录可以是多级路径，缺失的父目录会被创建。若 `main.js` 或 `package.json` 已存在，命令直接报错退出：

```text
Error: 'hello-wjsm/main.js' already exists. Use --force to overwrite.
```

加 `--force` 才会覆盖同名文件。命令不会清空目录里的其他内容。

## package.json 脚本

`wjsm run <name>` 在 `<name>` 不是已存在的文件时，会向上查找最近的 `package.json`，把 `<name>` 当作 `scripts` 条目执行：

```json
{
  "scripts": {
    "start": "wjsm run main.js",
    "check": "wjsm check main.js"
  }
}
```

```bash
wjsm run start
```

执行细节：

- 脚本经系统 shell 运行（Unix 用 `sh -c`，Windows 用 `cmd /C`），工作目录是 `package.json` 所在目录。
- `PATH` 前置 `<root>/node_modules/.bin` 与当前 `wjsm` 可执行文件所在目录，因此脚本里可以直接写 `wjsm` 和已安装包的 bin。
- 按 `pre<name>` → `<name>` → `post<name>` 顺序执行，任一非零退出即中断。
- `--` 之后的参数追加到脚本命令末尾，含特殊字符时会加引号。
- 脚本模式不支持 `--watch`。

## 深入了解

- [项目初始化、包安装与 Shell 补全的实现](../../internals/tooling/project-tools.md)
- [CLI 参数模型与配置合并](../../internals/tooling/cli-and-config.md)
