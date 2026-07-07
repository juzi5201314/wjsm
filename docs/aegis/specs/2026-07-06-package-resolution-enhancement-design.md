# Package Resolution Enhancement 设计（issue #309）

- 日期：2026-07-06
- 状态：待用户审阅
- 关联 issue：#309
- 输入：issue #309 正文与审查意见、`AGENTS.md` 模块架构约束、`crates/wjsm-module` 当前 resolver/graph/builtin_modules 代码

## 1. 背景与当前事实

issue #309 要完善 `wjsm-module` 的 npm 包解析能力：`exports`、`imports`、`type: module`、`.mjs/.cjs`、`browser`、自引用、`node:` 前缀。

当前代码事实：

| 能力 | 当前实现 | 缺口 |
|---|---|---|
| bare specifier | `resolver.rs` 已从父目录向上找 `node_modules/<pkg>`，支持 scoped 包名拆分 | 只支持包根 `module/main/index` 与裸子路径文件拼接 |
| package.json 入口 | `resolve_package_entry` 读取 `module`、`main`，再回退 `index.*` | 不读取 `exports`、`imports`、`type`、`browser` |
| `node:` 前缀 | `builtin_modules.rs` 已把 `node:path` canonicalize 到 `path`，未知 `node:` 报错 | 需要把这条能力纳入 issue #309 验收，避免重复实现 |
| 模块格式 | `load_resolved_module` 只用 AST 中 CJS 语法检测 `is_cjs` | `.cjs`、`.mjs`、package `type` 不能强制格式；纯 CJS `.js` 中无 CJS 语法时无法按包类型判定 |
| 图构建 | `graph.rs` 统一通过 `resolver.resolve(specifier, parent)` 解析静态 import、重导出、动态 import | 无法给解析器传入 import/require 条件上下文，也没有 `#imports` 当前包上下文 |
| 文件体量 | `resolver.rs` 1114 行、`cjs_transform.rs` 983 行 | issue #309 不应继续把解析全部堆进 `resolver.rs` |

Baseline Role Alignment：**aligned（scope: architecture）**。issue 目标与当前架构相符：解析属于 `wjsm-module` 编译期 bundler，不应下沉到 runtime；`node:` 的当前实现是正确 owner，只需纳入验证。

## 2. 目标与非目标

### 目标

1. 实现 Node 风格 PACKAGE_RESOLVE / PACKAGE_SELF_RESOLVE 的 wjsm 子集，覆盖 `exports`、`imports`、包自引用、exports 优先于 `module/main`。
2. 支持条件映射，按用户已确认的默认优先级：`wjsm > node > import/require > default`。
3. 支持 `exports` 字符串、对象、条件嵌套、子路径、子路径模式、`null` 显式阻止导出。
4. 支持 `imports` 的 `#name` / `#pattern` 包内别名，错误为 `ERR_PACKAGE_IMPORT_NOT_DEFINED`。
5. 支持 `.mjs` 强制 ESM、`.cjs` 强制 CJS、最近 package.json 的 `type: "module" | "commonjs"` 控制 `.js` 默认格式。
6. 支持 `browser` 字段：当条件集中包含 `browser` 时启用字符串入口替代与对象替换映射。
7. 保留现有 `node:` 前缀内置模块注册表语义，并补足 resolver/graph 层测试。

### 非目标

- workspace 协议：`workspace:`、pnpm/yarn workspace 解析。
- `file:`、`link:`、`pnpm:` 等包管理器协议。
- `module.createRequire` 运行时互操作。
- TypeScript 专用 `types` 条件作为运行时条件参与解析。
- `deno`、`bun` 等竞品运行时条件。
- 通用动态 `require()`：issue #309 只处理静态可分析的 import/export/dynamic import 与已由 CJS 转换提取出的静态 require。

## 3. First-principles / Architecture Integrity Lens

First-principles invariants：

- Non-negotiable goal：specifier 必须在编译期解析到唯一模块文件或内置模块；错误必须可诊断且遵循 Node 包边界语义。
- Non-negotiable constraints：解析 owner 是 `wjsm-module`；runtime 不参与 npm 包路径查找；`exports` 存在时禁止 `module/main` 回退；路径 target 不得逃出包目录。
- Historical assumptions to delete："bare 子路径 = package_dir.join(subpath)" 只适用于没有 `exports` 的旧包；"AST 语法决定 CJS/ESM" 不能代表 Node 文件格式规则。

