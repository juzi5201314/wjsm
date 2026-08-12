# 文件、内联源码与标准输入

`wjsm run` 接受三种输入方式：

| 方式 | 写法 | 脚本名 |
| --- | --- | --- |
| 文件路径 | `wjsm run app.ts` | 实际路径 |
| 内联源码 | `wjsm run -e 'console.log(1)'` | `[run-eval]` |
| 标准输入 | `cat foo.js \| wjsm run -` | stdin |

## 优先级

同时给出多种输入时，按 **文件路径 > `-e` > stdin** 取优先级高者，其余忽略。三者都不给会直接报错：

```text
Error: Either an input file or -e <code> is required
```

实际使用中几乎不会同时传多种输入。了解优先级主要是为了理解 `-` 不是「补充」而是「备选」：只有没有文件路径和 `-e` 时，stdin 才会被读取。

## 扩展名决定语法模式

文件路径的扩展名决定 SWC 解析时的语法配置：

| 扩展名 | 语法模式 | JSX |
| --- | --- | --- |
| `.ts` | TypeScript | 关 |
| `.tsx` | TypeScript | 开 |
| `.jsx` | ES | 开 |
| `.js` / `.mjs` / `.cjs` | ES | 关 |

`-e` 和 stdin 没有文件名，落到默认分支：TypeScript + JSX。这是最宽松的组合，所以内联源码可以直接写 TS 语法和 JSX，不会被拒绝。

`--script` 切换为脚本解析（而非模块），此时 `await` 可以当普通标识符用，与扩展名无关。

## 为什么默认是最宽松的

`-e` 内联源码经常用于快速测试，可能包含类型注解或 JSX。如果默认按 ES 解析，这些语法会报错。TypeScript + JSX 的组合接受所有合法 ES 语法，同时允许类型注解和 JSX 标签，因此作为无文件名入口的默认值最不容易出错。

类型注解在 lowering 阶段被擦除，不会影响运行时行为。

## 深入了解

- [`run` 命令](../cli/run.md)
- [解析阶段在流水线中的位置](../../internals/pipeline/parse.md)
- [SWC 解析边界](../../internals/frontend/parser.md)
