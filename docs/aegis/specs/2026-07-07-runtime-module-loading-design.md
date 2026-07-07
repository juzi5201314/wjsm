# Runtime Module Loading 设计（issue #312）

- 日期：2026-07-07
- 状态：待用户审阅
- 关联 issue：#312
- 输入：issue #312 正文与审查意见、`AGENTS.md` AOT/runtime 架构约束、issue #309 package resolution 已落地代码、当前 `wjsm-module` / `wjsm-semantic` / `wjsm-runtime` 动态 import 与 CJS transform 实现

## 1. 背景与当前事实

issue #312 要突破纯编译期 bundle 限制，支持 CJS 动态 `require()`、动态 `import()` 表达式、`import.meta.resolve()`、`require.resolve()` / `require.cache` 与运行时模块注册表。

当前代码事实：

| 能力 | 当前实现 | 缺口 |
|---|---|---|
| package resolution | issue #309 后 `wjsm-module` 已有 `ResolutionOptions`、`ResolutionKind::{Import, Require}`、`package_json.rs`、`exports.rs`、`module_format.rs` | 只能服务编译期图构建；运行时 `require.resolve` / `import.meta.resolve` 还没有可调用入口 |
| CJS `require()` | `cjs_transform.rs` 仅提取字符串字面量和无插值模板字符串，并转换成顶层 ESM `import` | 控制流、`try/catch`、计算路径的运行时语义缺失；现有 `cjs_conditional_require_false` fixture 证明 false 分支仍触发副作用 |
| 动态 `import()` | `resolver.rs` 收集静态可分析 specifier，`graph.rs` 预加载目标；semantic 降为 `Builtin::DynamicImport(module_id)`；runtime host import 返回已注册 namespace 的 resolved Promise | 带表达式模板字符串、变量、拼接路径在 resolver 与 semantic 两处都会报 AOT 错误；host import 只能按 `ModuleId` 查已知 namespace |
| namespace cache | runtime 有 `module_namespace_cache: HashMap<u32, i64>`，`register_module_namespace(i64, i64)` 把预构建 namespace 注册进缓存 | key 只是编译期 `ModuleId`；没有 path/specifier/module-object/cache-entry 状态；不能表达 loading、loaded、errored、deleted |
| `import.meta` | semantic 为 ESM 构造 `{ url, dirname, filename }` | 没有 `import.meta.resolve(specifier)` 方法 |
| runtime crate dependency direction | 现有依赖方向是 parser → semantic → ir ← backend-wasm → runtime → cli；runtime 不依赖 parser/semantic/backend | 不能把 swc + lowering + wasm compile 直接塞进 `wjsm-runtime`，否则依赖方向反转 |

Baseline Role Alignment：**aligned（scope: both）**。issue 目标是需求层新增运行时加载能力，架构层必须保持 runtime 不反向依赖编译管线。正确边界是：`wjsm-module` 继续拥有解析算法，`wjsm-runtime` 拥有执行期 registry/cache/host imports，CLI 或新 orchestrator 拥有“按运行时请求触发编译并实例化”的胶水。

## 2. 目标与非目标

### 目标

1. 支持 CJS 中运行时 `require(specifier)`：条件分支、循环内、函数内、`try/catch`、字符串拼接、模板字符串、变量路径。
2. 保留顶层无条件字面量 `require('./x')` 的编译期 bundle 快路径；把控制流内和计算参数的 `require()` 转为运行时调用。
3. 支持动态 `import(expr)`：变量、字符串拼接、带表达式模板字符串；返回 Promise，并复用 ESM namespace object。
4. 支持 `import.meta.resolve(specifier)`，同步返回按 issue #309 resolver 规则解析后的 file URL / `node:` URL / 内置模块 URL 字符串。
5. 支持 `require.resolve(id)`、`require.resolve.paths(id)`、`require.cache` 与 `delete require.cache[path]`。
6. 引入运行时模块注册表，按规范处理 `loading` / `loaded` / `errored` 状态与 CJS 循环引用的部分初始化 `exports`。
7. 支持 `require('./config.json')`，等价于读取 JSON 并返回解析后的 JS value；缓存遵循 `require.cache`。
8. 运行时加载普通 `.js/.mjs/.cjs/.json`。`.ts/.tsx/.jsx` 只允许编译期 eager 收集，不在运行时从文件系统即时编译。
9. 保持现有静态 import/export、多模块 bundle、静态 `import('./literal.js')` 行为兼容。

