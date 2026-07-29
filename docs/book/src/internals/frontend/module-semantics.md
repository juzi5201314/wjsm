# 模块语义

这一章讲多模块降级：`wjsm-module` 构图之后，语义层如何把多个模块合成一个 IR `Program`。

## 入口

`lower_modules` / `lower_modules_with_debug`（`crates/wjsm-semantic/src/lowerer_modules.rs`，约 1195 行）接收 `ModuleLoweringInput` 列表，每项带 `ModuleMetadata` 与 `ModuleKind`（ESM 或 CommonJS）。所有模块共用一个 `Lowerer`，因此共用一棵作用域树和一个常量池。

## 阶段顺序

`lower_modules` 内部的顺序是固定的，跳过任何一步都会破坏 TDZ 或导出可见性：

1. `setup_multi_module_lowerer`：建立共享 Lowerer 与各模块的顶层作用域。
2. `predeclare_module_exports` / `predeclare_cjs_host_bindings`：预声明导出名与 CJS 宿主绑定。
3. `apply_re_export_map` + `resolve_export_ir`：解析 re-export 链，确定每个导出名最终指向的 IR 名。
4. `process_import_aliases`：把 import 别名绑到目标模块的导出。
5. `init_entry_block` + `emit_global_constants`：建立入口块并发射全局常量。
6. `create_namespace_objects`：为需要命名空间对象的模块建对象。
7. `emit_cjs_host_bindings`：注入 `module`、`exports`、`require`、`__dirname`、`__filename` 等绑定。
8. `lower_module_bodies`：按依赖顺序降级各模块体。
9. `finalize_multi_module`：收尾。

预声明先于降级，是为了让循环依赖中「先执行的模块引用后执行模块的绑定」能拿到已存在的 IR 名，而不是报 undeclared。

## 作用域复用

多模块降级在 predeclare 与 lower 两个阶段之间需要重新激活某模块的顶层作用域。`ScopeTree::enter_scope(id)` 用于这个目的：它直接把 `current` 指回已存在的作用域 id，而不是 `push_scope` 新建一个。这保证同一模块在两个阶段里看到同一批绑定。

## 命名空间对象

`create_namespace_objects` 建立 `import * as ns` 需要的对象。`install_live_namespace_getters_for_source` 为其安装 live getter，使命名空间成员跟随源模块绑定变化；`set_namespace_string_tag` 设置 `Symbol.toStringTag`。

## CommonJS 绑定

CJS 模块的 `module`、`exports`、`__dirname`、`__filename` 等不是运行时魔法变量，而是降级期注入的普通绑定（`emit_cjs_host_bindings` 及其 `emit_cjs_*` 辅助函数）。这也解释了为什么在 `.mjs` 里写 `module.exports` 会得到 `undeclared identifier module`：ESM 路径不注入这批绑定。

## 深入了解

- [模块图构建与解析规则](../modules/graph-and-resolution.md)
- [CommonJS 到 ESM 的转换](../modules/commonjs-transform.md)
- [多模块 IR Program 的合并](../modules/program-bundling.md)
