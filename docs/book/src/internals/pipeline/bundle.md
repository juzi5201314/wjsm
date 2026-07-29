# 模块图与 Bundling 阶段

单文件源码直接进 lowering；一旦出现 `import`/`export` 或 CommonJS 标记，流水线就要先构建模块图。这一章说明分支判定与合并产物。

## 分支判定

`build_compile_plan`（`crates/wjsm-cli/src/lib.rs`）决定走哪条路：

1. 传了 `--root` → 直接 `bundle_plan_from_root`，并校验入口在 root 之下（否则报 `input file ... is not under root ...`）。
2. 未传 `--root` → 解析入口，用 `wjsm_module::is_es_module` 与 `is_commonjs_module` 判定。
3. 两者都为 false → `CompilePlan::SingleSource`，跳过模块图。
4. 否则 → `CompilePlan::Bundle`，root 取入口文件的父目录，entry 取文件名。

这是「单文件也能跑」与「多文件自动成图」共存的原因：判定依据是源码内容，不是命令行开关。

## Bundle 路径的阶段映射

`run_file_input_pipeline` 在 bundle 分支下按目标阶段调用不同入口：

| `--stage` | 调用 | 产物 |
| --- | --- | --- |
| `parse` | `wjsm_module::parse_entry_ast_with_options` | 入口 AST |
| `lower` | `wjsm_module::lower_bundle_with_options` | 合并后的 `Program` |
| `compile` / `execute` | `compile_bundle` → `lower_bundle_with_debug` + 后端 | WASM 字节 |

三条路径都接收同一个 `ResolutionOptions`，保证 `--browser` / `--condition` 在任意阶段行为一致。

## 合并语义

`wjsm-module` 不生成 WASM，它只产出 IR：图构建 → 解析每个模块的格式（ESM/CJS）→ CJS 转换 → 逐模块 lowering → 合并为单一 `Program`。因此 bundling 的输出与单文件 lowering 的输出是同一种类型，后端无需区分两者。

时间统计上，bundle 分支把整段耗时记入 `parse_us`/`lower_us`/`compile_us` 中对应目标阶段的一格，`--time` 输出因此在 bundle 与单文件之间形态一致。

## 深入了解

- [模块图构建与解析器实现](../modules/graph-and-resolution.md)
- [CommonJS 到 IR 的转换](../modules/commonjs-transform.md)
- [包解析条件与 browser 字段映射](../modules/package-conditions.md)
- [多个 Program 如何合并](../modules/program-bundling.md)
- [用户侧的多文件构建用法](../../user/projects/multi-file-builds.md)
