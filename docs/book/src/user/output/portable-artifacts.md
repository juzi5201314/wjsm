# Portable `.wjsm` 制品

`wjsm build` 默认生成 target-independent semantic artifact：

```bash
wjsm build app.ts -o /tmp/app.wjsm
wjsm validate /tmp/app.wjsm
wjsm run /tmp/app.wjsm
```

## 包含内容

- verified semantic IR；
- canonical module manifest 与 required builtins；
- semantic ABI/hash；
- 可选 source map 与 source text。

## 不包含内容

- 当前宿主机器码或 executable mapping；
- Cranelift object、relocation 或 raw pointer；
- native cache key/path；
- startup snapshot 私有地址。

因此同一 `.wjsm` 可以在支持平台间携带。运行时验证 artifact 后，由当前宿主把 IR 编译为 native image。磁盘缓存可用时按 digest、native ABI、codegen hash、target、Cranelift 版本和 settings 查找或写入磁盘缓存。

`--format native-executable` 产出同宿主 ELF/PE（stub + overlay + 源码快照），不能跨平台携带。runtime 私有 native image 本身仍不是 executable。详见[制品与宿主要求](wasm-artifacts.md)。

## 深入了解

- [制品与宿主要求](wasm-artifacts.md)
- [Artifact 校验与尺寸分析](../../internals/tooling/validation-and-size.md)
- [预编译执行与磁盘缓存](../../internals/tooling/precompiled-execution.md)
