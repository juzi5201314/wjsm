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

> <details><summary>完全限定语法调用 trait 有什么副作用？</summary>
>
> `<WasmBackend as JsBackend>::compile` 形式的调用在 Rust 里是「单态化」——编译器为每个后端生成专门的代码，没有 vtable 查表，调用是直接的函数调用。
>
> 与之相对的是 `dyn JsBackend`——编译时不知道具体类型，运行时通过 vtable 查函数。性能上每次调用多一次间接跳转，且 vtable 占空间、不能内联。
>
> 对于 wjsm 这种每条指令都可能调 builtin 的场景，「避免 vtable」是值得的：性能更好、编译器能内联更激进。
>
> 代价是失去一些动态性——必须编译时知道所有后端类型。但 wjsm 只需要 wasm + jit 两个，加一个后端是显式的代码改动，不是动态加载。
>
> </details>

## 后端侧入口

`wjsm-backend-wasm` 暴露的编译入口按用途分开：

| 函数 | 用途 |
| --- | --- |
| `compile` / `compile_with_options` | Normal 模式，产出独立 `Vec<u8>` |
| `compile_runtime_module_at` / `_with_options` | Normal 模式，指定 `data_base` / `table_base`，返回 `RuntimeCompiledModule` |
| `compile_eval` / `compile_eval_at_data_base` | Eval 模式，导入父实例的内存、global 与函数表 |

Normal 模式与 eval 模式的差别不只是入口不同：normal 模式从 `wjsm_support` 导入 10 个 helper，eval 模式没有独立 support instance，走内联 helper 路径（ADR 0004）。

## debug 插桩的传递

`--inspect` / `--inspect-brk` 会让 `Cli::wants_debug_codegen()` 为 true，该标记同时传给 lowering（发射 `DebugCheck`）和 codegen（生成 `wjsm_debug` 段与 `debug_break` 调用）。两侧必须同时开启，否则断点无法映射回源码位置。

## 深入了解

- [WASM 编译器的内部结构](../backend/compiler-architecture.md)
- [Normal 与 Eval 两种编译模式的差异](../backend/normal-and-eval-modes.md)
- [Support 模块与导入的 helper 集合](../backend/support-module.md)
- [多后端边界与 JsBackend 契约](../backend/multi-backend-boundary.md)
- [JIT 后端当前的 stub 形态](../backend/jit-backend.md)
