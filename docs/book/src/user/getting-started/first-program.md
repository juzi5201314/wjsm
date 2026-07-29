# 运行第一个程序

`wjsm run` 一步完成编译和执行，不需要先产出 `.wasm`。

## 内联源码

```bash
wjsm run -e 'const message: string = "Hello, wjsm"; console.log(`${message}: ${1 + 2}`)'
```

```text
Hello, wjsm: 3
```

`-e` 接收的字符串按 TypeScript 解析，所以类型注解可以直接写。wjsm 不做类型检查，注解在降级阶段被丢弃。

## 运行文件

```bash
cat > hello.ts <<'EOF'
function greet(name: string): string {
  return `Hello, ${name}`;
}
console.log(greet("wjsm"));
EOF

wjsm run hello.ts
```

```text
Hello, wjsm
```

## 只算一个表达式

`eval` 打印表达式结果，不需要自己写 `console.log`：

```bash
wjsm eval '1 + 2 * 3'
```

```text
7
```

## 只检查不执行

`check` 走到降级阶段就停止，用来快速确认源码能被解析和降级：

```bash
wjsm check hello.ts
```

成功时没有输出，退出码 `0`。出错时打印带源码片段的诊断：

```bash
wjsm check -e 'const x = ;'
```

```text
Error: error: Expression expected
 --> input.ts:1:11
1 | const x = ;
  |           ^
```

## 传参数给脚本

`--` 之后的内容进入 `process.argv`：

```bash
wjsm run hello.ts -- alpha beta
```

`process.argv[0]` 是 wjsm 自身，`process.argv[1]` 是脚本路径（`-e` 模式下是 `[run-eval]` 哨兵），其余是用户参数。

## 退出码

- `0`：成功。
- `1`：编译期错误（解析、降级、代码生成）。
- `2`：未捕获的运行时异常。
- `3`：命令行用法错误。
- 其他：`process.exit(n)` 指定的值原样返回。

```bash
wjsm run -e 'process.exit(7)'; echo $?
```

```text
7
```

## 深入了解

- [从源码到执行的完整流水线](../../internals/pipeline/index.html)
- [CLI 如何把源码输入交给编译编排](../../internals/tooling/source-input.md)
