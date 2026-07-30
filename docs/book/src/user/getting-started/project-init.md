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

> <details><summary>「脚本模式不支持 --watch」——如果想在脚本里监听文件改动怎么办？</summary>
>
> `wjsm run --watch` 的实现是 fork 一个子进程监听文件、改了之后重新走编译+执行。`package.json scripts` 模式下 wjsm 自身就是子进程，再 fork 一次就要在 `pre<name>` 之类的脚本钩子里管理生命周期，复杂度太高。
>
> 想要这个能力的两种 workaround：
>
> 1. **直接 `wjsm run --watch main.js`**，绕开 `npm scripts`。你失去了 `node_modules/.bin` 自动加到 `PATH` 的便利，但拿回了 `--watch`。
> 2. **用外部 watcher 工具**（`watchexec`、`entr`、`nodemon`）调用 `wjsm run main.js`，本质上等同于手动实现 wjsm 那一段 fork 逻辑。
>
> 短期看 wjsm 不太会做「脚本模式 + watch」的组合——这两者的语义有冲突，强行合并会引入难以解释的边界。
>
> </details>

## 深入了解

- [项目初始化、包安装与 Shell 补全的实现](../../internals/tooling/project-tools.md)
- [CLI 参数模型与配置合并](../../internals/tooling/cli-and-config.md)