Architecture Integrity Lens：

- Invariant：所有 specifier 到文件/内置模块的判定必须集中在 resolver 层，graph 只消费解析结果。
- Canonical owner / contract：`crates/wjsm-module/src/package_json.rs` 负责 package.json 读取与最近 package 查找；`exports.rs` 负责 exports/imports target 语义；`resolver.rs` 编排解析算法与缓存。
- Responsibility overlap：`builtin_modules.rs` 已拥有内置模块表，禁止在 `resolver.rs` 复制内置模块名单。
- Higher-level simplification：先建立 `ResolutionKind::{Import, Require}` 与 `ResolutionOptions`，让静态 import、CJS require、动态 import 共用同一个 package resolver，不在 graph 中做条件分支。
- Retirement / falsifier：旧 `resolve_package_entry` 的 `module/main/index` 逻辑降级为 `legacy_package_entry`，仅在无 `exports` 时被调用；测试必须证明 `exports` 存在时不会回退。
- Verdict：proceed，采用拆分 owner 后重构 resolver 的方案。

## 4. 方案对比

| 方案 | 内容 | 优点 | 风险 | 结论 |
|---|---|---|---|---|
| A. 在 `resolver.rs` 直接补齐全部逻辑 | 原文件内新增 exports/imports/type/browser | 改动集中 | `resolver.rs` 已 1114 行，会继续变成多职责巨型文件；难以单测 target 语义 | 拒绝 |
| B. 拆分 package resolver owner | 新增 `package_json.rs`、`exports.rs`、`resolution_options.rs`；`resolver.rs` 只编排 | 符合 ≤500 行文件纪律；target 解析可独立测试；旧逻辑可清晰退休 | 初次改动文件较多 | **推荐** |
| C. 引入第三方 Node resolution crate | 复用 npm 解析库 | 可能快速覆盖边界 | 依赖行为不一定可裁剪到 wjsm 的 AOT/根路径限制；错误码与模块格式仍需适配 | 拒绝 |

推荐方案 B。

## 5. 解析数据模型

新增内部数据结构，不扩大 `wjsm-module` 公共 API：

```rust
pub(crate) struct ResolutionOptions {
    pub extra_conditions: Vec<String>,
    pub browser: bool,
}

pub(crate) enum ResolutionKind {
    Import,
    Require,
}

pub(crate) struct PackageInfo {
    pub dir: PathBuf,
    pub name: Option<String>,
    pub exports: Option<serde_json::Value>,
    pub imports: Option<serde_json::Value>,
    pub module: Option<String>,
    pub main: Option<String>,
    pub browser: BrowserField,
    pub package_type: PackageType,
}

pub(crate) enum PackageType {
    Module,
    CommonJs,
}

pub(crate) enum BrowserField {
    Disabled,
    Entry(String),
    Map(BTreeMap<String, Option<String>>),
}
```

`ResolutionKind` 由调用边决定：

- ESM `import` / re-export / dynamic `import()`：`Import`。
- CJS 转换器提取出的 `require()`：`Require`。
- 入口文件：不参与条件选择；只按路径加载并按格式规则判定模块类型。

条件列表构造：

1. `extra_conditions` 中的显式条件，保留调用方顺序，但 `wjsm` 永远最高优先级。
2. 默认平台条件 `node`。
3. 当前边条件：`import` 或 `require`。
4. `default`。

当 `browser == true` 时，把 `browser` 插入 `node` 之前或由 `extra_conditions` 显式控制；默认 CLI/公共 API 不启用 `browser`。

## 6. package.json owner 与缓存

新增 `package_json.rs`：

- `read_package_info(package_dir: &Path) -> Result<Option<PackageInfo>>`：只读取当前目录 package.json，JSON 解析错误带路径上下文。
- `find_nearest_package(start: &Path, root: &Path) -> Result<Option<PackageInfo>>`：从文件所在目录向上找最近 package.json，但不越过 resolver `root_path`。
- `find_package_in_node_modules(package_name, from_dir)` 继续由 resolver 编排，但找到包目录后立刻读取 `PackageInfo`。
- 包信息按 `PathBuf` 缓存在 `ModuleResolver`，避免 graph BFS 重复读同一个 package.json。

