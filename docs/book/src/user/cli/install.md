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

## 深入了解

- [项目初始化、包安装与 Shell 补全的实现](../../internals/tooling/project-tools.md)
- [包解析、条件与 browser 字段映射](../../internals/modules/package-conditions.md)
