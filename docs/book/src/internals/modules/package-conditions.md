# 包解析、条件与 browser 映射

条件解析决定 `exports` / `imports` 字段选中哪个分支。owner 是 `resolution_options.rs`（条件序列）与 `exports.rs`（匹配算法）。

## 条件序列构造

`build_conditions` 按固定顺序拼装，`push_unique_condition` 保证去重：

1. `wjsm`
2. `browser`（仅 `--browser` 时）
3. 用户自定义条件（`--condition`，按给出顺序）
4. `node`
5. 边类型：import 边为 `import`，require 边为 `require`
6. `default`

边类型来自 `ResolutionKind::condition()`，同一份 `ResolutionOptions` 因此持有两套序列（`import_conditions` / `require_conditions`），由 `conditions_for_kind` 取用。

`is_reserved_custom_condition` 把 `wjsm`、`browser`、`node`、`import`、`require`、`default` 列为保留字：用户通过 `--condition` 传这些名字会被忽略，避免打乱固定优先级。

`wjsm` 永远在首位，这意味着包里存在 `"wjsm"` 键时它总是胜出，`browser` 和自定义条件都无法覆盖。

## exports 与 imports 匹配

`resolve_package_exports` / `resolve_package_imports` 实现 Node 的条件导出算法，返回 `PackageTarget { relative_path }`。

失败时按 Node 的错误码报错，便于对照上游行为：

| 情况 | 错误 |
| --- | --- |
| 子路径未导出 | `ERR_PACKAGE_PATH_NOT_EXPORTED` |
| `#` 导入未定义 | `ERR_PACKAGE_IMPORT_NOT_DEFINED` |

`normalize_package_subpath` 把 `""` 与 `"."` 归一为 `.`，其余补上 `./` 前缀。`package_error_context` 给错误附上包名、`package.json` 路径和字段名。

## browser 字段

`package_json.rs` 的 `BrowserField` 有两种形态：

```rust
enum BrowserField {
    Entry(String),                            // "browser": "./b.js"
    Map(BTreeMap<String, Option<String>>),     // "browser": { "./s.js": "./b.js", "fs": false }
}
```

`Map` 的值为 `None` 表示该模块被置为 false（屏蔽）。两种形态只在 `--browser` 打开时参与解析：`resolve_legacy_package_entry` 按 `browser entry → module → main` 顺序尝试。

## 无 exports 字段的回落

包没有 `exports` 时走 legacy 路径：`module` → `main` → 目录 `index.*`。扩展名候选顺序由 `MODULE_EXTENSIONS` 固定为 `js`、`ts`、`mjs`、`cjs`、`jsx`、`tsx`。

## 深入了解

- [用户视角的条件与包解析规则](../../user/projects/package-resolution.md)
- [`--browser` 与 `--condition` 的命令行语义](../../user/configuration/module-resolution.md)