`type` 字段解释：

- `"module"` → `.js` 默认 ESM。
- `"commonjs"` 或缺失 → `.js` 默认 CJS。
- 其他字符串忽略为 CommonJS，并保留与 Node 一致的默认行为；不做宽容 fallback 日志。

## 7. exports/imports 解析 owner

新增 `exports.rs`，只负责纯数据到 target 的解析，不读文件系统：

```rust
pub(crate) fn resolve_package_exports(
    package: &PackageInfo,
    package_subpath: &str,
    conditions: &[&str],
) -> Result<PackageTarget>;

pub(crate) fn resolve_package_imports(
    package: &PackageInfo,
    specifier: &str,
    conditions: &[&str],
) -> Result<PackageTarget>;
```

核心规则：

- `exports` 为字符串：只对应 `.`。
- `exports` 为对象：
  - keys 全是条件名（不以 `.` 开头）→ treat as conditional main export。
  - keys 含 `.` → treat as subpath map；禁止条件 key 与 subpath key 混用，混用报 `ERR_INVALID_PACKAGE_CONFIG`。
- 子路径导出：`mod/feature` 转为 `./feature`。
- 子路径模式：支持单个 `*` 替换；选择最长静态前缀、再按 key 插入顺序稳定匹配。
- target 为 `null`：报 `ERR_PACKAGE_PATH_NOT_EXPORTED`。
- target 为数组：Phase 2 不引入 fallback 数组；报 `ERR_INVALID_PACKAGE_TARGET`，避免把 fallback 语义做成隐式兼容路径。
- target 字符串必须以 `./` 开头；禁止绝对路径、URL、`..` 段、`node_modules` 段，报 `ERR_INVALID_PACKAGE_TARGET`。
- 条件嵌套递归按条件列表选择第一个命中分支；未命中且没有 default → 对 exports 为 not exported，对 imports 为 not defined。

错误字符串采用 Node 错误码前缀，便于 fixtures 断言：

- `ERR_PACKAGE_PATH_NOT_EXPORTED: Package subpath './x' is not defined by "exports" in <pkg>`
- `ERR_INVALID_PACKAGE_TARGET: Invalid "exports" target '<target>' in <pkg>`
- `ERR_PACKAGE_IMPORT_NOT_DEFINED: Package import specifier '#x' is not defined in <pkg>`
- `ERR_INVALID_PACKAGE_CONFIG: Invalid package config <path>`

## 8. resolver 编排算法

`ModuleResolver::resolve` 仍是 graph 的入口，但内部改为：

1. 先查 `builtin_modules::lookup(specifier)`；`node:` 不走文件系统。
2. `specifier.starts_with('#')`：从 parent 最近 package 读 `imports`，用当前边 kind 的条件解析 target，再落到文件系统。
3. bare specifier：
   1. 拆 package name + subpath。
   2. `PACKAGE_SELF_RESOLVE`：若 parent 最近 package 的 `name` 等于 package name，先尝试本包 exports。
   3. node_modules 查找 package dir。
   4. 若 package 有 `exports`：只走 exports；失败直接报错，不回退 `module/main/index`。
   5. 若无 exports 且有 subpath：沿旧子路径文件/目录解析。
   6. 若无 exports 且无 subpath：`browser` 字符串入口（browser 条件启用时） > `module` > `main` > `index.*`。
4. 相对路径：先应用 browser 对象映射（browser 条件启用且 parent 包存在映射），再文件/目录解析。
5. 绝对路径 specifier 继续拒绝；内部已解析出的绝对 filesystem target 只由 package target 验证产生。

`get_id_for_specifier` 必须使用同一套 `resolve_specifier_to_path`，且传入相同 `ResolutionKind`，避免 graph 二次解析与首次加载不一致。

## 9. 模块格式判定

新增 `module_format.rs`：

```rust
pub(crate) enum ModuleFormat {
    Esm,
    CommonJs,
}

pub(crate) fn detect_module_format(path: &Path, package: Option<&PackageInfo>) -> ModuleFormat;
```