### 非目标

- HMR、文件监听、热替换。
- `require.extensions`。
- `module.createRequire` 完整 API。
- 运行时 source map 支持。
- 动态 import code splitting / chunk 文件格式。
- 运行时 TypeScript 编译。
- npm 安装、包管理器协议、远程 URL import。
- Worker/多 isolate 的跨实例模块共享。

## 3. First-principles / Architecture Integrity Lens

First-principles invariants：

- Non-negotiable goal：运行时表达式 specifier 必须在调用发生时解析、加载、缓存，并向 JS 暴露 Node/ESM 兼容的成功或错误结果。
- Non-negotiable constraints：runtime crate 不拥有 parser/semantic/backend；specifier 解析算法继续归 `wjsm-module`；模块缓存以规范身份（绝对 path / URL / builtin key）为准；CJS 循环必须返回同一个部分初始化 `exports` 对象。
- Historical assumptions to delete：`require()` 全部能安全改写为顶层 `import`；`dynamic import` 的目标一定能在编译期枚举；`ModuleId` 足以作为运行时模块身份；false 分支 require 可以触发副作用。

Architecture Integrity Lens：

- Invariant：解析、编译、执行状态三类 owner 不混放；runtime 只通过 trait/handle 请求加载，不直接调用编译 crate。
- Canonical owner / contract：`wjsm-module` 暴露运行时解析入口；`wjsm-runtime` 新增 `runtime_module_registry` 和 host import；CLI 或新 `wjsm-loader` crate 组装 resolver + backend + runtime instantiation。
- Responsibility overlap：淘汰 `module_namespace_cache: ModuleId -> namespace` 作为唯一模块缓存；它可以变成 registry 的 namespace 子索引，但不能继续承担完整 cache 语义。
- Higher-level simplification：统一建立 `ModuleRegistryKey` 和 `RuntimeModuleState`，让 `require()`、`import()`、`require.cache`、`import.meta.resolve` 共享同一解析与缓存状态，而不是各自维护字典。
- Retirement / falsifier：当 `require(false branch)` 不再触发副作用、动态 `import(path)` 不再走编译期报错、`delete require.cache[path]` 后再次加载会重新执行模块时，旧 AOT-only 假设才算退出主路径。
- Verdict：proceed，采用“运行时 registry + 注入式 loader + 编译期候选预注册”的混合架构。

Anti-Entropy Declaration：

- Deletion Class：code-retirement / contract-carrying code。
- Old Path/Object：`cjs_transform` 对所有静态字面量 require 的无条件顶层 import 化；`resolver.rs` / semantic 对动态 import 表达式的 AOT-only 报错；runtime `module_namespace_cache` 单一缓存模型。
- New Canonical Owner：运行时 registry 状态机 + `wjsm-module` resolver runtime API + loader trait。
- Expected Preserved Behavior：顶层无条件字面量 require、静态 import/export、静态动态 import fixture 继续通过。
- Expected Retired Behavior：false 分支 require 的副作用、动态 import 表达式编译时报错、仅按 ModuleId 查 namespace。
- External Boundary Touched：yes，JS 用户可见模块加载语义。
- Source-of-Truth Data Risk：none，只有文件读取与内存缓存；不删除用户持久数据。
- User Confirmation Required：no。

Retirement Decision：

- Path：delete-first for internal AOT-only assumptions, compat-exception only for documented static fast path。
- Why：保留顶层字面量 require 的编译期优化不改变可观察顺序；控制流 require 继续 import 化会违反 Node 语义。
- Non-edits：不触碰 package manager 协议、HMR、runtime TS 编译。

## 4. 方案对比

