# 安装 npm 包

`wjsm install` 从 npm registry 下载包并解压到当前目录的 `node_modules/`。

```bash
wjsm install lodash
wjsm install lodash@4.17.21 @scope/pkg@1.2.3
```

装完后 `package.json` 的 `dependencies` 会记上 `"lodash": "^4.17.21"`，并可以直接导入：

```js
import _ from "lodash";
```

参数格式与版本选择规则见 [`install` 命令](../cli/install.md)。

## 与 npm / pnpm 的差别

`wjsm install` 是最小实现，几个关键限制：

- **不装传递依赖**。只下载你显式列出的包；它的依赖需要你自己再装一遍。
- **不支持版本范围**。`^1.0.0`、`>=2` 这类选择器会被拒绝，只接受精确版本或 dist-tag。
- **没有 lockfile**，也不做完整性校验。
- **不执行安装脚本**，`postinstall` 之类不会运行。

依赖树复杂时，用 `npm install` 或 `pnpm install` 生成 `node_modules`，wjsm 直接读取即可——解析逻辑遵循 `node_modules` 惯例，不依赖 `wjsm install` 写下的元数据。

## 包能装上不等于能跑

wjsm 的 JavaScript 语义和 Node API 都是子集。运行时失败通常来自三类原因：

| 原因 | 表现 |
| --- | --- |
| 包依赖未实现的 Node API | 运行时报缺失的函数或模块 |
| 包含原生插件（N-API / `.node`） | 无法加载，wjsm 不支持原生插件 |
| 依赖 V8 特有行为或未覆盖的语义 | 行为不符或运行时错误 |

纯 JavaScript、依赖面窄的包成功率最高。选包前可以先用 `wjsm check` 对入口文件做一次解析验证。

> <details><summary>「包能装上不等于能跑」具体有多严重？</summary>
>
> 实测来看，依赖面窄的纯 JS 包成功率 80%+；含 native module 的 0%；依赖冷门 Node API 的看运气。常见「能装不能跑」的例子：
>
> - `bcrypt`：需要 native binding。
> - `node-gyp` 系列：依赖 node-gyp 工具链编译。
> - `puppeteer`：要下载 Chromium。
> - `chokidar`：依赖 `fsevents`（macOS 特有 native 模块）。
> - 任何用 `vm.runInNewContext` 配合字符串拼接的动态执行——这类行为在 wjsm 下要走 eval 编译路径，不是每次都成功。
>
> 装包前先看它的 `package.json` 的 `dependencies` 列表，扫一眼有没有 native binding 关键词；再看 `main` 入口的源码，扫一眼有没有 `require('xxx')` 涉及未实现的 Node API。
>
> </details>

## 深入了解

- [包安装的实现与 registry 交互](../../internals/tooling/project-tools.md)
- [包解析、条件与 browser 字段映射](../../internals/modules/package-conditions.md)
