# IR Program Bundling

`bundler.rs` 把模块图变成单个 `wjsm_ir::Program`。它是 `wjsm-module` 的出口，也是该 crate 不依赖任何后端的原因——bundler 只产出 IR。

## ModuleBundler

```rust
pub struct ModuleBundler {
    root_path: PathBuf,
    options: ResolutionOptions,
    emit_debug_checks: bool,
}
```

`with_resolution_options` 注入条件解析选项，`with_emit_debug_checks` 是 `--inspect` 路径的开关（语句级 `DebugCheck` 插桩）。

## lower_bundle 流程

1. `ModuleGraph::build_with_options` 构图。
2. `topological_order` 取执行顺序；返回的 `cycles` 在此处被显式丢弃（`let _ = cycles`），循环不阻断编译。
3. `analyze_module_links` 得到 `ModuleLinkResult`。
4. 按拓扑序为每个节点构造 `ModuleLoweringInput { id, ast, metadata, source }`。
5. 调用 `wjsm_semantic::lower_modules_with_debug` 一次性 lower 全部模块。

`source` 以 `Arc<str>` 传入，供诊断和 debug 插桩解析行列，多模块间共享不复制。

## 公共入口

`lib.rs` 暴露的函数按用途分层：

| 函数 | 用途 |
| --- | --- |
| `lower_bundle` / `lower_bundle_with_options` / `lower_bundle_with_debug` | AOT 主路径，产出 `Program` |
| `bundle_program` / `bundle_program_with_options` | 同上，错误信息附带 `entry` 与 `root_path` |
| `lower_runtime_entry_bundle_*` | 运行时动态加载入口，返回 `RuntimeEntryBundle` |
| `lower_runtime_builtin_bundle_*` | 运行时加载 Node 内置模块 |
| `parse_entry_ast_with_options` | `dump-ast --root` 路径，只建图取入口 AST |

`RuntimeEntryBundle` 额外携带 `entry_module_id` 和 `module_id_span`：运行时把新 bundle 的 module id 偏移到已有区间之后，避免与已加载模块冲突。偏移由 `wjsm_ir::offset_module_id` 完成，溢出返回 `ModuleIdOffsetError` 而非 panic。

> <details><summary>为什么 module id 需要偏移？</summary>
>
> 运行时场景下，进程里可能同时存在多个 bundle（eval、动态 import 加载的新模块）。每个 bundle 都有自己的 module id 编号。
>
> 如果不偏移，两个 bundle 的 module id 0 可能指不同模块，运行时查找会冲突。
>
> 偏移的做法是：第二个 bundle 的所有 module id 加一个基数（base），保证不同 bundle 编号不重叠。基数由前一个 bundle 的 `module_id_span`（id 范围）决定。
>
> 这就是为什么 `offset_module_id` 是 `checked_add`——理论上一个进程可以加载无数模块，溢出是真实风险，返回 `ModuleIdOffsetError` 让调用方决定怎么办。
>
> </details>

## 与单文件路径的关系

CLI 的 `build_compile_plan` 决定走 bundle 还是单文件：只有含 `import` / `export` / CJS 标记，或显式传 `--root` 时才进入本章路径。纯脚本直接 `parse → lower_module`，不建图。

## 深入了解

- [编译编排如何选择 bundle 或单文件](../pipeline/orchestration.md)
- [语义层的多模块 lowering](../frontend/module-semantics.md)
- [ModuleId 与 IR 标识符](../ir/identifiers-and-display.md)