| 方案 | 内容 | 优点 | 风险 | 结论 |
|---|---|---|---|---|
| A. Eager bundle 枚举所有可能目标 | 编译期通过 glob/目录枚举把候选模块全部预编译，运行时只查表 | 保持纯 AOT；wasmtime 多实例压力低 | 不能覆盖任意字符串拼接、用户输入路径、optional dependency；false 分支副作用仍需 runtime require 才正确 | 不足以完成 issue #312 |
| B. runtime crate 直接嵌入 parser/semantic/backend | `wjsm-runtime` host import 里读文件、解析、lower、compile、instantiate | 调用链直观 | 反转当前依赖方向；runtime binary 被编译器污染；后续 snapshot/support ABI 更难治理 | 拒绝 |
| C. 注入式 RuntimeModuleLoader + registry | runtime 维护 registry/cache/host import；CLI/新 orchestrator 实现 loader，调用 `wjsm-module` + backend 编译并实例化共享 env | 保持 crate 边界；支持真动态路径；静态候选仍可预注册；cache/循环/Promise 语义有统一 owner | 需要新增 runtime instantiation contract 与 registry 状态机 | **推荐** |

推荐方案 C。Phase 3.1 的 eager bundle 不作为终点，而作为 registry 的“预注册候选来源”：能静态发现的目标先编译进 bundle；运行时表达式命中预注册目标时直接激活，未命中时交给 loader。

## 5. 模块身份与 registry 数据模型

新增 runtime owner：`crates/wjsm-runtime/src/runtime_module_registry.rs`。

核心数据结构建议：

```rust
pub enum RuntimeModuleKey {
    File(PathBuf),
    Builtin(String),
    Json(PathBuf),
}

pub enum RuntimeModuleState {
    Loading { module_object: i64, exports_object: i64 },
    Loaded { module_object: i64, exports_object: i64, namespace_object: i64 },
    Errored { error_value: i64 },
}

pub struct RuntimeModuleRegistry {
    by_key: HashMap<RuntimeModuleKey, RuntimeModuleState>,
    by_module_id: HashMap<u32, RuntimeModuleKey>,
    cache_object: Option<i64>,
}
```

规则：

1. `RuntimeModuleKey` 必须来自 resolver 的规范化结果，不直接使用用户传入字符串。
2. CJS `require.cache` 的 key 是绝对路径字符串；ESM namespace 也通过同一个 registry 状态读取。
3. `Loading` 状态在执行模块体前插入，用于循环引用；循环 `require()` 返回同一个 `exports_object`。
4. `Loaded` 状态包含 `module_object`、`exports_object`、`namespace_object`，其中 namespace object 对 ESM 是 module namespace，对 CJS 是合成 namespace/default。
5. `Errored` 状态用于 ESM dynamic import Promise rejection；CJS `require()` 遇到 errored cache 重新抛出同一错误值。
6. `delete require.cache[path]` 删除 File/Json key 的 cache entry；若模块正在 `Loading`，删除应返回 false 或保留 entry，避免破坏循环执行中状态。
7. GC roots 从 registry 收集所有 module/export/namespace/error value，替代只扫 `module_namespace_cache`。

## 6. Loader 与跨 crate 边界

新增 runtime trait（名字可在实现时按项目风格调整）：

```rust
pub trait RuntimeModuleLoader: Send + Sync {
    fn resolve_for_runtime(
        &self,
        referrer_key: RuntimeModuleReferrer,
        specifier: &str,
        kind: RuntimeResolutionKind,
    ) -> Result<RuntimeResolvedModule, RuntimeModuleLoadError>;

    fn instantiate_runtime_module(
        &self,
        resolved: &RuntimeResolvedModule,
        env: RuntimeInstantiationEnv,
    ) -> Result<RuntimeInstantiatedModule, RuntimeModuleLoadError>;
}
```

边界：

- `wjsm-runtime` 定义 trait 与 plain 数据结构，不依赖 `wjsm-module`、`wjsm-parser`、`wjsm-semantic`、`wjsm-backend-wasm`。
- `wjsm-cli` 或新增 `wjsm-loader` crate 实现 trait，内部使用 `wjsm-module::ResolutionOptions`、`ModuleResolver`、`bundle_with_options` / lower+compile。
- `RuntimeOptions` 增加可选 loader handle；未安装 loader 时，动态运行时加载抛出 `ERR_MODULE_NOT_FOUND` / reject Promise，但静态预注册模块仍可按现有路径工作。
- loader 需要拿到当前 runtime 的 shared memory/table/globals/linker env，不新建孤立 runtime；动态实例共享同一 `RuntimeState`、memory、table、support cwasm、host imports。

