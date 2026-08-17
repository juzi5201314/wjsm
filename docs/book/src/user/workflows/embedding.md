# 作为 Rust 库嵌入

`wjsm-runtime` 是 direct native runtime 的稳定 facade：

```toml
[dependencies]
wjsm-runtime = { path = "../wjsm/crates/wjsm-runtime" }
```

## 执行源码

```rust
use wjsm_runtime::{RuntimeInput, RuntimeOptions, execute_with_writer_with_options};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut output = Vec::new();
    let execution = execute_with_writer_with_options(
        RuntimeInput::Source("console.log('from embedded host')"),
        &mut output,
        RuntimeOptions::default(),
    )?;
    assert_eq!(execution.exit_code, 0);
    Ok(())
}
```

## 构建后执行

`compile_source` 返回 canonical portable `.wjsm` bytes。可以保存或传输这些 bytes，再用 `RuntimeInput::Artifact` 执行：

```rust
use wjsm_runtime::{
    RuntimeInput, RuntimeOptions, compile_source, execute_with_writer_with_options,
};

let artifact = compile_source("console.log(42)")?;
let mut output = Vec::new();
let execution = execute_with_writer_with_options(
    RuntimeInput::Artifact(&artifact),
    &mut output,
    RuntimeOptions::default(),
)?;
# Ok::<(), wjsm_runtime::NativeRuntimeError>(())
```

`RuntimeOptions` 配置 cache dir、module root、working directory、环境变量、collector、heap 上限与 source compile options。执行 API 是同步 owner-thread API；I/O/timer 等异步语义由 `NativeRuntime` 自己的 event loop 驱动。

`NativeRuntime` 不能跨线程移动或并发调用。每个 agent/runtime 拥有独立 heap、scheduler 与 mutable side tables。

## 深入了解

- [实例化与执行生命周期](../../internals/host-runtime/instantiation-and-lifecycle.md)
- [Host 能力 Trait](../../internals/host-runtime/host-traits.md)
