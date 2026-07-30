# 文件、内联源码与标准输入

同一段代码有三种交给 wjsm 的方式——它们的模块解析行为和诊断文件名都不同。

| 方式 | 写法 | 模块解析 | 诊断中的文件名 |
| --- | --- | --- | --- |
| 文件 | `wjsm run app.ts` | 按文件位置解析相对导入与 `node_modules` | 真实路径 |
| 内联 | `wjsm run -e '…'` | 无文件位置，不能用相对导入 | `input.ts` |
| 标准输入 | `wjsm run -` | 同内联，无文件位置 | `input.ts` |

## 文件

接受 `.js`、`.mjs`、`.cjs`、`.jsx`、`.ts`、`.tsx`。按扩展名选择解析配置，`.ts`/`.tsx` 会剥离类型语法。

```bash
wjsm run src/main.ts
wjsm check src/main.ts
```

带 `--root` 时进入多文件 bundling 路径，入口的导入按该根目录解析：

```bash
wjsm run src/main.ts --root src
```

## 内联源码

`-e` / `--eval` 适合一次性验证语义，`build`、`run`、`test`、`check`、`lint`、`dump-*`、`repl` 都支持：

```bash
wjsm run -e 'console.log([1, 2, 3].map((n) => n * 2))'
wjsm check -e 'const x: number = 1'
```

内联源码按 TypeScript 语法解析，因此类型标注可以直接写。它没有所在目录，`import './other.js'` 无法解析；需要多文件就写成真实文件。

`eval` 子命令是另一回事：它只接受一个表达式并打印求值结果，不接受语句。

```bash
wjsm eval '1 + 2 * 3'
```

```text
7
```

## 标准输入

文件参数写 `-` 时从 stdin 读取全部内容：

```bash
echo 'console.log("from stdin")' | wjsm run -
cat app.ts | wjsm check -
```

`build -o -` 把 WASM 写到标准输出，用于管道。检测到 stdout 是终端时命令会拒绝执行，避免二进制刷屏：

```bash
wjsm build app.ts -o - > /tmp/app.wasm
```

## 模块与脚本

默认按 ES 模块解析，`await` 是保留字。`--script` 切到脚本解析，此时 `await` 可以当标识符：

```bash
wjsm check -e 'var await = 1'          # 报错
wjsm check -e 'var await = 1' --script # 通过
```

模块模式下的错误形如：

```text
error: `await` cannot be used as an identifier in an async context
  --> input.ts:1:5
1 | var await = 1;
  |     ^^^^^
```

> <details><summary>模块模式和脚本模式的本质区别</summary>
>
> 两者不是「宽松严格」的关系，而是 ECMAScript 规范明确区分的两种语法：
>
> - **脚本（Script）**：传统浏览器里 `<script>` 标签里的代码。`await` 不是保留字（因为早期浏览器脚本里没人写 `await`），可以在 `var` 名字里用。
> - **模块（Module）**：`<script type="module">` 和 ESM 文件里的代码。`await` 是顶层保留字（因为模块语法里 `await` 出现在顶层是合法且常见的——顶层 await），不能用作标识符。
>
> wjsm 默认走模块模式，因为现代代码几乎都按 ESM 写。但有些老代码、迁移代码、`-e` 临时表达式会希望脚本语义——`--script` 切过去就行。
>
> 切换不影响编译产物格式，只影响解析阶段的语法检查。
>
> </details>

## 传参给脚本

`run` 的 `--` 之后的参数进入 `process.argv`：

```bash
wjsm run app.js -- a b
```

`process.argv[0]` 是 wjsm 可执行文件，`argv[1]` 是脚本路径（内联时为 `[run-eval]` 哨兵），其余为用户参数。

## 深入了解

- [源码输入与编译编排](../../internals/tooling/source-input.md)
- [解析阶段如何按扩展名选择语法配置](../../internals/pipeline/parse.md)