实例化 contract：

1. loader 编译单个目标模块及其静态依赖子图，复用同一 root 与 package resolution options。
2. 编译结果导入当前 `env.memory`、`env.__table` 和 27 globals，不创建第二套 JS heap。
3. 新实例执行入口前，registry 插入 `Loading`；执行成功后转 `Loaded`；执行 trap 后转 `Errored` 并恢复可抛出的 JS Error value。
4. 动态子图中的静态依赖如果 registry 已 loaded，lowering/linking 应把它们视为外部已注册 namespace，而不是重复执行。

## 7. CJS `require()` 语义

### 7.1 静态快路径与运行时路径分流

`cjs_transform.rs` 需要从“所有静态字面量 require 都 import 化”改为基于位置和语义分类：

| require 形态 | 新行为 |
|---|---|
| 顶层 `const x = require('./x')`、顶层普通表达式且无控制流包裹 | 保持编译期 import 化，复用现有 bundle 快路径 |
| `if/for/while/switch/try/catch/finally/function/class method` 内的字面量 require | 保留运行时调用，不提前执行目标模块 |
| 非字符串参数：拼接、模板插值、变量 | 保留运行时调用，lower 为 host import |
| `require.resolve`、`require.cache` | 不由 import transform 处理，由 CJS module scope 绑定提供 |

这会有意修正现有 fixture：`fixtures/modules/cjs_conditional_require_false/main.expected` 不应再包含 `side effect`。

### 7.2 CJS module scope 绑定

semantic 多模块 lowering 在 CJS module scope 初始化时新增：

- `require`：module-local native function，捕获当前 module key/referrer。
- `module`：包含 `exports` 与必要 metadata。
- `exports`：`module.exports` 初始别名。
- `__filename` / `__dirname`：沿用当前实现。

运行时 `require(specifier)` 流程：

1. 对参数执行 ToString；失败则抛 TypeError。
2. 调用 loader resolve，kind = `Require`，referrer = 当前 module key。
3. 若 registry 中已有 `Loaded`，返回 `exports_object`。
4. 若 registry 中已有 `Loading`，返回部分初始化 `exports_object`。
5. 若 registry 中已有 `Errored`，抛出缓存的错误值。
6. 若 `.json`，读取文件、`JSON.parse`、设置 `module.exports` 为解析值、转 `Loaded`。
7. 若 `.js/.cjs/.mjs`，通过 loader 编译实例化并执行，返回 `module.exports`。

CJS 与 ESM 互操作：

- `require(esm)` 在本 phase 返回 ESM namespace object，不做 Node 当前的 `ERR_REQUIRE_ESM` 限制；这是 wjsm 现有 CJS transform 导入 ESM 的延续。
- ESM `import(cjs)` 返回 namespace，其中 `default` 指向 `module.exports`，命名导出沿用现有 CJS transform 可静态识别的命名属性。

## 8. 动态 `import(expr)`

semantic 需要把 `lower_dynamic_import_call` 从“提取静态字符串并查 ModuleId”改为双路径：

1. 参数是静态字符串且 `dynamic_import_specifier_map` 命中：保留 `Builtin::DynamicImportStatic(module_id)` 或继续使用现有 `DynamicImport(module_id)` 快路径。
2. 参数是任意表达式：lower 表达式值，执行 ToString，调用新 host import `dynamic_import_runtime(referrer_module_id, specifier_value)`，返回 Promise。

runtime `dynamic_import_runtime`：

- resolve/load 成功：Promise fulfill namespace object。
- resolve/load 失败：Promise reject Error value。
- 命中正在 `Loading` 的 CJS：Promise fulfill 当前 namespace/default 视图；命中正在 `Loading` 的 ESM 时需遵循 ESM 循环初始化规则，返回已存在 namespace object，属性 live binding 后续填充。

现有 `dynamic_import(i64 module_id)` 可保留为静态快路径，但应成为 registry 的薄封装：通过 `by_module_id` 找 key，再返回 namespace。

## 9. `import.meta.resolve()`

