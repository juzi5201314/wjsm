# 编译与执行流水线

流水线是 wjsm 的主干：源码经过解析、语义 lowering、可选的模块 bundling，编译为 WASM，最后由 Wasmtime 宿主实例化执行。

阶段之间只通过明确的数据结构交接：AST（`swc_ast::Module`）、IR（`wjsm_ir::Program`）、WASM 字节（`Vec<u8>`）。这个边界让每个阶段可以单独 dump 和测试。

- [编译编排入口](orchestration.md)
- [解析阶段](parse.md)
- [语义 Lowering 阶段](lower.md)
- [IR 阶段](ir.md)
- [模块图与 Bundling 阶段](bundle.md)
- [WASM 编译阶段](compile.md)
- [实例化与执行阶段](execute.md)
- [阶段隔离与诊断输出](stage-isolation.md)
