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

> <details><summary>`disasm` 能不能分析别人的 wasm？</summary>
>
> 能，但「看懂」程度有限。`disasm` 用 wasmprinter 输出 WAT 文本，这一步对任何合法 WASM 都行。问题是 wjsm 产物里的函数名是 WAT 里的「符号」（`$foo`、`$$module_main`），但调用约定是 wjsm 私有的——`(param i64 i64 i32 i32)` 是什么意思、`type 12` 是哪种函数、env global 各代表什么，这些都在 wjsm 内部才有意义。
>
> 拿到一个不认识的 wjsm 产物时，第一步是用 `disasm` 看结构，第二步是看 import 列表和 env global 名字——这些是 ABI 公开的部分，能让你推测它在做什么。第三步是没办法的，要逆向函数体意义就只能看 wjsm 源码。
>
> 拿到 wazero/wasi 那种通用 WASM？`disasm` 完全够用，因为那是标准 WAT。
>
> </details>

## 深入了解

- [反汇编与 WAT 输出的实现](../../internals/tooling/dump-and-disassembly.md)
