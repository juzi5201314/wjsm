# 包解析与条件导出

裸包名（`import x from "pkg"`）从引用文件所在目录开始，逐级向上在 `node_modules` 里查找。找到包目录后，入口由 `package.json` 的 `exports` 或旧式字段决定。

## 条件顺序

`exports` 的条件对象按固定优先级匹配，第一个命中的键生效：

1. `wjsm`
2. `browser`（仅当传了 `--browser`）
3. 你用 `--condition` 追加的条件，按命令行给出的顺序
4. `node`
5. 边类型：`import` 或 `require`
6. `default`

因为 `wjsm` 永远排在最前，包里写了 `"wjsm"` 键就会屏蔽其他所有条件。验证条件命中时要留意这一点。

```json
{
  "exports": {
    ".": {
      "browser": "./browser.js",
      "dev": "./dev.js",
      "node": "./node.js",
      "import": "./import.js",
      "default": "./default.js"
    }
  }
}
```

| 命令 | 命中 |
| --- | --- |
| `wjsm run main.js` | `node` |
| `wjsm --browser run main.js` | `browser` |
| `wjsm --condition dev run main.js` | `dev` |
| `wjsm --browser --condition dev run main.js` | `browser` |

`--condition` 里传保留名（`wjsm`、`browser`、`node`、`import`、`require`、`default`）会被忽略，因为它们已在固定序列中：`--condition default` 不会把 `default` 提前。

> <details><summary>为什么 `wjsm` 条件永远在最前？</summary>
>
> 这是个有意的设计：让包作者可以为 wjsm 单独写一份入口，不必和 Node 兼容。如果 `wjsm` 在中间或末尾，包作者想给 wjsm 优化就没办法——比如「wjsm 下可以删掉 polyfill」这类决策。
>
> 实际效果：很多 npm 包可能没有 `wjsm` 条件，这时按 `node` 条件选——这通常意味着「当 Node 跑」，在 wjsm 下也可能跑得动也可能跑不动。跑不动的话，给 wjsm 单独写一份是包作者的事。
>
> 想让某个包强制走「Node 行为」而不是 `wjsm` 行为？`--condition node` 不行（这是保留名）。你只能改包的 `exports` 或者 fork 那个包。
>
> </details>

## 旧式入口字段

没有 `exports` 时按以下顺序取入口：

1. `browser`（仅 `--browser`，且该字段是字符串形式）
2. `module`
3. `main`
4. 目录下的 `index.<ext>`

`--browser` 还会启用 `browser` 字段的对象形式映射，把包内某个文件替换成另一个文件。

## 子路径

`exports` 中的子路径键（`"./sub"`）和通配（`"./*"`）按同一套条件规则解析。`imports` 字段的 `#` 私有导入同样支持。

## 常见失败

```text
Cannot find module './util' from '/path/to/main.js'. Tried: [...]
```

错误里会列出实际尝试过的候选路径，对照它能直接看出是扩展名、目录 `index` 还是包名的问题。

```text
Cannot find module 'pkg' from '/path/to/main.js'
```

包没装，或者 `node_modules` 不在从引用文件向上的路径里。

## 深入了解

- [包解析、条件与 browser 映射的实现](../../internals/modules/package-conditions.md)
- [模块图与解析器](../../internals/modules/graph-and-resolution.md)
