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
