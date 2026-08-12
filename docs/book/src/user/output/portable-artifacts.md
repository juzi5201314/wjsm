# Portable `.wjsm` 制品

`wjsm build` 生成的是 target-independent semantic artifact：

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

因此同一 `.wjsm` 可以在支持平台间携带。运行时验证 artifact 后，以其 digest、native ABI、codegen hash、target、Cranelift 版本和 settings 查找 native cache；miss 时直接从 IR 编译。

`--format native-executable` 当前明确未实现。runtime 私有 native image 不能当作可分发 executable。
