# `dump-clif`

输出源码或 portable artifact 对应的 Cranelift IR，用于定位 direct native codegen 问题：

```bash
wjsm dump-clif -e 'const x = 1'
wjsm dump-clif app.ts
wjsm dump-clif /tmp/app.wjsm
```

源码输入支持 `--root`、`--script` 与 `-e`。artifact 输入会先做 bounded decode 与 verification。

诊断顺序通常是：

```text
dump-ast → dump-ir → dump-clif → disasm
```

AST 正确而 IR 错误，问题属于 semantic lowering；IR 正确而 CLIF 错误，问题属于 native lowering；CLIF 正确而机器码/relocation 错误，再看 `disasm` 与 image loader。
