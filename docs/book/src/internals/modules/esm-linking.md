# ESM 链接与求值

链接阶段把「谁导出什么、谁导入什么」变成 IR 可消费的绑定表，并在此处拒掉重复导出和缺失导出。

## ModuleLinkResult

`semantic.rs` 的 `analyze_module_links` 遍历整张图，产出五张表：

| 字段 | 内容 |
| --- | --- |
| `import_map` | `ModuleId → Vec<ImportBinding>`，已解析到源模块 |
| `export_names` | `ModuleId → BTreeSet<String>`，该模块对外可见的名字 |
| `dynamic_import_targets` | 每个模块动态 import 的目标模块 |
| `dynamic_import_specifiers` | `(specifier, ModuleId)` 对，供语义层建查找表 |
| `re_export_map` | `ModuleId → Vec<ReExportBinding>` |

`export_names` 用 `BTreeSet` 而非 `HashSet`：名字集合参与后续 IR 生成，有序保证同样输入产出同样的 IR 文本，这是快照测试的前提。

## 导出收集与早期错误

每个模块的导出汇总到 `CollectedExports`：

```rust
struct CollectedExports {
    names: BTreeSet<String>,
    has_wildcard_reexport: bool,
}
```

`ExportEntry` 有四类：`Named`、`NamedReExport`、`Declaration`、`Default`（`default` 也占用一个名字）。任一类插入失败即报 `Duplicate export '<name>' in module '<path>'`，在 lowering 之前就拒绝。

`has_wildcard_reexport` 让 `supports_name` 放宽判定：存在 `export * from` 时，无法静态枚举的名字也视为受支持，避免把合法代码误判为缺失导出。

## 导入校验

导入侧按 `export_names` 检查：`import { x } from './m'` 要求 `m` 的集合包含 `x`，或 `m` 带通配重导出。`import * as ns` 用 `"*"` 作为 imported 名，`import d from` 用 `"default"`。这套编码定义在 `wjsm_ir::ImportBinding`，与语义层共享。

## 求值顺序

`ModuleGraph::topological_order` 返回 `(order, cycles)`。`order` 是依赖优先的求值顺序，bundler 按此顺序把模块喂给 `lower_modules_with_debug`，因此被依赖者的顶层语句先执行。

## 深入了解

- [用户视角的 ESM 用法与限制](../../user/projects/esm.md)
- [IR Program 如何合并多模块](program-bundling.md)
- [语义层如何生成命名空间对象](../frontend/module-semantics.md)
