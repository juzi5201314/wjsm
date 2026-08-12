# `size`

报告 portable artifact 与当前宿主 native image 的尺寸：

```bash
wjsm size /tmp/app.wjsm
```

命令先验证 `.wjsm`，再按当前宿主 ISA 准备 native image。输出可用于区分 portable bytes 与当前 target 派生机器码；native image 不会写回 artifact。

需要比较跨平台制品时比较 `.wjsm` 的字节数与 SHA-256；需要比较当前宿主 codegen 时在同一 target/CPU/Cranelift settings 下比较 native image 数据。
