# disasm

反汇编已有的 `.wasm` 文件，输出 WAT 文本。

```bash
wjsm disasm app.wasm
wjsm disasm app.wasm --skeleton
wjsm disasm app.wasm --func '$foo'
```

## 参数

| 参数 | 说明 |
| --- | --- |
| `<INPUT>` | 要反汇编的 `.wasm` 文件，必填 |
| `--skeleton` | 只打印函数签名，不打印指令体 |
| `--func <NAME>` | 只打印指定函数 |

`--skeleton` 与 `--func` 互斥，同时给出会报错：

```text
Error: --skeleton and --func are mutually exclusive
```

## 与 dump-wat 的区别

两个命令的输出格式一致，输入不同：

- `dump-wat` 输入是 JS/TS 源码，会先跑一遍编译流水线。
- `disasm` 输入是已经存在的 `.wasm` 文件，不涉及编译。

排查「我发布出去的这个 wasm 到底是什么」用 `disasm`；排查「这段源码会编译成什么」用 [`dump-wat`](dump-wat.md)。

## 深入了解

- [反汇编与 WAT 输出的实现](../../internals/tooling/dump-and-disassembly.md)
