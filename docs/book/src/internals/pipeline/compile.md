# WASM 编译阶段

这一章说明 IR 到 WASM 字节的转换如何被调用，以及后端在此处如何静态分发。

## 静态分发入口

`compile_program_to_wasm`（`crates/wjsm-cli/src/lib.rs`）是唯一的后端选择点：

```rust
let bytes: Vec<u8> = match target {
    Target::Wasm => {
        let a = <runtime::WasmBackend as runtime::JsBackend>::compile(
            &runtime::WasmBackend, program, debug_codegen,
        )?;
        <runtime::WasmBackend as runtime::JsBackend>::artifact_bytes(&a)
            .map(|b| b.to_vec()).unwrap_or_default()
    }
    Target::Jit => { /* JitBackend，同形状 */ }
};
```

两点值得注意：

- 用完全限定语法调用 trait 方法，没有 `dyn JsBackend`，没有 vtable。
- `Target::Jit` 不是特殊分支：它走完全相同的 `compile` + `artifact_bytes` 形状，只是 `JitBackend::compile` 内部 `bail!("JIT backend is not implemented yet")`。新后端在此 match 加一个分支即可接入，这是 ADR 0013 的契约。

## 后端侧入口

`wjsm-backend-wasm` 暴露的编译入口按用途分开：

| 函数 | 用途 |
| --- | --- |
| `compile` | 普通模块编译 |
| `compile_with_options` | 带 `CompileOptions { debug }` |
| `compile_runtime_module_at` / `_with_options` | 运行时加载的模块，指定数据段基址 |
| `compile_eval` / `compile_eval_at_data_base` | eval 模式，产出 `RuntimeCompiledModule` |

Normal 模式与 eval 模式的差别不只是入口不同：normal 模式从 `wjsm_support` 导入 10 个 helper，eval 模式没有独立 support instance，走内联 helper 路径（ADR 0004）。

## debug 插桩的传递

`--inspect` / `--inspect-brk` 会让 `Cli::wants_debug_codegen()` 为 true，该标记同时传给 lowering（发射 `DebugCheck`）和 codegen（生成 `wjsm_debug` 段与 `debug_break` 调用）。两侧必须同时开启，否则断点无法映射回源码位置。

## 深入了解

- [WASM 编译器的内部结构](../backend/compiler-architecture.md)
- [Normal 与 Eval 两种编译模式的差异](../backend/normal-and-eval-modes.md)
- [Support 模块与导入的 helper 集合](../backend/support-module.md)
- [多后端边界与 JsBackend 契约](../backend/multi-backend-boundary.md)
- [JIT 后端当前的 stub 形态](../backend/jit-backend.md)
