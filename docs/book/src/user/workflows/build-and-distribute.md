# 编译与分发 WebAssembly

## 生成产物

```bash
wjsm build src/main.ts --root . -o dist/app.wasm
```

`--root` 存在时先做模块 bundling，把整个依赖图编进一个模块。

## 检查产物

```bash
wjsm validate dist/app.wasm
wjsm size dist/app.wasm
```

`validate` 只做 WebAssembly 结构与类型校验，不检查 wjsm 宿主 ABI 是否匹配。`size` 按段列出体积，Import 和 Export 段通常占大头，因为宿主函数与运行时全局都在其中。

## 产物的运行前提

生成的模块不是独立程序：它 import 数百个宿主函数、三块内存（含 shared memory64 对象堆）和一张函数表。分发时必须同时说明由 wjsm 或基于 `wjsm-host-wasm` 的宿主来实例化。详见[WASM 产物与宿主要求](../output/wasm-artifacts.md)。

> <details><summary>「产物能分发」实际意味着什么？</summary>
>
> wjsm 产物的「可分发」和 Node 二进制的「可分发」是两种不同的东西：
>
> - **Node 二进制**：自包含可执行文件，双击能跑。
> - **wjsm 产物**：WASM 字节码 + 需要 wjsm 宿主（或 wjsm 自身）来运行。
>
> 这意味着分发的「最小集」至少是 `app.wasm` + `wjsm` 二进制（让用户运行）。如果你的程序要访问外部 npm 包，那些包的代码已经被 bundle 进 `.wasm`——所以再次强调，**改依赖要重新编译**。
>
> 对于「嵌入到 Rust 宿主」的场景：你的程序是 Rust 进程的一部分，Rust 进程用 `wjsm-runtime` crate 把 `.wasm` 加载进来，宿主提供那 500+ 个 import 函数。
>
> </details>

## 边构建边执行

```bash
wjsm build app.ts --stage execute -o /tmp/app.wasm
```

写出产物并立即执行，适合确认这一份字节码本身可运行。

## 统计与耗时

```bash
wjsm --stats --time build app.ts -o /tmp/app.wasm
```

`--stats` 打印常量、函数、基本块、指令数与产物大小；`--time` 打印各阶段耗时。两者都写 stderr，可单独重定向。

## 深入了解

- [WASM Import、Export 与主模块 ABI 的完整约定](../../internals/backend/imports-exports-and-abi.md)
- [模块图如何合并为单个 IR Program](../../internals/modules/program-bundling.md)
- [产物体积分析与校验工具的实现](../../internals/tooling/validation-and-size.md)