规则：

1. `.mjs` → ESM。
2. `.cjs` → CommonJS。
3. `.js` → 最近 package `type` 决定；有 package 但无 `type` 为 CommonJS；没有任何 package 边界时保留 wjsm 既有 AST 语法/CJS 检测，避免破坏现有无 package fixtures/入口。
4. `.ts/.tsx/.jsx`：保留现有行为，按 AST 语法/CJS 检测判定；不把 TypeScript 默认纳入 Node `.js` type 规则。
5. 内置模块虚拟路径固定 ESM。

`load_resolved_module` 的 `is_cjs` 改为：

- format = CommonJS → 必须跑 `cjs_transform`，即便 AST 中没有 CJS 特征；这样 `type: commonjs` 下的无 import/export `.js` 与 `.cjs` 均按 CJS 元数据处理。
- format = Esm → 不允许因出现 `require` 或 `module.exports` 就转换为 CJS；这类代码在后续语义/运行时阶段按 ESM 中未绑定标识符失败。
- 对 `.ts/.tsx/.jsx` 使用旧检测，避免扩大 TypeScript 语义范围。

## 10. browser 字段

`browser` 默认关闭；当 CLI/调用方提供 `browser` condition 时启用。

- 字符串形式：仅作为包入口替代，优先于 `module/main`，但仍低于 `exports`。
- 对象形式：
  - key 规范化为包内 `./relative.js`。
  - value 为字符串 → 替换到该文件。
  - value 为 `false` → Phase 2 不生成空 shim；解析时报 `ERR_PACKAGE_PATH_DISABLED_BY_BROWSER`，因为 wjsm 当前没有零副作用空模块约定。
- browser 替换不影响 `node:` 内置模块；`node:` 是强制内置解析。

CLI 侧不复用现有 `--target`（当前是 WASM/JIT 后端选择），计划新增模块解析专用参数：

- `--condition <name>`：可重复；传给 `ResolutionOptions.extra_conditions`。
- `--browser`：等价于 `--condition browser` 并启用 browser 字段映射。

公共 API 保持现有 `bundle/lower_bundle/parse_entry_ast` 默认行为；新增 crate-private options 管道先服务 CLI。若后续需要公共 options API，单独设计，不在本 issue 扩大公共面。

## 11. 测试策略与验收线

### 单元测试

新增 `exports_tests.rs` / `package_json_tests.rs` / `module_format_tests.rs`：

- exports 字符串入口、条件入口、嵌套条件、子路径、模式、null 阻止、非法 target。
- imports `#name`、`#pattern`、条件、未定义错误。
- `type` 与 `.mjs/.cjs/.js` 格式判定。
- browser 字符串入口、对象替换、false 禁用错误。

扩展 `resolver_tests.rs`：

- `exports` 存在时不回退 `main/module`。
- 自引用先走本包 exports。
- `node:fs` 与 `fs` 指向同一虚拟内置模块；`node:not_real` 报 unknown builtin。
- `get_id_for_specifier` 与 `resolve` 对 package exports 返回同一 ModuleId。

### fixtures

新增 `fixtures/modules/package_resolution/` 子集：

- `exports_condition_import`：默认命中 `wjsm`，无 wjsm 时命中 `node`，再按 import/default。
- `exports_condition_require`：CJS require 边命中 `require`。
- `exports_subpath_pattern`。
- `imports_private_alias`。
- `type_module_js` 与 `cjs_extension_override`。
- `self_reference_exports`。
- `browser_condition_default_boundary`（generated fixtures 使用默认解析选项；显式 browser 行为由 Task 6 CLI/resolver 测试覆盖）。

更新 `.expected`，使用 `WJSM_UPDATE_FIXTURES=1 cargo nextest run -E 'test(modules__package_resolution_)'` 生成后人工审阅 diff。

### 验证命令

- `cargo nextest run -p wjsm-module`
- `cargo nextest run -E 'test(modules__package_resolution_)'`
- `cargo run -- run fixtures/modules/package_resolution/<case>/main.js --root fixtures/modules/package_resolution/<case>`（针对关键场景手动冒烟）
- 若改 CLI options：`cargo nextest run -p wjsm-cli -E 'test(cli_)'` 中相关新增测试

