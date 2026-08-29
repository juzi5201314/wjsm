# 项目目标与非目标

这一章界定 wjsm 要解决什么、明确不做什么。后续所有设计取舍都以此为前提。

## 目标

**把静态可见的 JavaScript/TypeScript 编成 generic native，不依赖 V8。** 整条链路是 `swc_core` 解析 → 自有语义 IR → Cranelift → native image → 当前宿主执行。没有字节码解释器当主路径，也没有第二套执行引擎。语言仍是动态的；`eval` 与热路径 overlay 在运行时调用同一 `NativeCompiler`，不要把「AOT」写成「运行时不再编译」。

**ECMAScript 是语义的唯一权威。** 不接受「先跑通再补齐」的部分语义。early error、TDZ、hoisting 顺序都必须与规范一致，由 `fixtures/` 与 test262 持续验证。

**后端可替换。** ADR 0013 定义了 `JsBackend` + `ExecContext` 契约：JS 语义算法在 `wjsm-builtins`（泛型 `<E: ExecContext>` 单态化），对象模型 `HeapAccessV2<M>` 在 `wjsm-gc`（泛型 `M: GrowableHeapMemory`）。新后端只需实现三个 trait，不需要碰语义代码。

**统一 ManagedHeap + 并发分代 ZGC。** 生产路径只有 `GenerationalZgc`，对象堆后备是 `NativeHeapMemory`；无 `--gc` 或 `WJSM_GC` 选择面。

**启动成本压到构建期。** Native cache 按 artifact hash + ABI + codegen source hash + target 计算键，运行时按需编译或命中缓存，不依赖用户机器上的预编译工件。

> <details><summary>「后端可替换」具体省下了什么？</summary>
>
> 这是 wjsm 架构的核心目标之一。代码量上 `wjsm-builtins` + `wjsm-gc` 加起来约 2.8 万行——这部分是「ECMAScript 语义的实现」和「对象堆管理」，任何后端都需要。
>
> 新加一个后端（比如 native code、V8 嵌入、JIT 编译器），只需要实现 `HeapMemory` / `GrowableHeapMemory`、`ExecContext`、`JsBackend` 三组 trait。语义代码、GC 算法、对象布局、IR 数据结构全部复用。
> 反过来想：没有这个边界，每个新后端都要重写一遍 2.8 万行代码，或者把后端专有类型泄漏到每个文件里。前者成本爆炸，后者让代码绑定特定执行引擎。
>
> 现实中 Cranelift native 是唯一生产后端。`JsBackend` 契约仍在，用来卡住语义层与宿主的边界；没有 JIT / Wasm stub 可以回退。
>
> </details>

## 非目标

**不是 Node.js 替代品。** 24 个内置模块是自有 JS 实现的子集，不追求 API 逐位对齐。

**不是类型检查器。** TypeScript 语法参与解析和 lowering，类型不做检查。这是 `tsc` 的职责。

**`.wjsm` 不是独立可执行文件。** 默认 `build` 输出的 portable 制品依赖 wjsm 宿主 runtime。`--format native-executable` 在当前宿主上打包预链 stub 与预编译 object，得到可直接运行的 ELF/PE；它不是跨平台制品，也不调用系统 linker。

**不引入 ICU4C / 宿主 ICU。** 国际化数据由 ICU4X compiled_data 嵌入 `wjsm` / `wjsm-exec` stub，覆盖为 full locale，不读 system ICU、`NODE_ICU_DATA` 或联网下载。JS `Intl` 与 locale 敏感方法消费同一 crate；数据契约见 [国际化数据契约](intl-data.md)。

**不为兼容而保留旧路径。** 切换实现时直接迁移全部调用方，不留 shim、别名或双写。ManagedHeap 取代 memory32 对象堆后，旧堆路径已完全移除。

## 深入了解

- [设计原则与规范来源](design-principles.md)
- [多后端边界如何落地](../backend/multi-backend-boundary.md)
- [统一 ManagedHeap 的所有权模型](../gc/managed-heap.md)
