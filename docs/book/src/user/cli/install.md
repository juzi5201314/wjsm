# install

从 npm registry 下载包并解压到当前目录的 `node_modules/`。

```bash
wjsm install lodash
wjsm install lodash@4.17.21 @scope/pkg@1.2.3
```

至少要给一个包名，否则报 `install requires at least one package`。

## 版本选择

`<name>@<version>` 中的版本先按精确版本在 registry 的 `versions` 里查找；找不到就当作 dist-tag 查
`dist-tags`。不带版本时按 `latest` 标签解析。语义化范围（`^1.0.0`、`>=2`）不支持，会报
`unsupported package version selector`。

## 安装行为

1. 请求 `https://registry.npmjs.org/<name>`（scoped 包名中的 `/` 编码为 `%2f`）读取元数据。
2. 下载对应版本的 tarball，剥掉顶层目录后解压到 `node_modules/<name>/`。
3. 目标目录已存在时先整体删除再写入，不做增量合并。
4. 把 `"<name>": "^<version>"` 写进当前目录 `package.json` 的 `dependencies`（文件不存在则新建）。

依赖不会递归安装：只装你显式列出的包，被依赖的传递包需要自己再 `wjsm install`。也没有 lockfile
和完整性校验。

> <details><summary>为什么 `wjsm install` 不做 npm install 的那些事？</summary>
>
> `npm install` / `pnpm install` 是完整的包管理器——lockfile、依赖解析、peerDeps、生命周期脚本、bundled deps、原生模块编译…… 几十年的兼容性包袱。`wjsm install` 只做一件事：把 tarball 下载下来解压。
>
> 这个简化有两个直接后果：
>
> - **不能用来开发库**：库的依赖通常是复杂的传递关系，没有 lockfile 就装不出可复现的环境。
> - **没有 postinstall**：很多 npm 包用 `postinstall` 做「下载 native binding」「编译原生模块」之类的事情，wjsm 直接跳过这些步骤，结果就是原生模块一定跑不起来。
>
> 推荐用法：依赖树简单时用 `wjsm install` 装；复杂时用 `npm install` 或 `pnpm install` 生成 `node_modules`，wjsm 不在乎是谁生成的，只看 `node_modules/` 里的文件结构。
>
> </details>

## 深入了解

- [项目初始化、包安装与 Shell 补全的实现](../../internals/tooling/project-tools.md)
- [包解析、条件与 browser 字段映射](../../internals/modules/package-conditions.md)
