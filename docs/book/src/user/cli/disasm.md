# `disasm`

把已验证 portable `.wjsm` 编译为当前宿主 native image，并反汇编机器码：

```bash
wjsm disasm /tmp/app.wjsm
```

该命令只接受 portable artifact。源码先用 `build` 生成 `.wjsm`，或用 `dump-clif` 查看源码对应的 Cranelift IR。

`disasm` 的输出绑定当前 target、CPU feature 与 codegen settings，不能作为跨平台 artifact。定位问题时先比较 `dump-ir` 与 `dump-clif`；只有 CLIF 正确而最终指令、relocation 或 unwind 异常时再查看反汇编。
