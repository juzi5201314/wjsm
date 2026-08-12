# Artifact 校验与尺寸分析

这一章说明 `validate` 和 `size` 命令的内部实现。

## validate

`validate` 解码并校验 portable `.wjsm`，不执行、不生成当前宿主机器码，也不碰 native cache：

```bash
wjsm validate /tmp/app.wjsm
```

校验覆盖容器 magic/version、header/section 长度与哈希、section 重叠和重复、资源上限、module manifest、required builtins、cross-reference、semantic ABI 与 IR invariants。通过后打印 `valid: <sha256>`。

没有 `validate_wasm` / `wasmparser::Validator`。输入损坏、截断、超限或与当前 semantic ABI 不兼容时返回退出码 1。

## size

`size` 先验证 `.wjsm`，再按当前宿主 ISA 做一次 `NativeCompiler::diagnostics`，报告 portable 与派生 native object 的规模：

```text
artifact_bytes
sections
ir_functions / ir_blocks / ir_instructions
native_object_bytes
native_functions
native_frame_bytes
```

native image 不会写回 artifact，也不会写入 `WJSM_CACHE_DIR`。比较跨平台制品时看 `.wjsm` 字节数与 digest；比较当前宿主 codegen 时在同一 target / Cranelift settings 下看 native object。

## 与 build 的关系

`build` 产出 portable `.wjsm`。`validate` / `size` 都吃这份制品：前者只做 target-independent 校验，后者额外编译当前宿主 object 做尺寸对照。

## 深入了解

- [源码输入与编译编排](source-input.md)
- [IR、AST 与反汇编工具](dump-and-disassembly.md)
- [用户侧的 validate](../../user/cli/validate.md)
- [用户侧的 size](../../user/cli/size.md)