## 12. 复杂度与文件边界

Complexity Budget：

- Artifact class：核心 resolver 架构改造。
- Target files：`resolver.rs`、`graph.rs`、`lib.rs`、新增 `exports.rs/package_json.rs/module_format.rs/resolution_options.rs`、对应 tests。
- Current pressure：`resolver.rs` 1114 行、`cjs_transform.rs` 983 行，均已超过项目 ≤500 行指导；本 issue 不能继续扩大 `resolver.rs`。
- Projected post-change pressure：拆分后 `resolver.rs` 应下降或保持在编排职责；新增文件各自 ≤500 行。
- Budget result：at-risk，但可通过新增 owner 文件治理。
- Planned governance：先提取 package/exports/format owner，再改 resolver 调用；禁止把 exports 解析实现进 graph 或 cjs_transform。

Plan-Time Complexity Check：

- Better file boundary：`exports.rs` 做纯 target 解析，`package_json.rs` 做 FS/package 元数据，`module_format.rs` 做格式，`resolver.rs` 做算法编排。
- Recommendation：add owner files + edit-in-place for resolver orchestration。

## 13. 兼容边界与 ADR 信号

兼容边界：

- 现有相对路径、目录 index、`module/main` 包入口在无 `exports` 的包上继续工作。
- `exports` 存在时按 Node 规则阻断旧回退；这是有意的 breaking correction。
- 内置模块 `node:` 解析不触碰 runtime，只加载 `builtin_js` 虚拟模块。
- 公共 API 默认行为不新增 browser 条件；CLI browser 条件显式启用。

ADR 信号：无必须新增 ADR。该设计改变的是 `wjsm-module` 内部解析 owner，不改变跨 crate 公共架构边界；若执行期决定新增公共 `ModuleResolutionOptions` API，再补 ADR/基线同步。

## 附录：工作制品

TaskIntentDraft：

- Requested outcome：完成 issue #309 的设计，批准后进入 writing-plans，再实现 package resolution enhancement。
- Success evidence：spec 覆盖 issue 全部实现项与审查意见；计划阶段能映射到明确 owner 文件、测试与验收命令。
- Stop condition：用户审阅通过并进入 implementation plan；若用户要求调整条件优先级或 browser API，则回到 spec 修改。
- Non-goals：见第 2 节。
- Risks：Node exports 细节复杂；browser `false` 是否需要空模块语义；公共 API 是否需要 options。

BaselineReadSetHint：

- Required baseline refs：issue #309、AGENTS.md wjsm-module 架构与规则、`resolver.rs`、`graph.rs`、`builtin_modules.rs`、`resolver_tests.rs`、`Cargo.toml`。
- Authority gaps：没有既有 package resolution ADR；以 issue #309 + current code 为本 spec 权威输入。

BaselineUsageDraft：

- Required baseline refs：同上。
- Delivered context refs：AGENTS.md、issue #309。
- Acknowledged before plan refs：issue #309、`resolver.rs`、`graph.rs`、`builtin_modules.rs`、`resolver_tests.rs`、`Cargo.toml`、Aegis INDEX。
- Cited in design refs：第 1、3、8、9、11、12 节。
- Missing refs：无。
- Decision：continue。

ImpactStatementDraft：

- Affected layers：`wjsm-module` resolver/graph/CJS import extraction/CLI option plumbing；fixtures modules；可能触及 `wjsm-cli` 参数传递。
- Owners：`builtin_modules.rs` 保持内置模块 owner；新增 package/exports/format owner；`resolver.rs` 编排。
- Invariants：解析必须编译期确定；target 不逃出 package/root；exports 优先于 legacy；node: 不查 node_modules。
- Compatibility：无 exports 包继续 legacy；有 exports 包遵循 Node 阻断回退。

Product Risk Lens：

- Value：解锁现代 npm 包入口解析，是后续生态兼容的基础。
- Non-goals：workspace/protocol/module.createRequire/types 条件/竞品 runtime 条件。
- Trade-offs：实现完整 target 语义比局部修补复杂，但可避免 npm 包解析长期碎片化。
- Decision needed：条件优先级已按用户选择包含 `node`。
