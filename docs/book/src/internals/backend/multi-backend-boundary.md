# 多后端边界与 JsBackend 契约

ADR 0014 删除了 Wasm/Wasmtime 生产路径和 JIT stub，确立了 Direct Cranelift 为唯一生产后端。这一章说明当前的后端边界和未来接入新后端的契约。

## 当前状态

Direct native 是唯一生产执行后端。没有 backend selector，没有 target/compiler 选项，没有兼容 fallback。不支持的宿主在 `NativeCompiler::new()` 时返回 `UnsupportedTargetCapability`。

## JsBackend trait

`wjsm-host` 定义 `JsBackend` trait，新后端需要实现它。trait 覆盖：

- 编译 `Program` 为后端专有 artifact；
- 提取 artifact bytes；
- 编译模式控制（normal / eval）。

在当前代码中，`JsBackend` 的唯一实现是 `NativeBackend`（在 `wjsm-backend-native`）。旧代码里的 `WasmBackend` 和 `JitBackend` 已删除。

## ExecContext trait

`ExecContext` 是后端必须实现的执行上下文 trait，约 330 个方法。`wjsm-builtins` 的全部算法以 `<E: ExecContext>` 泛型实例化，编译期单态化，零 vtable 开销。

新后端实现 `ExecContext` 后，可以复用 `wjsm-builtins`（约 17000 行语义算法）和 `wjsm-gc`（ManagedHeap、HandleTableV2、三种回收器），不需要重写 ECMAScript 语义。

## HeapContext

`HeapContext` 是 `ExecContext` 的子 trait，定义堆读写、句柄操作和对象分配。拆分它是因为部分场景只需要堆操作（如 support function 的 helper body），不需要完整的 `ExecContext`。

## 接入新后端

如果要重新引入其他执行后端（如 JIT 编译器），必须：

1. 先以新 ADR 定义 artifact、runtime ownership 与语义验证契约。
2. 实现 `JsBackend` + `ExecContext` + `HeapMemory` / `GrowableHeapMemory` 三组 trait。
3. 确保 `NATIVE_ABI_HASH` 或等价 ABI hash 覆盖新后端的 wire 布局。
4. 不引入 backend selector——旧后端路径已删除，新后端替代旧后端而非共存。

> <details><summary>为什么不再有 Wasm/Wasmtime 路径？</summary>
>
> ADR 0014 记录了这个决策。Direct Cranelift 路径消除了 Wasmtime 运行时开销和 Wasm 编码/解码步骤。`.wjsm` artifact 直接包含 semantic IR，运行时直接编译为当前宿主机器码。
>
> 旧路径的全部代码（包括 `wjsm-backend-wasm`、`wjsm-host-wasm`、WASM artifact 编码/解码）已删除。这不是"暂时下线"——如果未来需要 Wasm 后端，必须从新 ADR 开始，不复用旧代码。
>
> </details>

## 深入了解

- [Host 能力 Trait](../host-runtime/host-traits.md)
- [ExecContext 与 Builtins 的解耦设计](../host-runtime/exec-context-and-builtins.md)
- [跨 crate 所有权与依赖边界](../foundations/ownership-and-dependencies.md)
- [ADR 导航](../reference/adr-index.md)
