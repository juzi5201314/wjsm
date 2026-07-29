# 后端与 WASM 代码生成

这一部分讲 IR 如何变成 WebAssembly 字节，以及产物与宿主之间的 ABI 契约。

`wjsm-backend-wasm` 是纯 codegen crate：输入 `&Program`，输出 `Vec<u8>`，依赖只有 `anyhow`、`swc_core`、`wasm-encoder`、`wjsm-ir`。它不依赖 wasmtime，也不知道宿主如何执行产物——两边通过 import 名、type index 和 global 索引对齐。

先读[多后端边界](multi-backend-boundary.md)理解为什么 codegen 与执行分离，再读[编译器架构](compiler-architecture.md)了解 `Compiler` 的组织方式。ABI 细节集中在[Import、Export 与主模块 ABI](imports-exports-and-abi.md)与[Support 模块](support-module.md)。
