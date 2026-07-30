# dump-ast

把 SWC 解析出的 AST 以 JSON 打印到标准输出，用于确认解析结果是否符合预期。

```bash
wjsm dump-ast app.ts
wjsm dump-ast -e 'const x = 1'
wjsm dump-ast - < app.js
```

输出是 SWC AST 的序列化形式，节点带 `type` 和字节偏移 `span`：

```json
{
  "type": "Module",
  "span": { "start": 1, "end": 10 },
  "body": [
    {
      "type": "VariableDeclaration",
      "kind": "const",
      "declare": false,
```

`span` 的 `start` / `end` 是 SWC 全局字节偏移，不是行列号，也不从 0 开始。

结构由 SWC 版本决定，字段名和嵌套形式可能随 `swc_core` 升级变化，不要把它当作稳定接口来解析。
`--root` 与 `--script` 的含义与 `run` 一致；`--script` 会改变 `await` 等标识符的解析结果。

同一份 AST 也可以用 `wjsm build --stage parse` 得到，两者输出相同。

> <details><summary>为什么 dump-ast 输出这么丑？</summary>
>
> 因为它是 SWC 内部 AST 的 1:1 序列化，没有为了人类可读性做过优化。SWC 的 AST 是「编译器内部表示」而不是「展示用数据结构」——所有信息都暴露出来（包括一些只在 lowering 时才会用到的元数据）。
>
> 实际用途：诊断「为什么这段代码被解析成这个样子」——比如看到「TS 类型注解在哪里」、确认某个语法变体被识别成什么节点。
>
> 不适合做：AST diff、AST grep。这两件事更适合用专门的工具（Babel、ts-morph 之类），或者直接看 `dump-ir`——IR 比 AST 紧凑得多。
>
> </details>

## 深入了解

- [SWC 解析边界与 wjsm 的封装](../../internals/frontend/parser.md)
- [解析阶段在流水线中的位置](../../internals/pipeline/parse.md)