当前 `import.meta` 对象有 `url`、`dirname`、`filename`。设计新增 `resolve` 方法：

- 只在 ESM module metadata 可用时创建。
- 方法为 native callable，捕获当前 module key/referrer。
- 参数执行 ToString。
- 调用 resolver runtime API，kind = `Import`。
- 返回字符串：
  - file module：`file://` URL。
  - builtin：`node:<name>`。
  - JSON/file 仍返回 file URL。
- 错误同步抛出，错误码沿 resolver 的 Node-style code。

实现时不把 resolver 复制到 runtime；`resolve` native callable 仍通过 loader trait 调用 CLI/orchestrator 的解析实现。

## 10. `require.resolve()` / `require.resolve.paths()` / `require.cache`

### `require.resolve(id)`

- module-local function，捕获当前 referrer。
- kind = `Require`。
- 返回绝对路径字符串；builtin 返回 canonical builtin key（例如 `node:path`）。
- 对 package `exports` / `imports` / `type` / `browser` 条件使用 issue #309 的 require 条件集合。

### `require.resolve.paths(id)`

- 对 relative/absolute id 返回 `null`。
- 对 bare package 返回从 referrer 目录向上可搜索的 `node_modules` 目录数组。
- 对 builtin 返回 `null`。
- 数组元素为字符串；不触发文件读取。

### `require.cache`

- `require.cache` 是 registry 的 JS view，不是一次性快照。
- 读取属性：按绝对路径返回 module object。
- `delete require.cache[path]`：从 registry 移除 loaded/errored File/Json entry；返回 true。
- 删除 builtin entry 返回 true 但不改变 builtin registry。
- 删除 loading entry 不破坏执行中模块，返回 false。
- 再次 require 被删除 entry 会重新执行模块。

实现建议：复用 Proxy host traps 作为 cache view；如果现有 Proxy trap 无法承载 host-backed ownKeys/get/delete，则先新增 `NativeCacheObject` host object 类型，不把 cache 映射复制成普通对象。

## 11. 运行时 JSON 加载

JSON require 是 issue 评论明确遗漏项，应纳入本 issue：

- resolver 识别 `.json` 文件；`RuntimeResolvedModule` 标记 `Json`。
- runtime loader 读取 UTF-8 文本，调用已有 `JSON.parse` 语义或内部 JSON parser 路径，产出 JS value。
- `module.exports` 直接等于解析值。
- cache key 是 JSON 绝对路径。
- `import('./x.json')` 在本 issue 不引入 import assertion；动态 import JSON 可 reject `ERR_IMPORT_ASSERTION_TYPE_MISSING` 或报“不支持无 assertion 的 JSON import”。推荐本 issue只保证 CJS `require('./x.json')`。

## 12. 错误与安全边界

错误码与可观察行为：

- `ERR_MODULE_NOT_FOUND`：解析不到文件/package。
- `ERR_PACKAGE_PATH_NOT_EXPORTED`、`ERR_PACKAGE_IMPORT_NOT_DEFINED`、`ERR_INVALID_PACKAGE_TARGET`：沿 issue #309 resolver。
- `ERR_DYNAMIC_MODULE_LOADER_UNAVAILABLE`：runtime 没安装 loader 且请求非预注册动态模块。
- `ERR_REQUIRE_ESM` 暂不采用；wjsm 当前 CJS→ESM 互操作继续返回 namespace/default。
- JSON parse error 保留 `SyntaxError`。

文件系统边界：

- loader 解析不得越过 configured root / fs read roots。
- runtime `fs_read_roots` 应约束动态 module loader 读源文件；CLI 安装 loader 时必须传递 root。
- `node_modules` 解析沿 issue #309 package resolver，不额外扫描全盘。

## 13. 测试策略与验收线

### 单元测试

新增 / 扩展：

- `cjs_transform_tests.rs`
  - 顶层字面量 require 仍生成 import。
  - if false 内字面量 require 不生成 import。
  - try/catch 内字面量 require 不生成 import。
  - 拼接路径 require 不生成 import 且保留 call。
