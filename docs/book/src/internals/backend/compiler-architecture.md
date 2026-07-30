# WASM 编译器架构

`wjsm-backend-wasm` 把 IR `Program` 编译成 WASM 字节。这一章说明 `Compiler` 的组成、两遍编译顺序，以及各子模块的职责划分。

## 公开入口

`crates/wjsm-backend-wasm/src/lib.rs` 暴露四组入口：

| 函数 | 用途 |
| --- | --- |
| `compile` / `compile_with_options` | Normal 模式，产出独立 `Vec<u8>` |
| `compile_runtime_module_at(_with_options)` | Normal 模式，指定 `data_base` / `table_base`，返回 `RuntimeCompiledModule` |
| `compile_eval` / `compile_eval_at_data_base` | Eval 模式，导入父实例的内存、global 与函数表 |
| `emit_support_module` | 构建期产出 support 模块 |

`RuntimeCompiledModule` 除 `wasm` 还带 `table_len` 与 `data_len`，供运行时把下一个模块接在已用区间之后。

`CompileOptions { debug }` 控制是否发射 `wjsm_debug` 自定义段，并把 `DebugCheck` 编译成对 `env.debug_break` 的调用。

## Compiler 状态

`Compiler` 是单结构体，聚合 `wasm-encoder` 的各个 section（`types`、`imports`、`functions`、`exports`、`codes`、`memory`、`data`、`table`、`elements`、`globals`）以及编译期映射表：

- `string_data` / `data_base` / `data_offset`：字符串与常量的数据段布局。
- `var_locals` / `var_memory_offsets` / `next_var_local`：变量到 WASM local 或 eval 帧偏移的映射。
- `phi_locals`：Phi 目标 `ValueId` 到 local 索引。
- `function_name_to_wasm_idx` / `function_id_to_wasm_idx` / `function_table`：函数索引与函数表下标。
- `gc_analysis`：模块级 GC 分析结果。
- `source_map_entries`：函数级 `line:col`，写入自定义段供运行时映射错误堆栈。

## 编译顺序

`compiler_module/module_compile.rs` 的 `compile_module` 按固定顺序推进：

1. **Pass 0**：`GcAnalysis::analyze(module)` 做模块级 GC 分析，并记录源文件路径。
2. **Pass 1**：遍历所有 IR 函数登记 WASM 函数索引、参数个数、`needs_prototype`、函数表下标与 source span。模块入口 `$module_main` 在 Normal 模式用 Type 4（`() -> i64`），Eval 模式用 Type 3（`(i64) -> i64`）；其余 JS 函数统一 Type 12。
3. **导出入口**：Normal 模式导出 `main`，Eval 模式导出 `__eval_entry`。
4. **预留 helper 索引**：绑定 support helper 与 `Array.prototype` 方法表，使用户函数编译时索引已确定。
5. **函数体编译**：逐函数生成指令。
6. **收尾**：`finish()` 组装各 section 成模块字节。

先登记全部函数再编译函数体，是因为函数体内的直接调用与 `call_indirect` 都需要提前确定索引。

> <details><summary>为什么不「边解析边编译」？</summary>
>
> 表面上「遇到一个函数就编译一个」更简单，实际不行——函数体内可能有 `call_indirect`，需要知道函数表里有什么；可能有对其他函数的直接调用，需要知道它们的索引。
>
> 两遍编译的代价：函数信息要存两遍（Pass 1 存元数据，Pass 5 再用）。但这个开销不大——元数据只是几条数字。
>
> 收益是函数体编译时所有外部引用都已知，不用回填、不用修补。codegen 是线性的、确定的。
>
> </details>

## 子模块划分

编译器按职责切成目录：

- `compiler_core.rs`：section 初始化、import 构造、global 布局。
- `compiler_module/`：模块级编排（`module_compile`、`module_setup`、`module_bootstrap`）。
- `compiler_instructions/`：指令翻译（`instr_main`、`instr_calls`、`instr_helpers`、`instr_super`）。
- `compiler_control/`：控制流（`control_branch`、`control_structured`、`control_switch`、`control_analysis`、`control_locals`）。
- `compiler_builtins*.rs`：Builtin 分派，按 core / collections / string_math / async_proxy / runtime 分组。
- `analysis_liveness.rs` / `analysis_value_ty.rs` / `compiler_gc_analysis.rs`：活跃性、值类型与 GC 分析。
- `host_import_registry/`：host import 规格表。
- `support_module.rs` / `shared_types.rs`：support 模块与共享 type section。

`GcAnalysis` 被 `pub use` 导出，因为 WAT 层面无法明确区分 GC 决策，集成测试需要直接断言 `call_may_trigger_gc`。

## 深入了解

- [Normal 与 Eval 两种编译模式的差异](normal-and-eval-modes.md)
- [共享 type section 与 support 模块的 ABI 对齐](support-module.md)
- [控制流如何从 CFG 还原为结构化 WASM](control-flow-codegen.md)
