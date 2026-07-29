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

## 深入了解

- [SWC 解析边界与 wjsm 的封装](../../internals/frontend/parser.md)
- [解析阶段在流水线中的位置](../../internals/pipeline/parse.md)