- `wjsm-semantic` lowering tests / `.ir` snapshots
  - `import(path)` 表达式 lower 为 runtime dynamic import host call。
  - 静态 `import('./x.js')` 保持 static fast path。
  - CJS module scope 声明 `require` / `require.resolve` / `require.cache`。
- `wjsm-runtime` registry tests
  - loading 状态返回部分 exports。
  - loaded 状态命中 cache。
  - delete cache 后重新执行。
  - errored 状态 require 重新抛错、import reject。
  - GC roots 覆盖 registry values。
- `wjsm-module` resolver runtime API tests
  - `resolve_for_runtime` 对 import/require 条件选择正确。
  - `resolve.paths` bare package 搜索路径正确。
  - `import.meta.resolve` 输出 file URL / node builtin。

### fixtures

新增 `fixtures/modules/runtime_loading/` 子集：

- `cjs_conditional_require_true`：true 分支加载并输出目标值。
- `cjs_conditional_require_false`：false 分支不输出目标副作用，只输出 `undefined`。
- `cjs_try_optional_missing`：missing optional dependency 被 catch 捕获。
- `cjs_computed_require`：`require('./mods/' + name + '.js')`。
- `cjs_require_json`：JSON require 返回对象。
- `cjs_require_cache_delete`：删除 cache 后模块重新执行。
- `cjs_circular_partial_exports`：循环 require 看到部分初始化 exports。
- `esm_dynamic_import_template`：`` import(`./locale/${lang}.js`) ``。
- `esm_dynamic_import_variable`：`import(modulePath)`。
- `esm_import_meta_resolve`：输出解析后的 file URL。
- `require_resolve_paths`：输出搜索路径数组关键元素。

需要同步修正现有 `fixtures/modules/cjs_conditional_require_false/main.expected`，这是语义修复，不是绕过逻辑。

### 验证命令

- `cargo nextest run -p wjsm-module -E 'test(cjs_) | test(runtime_resolve_) | test(resolve_paths)'`
- `cargo nextest run -p wjsm-semantic -E 'test(dynamic_import) | test(require_runtime)'`
- `cargo nextest run -p wjsm-runtime -E 'test(module_registry) | test(require_cache) | test(dynamic_module)'`
- `cargo nextest run -E 'test(modules__runtime_loading_) | test(modules__cjs_conditional_require_false)'`
- 冒烟：`cargo run -- run fixtures/modules/runtime_loading/cjs_computed_require/main.js --root fixtures/modules/runtime_loading/cjs_computed_require`

## 14. 复杂度与文件边界

Complexity Budget：

- Artifact class：跨 module/semantic/backend/runtime/CLI 的核心模块加载架构。
- Target files：`crates/wjsm-module/src/resolver.rs`、`cjs_transform.rs`、`graph.rs`、新增 runtime resolve API；`crates/wjsm-semantic/src/lowerer_*`；`crates/wjsm-ir/src/builtin.rs`；`crates/wjsm-backend-wasm/src/host_import_registry/*`；`crates/wjsm-runtime/src/runtime_module_registry.rs`、host imports、GC roots；`crates/wjsm-cli/src/lib.rs` 或新 loader crate；fixtures。
- Current pressure：`resolver.rs` 1576 行、`cjs_transform.rs` 984 行，已超过项目文件体量纪律；runtime `lib.rs` 2115 行，不应继续加职责。
- Projected post-change pressure：必须新增 owner 文件；禁止把 registry、loader、runtime resolver 塞进现有大文件。
- Budget result：over-budget unless split owners。
- Planned governance：先建立 registry/loader/resolve API owner，再迁移 host imports；每个 slice 有独立 tests。

Plan-Time Complexity Check：

- Better file boundary：
  - `wjsm-module/src/runtime_resolution.rs`：运行时解析 API 与 resolve.paths。
  - `wjsm-module/src/cjs_require_analysis.rs`：require 位置分类。
  - `wjsm-runtime/src/runtime_module_registry.rs`：registry 状态机。
  - `wjsm-runtime/src/runtime_module_loader.rs`：trait 与 plain DTO。
  - `wjsm-runtime/src/host_imports/modules.rs`：require/import/meta resolve host imports。
  - `wjsm-cli/src/runtime_loader.rs` 或新 `wjsm-dynamic-loader` crate：编译/实例化 orchestrator。
