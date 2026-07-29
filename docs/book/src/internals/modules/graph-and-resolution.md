# 模块图与解析器

依赖图是模块系统的第一步：从入口出发解析每个 specifier，读取源码，提取 import/export，最终得到一张可拓扑排序的图。

## ModuleGraph 与 GraphNode

`graph.rs` 的 `ModuleGraph` 持有 `HashMap<ModuleId, GraphNode>` 和入口 `ModuleId`。每个 `GraphNode` 记录：

| 字段 | 含义 |
| --- | --- |
| `id` / `path` | 模块标识与规范化后的绝对路径 |
| `source` / `ast` | 源文本与 SWC AST，供后续 lowering 复用 |
| `imports` | `(ModuleId, ImportEntry)`，已解析到具体模块 |
| `exports` | `ExportEntry` 列表 |
| `dynamic_imports` | `(specifier, ModuleId)`，AOT 阶段已解析的动态 import 目标 |
| `is_cjs` | 是否走 CommonJS 转换路径 |

入口路径不经 specifier 拼接：`build_with_options` 对绝对路径直接使用，相对路径用 `root.join(entry)`，再交给 `resolver.resolve_entry_path`。这样非 UTF-8 路径也不会在解析中被破坏。

## ModuleResolver

`resolver.rs` 的 `ModuleResolver` 是路径解析 owner。候选扩展名固定为：

```rust
const MODULE_EXTENSIONS: &[&str] = &["js", "ts", "mjs", "cjs", "jsx", "tsx"];
```

解析顺序：路径本身 → 逐个补扩展名 → 目录 `index.<ext>`。目录内找不到 index 时报 `No index file in directory`。包解析缓存在 `package_cache`（`RefCell<HashMap>`），同一个 `package.json` 只读一次。

## 内置模块前置

`builtin_modules::lookup` 在文件系统解析之前介入。它返回三态：

- `Found` — 命中 24 个内置模块之一，源码来自 `builtin_js/` 下 `include_str!` 的 JS 实现。
- `UnknownNodeBuiltin` — 带 `node:` 前缀但不在表内，报 `Unknown built-in module`。
- `NotBuiltin` — 交回普通路径解析。

内置模块被赋予虚拟路径 `/__wjsm_builtin__/node/<canonical>.mjs`，`is_builtin_virtual_path` 用它区分虚拟节点与真实文件。因为查表先行，裸 specifier（`path`、`events`）也会命中内置实现，而不是 `node_modules` 里的同名包。

## 深入了解

- [用户视角的包解析规则与条件](../../user/projects/package-resolution.md)
- [包条件与 browser 字段的求值顺序](package-conditions.md)
- [循环依赖如何处理](cycles-and-cache.md)
