# validate

校验一个 `.wasm` 文件是否为合法的 WebAssembly 模块。

```bash
wjsm validate app.wasm
```

通过时打印并返回退出码 0：

```text
✓ app.wasm is valid WASM
```

不通过时标准输出打印 `✗ ... is NOT valid WASM`，标准错误打印具体校验错误，退出码为 1。

校验只检查 WebAssembly 模块结构本身，不检查它能否在 wjsm 宿主中运行。wjsm 生成的模块会 import 大量宿主函数，缺少这些 import 的宿主无法实例化它——那类问题 `validate` 不会报出来，需要 `wjsm run` 或宿主侧实例化时才会暴露。

对同一个文件想看体积构成用 `size`，想看指令用 `disasm`。

> <details><summary>`validate` 通过 ≠ wjsm 能跑</summary>
>
> WebAssembly 规范校验只检查「这个文件结构上是不是合法的 WASM」——magic number、版本号、section 结构、类型系统、指令合法性等等。wjsm 生成的产物一定会通过 `validate`（除非有 codegen bug）。
>
> 但 wjsm 产物依赖一组特定的 host import 和三块 memory。通用 WASI 运行时、wazero、wasmtime 默认配置都可能无法实例化它——它们缺 wjsm 那 500+ 个 import 函数。
>
> 所以 `validate` 是「字节码合法」的必要条件，不是「可以在 wjsm 之外运行」的充分条件。两个完全不同的概念。
>
> </details>

## 深入了解

- [WASM 校验与尺寸分析的实现](../../internals/tooling/validation-and-size.md)
- [主模块 ABI 与 import 需求](../../internals/backend/imports-exports-and-abi.md)