- Recommendation：add owner files + edit existing files only for wiring。

## 15. 兼容边界与 ADR 信号

兼容边界：

- 静态 import/export 与现有 package resolution 行为不变。
- 顶层无条件字面量 require 的编译期 bundle 快路径保留。
- 控制流内字面量 require 的副作用顺序会变成 Node 兼容语义；现有 false-branch fixture 预期需要更新。
- runtime 未安装 loader 时，预注册静态 dynamic import 继续工作；真正运行时路径给出明确错误，不静默跳过。
- runtime TS 编译不支持；`.ts/.tsx/.jsx` 只在初始编译期图内处理。

ADR 信号：建议新增 ADR。该设计改变跨 crate runtime/compiler 边界，引入 runtime loader trait、multi-instance shared env contract、module registry cache 作为新的执行期 source-of-truth。ADR 应记录为什么不让 `wjsm-runtime` 依赖 parser/semantic/backend，以及 registry 如何取代 `module_namespace_cache` 的主路径职责。

## 附录：工作制品

TaskIntentDraft：

- Requested outcome：完成 issue #312 的设计，批准后进入 writing-plans，再实现 runtime module loading。
- Success evidence：spec 覆盖动态 require、动态 import、import.meta.resolve、require.resolve/cache、JSON require、module registry、循环引用、runtime loader 边界与验证命令。
- Stop condition：设计与计划落盘；执行前需按计划逐 slice 实现并验证。
- Non-goals：见第 2 节。
- Risks：wasmtime 多实例共享 env、CJS 循环状态、cache view delete trap、runtime loader 权限边界。

BaselineReadSetHint：

- Required baseline refs：issue #312、AGENTS.md 架构与 spec compliance、issue #309 package resolution spec/plan/current code、`resolver.rs`、`graph.rs`、`cjs_transform.rs`、`lowerer_async_eval/async_import_promise.rs`、`lowerer_jsx_objects/jsx_expressions.rs`、`host_imports/misc.rs`、runtime `lib.rs` registry fields、GC roots。
- Authority gaps：没有既有 runtime module loading ADR；以本 spec 作为后续 plan 输入，并在实现完成后补 ADR。

BaselineUsageDraft：

- Required baseline refs：同上。
- Delivered context refs：AGENTS.md、issue #312。
- Acknowledged before plan refs：issue #312、issue #309 spec/current code、module resolver/graph/CJS transform、semantic dynamic import/import.meta、runtime host import/cache/GC roots、Aegis INDEX。
- Cited in design refs：第 1、3、5、6、7、8、9、10、13、14、15 节。
- Missing refs：无阻塞；执行期需用 wasmtime instantiation API 细化 `RuntimeInstantiationEnv`。
- Decision：continue。

ImpactStatementDraft：

- Affected layers：`wjsm-module` resolver/runtime API/CJS transform/graph；`wjsm-semantic` CJS scope and dynamic import lowering；`wjsm-ir` builtins；`wjsm-backend-wasm` host import registry/codegen；`wjsm-runtime` registry/loader/host imports/GC roots；`wjsm-cli` loader installation；fixtures modules。
- Owners：`wjsm-module` owns resolution; runtime registry owns cache/state; CLI/orchestrator owns compile+instantiate; backend only emits host calls.
- Invariants：runtime 不依赖编译 crate；cache key canonical；循环先插入 Loading；动态路径受 root/read-roots 限制；specifier resolution 条件与 issue #309 一致。
- Compatibility：静态快路径保持；控制流 require 修正为 Node-compatible 行为；缺 loader 明确错误。

Product Risk Lens：

- Value：解锁可选依赖、按语言/平台加载、插件式目录加载、动态 locale import，是 npm 生态兼容的关键能力。
- Non-goals：HMR、runtime TS、code splitting、createRequire 完整 API、远程 import。
- Trade-offs：注入式 loader 比 eager-only 复杂，但能保持架构边界并覆盖真实动态路径；直接把编译器塞进 runtime 虽直观但长期污染 crate 分层。
- Decision needed：推荐方案 C；若用户坚持纯 AOT，则只能声明 issue #312 的动态表达式能力无法完整满足。
