# 作为 Rust 库嵌入

除命令行外，wjsm 可以作为 Rust 依赖直接编译并执行 JavaScript。

## 依赖

`wjsm-runtime` 是兼容 facade，re-export 执行引擎、GC 与宿主 trait 的全部公开 API：

```toml
[dependencies]
wjsm-runtime = { path = "../wjsm/crates/wjsm-runtime" }
tokio = { version = "1", features = ["rt", "macros"] }
```

## 编译并执行

`compile_source` 把源码编译为 WASM 字节，`execute_with_options` 在 Wasmtime 上执行。执行是异步的，需要 Tokio 运行时：

```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let wasm = wjsm_runtime::compile_source("console.log('from embedded host')")?;
    let options = wjsm_runtime::RuntimeOptions::default();
    wjsm_runtime::execute_with_options(&wasm, options).await?;
    Ok(())
}
```

编译和执行是分开的两步，可以缓存 `wasm` 字节反复执行，或者把编译放在构建期。

## 配置执行环境

`RuntimeOptions` 是执行侧的配置入口，命令行的运行时选项最终都落到它上面：

| 字段 | 作用 |
| --- | --- |
| `max_heap_size` | JavaScript 堆预算上限 |
| `shadow_stack_max` | 影子栈软上限，默认 16MiB |
| `gc_algorithm` | 回收器，默认 `GcAlgorithmKind::Zgc` |
| `compiler` | 显式指定 Cranelift 或 Winch |
| `argv` / `env` / `cwd` | 暴露给 `process` 的进程信息 |
| `fs_read_roots` / `fs_write_roots` | 文件系统沙箱允许的根目录 |
| `fs_allow_write_anywhere` | 解除写路径限制 |
| `inspect` | CDP inspector 监听配置 |

默认值不开放文件写入之外的额外权限，沙箱根目录需要显式给出。

> <details><summary>为什么必须用 Tokio？</summary>
>
> wjsm 的执行是「异步」是因为它的运行时本身是异步的——Wasmtime 内部依赖 async 来管理 host function 调用、Promise settle、I/O 等待等。`execute_with_options` 返回 `impl Future`，调用方需要驱动它。
>
> 选择 Tokio 而不是 `async-std` 或 smol 是 wasmtime 决定的——wasmtime 自身的 async runtime 集成基于 Tokio。如果你的项目不用 Tokio，需要加一个；或者用 `block_on` 包一层（同步用）：
>
> ```rust
> let wasm = wjsm_runtime::compile_source(...)?;
> let options = wjsm_runtime::RuntimeOptions::default();
> tokio::runtime::Runtime::new()?.block_on(
>     wjsm_runtime::execute_with_options(&wasm, options)
> )?;
> ```
>
> 同步用法在 CLI 工具里没问题；生产服务里建议整个进程用 Tokio，wjsm 是其中一个 task。
>
> </details>

## 捕获输出

`execute_with_options` 把程序输出写到进程的标准输出。需要拿到输出内容时用 `execute_with_writer_with_options`，它接受一个 `Write` 实现并返回该 writer 和诊断字节。

## 深入了解

- [Runtime facade 与公共 API 的组织方式](../../internals/host-runtime/runtime-facade.md)
- [Engine 配置：编译器、内存预留与池化键](../../internals/host-runtime/engine-configuration.md)
- [实例化与执行生命周期的完整步骤](../../internals/host-runtime/instantiation-and-lifecycle.md)
- [新后端需要实现的契约](../../internals/backend/multi-backend-boundary.md)
