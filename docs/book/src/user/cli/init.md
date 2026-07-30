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

任一目标文件已存在时命令中止，提示 `Use --force to overwrite.`；`--force` 会覆盖这两个文件。目录本身按需递归创建，已有目录不会被清空。

模板不包含 `wjsm.toml`。需要项目级默认选项时自己新建，键名见[配置文件](../configuration/project-files.md)。

> <details><summary>为什么 `init` 只生成两个文件？</summary>
>
> 看起来「简陋」是故意的。`npm create vite` 之类的工具会铺出二十多个文件、ESLint 配置、TypeScript 配置、测试脚手架——这些选择很难让所有人都满意。
>
> wjsm 的 `init` 只给一个最小可运行点：`package.json`（让 wjsm 知道这是 ESM）和 `main.js`（让 `wjsm run main.js` 能跑通）。其他东西按需加：
>
> - 要 TypeScript？把 `main.js` 改成 `main.ts`。
> - 要 wjsm 默认配置？建 `wjsm.toml`。
> - 要测试脚手架？自己写 `tests/foo.test.ts`。
>
> 这种「给最小」的做法让 wjsm 不会和项目自己的偏好冲突——你想用 Oxlint 还是 ESLint、Vitest 还是 Node test runner，wjsm `init` 都不替你决定。
>
> </details>

## 深入了解

- [项目初始化模板的实现](../../internals/tooling/project-tools.md)
