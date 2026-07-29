# 术语表

手册中反复出现的概念。实现层术语见[内部手册术语表](../../internals/reference/glossary.md)。

**AOT 编译**：运行前把源码整体编译成目标代码。wjsm 在执行前把 JS/TS 编译成 WebAssembly，运行时不再解析 JavaScript。

**Wasmtime**：wjsm 使用的 WebAssembly 运行时，负责加载模块、提供宿主函数、管理线性内存。

**宿主 ABI**：编译产物与 wjsm 运行时之间的接口约定，包括 500 多个 import 函数、三块内存和一组全局变量。产物不能脱离 wjsm 运行。

**support 模块**：与用户模块一同实例化的辅助 WebAssembly 模块，提供对象和数组操作等共享 helper，构建时已预编译。

**启动快照**：把 bootstrap 后的运行时状态固化下来，跳过每次启动的初始化工作。默认启用。

**ManagedHeap**：JavaScript 对象的统一托管堆，位于共享的 memory64 线性内存中，由所选垃圾回收器管理。

**影子栈**：独立于对象堆的线性内存区域，用于传递变长参数和在 GC 安全点保存值。冷启动 64KiB，按需增长，默认软上限 16MiB。

**语义 IR**：解析与代码生成之间的中间表示。`wjsm dump-ir` 输出的就是它。

**Lowering**：把 AST 转换成语义 IR 的过程，作用域分析、提升和 TDZ 判定都在这一步完成。

**Bundling**：多文件项目把入口及其依赖合并成单个 IR Program，再整体编译成一个 `.wasm`。

**包解析条件**：`package.json` 的 `exports` / `imports` 字段按条件名选择目标文件。wjsm 默认按 `wjsm` → `node` → `import`/`require` → `default` 顺序匹配。

**Realm**：一套独立的全局对象与内置对象。`node:vm` 在同一个堆上创建多个 realm。

**Inspector**：基于 Chrome DevTools Protocol 的调试接口，通过 `--inspect` 启用。

**Test262**：ECMAScript 官方一致性测试套件。wjsm 用它衡量语义覆盖，不作为兼容性承诺。
