# 多后端边界

这一章说明为什么 codegen、宿主能力、GC 算法被拆成互不依赖的 crate，以及新后端的接入点在哪里。

## 边界规则

ADR 0011 到 0013 定下一条硬约束：wasmtime 与 wasm 工具链依赖只允许出现在 `wjsm-backend-wasm` 和 `wjsm-host-wasm`。`wjsm-builtins`、`wjsm-host`、`wjsm-gc`、`wjsm-module`、`wjsm-ir` 全部保持后端无关。

这条约束不是审美偏好。它决定了三件事：

- JS 语义算法（Promise、Proxy、Streams、Fetch、JSON）写一次就能被任何后端复用，因为它们以 `<E: ExecContext>` 泛型单态化，不含 `dyn`。
- GC 算法与对象模型泛型化在 `M: GrowableHeapMemory` 上，不绑定 wasmtime 的 shared memory 类型。
- 加一个后端不需要改语义层，只需要提供内存、执行上下文和编译入口三个实现。

## JsBackend 契约

`crates/wjsm-host/src/backend.rs` 定义编译后端契约：

```rust
pub trait JsBackend {
    type Artifact: Send;
    type ExecOptions;
    fn name(&self) -> &'static str;
    fn compile(&self, program: &Program, debug: bool) -> Result<Self::Artifact>;
    fn artifact_bytes(artifact: &Self::Artifact) -> Option<&[u8]>;
    fn execute<'a, W: Write + 'a>(
        &'a self, artifact: &'a Self::Artifact, options: Self::ExecOptions, writer: W,
    ) -> impl Future<Output = Result<(W, Vec<u8>)>> + 'a;
}
```

`Artifact` 因后端而异：wasm 后端是字节向量，native 后端可以是镜像或 C 源码包。不可序列化的制品让 `artifact_bytes` 返回 `None`，`build -o` 据此报错而不是写出空文件。

`execute` 返回的 Future 不要求 `Send`。CLI 用单线程 `block_on` 驱动，而 wasmtime 执行链持有 `MutexGuard`，加上 `Send` 约束会直接编译不过。未来的多线程后端可以自行加约束。

## 静态分发

CLI 按 `Target` enum 分发，不走 `dyn`：

```rust
match target {
    Target::Wasm => <runtime::WasmBackend as runtime::JsBackend>::compile(...),
    Target::Jit  => <wjsm_backend_jit::JitBackend as runtime::JsBackend>::compile(...),
}
```

完全限定语法是必要的，因为两个后端的 `Artifact` 类型不同，无法通过 trait 对象统一。每个分支单态化，零 vtable 开销。

## 真实接缝是 ExecContext

`HostRuntime` 是 ADR 0011 的设计遗留：一个 marker trait 加 blanket impl，作为 facade 的公开 API 保留。新后端不需要实现它。

真正要实现的是 `ExecContext`（builtins 完整能力集）和 `JsBackend`（编译执行入口）。完整六步接入路径见 `docs/backend-implementation-guide.md`。

> <details><summary>「六步接入法」具体是哪六步？</summary>
>
> 1. **实现 `HeapMemory` + `GrowableHeapMemory`**：提供堆内存读写和增长能力。
> 2. **实现 `ExecContext`**：把 builtins 需要的约 330 个方法接到自己的运行时。
> 3. **实现 `JsBackend::compile`**：从 IR 编译到目标格式（native code、WASM、字节码……）。
> 4. **实现 `JsBackend::execute`**：驱动生成的代码并收集输出。
> 5. **引擎集成**：让后端能被 wasmtime 风格的「engine 池 + module + store + linker」流程调度。
> 6. **测试验证**：跑 fixture 和 test262，确保语义兼容。
>
> 步骤 1-2 是「实现内存和上下文」，是 90% 的工作量；步骤 3-4 是「实现编译执行」，因后端形态而异；步骤 5-6 是「接进 wjsm 体系」。
>
> 当前只有一个真后端（wasmtime + WASM）和一个 stub（`JitBackend`），证明契约可以工作。
>
> </details>

## 深入了解

- [JIT 后端 stub 的当前状态](jit-backend.md)
- [ExecContext 如何解耦 builtins 与后端](../host-runtime/exec-context-and-builtins.md)
- [跨 crate 所有权与依赖边界](../foundations/ownership-and-dependencies.md)
