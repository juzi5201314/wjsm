# 编译编排入口

编排逻辑的 owner 是 `crates/wjsm-cli/src/lib.rs`。它决定单文件还是 bundle、在哪个阶段停止、以及产物交给哪个后端。

## PipelineResult 与 PipelineTimings

`PipelineResult` 用三个 `Option` 字段承载阶段产物，停在哪个阶段就只有对应字段被填充：

```rust
pub(crate) struct PipelineResult {
    pub(crate) ast: Option<swc_core::ecma::ast::Module>,
    pub(crate) program: Option<Program>,
    pub(crate) wasm: Option<Vec<u8>>,
    pub(crate) timings: PipelineTimings,
}
```

`PipelineTimings` 记录四段微秒耗时（`parse_us`、`lower_us`、`compile_us`、`execute_us`）。`--time` 触发 `PipelineTimings::print`：默认按毫秒输出，`-v` 起按微秒输出。`execute_us` 为 0 时不打印执行段。

> <details><summary>为什么 `execute_us` 为 0 时不打印？</summary>
>
> 不是 bug，是有意设计。`--stage compile` 不执行用户代码，`execute_us` 没值；`--stage execute` 才跑。
>
> 打印时跳过 0 段能避免：
>
> - 用户看到 `Timing: parse=6ms, lower=10ms, compile=6ms, execute=0ms` 误以为「execute 跑了但没花时间」。
> - 看到「0ms」会去找「为什么是 0」，而答案是「根本没跑」。
>
> 这种小细节不影响功能，但能减少用户的疑问。生产工具应当「少问为什么」。
>
> </details>

## CompilePlan：单文件还是 bundle

`build_compile_plan` 只有两个结果：

```rust
enum CompilePlan {
    Bundle { entry: PathBuf, root: PathBuf },
    SingleSource { source: String, filename: String },
}
```

判定顺序：

1. 显式 `--root` → 走 `bundle_plan_from_root`，校验入口在 root 之下，否则报 `input file ... is not under root ...`。
2. 无 `--root` 时先解析一次，用 `wjsm_module::is_es_module` 和 `is_commonjs_module` 判定。
3. 两者都不是 → `SingleSource`，完全绕过模块图。
4. 是模块 → `Bundle`，root 取入口文件所在目录，entry 取文件名。

这解释了一个可观察行为：不带 `--root` 运行含 `import` 的文件也能工作，因为入口目录被自动当作 root。

## 后端静态分发

`compile_program_to_wasm` 通过完全限定语法调用 `JsBackend`，避免 `dyn`：

```rust
match target {
    Target::Wasm => <runtime::WasmBackend as runtime::JsBackend>::compile(...),
    Target::Jit => <wjsm_backend_jit::JitBackend as runtime::JsBackend>::compile(...),
}
```

`JitBackend::compile` 直接 `bail!`，所以 `--target jit` 的错误来自后端本身而非 CLI 的前置检查。新增后端就是在这个 `match` 里加一个分支。

## 执行入口

`block_on_wasm_execute` 在进程级共享的 Tokio multi-thread runtime 上 `block_on`。共享用 `LazyLock`，避免每次 in-process 测试重建 runtime。

`run_compile_then_execute` 在执行前把 raw WASM 写入 pipeline cache 并设置 `current_entry`，供同入口 fork AOT handoff 复用；执行错误先经 `process_exit_code_from_error` 判定是否为 `process.exit`，是则透传退出码，否则输出 `Runtime error:` 并返回 2。

## 相关章节

- [用户视角的 `--stage` 与产物选择](../../user/cli/build.md)
- [阶段隔离与诊断输出](stage-isolation.md)
- [CLI 参数模型与配置合并](../tooling/cli-and-config.md)
