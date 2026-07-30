# JIT 后端边界

这一章记录 `wjsm-backend-jit` 当前的真实状态，以及它为什么以 stub 形式存在于 workspace 中。

## 当前状态

`crates/wjsm-backend-jit/src/lib.rs` 是完整文件，只有一个类型和一个 trait 实现：

```rust
pub struct JitBackend;

impl JsBackend for JitBackend {
    type Artifact = Vec<u8>;
    type ExecOptions = ();

    fn name(&self) -> &'static str { "jit" }

    fn compile(&self, _program: &Program, _debug: bool) -> Result<Self::Artifact> {
        bail!("JIT backend is not implemented yet")
    }

    fn artifact_bytes(_artifact: &Self::Artifact) -> Option<&[u8]> { None }

    async fn execute<'a, W: Write + 'a>(...) -> Result<(W, Vec<u8>)> {
        bail!("JIT backend is not implemented yet")
    }
}
```

依赖只有 `anyhow`、`wjsm-ir`、`wjsm-host`。`compile` 和 `execute` 都返回同一句错误，用户执行 `wjsm --target jit run app.js` 看到的就是它。

## stub 存在的理由

这个 crate 不是占位垃圾，它承担一个具体职责：证明 `JsBackend` 契约可以被第二个后端实现，并让 CLI 的分发路径有真实的第二分支。

在 ADR 0013 之前，`Target::Jit` 在 CLI 里有三处 `bail!`，分发逻辑只有一条真实路径。改成静态分发后，两个分支都是 `<Backend as JsBackend>::compile` 调用，编译器会检查两个实现是否都满足契约。如果 `JsBackend` 的签名设计有问题（比如强加 `Send` 约束），stub 会立刻编译失败。

`artifact_bytes` 返回 `None` 也是有意的：它演示了不可序列化制品的正确处理方式，`build -o` 会因此拿到空字节而不是假装成功。

> <details><summary>为什么 stub 也要走完整 `JsBackend` trait？</summary>
>
> 物理上看是个 36 行的「占位」crate，但它让类型系统替我们检查契约：
>
> - `JsBackend` 加了新方法？JitBackend 编译失败，提示补实现。
> - `JsBackend::execute` 改了 Future 形状？JitBackend 必须跟着改。
> - `JsBackend` 加了关联类型？JitBackend 必须给出占位但有效的定义。
>
> 这种「让类型系统检查协议完整性」的技巧在工业代码里很常见。比如 tokio 的「测试 runtime」、Rust 标准库的「Phantom」类型，都是同一个思路。
>
> </details>

## 实现它需要什么

按 `docs/backend-implementation-guide.md` 的六步接入法，一个真实的 JIT 后端需要：

1. `HeapMemory` + `GrowableHeapMemory` 实现，提供堆内存读写与增长。
2. `ExecContext` 实现，把 builtins 需要的约 330 个方法接到自己的运行时。
3. `JsBackend` 的 `compile`：IR → 本地代码。
4. `JsBackend` 的 `execute`：驱动生成的代码并收集输出。

语义算法不需要重写——`wjsm-builtins` 的全部内容会随 `ExecContext` 实现自动可用。

## 不要做的事

不要为了让 `--target jit` "看起来能跑"而让它静默回落到 wasm 后端。用户选择了后端，得到的必须是该后端的真实行为或明确的错误。

## 深入了解

- [多后端边界与 JsBackend 契约](multi-backend-boundary.md)
- [ExecContext 的方法域划分](../host-runtime/exec-context-and-builtins.md)
