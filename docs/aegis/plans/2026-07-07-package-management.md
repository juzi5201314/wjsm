# wjsm 包管理实现计划（wjsm-pm）

Goal: 实现 wjsm 的 npm 生态包管理能力（`wjsm install/add/remove/task/x` + workspaces）：无 `node_modules`、全局内容寻址存储（CAS blob + SQLite + zstd + packfile）直供编译器；lockfile 精确记录 package source locator 与解析实例；`run/build` 可在仅有 lockfile 且 store 缺失时惰性补齐；依赖生命周期脚本默认跳过并记录 `has_install_script=true,scripts_trusted=false`，经 `trustedDependencies`/`--allow-scripts` 显式授权后才在隔离目录执行并重扫产物；并实现 AOT 独有的跨项目分层编译产物复用（L1 可重定位 IR + 可验证的 L2 artifact）。批准的设计见 `docs/aegis/specs/2026-07-07-package-management-design.md`。

Architecture: 新增独立 crate `wjsm-pm`，拥有 registry client / CAS store / PubGrub solver / lockfile / scripts / workspace。包身份分三层：`SourceId`（`blake3(registry/resolved/integrity_or_shasum)`，下载前即可从 packument/lockfile 计算，用于 lazy materialization 查询）、`ContentId`/`manifest_hash`（解包后由文件清单内容计算，用于校验 source 实际字节）、`InstanceId`（solver/lockfile/overlay 的解析实例身份，含 peer-set/workspace 上下文）。`wjsm-module` 新增 `Vfs` + `ResolutionOverlay` + `ResolutionBoundary` 三个 trait/结构（定义在 module 侧，实现在 pm 侧）；`Vfs` 抽象 `ModuleResolver` 解析算法实际路由的**全部**文件系统谓词（`read_to_string`×1 / `canonicalize`×12 / `is_file`×7 / `is_dir`×6，resolver.rs 内共 **26 处**）+ `package_json.rs` 的 `read_package_json`（合并原 `fs::metadata`+`read_to_string`），而非仅三处读取；trait 另提供 `exists`（`FsVfs`=`Path::exists`）作抽象完整性、供 `CasVfs` 内部前缀判定复用——**resolver 自身不调用 `exists`**（已 grep 核实 `wjsm-module` 无 `.exists()` 触点）。`ResolutionBoundary` 显式列出项目根、workspace member 根、CAS vroot 等可读根，替代单一 `root_path.starts_with` 安全判断。`wjsm-module` **不反向依赖** `wjsm-pm`。`wjsm-semantic` 新增可重定位 IR 单包 lower + 链接阶段（服务 L1）。`wjsm-cli` 组装注入并新增子命令，且 #312 runtime loader / runtime_resolution 同步接入 `Vfs`/Overlay/Boundary，保证 computed `require()`、dynamic `import()`、`require.resolve()` 也能读 CAS。依赖方向：`wjsm-pm → wjsm-module`、`wjsm-pm → wjsm-snapshot-format`、`wjsm-cli → wjsm-pm`。

Tech Stack: Rust 2024；`rusqlite`（bundled SQLite，WAL）；`zstd`；`blake3`（内容哈希）；`tar` + `flate2`（tgz 解包）；`sha2` + `sha1` + base64（npm SSRI 完整性校验，支持多 token SRI 与历史 `shasum` 兼容校验）；`reqwest`（workspace 已有，async/tokio）；`tokio`（workspace 已有，`spawn_blocking` 隔离 SQLite 同步写、`JoinSet` 并发预取 packument——**不引入** `futures` crate）；`pubgrub` crate（版本求解）；`toml`（workspace 已有，lockfile）；`serde_json`（packument、package-lock/bun.lock 迁移——bun.lock 是 JSONC，先预处理去注释/尾逗号再解析）；`serde_yaml`（pnpm-lock + yarn berry v2 迁移；yarn classic v1 非合法 YAML，用手写行扫描器，见任务 3.1）；现有 fixture runner + nextest。

Baseline/Authority Refs:

- `docs/aegis/specs/2026-07-07-package-management-design.md`（本计划的批准设计）
- `AGENTS.md` / `CLAUDE.md`：AOT pipeline、crate 依赖方向、Rust 2024、注释中文、文件 ≤500 行/函数 ≤30 行体量纪律、ECMAScript/npm spec 兼容 hard rules、临时文件禁入项目树
- `docs/aegis/specs/2026-07-07-runtime-module-loading-design.md` + `docs/aegis/plans/2026-07-07-runtime-module-loading.md`（issue #312，P5 前置依赖：可重定位 IR / 分离编译地基）
- `docs/adr/0003-startup-snapshot-boundary.md`（relocatable heap 同源思路）
- `crates/wjsm-module/src/resolver.rs`（`ModuleResolver` struct L70、`with_options` L89、`find_package_in_node_modules` L328、源码读取 L754）
- `crates/wjsm-module/src/bundler.rs`（`ModuleBundler` L20、`with_resolution_options` L31）
- `crates/wjsm-module/src/graph.rs`（`ModuleGraph::build_with_options` L39）
- `crates/wjsm-module/src/package_json.rs`（`read_package_info` L48、`fs::read_to_string` L60）
- `crates/wjsm-module/src/resolution_options.rs`（`ResolutionKind` / `ResolutionOptions`）
- `crates/wjsm-semantic/src/lowerer_modules.rs`（`lower_modules` L37、`ModuleLoweringInput` L9、`ModuleMetadata` L16、`ModuleKind` L24）
- `crates/wjsm-semantic/src/scope.rs`（`ScopeTree` L43、`push_scope` L61 全局递增 arena；IR 名 `${scope_id}.{name}`）
- `crates/wjsm-cli/src/cli_args.rs`（`Commands` enum L150、`CacheCommand` L411）
- `crates/wjsm-cli/src/lib.rs`（`main_entry` L381 仅 `match execute(cli)`；真正的命令 dispatch 在 `execute`（L407）`match cli.command`，每臂返回 `Result<ExitCode>`；`Cache` 臂 L520；`cmd_cache` L1453；`run_file_in_process` L2120）
- `crates/wjsm-runtime/src/runtime_startup.rs`（`compile_or_load_cached` L55、`precompile_module` L85）
- `crates/wjsm-snapshot-format`（`abi_hash`、`register_abi_hash_external_input`）
- `tests/fixture_runner.rs`（E2E harness）

Compatibility Boundary:

- 无依赖 / 纯本地相对导入的现有项目行为不变：`FsVfs` + `NoOverlay` + 单项目 `ResolutionBoundary` 为默认，CAS 覆盖层仅在 lockfile/惰性解析确认有依赖时由 CLI 注入。
- 所有现有 fixture、`wjsm run file.js` 语义不变。
- `wjsm-module` 不依赖 `wjsm-pm`；trait/Boundary 定义在 module 侧。
- blob 内容寻址身份、package locator（source/integrity/content）、lockfile instance identity 三者分离；store 版本目录 `~/.wjsm/store/v1`。store 主键不得只用 `name@version`，否则 private/scope registry 与 peer instance 会串包。
- 已知代价：首版无物化 node_modules，外部 Node 工具链看不到依赖（wjsm 自有 check/lint/fmt/task/x 走 CAS/shim 不受影响）；`--node-modules-dir` 逃生舱不在本计划。
- 迁移不删除原生态 lockfile（除非 `--prune`）。迁移读取到的固定版本、resolved、integrity 作为 solver 优先级与 lazy 补齐输入，不作为不可校验的全局真相。
- P5 前置依赖 issue #312 已合并；可重定位 IR 分离编译产出与现有 `lower_modules` 整体路径**分级逐指令等价**（L2-a 叶子包 / L2-b import 边必过；L2-c re-export/shared-env 等价或明确切换到 L2-bundle）。L2 artifact 若不能证明可重定位 cwasm 正确性，则改用 exact-wasm/final-linked cwasm 缓存，不发布错误粒度的包片段缓存。
- tarball 必须 SSRI 或历史 `shasum` 校验通过才入库（证明字节 == registry 所发）；**且**解包必须防路径逃逸 + 拒绝链接条目（`extract_tgz` 三重守卫——SSRI 不覆盖此边界，恶意作者可发布 integrity 合法却含 `../`/符号链接的 tarball，见任务 2.2）。依赖包生命周期脚本默认跳过并警告，lockfile 记录 `has_install_script=true,scripts_trusted=false`；只有 `trustedDependencies` 或 `wjsm install --allow-scripts=<pkg|all>` 显式授权时才在隔离可写目录执行，产物复校验后重新进入 CAS。项目**自身** package.json 的 scripts 由 `wjsm task`（P4）显式执行，与依赖生命周期授权正交。

Verification:

- `cargo nextest run -p wjsm-pm`
- `cargo nextest run -p wjsm-module`（回归 Vfs/Overlay/Boundary 抽象不破坏 FS 模式）
- `cargo nextest run -p wjsm-semantic -E 'test(relocatable_ir)'`
- `cargo nextest run -p wjsm-cli -E 'test(pm_)'`（CLI 集成测试：install→run、仅 lockfile 且 store 缺失时 lazy 补齐、无 lockfile 首次 run 惰性解析、CAS 内 dynamic import/computed require/require.resolve、task/x shim，含无 node_modules 断言。pm 场景需 store/mock registry + install/run 多步，标准 fixture_runner（仅 happy/errors/modules 三 suite、纯 run 比对）无法表达，故用 `crates/wjsm-cli/tests/` 下自定义集成测试，不注册 fixtures/pm suite）
- `cargo nextest run --workspace`（全量回归）
- 冒烟：含依赖 fixture 项目 `wjsm install` 后 `wjsm run` 成功且磁盘无 node_modules；删除 store 后仅保留 `wjsm-lock.toml` 再 run 仍可补齐并成功

## Plan Basis

Facts（已核对代码）:

> 执行优先级说明：design spec §5.2/§14 中“三处磁盘访问抽象”的早期表述已被本计划的 26 处 fs 谓词清单取代；执行以任务 1.6 的全量 Vfs/Overlay/Boundary 方案为准。

- `ModuleResolver`（resolver.rs:70）字段 `root_path/options/package_cache/visited`；`with_options`（L89）是唯一带 options 构造器。**文件系统触点远不止三处**（已逐行核对 resolver.rs）：
  - `std::fs::read_to_string`：L754（源码读取，唯一读点）。
  - `Path::canonicalize`：L91、334、343、359、380、454、469、593、600、609、631、667（12 处，遍布 `resolve_file_or_directory`/`resolve_existing_module_path`/`resolve_package_target_path`/`resolve_directory_index`/`find_nearest_package`/`read_package_info`/`canonical_entry_path`/`find_package_in_node_modules`）。
  - `Path::is_file`：L453、468、592、599、608、630、663（7 处）。
  - `Path::is_dir`：L342、354、456、474、602、614（6 处）。
  - node_modules 遍历：`find_package_in_node_modules`（L328）；bare specifier 入口 `resolve_bare_specifier`（L242）先查 `find_nearest_package` 再遍历 node_modules（L265）。
  - package.json 读取：`package_json.rs:read_package_info`（L48，`fs::metadata` + `read_package_info_manifest`→L60 `fs::read_to_string`）。
  - **决定性事实**：`std::fs::canonicalize` 要求路径在真实磁盘存在。CAS 虚拟路径 `<vroot>/<encoded_instance_id>/…` 永不落盘，直接 canonicalize 必然失败。因此 CAS 切入**不是**"改三处读取"，而是"把 resolver 的全部 fs 谓词路由进 `Vfs`，并让 `Vfs::canonicalize` 对虚拟路径做恒等归一化"。这是 resolver 级重构（任务 1.6 承载），是 P2–P4 的地基。
- `ModuleBundler`（bundler.rs:20）持 `root_path/options`，`lower_bundle`（L39）/`bundle`（L102）均经 `ModuleGraph::build_with_options`（graph.rs:39）。注入点在 bundler + graph + resolver 构造链。
- `lower_modules`（lowerer_modules.rs:37）接收 `Vec<ModuleLoweringInput>` + 各种 `HashMap<ModuleId, _>`，所有模块共用一棵 `ScopeTree`，scope id 全局递增（scope.rs:61 `idx = self.arenas.len()`）并写进 IR 名 `${scope_id}.{name}`。这是 L1 跨项目复用的命门——必须模块局部化 + 重定位。
- CLI dispatch 在 **`execute`（lib.rs:411）** `match cli.command`（**不是** `main_entry`——`main_entry` L381 只做 `match execute(cli)` 收敛退出码）；`execute(cli: Cli) -> Result<ExitCode>`，**每个 match 臂返回 `Result<ExitCode>`**（如 `Commands::Cache { ref command } => cmd_cache(command)`，L520）；`Commands` enum 在 cli_args.rs:150；`cmd_cache`（L1453）。新子命令的 dispatch 臂**必须返回 `Result<ExitCode>`**（`cmd_install(dir).map(|()| ExitCode::from(EXIT_SUCCESS))`），不得返回裸 `ExitCode`。
- workspace 已有 `reqwest`（rustls-tls+stream）、`tokio`、`toml`、`serde_json`、`serde`。**新增依赖**：`rusqlite`、`zstd`、`blake3`、`tar`、`flate2`、`sha2`、`sha1`、`base64`、`pubgrub`、`serde_yaml`、`fs2`。`version-ranges` 仅作为 pubgrub `VS=Ranges<SemVer>` 候选依赖；task 2.3 spike 若走自定义 `VersionSet` 则不引入。
- `runtime_startup.rs:55` `compile_or_load_cached` 用 wasmtime `precompile_module`（L85）+ `deserialize_file` 做 cwasm 缓存，按 wasm bytes + wasmtime 版本/config 哈希 key——L2 复用必须继承 exact wasm / engine config 失效边界，不能只用 package manifest + `abi_hash`。
- fixture runner（tests/fixture_runner.rs）in-process 跑 `run_file_in_process`（lib.rs:2074），比对 exit+stdout+stderr。

Assumptions:

- issue #312 在 P5 开工时已合并，提供分离编译 loader 与 multi-instance shared-env（P5 任务假设其存在；若未合并，P5 阻塞，P1–P4 不受影响）。
- `pubgrub` crate（**0.4**，MSRV 1.92 / edition 2024，与本 workspace 一致；0.3 起为翻转式重写，引入 `get_dependencies`/`prioritize` + 关联类型 `P`/`V`/`VS`/`M`/`Priority`/`Err`，本计划 task 2.3 代码即按此形态写）支持自定义 `V`/`VS`（用于 npm SemVer 语义）。`VersionSet` trait（web 已核对 0.3=0.4 一致）要求 `empty`/`singleton`/`complement`/`intersection`/`contains` 五个方法。任务 2.3 的 spike 只保留两条候选实现并以等价测试裁决：① `VS=version_ranges::Ranges<SemVer>`，转换 npm comparator-set 时显式编码预发布排除点；② 自定义 `VersionSet`，由 comparator-`Range` 直接承载 `contains` 并实现集合运算。两条路都必须让 task 2.1 的 `prerelease_inclusion_rule` 转换后逐一等价；未达成等价的候选立即删除，不保留双轨代码。

Unknowns（计划内解决）:

- pubgrub 0.4 `DependencyProvider` 的确切关联类型默认值与 `Dependencies::Unavailable(M)` 形态（web 已确认 0.3=0.4 trait 一致：`prioritize`+`choose_version`+`get_dependencies`，`M` 为自定义不可用原因类型）→ 任务 2.3 首步 spike 用本地 `cargo doc` 复核并锁定 `M`/`Priority` 具体类型。
- `VS`（VersionSet）承载 npm 预发布包含规则的建模路径（`version_ranges::Ranges<SemVer>` 转换编码或自定义 `VersionSet`）→ 任务 2.3 首步 spike 定稿，验收基准是 task 2.1 `prerelease_inclusion_rule` 转换后逐一等价。
- peerDependencies 的宿主环境传播与 instance peer-set 表达 → 任务 2.3 以 npm peer 语义为准，不再把 peer 建模为全局单例；同一 `name@version` 可因 peer-set 不同产生不同 `InstanceId`。
- 可重定位 IR 重定位表需覆盖的引用种类完整清单 → 任务 5.1 首步用等价性快照测试驱动发现。

## BaselineUsageDraft

- Required baseline refs：spec 全文、AGENTS.md、resolver.rs/bundler.rs/graph.rs/package_json.rs、lowerer_modules.rs/scope.rs、cli_args.rs/lib.rs、runtime_startup.rs、snapshot-format、fixture_runner.rs、issue #312 spec/plan。
- Delivered context refs：pnpm/yarn/bun/deno 机制研究（DeepWiki，已在 spec §1）。
- Acknowledged before plan refs：以上全部已在写计划前读取核对（Facts 段为证）。
- Cited in plan refs：见各任务 Files/Why。
- Missing refs：pubgrub 0.4 `DependencyProvider` 的 `M`/`Priority` 具体默认类型（任务 2.3 spike 编译期复核；trait 形态已由 web 调研确认）、可重定位引用完整清单（任务 5.1 快照驱动）。
- Decision：continue。

## Files（owner 边界）

新建 crate `crates/wjsm-pm/`：
```
Cargo.toml
src/lib.rs                 # 公共 API：install/add/remove/resolve/link_provider
src/solver/{mod,npm_semver,provider,duplication,explain}.rs
src/registry/{mod,packument,tarball,npmrc}.rs
src/store/{mod,index,blob,manifest,artifact,vfs,overlay,gc}.rs
src/lockfile/{mod,wjsm_lock,migrate}.rs
src/scripts/mod.rs
src/workspace.rs
tests/mock_registry.rs      # 内置离线 mock registry 测试辅助
```
修改 `crates/wjsm-module/src/`：新增 `vfs.rs`（trait 定义 + `FsVfs`/`NoOverlay` 默认实现 + `ResolutionBoundary`）；`resolver.rs`（构造器接受 vfs/overlay/boundary，**全部 26 处 fs 谓词**——`read_to_string`×1/`canonicalize`×12/`is_file`×7/`is_dir`×6——改 trait 调用；`load_resolved_module` 的 root 安全判断改查 boundary；resolver 无 `exists`/独立 `metadata` 触点）；`runtime_resolution.rs`（新增 provider-aware runtime resolution）；`package_json.rs`（`read_package_info` 经 `Vfs::read_package_json`，合并原 `fs::metadata`+`fs::read_to_string`）；`bundler.rs`/`graph.rs`（注入透传）；`lib.rs`（导出 trait/boundary）。
修改 `crates/wjsm-semantic/src/`：新增 `relocatable/{mod,lower_one,relocate,link}.rs`；`lib.rs` 导出。
修改 `crates/wjsm-cli/src/`：`cli_args.rs`（新增 `Install/Add/Remove/Task/X` 子命令）；`lib.rs`（dispatch + `cmd_install` 等）；`runtime_loader.rs`（Vfs/Overlay/Boundary 注入，运行期源码读取、格式探测、package boundary 探测均不再直读真实 FS）；新增 `pm_commands.rs`。
修改根 `Cargo.toml`：workspace members + 新增依赖。

## Compatibility

- 不变式：`wjsm-module` 无 `wjsm-pm` 依赖；FS 模式默认；现有 fixture 全绿；`wjsm run file.js` 语义不变；blob 内容寻址；lockfile 记录实例图且可惰性补齐 store。
- 非目标（不实现）：物化 node_modules、`wjsm publish`、原生 node-gyp/平台二进制 postinstall 编译、git+/远程 tarball 依赖源、HMR、私有 registry 完整企业 auth。依赖生命周期脚本执行仅限显式 trust allowlist，未授权默认跳过并写入 lockfile 标记。
- 稳定接口：`Vfs`/`ResolutionOverlay`/`ResolutionBoundary` trait 一经定义即为 module↔pm 契约（ADR 信号 2）。

## Architecture Integrity Lens

- Invariant：解析算法、下载/存储、版本求解、编译器接入四类 owner 分离。
- Canonical owner / contract：`wjsm-pm` 拥有 store/solver/registry/lockfile/scripts/workspace；`wjsm-module` 拥有解析算法（不变）+ Vfs/Overlay trait 定义；`wjsm-semantic` 拥有可重定位 IR；`wjsm-cli` 拥有组装。
- Responsibility overlap：`wjsm-pm` 不重复实现 exports/imports/main 解析——把 CAS 包呈现为虚拟树，复用 module 现有解析。
- Higher-level simplification：node_modules 查找统一抽象为 Overlay + Vfs，同一抽象服务 FS 模式与 CAS 模式，避免两套解析代码。
- Retirement / falsifier：`wjsm install` 后无 node_modules、`wjsm run` 从 CAS 编译成功、同包跨项目零重复编译 → 旧"物化 node_modules"假设退出。
- Verdict：proceed。

## Plan Pressure Test

- Owner / contract / retirement：owner 清晰（新 crate + trait 契约）；无删除现有主路径（new-capability）。
- Architecture integrity / higher-level path：Overlay/Vfs 抽象即最高层简化，已采纳。
- Verification scope：每阶段独立 nextest 目标 + 全量回归 + fixture 冒烟。
- Task executability：任务含完整代码与命令；pubgrub 版本/形态已由 web 调研锁定 0.4（残余 `M`/`Priority` 具体类型由任务 2.3 spike 编译期复核）、可重定位引用清单由任务 5.1 快照驱动首步收敛。
- Pressure result：proceed。

## Plan-Time Complexity Check

Complexity Budget：
- Artifact class：新 crate（承载主复杂度）+ 现有 crate 微创注入 + semantic 架构演进。
- Target files：`wjsm-pm/*`（全新，每文件单一职责 ≤500 行）；`wjsm-module`（新 vfs.rs 承载 trait + 默认实现；resolver.rs 全部 fs 谓词路由进 Vfs——是有实质工作量的重构，非"改几处签名"）；`wjsm-semantic/relocatable/*`（新 owner 文件，非改大文件）；`wjsm-cli`（新 pm_commands.rs + dispatch 微创）。
- Current pressure：`resolver.rs` 1567 行已超纪律——**禁止**往其加包管理逻辑；fs 谓词 Vfs 化是"把散落的 `std::fs`/`Path` 调用替换为 `self.vfs.*` 调用"的等量替换，不新增业务逻辑，行数基本持平。若替换后逼近 1600 行，将解析算法按类别（相对/bare/package-target）拆到 resolver 子模块，作为该任务收尾。
- Projected post-change pressure：主复杂度进新文件；resolver.rs 行数持平（等量替换）；现有大文件不增业务负担。
- Budget result：within-budget。
- Planned governance：每子模块独立文件；resolver/lib 只做 wiring。

Plan-Time Complexity Check：
- Better file boundary：pm 逻辑全进 `wjsm-pm`；semantic 可重定位进 `relocatable/` 子模块；CLI 命令进 `pm_commands.rs`。
- Recommendation：add owner files + edit existing only for wiring。

---

# 阶段 P1：存储与解析地基

## 任务 1.1：创建 wjsm-pm crate 骨架 + 依赖

Files:
- 修改 `Cargo.toml`（根，workspace members + deps）
- 创建 `crates/wjsm-pm/Cargo.toml`
- 创建 `crates/wjsm-pm/src/lib.rs`

Why: 建立 pm crate 与依赖，作为其余 pm 逻辑的容器。

Impact/Compatibility: 纯新增；不触碰现有 crate。

Verification: `cargo build -p wjsm-pm`

Steps:

- [ ] **写 crate 编译测试**。创建 `crates/wjsm-pm/src/lib.rs`：
  ```rust
  // wjsm-pm: wjsm 包管理器（CAS 存储 + PubGrub 求解 + registry + lockfile）
  #![allow(dead_code)]

  pub mod store;

  #[cfg(test)]
  mod tests {
      #[test]
      fn crate_builds() {
          assert_eq!(2 + 2, 4);
      }
  }
  ```
  创建 `crates/wjsm-pm/src/store/mod.rs`：`// CAS 存储引擎入口`（当前任务只交付 crate/module 边界；子模块在对应任务接入）。
- [ ] **Verify RED/编译失败**：先运行 `cargo build -p wjsm-pm`，预期报 workspace members 未注册；随后注册 workspace member 并补齐依赖。
- [ ] **完整实现**。根 `Cargo.toml` `members` 追加 `"crates/wjsm-pm"`；`[workspace.dependencies]` 追加：
  ```toml
  rusqlite = { version = "0.32", features = ["bundled"] }
  zstd = "0.13"
  blake3 = "1"
  tar = "0.4.45"   # ≥0.4.45：修复 RUSTSEC-2026-0067（unpack_in 经符号链接 chmod 外部目录）；本项目 extract_tgz 另有独立守卫
  flate2 = "1"
  sha2 = "0.10"
  sha1 = "0.10"
  base64 = "0.22"
  pubgrub = "0.4"
  version-ranges = "0.1"
  serde_yaml = "0.9"
  fs2 = "0.4"      # store 级跨进程 advisory flock（并发 install / gc 串行化）
  ```
  创建 `crates/wjsm-pm/Cargo.toml`：
  ```toml
  [package]
  name = "wjsm-pm"
  version.workspace = true
  edition.workspace = true

  [dependencies]
  anyhow = { workspace = true }
  rusqlite = { workspace = true }
  zstd = { workspace = true }
  blake3 = { workspace = true }
  tar = { workspace = true }
  flate2 = { workspace = true }
  sha2 = { workspace = true }
  sha1 = { workspace = true }
  base64 = { workspace = true }
  pubgrub = { workspace = true }
  version-ranges = { workspace = true }  # 候选路：VS=Ranges<SemVer>；spike 定稿若选自定义 VersionSet 则移除
  reqwest = { workspace = true }
  tokio = { workspace = true }
  toml = { workspace = true }
  serde = { workspace = true }
  serde_json = { workspace = true }
  serde_yaml = { workspace = true }
  fs2 = { workspace = true }
  wjsm-module = { path = "../wjsm-module" }
  wjsm-snapshot-format = { path = "../wjsm-snapshot-format" }
  ```
- [ ] **Verify GREEN**：`cargo build -p wjsm-pm && cargo nextest run -p wjsm-pm` 预期通过 `crate_builds`。
- [ ] **Commit**：`git add -A && git commit -m "feat(wjsm-pm): crate 骨架与依赖"`

## 任务 1.2：内容寻址 blob 存储（zstd + packfile）

Files:
- 创建 `crates/wjsm-pm/src/store/blob.rs`
- 修改 `crates/wjsm-pm/src/store/mod.rs`

Why: blob 层是 CAS 核心——文件内容按 blake3 哈希去重、zstd 压缩、追加进 packfile，解决小文件 inode 爆炸。

Impact/Compatibility: 纯新增。packfile 追加式；单 pack 软上限轮转由 `Store`（任务 1.5，经 index `active_pack_id`/`bump_pack`）驱动，本任务的 `PackWriter` 只提供 `len()`（供轮转判定）与 `sync()`（fsync）。写中断产生的孤儿尾部字节由 `wjsm store gc`（任务 1.5b）回收。

Verification: `cargo nextest run -p wjsm-pm -E 'test(blob)'`

Steps:

- [ ] **写失败测试**。`store/blob.rs`：
  ```rust
  // blob 层：文件内容 blake3 寻址 + zstd 压缩 + 追加式 packfile
  use anyhow::{Context, Result};
  use std::fs::{File, OpenOptions};
  use std::io::{Read, Seek, SeekFrom, Write};
  use std::path::{Path, PathBuf};

  /// blob 在 packfile 中的位置。
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub struct BlobLoc {
      pub pack_id: u32,
      pub offset: u64,
      pub clen: u32,
      pub ulen: u32,
  }

  /// blake3 内容哈希（32 字节）。
  pub type BlobHash = [u8; 32];

  pub fn hash_content(bytes: &[u8]) -> BlobHash {
      blake3::hash(bytes).into()
  }

  /// 追加式 packfile 写入器。
  pub struct PackWriter {
      pack_id: u32,
      path: PathBuf,
      file: File,
      offset: u64,
  }

  impl PackWriter {
      pub fn open(packs_dir: &Path, pack_id: u32) -> Result<Self> {
          std::fs::create_dir_all(packs_dir)?;
          let path = packs_dir.join(format!("{pack_id:04}.pack"));
          let file = OpenOptions::new().create(true).read(true).append(true).open(&path)?;
          let offset = file.metadata()?.len();
          Ok(Self { pack_id, path, file, offset })
      }

      /// 写入一个 blob，返回位置。内容独立 zstd 压缩。
      ///
      /// 注意：文件以 `.append(true)` 打开，OS 保证每次 `write_all` 落到**真实 EOF**，
      /// 但 `BlobLoc.offset` 必须记录该次写入的真实起点。因此**不信任缓存 `self.offset`**——
      /// 写前 `seek(End(0))` 取真实偏移作为 `offset`（`Store::add_package_from_dir` 持
      /// store 级独占写锁，故单进程内 `&mut self` 串行 + 跨进程 flock 串行，此 seek 结果稳定）。
      pub fn append(&mut self, content: &[u8]) -> Result<BlobLoc> {
          let compressed = zstd::encode_all(content, 19).context("zstd 压缩 blob")?;
          let offset = self.file.seek(SeekFrom::End(0)).context("定位 packfile 尾部")?;
          self.file.write_all(&compressed).context("追加 blob 到 packfile")?;
          self.offset = offset + compressed.len() as u64;
          Ok(BlobLoc {
              pack_id: self.pack_id,
              offset,
              clen: compressed.len() as u32,
              ulen: content.len() as u32,
          })
      }

      /// 当前 packfile 已写字节数（供 Store 判断是否轮转）。
      pub fn len(&self) -> u64 { self.offset }
      pub fn pack_id(&self) -> u32 { self.pack_id }
      /// fsync：确保 blob 字节落盘先于 index 事务提交（崩溃一致性）。
      pub fn sync(&self) -> Result<()> { self.file.sync_all().context("fsync packfile")?; Ok(()) }
  }

  /// 从 packfile 读取并解压一个 blob。
  pub fn read_blob(packs_dir: &Path, loc: BlobLoc) -> Result<Vec<u8>> {
      let path = packs_dir.join(format!("{:04}.pack", loc.pack_id));
      let mut file = File::open(&path).with_context(|| format!("打开 packfile {}", path.display()))?;
      file.seek(SeekFrom::Start(loc.offset))?;
      let mut buf = vec![0u8; loc.clen as usize];
      file.read_exact(&mut buf)?;
      let out = zstd::decode_all(&buf[..]).context("zstd 解压 blob")?;
      anyhow::ensure!(out.len() == loc.ulen as usize, "blob 解压长度不匹配");
      Ok(out)
  }

  #[cfg(test)]
  mod tests {
      use super::*;

      fn temp_dir(name: &str) -> PathBuf {
          let d = std::env::temp_dir().join(format!("wjsm_pm_blob_{name}_{}", std::process::id()));
          let _ = std::fs::remove_dir_all(&d);
          std::fs::create_dir_all(&d).unwrap();
          d
      }

      #[test]
      fn blob_roundtrip_and_dedup_hash() {
          let dir = temp_dir("roundtrip");
          let mut w = PackWriter::open(&dir, 0).unwrap();
          let content = b"export const x = 1;\n".repeat(50);
          let loc = w.append(&content).unwrap();
          drop(w);
          let got = read_blob(&dir, loc).unwrap();
          assert_eq!(got, content);
          // 相同内容哈希一致（去重依据）
          assert_eq!(hash_content(&content), hash_content(&content));
          assert_ne!(hash_content(&content), hash_content(b"other"));
      }

      #[test]
      fn two_blobs_distinct_offsets() {
          let dir = temp_dir("two");
          let mut w = PackWriter::open(&dir, 0).unwrap();
          let a = w.append(b"aaaa").unwrap();
          let b = w.append(b"bbbb").unwrap();
          assert_ne!(a.offset, b.offset);
          assert_eq!(read_blob(&dir, a).unwrap(), b"aaaa");
          assert_eq!(read_blob(&dir, b).unwrap(), b"bbbb");
      }
  }
  ```
  `store/mod.rs` 追加：`pub mod blob;`
- [ ] **Verify RED**：`cargo nextest run -p wjsm-pm -E 'test(blob)'` 预期未编译前失败 → 补齐后运行。
- [ ] **完整实现**：上面 `PackWriter`（含 `len`/`sync`/`pack_id`）即完整。轮转决策**不在** blob 层——`PackWriter` 只对单一 `pack_id` 负责；「活跃 pack 选择 + 超软上限换新 pack」是 `Store::active_writer`（任务 1.5）经 `index.active_pack_id()`/`bump_pack()` 决定，pack 元数据落 `packs` 表（任务 1.4）。此分层避免 blob 层扫描目录、避免两处各存一份"当前 pack"状态。
- [ ] **Verify GREEN**：`cargo nextest run -p wjsm-pm -E 'test(blob)'` 两个测试通过。
- [ ] **Commit**：`git add -A && git commit -m "feat(wjsm-pm): blob 层 zstd+packfile 内容寻址"`

## 任务 1.3：包文件清单（manifest）

Files:
- 创建 `crates/wjsm-pm/src/store/manifest.rs`
- 修改 `crates/wjsm-pm/src/store/mod.rs`

Why: manifest 是"包级归档" = 有序 `rel_path → (blob_hash, mode)` 清单（git tree 式），本身内容寻址。是包身份到 blob 的桥。

Impact/Compatibility: 纯新增。

Verification: `cargo nextest run -p wjsm-pm -E 'test(manifest)'`

Steps:

- [ ] **写失败测试**。`store/manifest.rs`：
  ```rust
  // 包文件清单：有序 rel_path → (blob_hash, mode)，清单本身内容寻址
  use crate::store::blob::{hash_content, BlobHash};
  use serde::{Deserialize, Serialize};

  #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
  pub struct ManifestEntry {
      pub rel_path: String,
      #[serde(with = "hex_hash")]
      pub blob_hash: BlobHash,
      pub mode: u32,
  }

  #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
  pub struct Manifest {
      /// 按 rel_path 升序，保证清单内容确定性。
      pub entries: Vec<ManifestEntry>,
  }

  impl Manifest {
      pub fn from_entries(mut entries: Vec<ManifestEntry>) -> Self {
          entries.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
          Self { entries }
      }

      /// 清单内容哈希：确定性序列化后 blake3。
      pub fn hash(&self) -> BlobHash {
          let bytes = serde_json::to_vec(&self.entries).expect("manifest 序列化");
          hash_content(&bytes)
      }

      pub fn lookup(&self, rel_path: &str) -> Option<&ManifestEntry> {
          self.entries
              .binary_search_by(|e| e.rel_path.as_str().cmp(rel_path))
              .ok()
              .map(|i| &self.entries[i])
      }
  }

  mod hex_hash {
      use serde::{Deserialize, Deserializer, Serializer};
      pub fn serialize<S: Serializer>(h: &[u8; 32], s: S) -> Result<S::Ok, S::Error> {
          s.serialize_str(&hex(h))
      }
      pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<[u8; 32], D::Error> {
          let s = String::deserialize(d)?;
          let mut out = [0u8; 32];
          for (i, b) in out.iter_mut().enumerate() {
              *b = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).map_err(serde::de::Error::custom)?;
          }
          Ok(out)
      }
      fn hex(h: &[u8; 32]) -> String {
          h.iter().map(|b| format!("{b:02x}")).collect()
      }
  }

  #[cfg(test)]
  mod tests {
      use super::*;

      fn entry(p: &str, seed: u8) -> ManifestEntry {
          ManifestEntry { rel_path: p.into(), blob_hash: [seed; 32], mode: 0o644 }
      }

      #[test]
      fn manifest_is_order_independent() {
          let m1 = Manifest::from_entries(vec![entry("b.js", 2), entry("a.js", 1)]);
          let m2 = Manifest::from_entries(vec![entry("a.js", 1), entry("b.js", 2)]);
          assert_eq!(m1.hash(), m2.hash(), "清单哈希应与输入顺序无关");
      }

      #[test]
      fn manifest_lookup_and_hash_differs_on_content() {
          let m = Manifest::from_entries(vec![entry("a.js", 1), entry("b.js", 2)]);
          assert_eq!(m.lookup("a.js").unwrap().blob_hash, [1u8; 32]);
          assert!(m.lookup("missing").is_none());
          let m3 = Manifest::from_entries(vec![entry("a.js", 9), entry("b.js", 2)]);
          assert_ne!(m.hash(), m3.hash());
      }
  }
  ```
  `store/mod.rs` 追加 `pub mod manifest;`
- [ ] **Verify RED**：`cargo nextest run -p wjsm-pm -E 'test(manifest)'`。
- [ ] **完整实现**：上面即完整。
- [ ] **Verify GREEN**：两个测试通过。
- [ ] **Commit**：`git add -A && git commit -m "feat(wjsm-pm): 包文件清单 manifest 内容寻址"`

## 任务 1.4：SQLite 索引（index.db，locator/instance 安全）

Files:
- 创建 `crates/wjsm-pm/src/store/index.rs`
- 修改 `crates/wjsm-pm/src/store/mod.rs`

Why: SQLite index.db（WAL）统管 blobs/manifests/package locators/artifacts 映射，取代海量 JSON 小文件元数据；同时避免把不同 registry/private source 的同名同版本包折叠成同一条记录。

Impact/Compatibility: 纯新增。事务写保证中断可回滚。**身份修正**：store 查询主键是 `source_id = blake3(registry_url || resolved_tarball_url || integrity_or_shasum)`，下载前即可从 packument/lockfile 计算；`manifest_hash`/`content_id` 是解包后清单内容哈希，用于校验同一 source 实际字节。`name/version` 只做元数据索引；lockfile/runtime 以 `source_id` 读取内容，并复核 `manifest_hash`，绝不只用 `name@version`。

**与批准设计 §6.3 的偏离（显式记录）**：设计 §6.3 用规范化三表 `manifests(id) + manifest_entries(manifest_id, rel_path, blob_hash, mode)` + `packages.meta BLOB(MessagePack)`。本计划首版保留两处偏离：① manifest 存储塌缩为 `manifests(hash, body BLOB)`（manifest 整体 JSON 存 body）；② `packages.meta` 编码用 `serde_json`（JSON BLOB）而非 MessagePack。偏离不影响 source/content identity：`packages.source_id` 是可预知 source locator 键，`manifest_hash` 是解包后的内容清单键。

Verification: `cargo nextest run -p wjsm-pm -E 'test(index)'`

Steps:

- [ ] **写失败测试**。`store/index.rs` 定义：
  - `StoreIndex { conn: Mutex<Connection> }`，`open(db_path)` 启用 WAL 并建表。
  - `packages(source_id BLOB PRIMARY KEY, name TEXT, version TEXT, registry TEXT, resolved TEXT, integrity TEXT, shasum TEXT, manifest_hash BLOB NOT NULL, meta BLOB, UNIQUE(name, version, registry, resolved, integrity, shasum))`；另建 `(name, version)` 普通索引用于统计/展示，不用于读取唯一包。
  - `blobs(hash BLOB PRIMARY KEY, pack_id, offset, clen, ulen)`；`manifests(hash BLOB PRIMARY KEY, body BLOB)`；`artifacts(tier INTEGER NOT NULL, key_hash BLOB NOT NULL, source_id BLOB, instance_id TEXT, compiler_version TEXT, abi_hash INTEGER, gc_flavor TEXT, wasmtime_version TEXT, engine_config_hash BLOB, target_triple TEXT, opt_level TEXT, pack_id, offset, clen, ulen, metadata BLOB, PRIMARY KEY(tier, key_hash))`；`packs(pack_id PRIMARY KEY, committed_len)`。
  - `put_package_locator(source_id, name, version, registry, resolved, integrity, shasum, manifest_hash, meta_json)` / `get_package_manifest(source_id)` / `get_locator(source_id)`。
  - `put_blob` / `get_blob`、`put_manifest_raw` / `get_manifest_raw`。
  - `with_txn<R>(&self, f: impl FnOnce(&Transaction) -> Result<R>) -> Result<R>`；事务作用域自由函数 `txn_put_blob` / `txn_put_manifest_raw` / `txn_put_package_locator` / `txn_put_artifact_loc`。闭包内不得回调会重新 `lock()` 的 `&self` 方法，避免同一 `Mutex` 二次锁死。
  - `active_pack_id()` / `bump_pack()`。
  - `reachable_package_blob_hashes()` 从 packages→manifests 收集文件 blob；`reachable_artifact_locs()` 返回 artifacts 表仍有效的 pack loc；GC 使用两者组成统一可达对象集。
- [ ] **Verify RED**：`cargo nextest run -p wjsm-pm -E 'test(index)'`。
- [ ] **完整实现**：补齐上面 API；测试覆盖：`index_blob_put_get`、`index_package_locator_keeps_same_name_version_sources_distinct`（同 `name@version`、不同 registry/resolved/integrity → 两个 source_id）、`index_txn_rolls_back_all_rows`、`index_reachable_objects_include_artifacts`。
- [ ] **Verify GREEN**：全部 `index_*` 测试通过。
- [ ] **Commit**：`git add -A && git commit -m "feat(wjsm-pm): SQLite index.db（locator 身份 + artifacts/GC 可达集 + 事务）"`

## 任务 1.5：Store 统一入口（写包 + locator 读文件事务）

Files:
- 修改 `crates/wjsm-pm/src/store/mod.rs`

Why: 把 blob/manifest/index 组装成 `Store`——`add_package_from_dir(locator, dir)`（解包目录→blob→manifest→事务入库）与 `read_package_file(source_id, rel_path)`（lockfile 指定 source→源码）。这是编译器直供读取路径，必须按 source identity 读取，并按 content identity 复核，避免跨 registry 串包。

Impact/Compatibility: 纯新增。`PackageLocator` 由 registry/install 层构造，包含 `source_id/name/version/registry/resolved/integrity/shasum/expected_manifest_hash`；store 不用 `name@version` 做唯一读取键。

Verification: `cargo nextest run -p wjsm-pm -E 'test(store_integration)'`

Steps:

- [ ] **写失败测试**。`store/mod.rs` 定义：
  - `pub const STORE_VERSION: &str = "v1"`；`PackageLocator { source_id: [u8;32], name, version, registry, resolved, integrity: Option<String>, shasum: Option<String>, expected_manifest_hash: Option<[u8;32]> }`。
  - `Store::open(store_root)`；`write_lock()` 使用 `<root>/.write.lock` + `fs2::FileExt::lock_exclusive`；`active_writer()` 经 `index.active_pack_id()`/`bump_pack()` 轮转 pack。
  - `add_package_from_dir(&self, locator: &PackageLocator, dir: &Path, meta_json: &[u8]) -> Result<[u8;32]>`：遍历 `dir` 时用 `symlink_metadata()`，目录继续，普通文件入库，**符号链接/硬链接/设备/FIFO/特殊文件一律报错**；文件 mode 只保留可执行位；blob 字节先 append + `writer.sync()`，再 `with_txn` 写 blobs/manifest/package locator。返回实际 `manifest_hash`，并在 `expected_manifest_hash` 存在时强制相等，否则报 source/content mismatch。
  - `read_package_file(&self, source_id: &[u8;32], rel_path: &str) -> Result<Option<Vec<u8>>>`；`manifest_has_prefix(source_id, rel_prefix)`；`package_manifest_hash(source_id)`；`has_package(source_id)`。
- [ ] **Verify RED**：`cargo nextest run -p wjsm-pm -E 'test(store_integration)'`。
- [ ] **完整实现**：测试覆盖 `store_integration_add_and_read_by_source_id`、`store_integration_same_name_version_different_source_ids_do_not_alias`、`store_integration_rejects_symlink_in_package_dir`、`store_integration_atomic_rollback`、`store_integration_concurrent_writers`。
- [ ] **Verify GREEN**：上述测试通过。
- [ ] **Commit**：`git add -A && git commit -m "feat(wjsm-pm): Store 统一入口（source locator 读写 + 整包事务 + pack 轮转）"`

## 任务 1.5b：store gc（package blobs + artifacts 统一可达集）

Files:
- 创建 `crates/wjsm-pm/src/store/gc.rs`
- 修改 `crates/wjsm-pm/src/store/mod.rs`

Why: packfile 是追加式；写中断尾部、被 `--prune` 移除的包、过期 artifact 都会产生不可达对象。GC 必须同时保留 package blobs 与 artifacts，否则 P5 后会删除 cwasm/IR artifact 所在 pack 造成悬空 offset。

Impact/Compatibility: 纯新增。gc 走「标记-复制」：可达对象 = packages→manifests→blob hashes + artifacts 表 locs。复制后在同一事务更新 blobs/artifacts loc，原子替换 packs。gc 期间加 store 级文件锁串行化。CLI `cache gc` 接入延后到任务 3.3（wjsm-cli 引入 wjsm-pm 依赖之后），本任务只交付 store owner。

Verification: `cargo nextest run -p wjsm-pm -E 'test(gc)'`

Steps:

- [ ] **写失败测试**。`store/gc.rs`：`gc(store) -> GcStats { reclaimed_indexed_blobs, reclaimed_artifacts, reclaimed_orphan_bytes }`。测试：① `gc_rewrites_package_blobs_and_preserves_reads`；② `gc_preserves_artifact_rows_and_rewrites_offsets`；③ `gc_reclaims_unindexed_orphan_tail_bytes`（直接 `PackWriter::append` 但不写 index，只断言 bytes 回收，不断言 blob 数）。
- [ ] **Verify RED**：`cargo nextest run -p wjsm-pm -E 'test(gc)'`。
- [ ] **完整实现**：实现统一可达对象复制 + store 级 flock；提供 `pub fn gc(store: &Store) -> Result<GcStats>`，不触碰 CLI。
- [ ] **Verify GREEN**：测试通过。
- [ ] **Commit**：`git add -A && git commit -m "feat(wjsm-pm): store gc 保留 package blobs 与 artifacts（标记-复制 + 文件锁）"`

## 任务 1.6：wjsm-module 新增 Vfs/ResolutionOverlay/ResolutionBoundary（全量 fs 谓词抽象，默认零破坏）

Files:
- 创建 `crates/wjsm-module/src/vfs.rs`
- 修改 `crates/wjsm-module/src/lib.rs`（导出）
- 修改 `crates/wjsm-module/src/resolver.rs`（构造器接受 trait 对象与 boundary；resolver 实际调用的 fs 谓词——`read_to_string`×1/`canonicalize`×12/`is_file`×7/`is_dir`×6——改 trait 调用。注：resolver 自身**不调用** `Path::exists`，`Vfs::exists` 仅为 `CasVfs` 内部 `is_file`/`is_dir` 复用与 trait 完备性而定义）
- 修改 `crates/wjsm-module/src/runtime_resolution.rs`（新增 provider-aware runtime resolution API）
- 修改 `crates/wjsm-module/src/package_json.rs`（`read_package_info` 经 Vfs）
- 修改 `crates/wjsm-module/src/bundler.rs` / `graph.rs`（注入透传）

Why: 让 CAS 无缝切入解析且不反转依赖方向。除此之外，CAS 虚拟包根不在项目根下，现有 `load_resolved_module` 的 `path.starts_with(root_path)` 会直接拒绝虚拟依赖；必须把安全边界显式建模为 `ResolutionBoundary`。

Impact/Compatibility: **最关键兼容任务**。`FsVfs` 每个方法必须与被替换的 `std::fs`/`Path` 调用语义逐字节等价；默认 boundary 只含项目根，现有行为不变。CAS 模式 boundary 增加 vroot 与 workspace member 根。

Verification: `cargo nextest run -p wjsm-module && cargo nextest run --workspace`

Steps:

- [ ] **写失败测试**。`vfs.rs` 定义 `Vfs`、`ResolutionOverlay`、`ResolutionBoundary { allowed_roots: Vec<PathBuf> }`；`ResolutionBoundary::allows(path)` 对 `FsVfs` canonical path 和 `CasVfs` normalized virtual path 都可判定。保留 `normalize_virtual(path)`。测试 `fs_vfs_predicates_match_std`、`no_overlay_returns_none`、`boundary_allows_project_and_vroot_but_rejects_other`。
- [ ] **Verify RED**：`cargo nextest run -p wjsm-module -E 'test(vfs) | test(boundary)'`。
- [ ] **完整接入**：
  - `ModuleResolver` 增 `vfs: Arc<dyn Vfs>`、`overlay: Arc<dyn ResolutionOverlay>`、`boundary: ResolutionBoundary`；`with_options` 默认 `FsVfs/NoOverlay/Boundary(project_root)`；新增 `with_providers(root, options, vfs, overlay, boundary)`。
  - 全部 fs 谓词按 Plan Basis 列表改经 `self.vfs`。
  - `load_resolved_module` 的 outside-root 检查改为 `boundary.allows(&path)`，测试 `resolver_allows_virtual_dependency_inside_boundary` 覆盖项目入口 import CAS vroot 依赖。
  - `find_package_in_node_modules` 入口先查 overlay，未命中再查真实 node_modules 遍历。
  - `package_json.rs`：`read_package_info(dir, vfs: &dyn Vfs)`。
  - `bundler.rs`/`graph.rs`：`with_providers` 透传 boundary。
  - `runtime_resolution.rs` 新增 `resolve_runtime_specifier_with_providers` / `resolve_runtime_paths_with_providers`，保持旧 API 走默认 provider。
- [ ] **Verify GREEN**：`cargo nextest run -p wjsm-module && cargo nextest run --workspace` 全绿。
- [ ] **Commit**：`git add -A && git commit -m "feat(wjsm-module): Vfs/Overlay/Boundary 抽象解析与运行期 resolution"`

---

# 阶段 P2：registry client + PubGrub solver

## 任务 2.1：npm 精确 SemVer 语义

Files:
- 创建 `crates/wjsm-pm/src/solver/mod.rs`
- 创建 `crates/wjsm-pm/src/solver/npm_semver.rs`
- 修改 `crates/wjsm-pm/src/lib.rs`

Why: npm 区间语义（`^`/`~`/x-range/hyphen/比较运算符/`||`/预发布包含规则）与通用 semver 有差异，必须精确匹配 node-semver。设计 §7.3 要求**精确匹配**——按项目 hard rule「No partial implementations」，本任务一次性覆盖 node-semver 全部区间形态，不留按 fixture 补齐语义的缺口。

**node-semver 语义要点（决定实现结构）**：
1. **预发布包含规则**（最易错）：带预发布的版本（如 `2.0.0-alpha`）只有当**同一 comparator set 中存在某个 comparator，其 `[major,minor,patch]` 元组与该版本相同且该 comparator 自身带预发布**时才可能匹配。即 `^1.2.3`（无预发布，展开上界 `<2.0.0`）**不匹配** `2.0.0-alpha`——原计划的 `(lo,hi)` 元组模型此处有硬伤（`2.0.0-alpha < 2.0.0` 会误判命中）。故 Range 不能塌缩为单一 `(lo,hi)`，须保留 comparator 结构以携带各 comparator 的预发布信息。
2. **comparator set = 交集**（空格分隔），**多 set = 并集**（`||`）。
3. **x-range**：`*`/空/`x`/`X`/缺省段（`1`、`1.2`、`1.x`、`1.2.x`）——按缺省位置展开为范围。
4. **hyphen range**：`1.2.3 - 2.3.4`（含两端，右端部分版本按"补齐上界"展开）。
5. **`~`/`^` 部分版本**：`~1`→`>=1.0.0 <2.0.0`；`~1.2`→`>=1.2.0 <1.3.0`；`^1`→`>=1.0.0 <2.0.0`；`^0`→`>=0.0.0 <1.0.0`；`^0.0`→`>=0.0.0 <0.1.0`。

Impact/Compatibility: 纯新增。

Verification: `cargo nextest run -p wjsm-pm -E 'test(npm_semver)'`

Steps:

- [ ] **Spike 首步：锁定对照表**。以 node-semver README 的 Ranges 表 + `test/fixtures/range-parse.js` 行为为准，把下列样例写成断言表（GREEN 目标）。若与本任务代码有分歧，以 node-semver 实际行为为准修正代码，不修正测试。
- [ ] **写失败测试**（comparator 结构模型，携带预发布信息）。`solver/npm_semver.rs`：
  ```rust
  // npm 精确 SemVer：版本序 + comparator（op+partial）集合，精确匹配 node-semver
  use std::cmp::Ordering;

  #[derive(Debug, Clone, PartialEq, Eq)]
  pub struct SemVer {
      pub major: u64,
      pub minor: u64,
      pub patch: u64,
      pub pre: Vec<PreId>,       // 预发布标识（数字/字母混合，按 semver 规则排序）
  }

  #[derive(Debug, Clone, PartialEq, Eq)]
  pub enum PreId { Num(u64), Alpha(String) }

  impl SemVer {
      pub fn parse(s: &str) -> Option<Self> {
          let s = s.trim().trim_start_matches('v');
          let core_pre = s.split('+').next().unwrap_or(s); // 去 build metadata
          let (core, pre) = match core_pre.split_once('-') {
              Some((c, p)) => (c, parse_pre(p)),
              None => (core_pre, Vec::new()),
          };
          let mut it = core.split('.');
          let major = it.next()?.parse().ok()?;
          let minor = it.next()?.parse().ok()?;
          let patch = it.next()?.parse().ok()?;
          if it.next().is_some() { return None; }
          Some(SemVer { major, minor, patch, pre })
      }
      pub fn has_pre(&self) -> bool { !self.pre.is_empty() }
      fn tuple(&self) -> (u64, u64, u64) { (self.major, self.minor, self.patch) }
  }

  fn parse_pre(s: &str) -> Vec<PreId> {
      s.split('.').map(|id| match id.parse::<u64>() {
          Ok(n) if !id.starts_with('0') || id == "0" => PreId::Num(n),
          _ => PreId::Alpha(id.to_string()),
      }).collect()
  }

  impl PartialOrd for SemVer { fn partial_cmp(&self, o: &Self) -> Option<Ordering> { Some(self.cmp(o)) } }
  impl Ord for SemVer {
      fn cmp(&self, o: &Self) -> Ordering {
          self.tuple().cmp(&o.tuple()).then_with(|| cmp_pre(&self.pre, &o.pre))
      }
  }

  // 预发布排序：无预发布 > 有预发布；逐 id 比较（数字 < 字母，短者先耗尽为小）。
  fn cmp_pre(a: &[PreId], b: &[PreId]) -> Ordering {
      match (a.is_empty(), b.is_empty()) {
          (true, true) => Ordering::Equal,
          (true, false) => Ordering::Greater,
          (false, true) => Ordering::Less,
          (false, false) => {
              for (x, y) in a.iter().zip(b.iter()) {
                  let c = match (x, y) {
                      (PreId::Num(m), PreId::Num(n)) => m.cmp(n),
                      (PreId::Num(_), PreId::Alpha(_)) => Ordering::Less,
                      (PreId::Alpha(_), PreId::Num(_)) => Ordering::Greater,
                      (PreId::Alpha(m), PreId::Alpha(n)) => m.cmp(n),
                  };
                  if c != Ordering::Equal { return c; }
              }
              a.len().cmp(&b.len())
          }
      }
  }

  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub enum Op { Gt, Gte, Lt, Lte, Eq }

  /// 单个比较子：运算符 + 目标版本（已展开为具体 SemVer）。
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub struct Comparator { pub op: Op, pub ver: SemVer }

  impl Comparator {
      fn satisfies(&self, v: &SemVer) -> bool {
          match self.op {
              Op::Gt => v > &self.ver,
              Op::Gte => v >= &self.ver,
              Op::Lt => v < &self.ver,
              Op::Lte => v <= &self.ver,
              Op::Eq => v == &self.ver,
          }
      }
  }

  /// comparator set = 交集；Range = set 的并集。
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub struct Range { pub sets: Vec<Vec<Comparator>> }

  impl Range {
      pub fn parse(s: &str) -> Option<Self> {
          let mut sets = Vec::new();
          for part in s.split("||") {
              sets.push(parse_comparator_set(part.trim())?);
          }
          Some(Range { sets })
      }

      pub fn matches(&self, v: &SemVer) -> bool {
          self.sets.iter().any(|set| set_matches(set, v))
      }
  }

  // 预发布包含规则：带预发布的 v 只在「同 set 内存在某 comparator，其 tuple 与 v 相同且带预发布」时才可能匹配。
  fn set_matches(set: &[Comparator], v: &SemVer) -> bool {
      if !set.iter().all(|c| c.satisfies(v)) {
          return false;
      }
      if v.has_pre() {
          let allowed = set.iter().any(|c| c.ver.has_pre() && c.ver.tuple() == v.tuple());
          if !allowed { return false; }
      }
      true
  }

  // ---- 区间形态展开（x-range / hyphen / ~ / ^ / 运算符 / 精确）→ Vec<Comparator> ----
  fn parse_comparator_set(s: &str) -> Option<Vec<Comparator>> {
      if s.is_empty() || s == "*" || s == "x" || s == "X" {
          return Some(vec![Comparator { op: Op::Gte, ver: SemVer { major: 0, minor: 0, patch: 0, pre: vec![] } }]);
      }
      // hyphen range: "A - B"
      if let Some((lo, hi)) = split_hyphen(s) {
          let mut out = expand_lower_bound(lo)?;
          out.extend(expand_upper_bound(hi)?);
          return Some(out);
      }
      let mut comps = Vec::new();
      for tok in s.split_whitespace() {
          comps.extend(parse_single(tok)?);
      }
      Some(comps)
  }

  // 详见实现步骤：split_hyphen / expand_lower_bound / expand_upper_bound / parse_single
  // parse_single 处理 ^ ~ >= > <= < = 与 x-range 部分版本展开。

  #[cfg(test)]
  mod tests {
      use super::*;
      fn v(s: &str) -> SemVer { SemVer::parse(s).unwrap() }
      fn r(s: &str) -> Range { Range::parse(s).unwrap() }

      #[test]
      fn npm_semver_caret_nonzero_major() {
          let x = r("^1.2.3");
          assert!(x.matches(&v("1.2.3")) && x.matches(&v("1.9.0")));
          assert!(!x.matches(&v("2.0.0")) && !x.matches(&v("1.2.2")));
      }
      #[test]
      fn npm_semver_caret_zero() {
          assert!(r("^0.2.3").matches(&v("0.2.9")) && !r("^0.2.3").matches(&v("0.3.0")));
          assert!(r("^0.0.3").matches(&v("0.0.3")) && !r("^0.0.3").matches(&v("0.0.4")));
          assert!(r("^0").matches(&v("0.9.9")) && !r("^0").matches(&v("1.0.0")));
      }
      #[test]
      fn npm_semver_tilde_partial() {
          assert!(r("~1.2.3").matches(&v("1.2.9")) && !r("~1.2.3").matches(&v("1.3.0")));
          assert!(r("~1.2").matches(&v("1.2.9")) && !r("~1.2").matches(&v("1.3.0")));
          assert!(r("~1").matches(&v("1.9.9")) && !r("~1").matches(&v("2.0.0")));
      }
      #[test]
      fn npm_semver_x_range() {
          assert!(r("1.x").matches(&v("1.9.9")) && !r("1.x").matches(&v("2.0.0")));
          assert!(r("1.2.x").matches(&v("1.2.9")) && !r("1.2.x").matches(&v("1.3.0")));
          assert!(r("1").matches(&v("1.5.5")) && r("*").matches(&v("9.9.9")));
      }
      #[test]
      fn npm_semver_hyphen_and_ops() {
          assert!(r("1.2.3 - 2.3.4").matches(&v("2.3.4")) && !r("1.2.3 - 2.3.4").matches(&v("2.3.5")));
          assert!(r("1.2 - 2.3").matches(&v("2.3.9")) && !r("1.2 - 2.3").matches(&v("2.4.0")));
          assert!(r(">=1.2.3 <2.0.0").matches(&v("1.5.0")) && !r(">=1.2.3 <2.0.0").matches(&v("2.0.0")));
          assert!(r("1.2.3").matches(&v("1.2.3")) && !r("1.2.3").matches(&v("1.2.4")));
      }
      #[test]
      fn npm_semver_union() {
          let u = r("^1.0.0 || ^2.0.0");
          assert!(u.matches(&v("1.5.0")) && u.matches(&v("2.5.0")) && !u.matches(&v("3.0.0")));
      }
      #[test]
      fn npm_semver_prerelease_ordering() {
          assert!(v("1.0.0") > v("1.0.0-alpha") && v("1.0.0-alpha") < v("1.0.0-beta"));
          assert!(v("1.0.0-alpha.1") < v("1.0.0-alpha.2") && v("1.0.0-alpha") < v("1.0.0-alpha.1"));
          assert!(v("1.0.0-1") < v("1.0.0-alpha")); // 数字 < 字母
      }
      #[test]
      fn npm_semver_prerelease_inclusion_rule() {
          // 关键：^1.2.3 不匹配 2.0.0-alpha（上界 comparator 无预发布 / tuple 不同）
          assert!(!r("^1.2.3").matches(&v("2.0.0-alpha")));
          // 只有 comparator 自身带预发布且 tuple 相同才纳入
          assert!(r(">=1.2.3-beta.1 <2.0.0").matches(&v("1.2.3-beta.2")));
          assert!(!r(">=1.2.3-beta.1 <2.0.0").matches(&v("1.9.0-rc.1"))); // tuple 不同 → 排除
      }
  }
  ```
  `solver/mod.rs`：`pub mod npm_semver;`；`lib.rs`：`pub mod solver;`
- [ ] **Verify RED**：`cargo nextest run -p wjsm-pm -E 'test(npm_semver)'`。
- [ ] **完整实现**：补齐上面注释处的四个展开函数（严格按 node-semver）：
  - `split_hyphen(s)`：识别 ` - ` 分隔（两侧各是一个 partial），返回 `(lo_str, hi_str)`。
  - `expand_lower_bound(partial)`：`X`/缺省段 → `>=0.0.0`；`1` → `>=1.0.0`；`1.2` → `>=1.2.0`；`1.2.3` → `>=1.2.3`。
  - `expand_upper_bound(partial)`：`1` → `<2.0.0`；`1.2` → `<1.3.0`；`1.2.3` → `<=1.2.3`；含 `X` 段同理向上补齐。
  - `parse_single(tok)`：依次匹配前缀
    - `^`：`^1.2.3`→`>=1.2.3 <2.0.0`；`^0.2.3`→`>=0.2.3 <0.3.0`；`^0.0.3`→`>=0.0.3 <0.0.4`；`^1`/`^1.x`→`>=1.0.0 <2.0.0`；`^0`→`>=0.0.0 <1.0.0`；`^0.0`→`>=0.0.0 <0.1.0`。含 `x` 段的 `^` 按"缺省段视为 0、上界由最高非通配段决定"。
    - `~`（指定 minor → 锁 minor；仅 major → 锁 major）：`~1.2.3`→`>=1.2.3 <1.3.0`；`~1.2`→`>=1.2.0 <1.3.0`（上界 `<{major}.{minor+1}.0`）；`~1`→`>=1.0.0 <2.0.0`（上界 `<{major+1}.0.0`）；`~0.2.3`→`>=0.2.3 <0.3.0`。
    - `>=`/`>`/`<=`/`<`/`=`：解析运算符后对 partial 补齐为具体 SemVer（缺省段补 0），生成单个 `Comparator`。
    - x-range / 精确：`1.2.x`→`>=1.2.0 <1.3.0`；`1.x`→`>=1.0.0 <2.0.0`；`1.2.3`→`=1.2.3`（`Op::Eq`）。
  - 所有展开产出的 comparator 中，**上界 `<X.Y.Z`（无预发布）保持无预发布**，从而 `set_matches` 的预发布包含规则正确排除跨版本预发布。
- [ ] **Verify GREEN**：全部 `npm_semver_*` 测试通过（含 `prerelease_inclusion_rule`）。
- [ ] **Commit**：`git add -A && git commit -m "feat(wjsm-pm): npm 精确 SemVer 区间语义（node-semver 全形态 + 预发布包含规则）"`

## 任务 2.2：registry client（packument + SSRI/shasum + tarball）

Files:
- 创建 `crates/wjsm-pm/src/registry/{mod,packument,tarball,npmrc}.rs`
- 创建 `crates/wjsm-pm/tests/mock_registry.rs`
- 修改 `crates/wjsm-pm/src/lib.rs`

Why: 从 registry 拉 packument 元数据、按 npm integrity 语义校验 tarball、解包 tgz。内置离线 mock registry 保证测试确定。

Impact/Compatibility: 纯新增。两条独立安全边界：① tarball 字节必须匹配 SSRI 或旧 `dist.shasum`；② 解包必须防路径逃逸 + 拒绝链接条目。SSRI 证明字节一致，不证明 tar 内容安全。

Verification: `cargo nextest run -p wjsm-pm -E 'test(registry) | test(mock_registry)'`

Steps:

- [ ] **写失败测试**。`registry/tarball.rs`：
  - `verify_integrity(bytes, integrity: Option<&str>, shasum: Option<&str>)`。
  - SSRI parser 支持空白分隔多个 token、token 末尾 `?metadata`、算法优先级 `sha512 > sha384 > sha256 > sha1`；`sha512/384/256` 用 `sha2`，旧 `sha1` 用 `sha1` crate。若无 `integrity` 但有 `shasum`，按十六进制 SHA-1 校验并把 lockfile 记录为可复核的 legacy source 字段；两者都无则拒绝入库。
  - 测试：`registry_ssri_accepts_multi_token_strongest`、`registry_ssri_rejects_tamper`、`registry_legacy_shasum_supported`。
  - `extract_tgz(bytes, dest)`：`dest` 先 `create_dir_all` 后 `canonicalize`；仅允许 regular/directory 与必要扩展头；符号链接、硬链接、设备、FIFO 一律拒绝；strip 顶层 `package/`；逐组件拒绝 `..`、绝对路径、Windows prefix；写入前再次确认 `out.starts_with(dest_abs)`。测试 path traversal、symlink/hardlink、正常 package 前缀。
- [ ] **写失败测试**。`registry/packument.rs`：
  - `Packument { versions, dist_tags }`。
  - `VersionMeta` 解析 `dependencies`、`devDependencies`、`peerDependencies`、`peerDependenciesMeta`、`optionalDependencies`、`bin`、`scripts`、`os`、`cpu`、`libc`、`engines`、`dist`。
  - `Dist { tarball: String, integrity: Option<String>, shasum: Option<String> }`。
  - optionalDependencies 与 dependencies 同名时在归一化阶段覆盖为 optional 边；`peerDependenciesMeta.<name>.optional=true` 进入 optional peer，而非硬 peer。
- [ ] **写失败测试**。`registry/npmrc.rs`：解析 default registry、scope registry、`//host/:_authToken`，并支持 `${ENV}` 环境变量替换；scope registry 参与 `PackageLocator.registry`。
- [ ] **Verify RED**：`cargo nextest run -p wjsm-pm -E 'test(registry)'`。
- [ ] **完整实现**：`registry/mod.rs` 提供 async `fetch_packument`/`fetch_tarball`；mock registry 端到端验证 fetch→integrity→extract→`Store::add_package_from_dir(locator, …)`。
- [ ] **Verify GREEN**：`cargo nextest run -p wjsm-pm -E 'test(registry) | test(mock_registry)'` 通过。
- [ ] **Commit**：`git add -A && git commit -m "feat(wjsm-pm): registry client（packument/SSRI/shasum/tarball/npmrc）"`

## 任务 2.3：PubGrub DependencyProvider + npm instance/peer 求解

Files:
- 创建 `crates/wjsm-pm/src/solver/{provider,duplication,explain,package_spec}.rs`
- 修改 `crates/wjsm-pm/src/solver/mod.rs`

Why: PubGrub 内核求去重最大化解；npm 适配层负责 dependency source 分类、instance-splitting、peer host 环境传播、optional 跳过与解释。

**求解语义（明确定义）**：
- `PackageSpec` 分类层先把 package.json dependency value 分为：`RegistryRange`、`RegistryTag`、`Alias(npm:<real>@<range/tag>)`、`Workspace`/`FileLink`、`Unsupported(Git|RemoteTarball|Other)`。Unsupported 在读取 manifest 阶段报清楚错误；workspace/file 生成 local source instance，不进入 registry fetch。
- `dependencies` 为硬约束；`optionalDependencies` 覆盖同名 `dependencies`，作为 optional 边在主解完成后尝试纳入，失败/缺失不影响主解；platform-incompatible optional（`os/cpu/libc` 不匹配）直接跳过。
- `peerDependencies` 不是全局单例，也不是普通子依赖。peer 约束向当前 instance 的 host/ancestor 环境传播并形成 `PeerSet`；同一 `name@version` 可因 `PeerSet` 不同产生不同 `InstanceId`。只有同一 peer-set 内约束不可满足才失败。`peerDependenciesMeta.optional=true` 不进入硬 peer set，缺失不失败。
- instance-splitting 以 dependent 子树/peer-set 为边界；普通 dependency 的同名包无单版本交集时可分裂；peer binding 按宿主环境解析，不把两个无关子树误判为全局冲突。

Impact/Compatibility: 纯新增。

Verification: `cargo nextest run -p wjsm-pm -E 'test(solver)'`

Steps:

- [ ] **Spike 首步：核对 pubgrub 0.4 API**。用 `cargo doc -p pubgrub --no-deps`（不打开浏览器）或读取本地 registry 源确认 `DependencyProvider` 关联类型/方法，写进 `provider.rs` 顶部注释。
- [ ] **写失败测试**。`package_spec.rs` 覆盖 semver range、dist-tag、`npm:` alias、`workspace:*`、`file:../pkg`、git/remote tarball unsupported 诊断。
- [ ] **定义同步 `PackageIndex` 与 `ResolvedGraph`**：`PackageIndex` 提供 versions/dependencies/peers/optional/platform metadata 的同步读；`RegistryIndex` 只读 install 预取好的 packuments；`ResolvedInstance { instance_id, source_id, name, version, peer_set, deps: Vec<(dep_name, instance_id)> }`。
- [ ] **写失败测试**：
  - `solver_single_version_dedup`。
  - `solver_instance_split_multi_version`。
  - `solver_peer_sets_allow_same_plugin_under_different_hosts`（两个子树各有自己的 host peer，必须可解）。
  - `solver_peer_conflict_in_same_host_explains`。
  - `solver_optional_dep_missing_is_skipped`。
  - `solver_optional_overrides_dependency`。
  - `solver_optional_peer_missing_is_skipped`。
  - `solver_alias_resolves_real_package_but_keeps_requested_name_edge`。
- [ ] **Verify RED**：`cargo nextest run -p wjsm-pm -E 'test(solver)'`。
- [ ] **完整实现**：实现 PubGrub provider、peer-set expansion、instance id 稳定生成、optional pass、`DefaultStringReporter`/自定义 reporter 错误解释。
- [ ] **Verify GREEN**：上述 solver/package_spec 测试通过。
- [ ] **Commit**：`git add -A && git commit -m "feat(wjsm-pm): PubGrub solver（instance splitting + peer-set + optional/source spec）"`

---

# 阶段 P3：install / lockfile / CLI + 编译器接入

## 任务 3.1：自有 lockfile（wjsm-lock.toml）+ 迁移读取

Files:
- 创建 `crates/wjsm-pm/src/lockfile/{mod,wjsm_lock,migrate}.rs`
- 修改 `crates/wjsm-pm/src/lib.rs`

Why: 确定性 lockfile 记录解析实例图 + source locator + integrity + lazy materialization 所需 resolved URL；迁移读取 package-lock/pnpm-lock/yarn.lock/bun.lock 无缝接管存量项目。

Impact/Compatibility: 纯新增。不删除原生态 lockfile（除非 `--prune`）。lockfile 不再用 `name@version` 表达边，所有边指向 `InstanceId`。

Verification: `cargo nextest run -p wjsm-pm -E 'test(lockfile)'`

Steps:

- [ ] **写失败测试**。`lockfile/wjsm_lock.rs` 定义：
  - `WjsmLock { lock_version, compiler_version, root_deps: Vec<LockedEdge>, packages: Vec<LockedPackage> }`。
  - `LockedEdge { name: String, instance_id: String }`。
  - `LockedPackage { instance_id, source_id_hex, name, version, registry, resolved, integrity: Option<String>, shasum: Option<String>, manifest_hash_hex, peer_set: Vec<LockedPeer>, deps: Vec<LockedEdge>, optional_deps: Vec<LockedEdge>, bin: BTreeMap<String,String>, has_install_script: bool, scripts_trusted: bool }`。
  - `to_toml()` 稳定排序：packages 按 `instance_id`，边按 `name,instance_id`，peer_set 按 name。
  - 测试 `lockfile_deterministic_roundtrip_with_duplicate_name_version_instances`：同一 `name@version` 两个不同 `instance_id/peer_set` 必须都保留。
- [ ] **写失败测试**。`lockfile/migrate.rs` 输出 `LockHints { pins: BTreeMap<String, Vec<PinHint>> }`，`PinHint` 至少含 `name/version/resolved/integrity/shasum`，供 solver prioritization 与 lazy 补齐；不直接生成最终 wjsm lock。覆盖 package-lock v3、pnpm-lock、yarn classic v1、yarn berry v2、bun JSONC；`bun.lockb` 明确报错。
- [ ] **Verify RED**：`cargo nextest run -p wjsm-pm -E 'test(lockfile)'`。
- [ ] **完整实现**：补齐四格式迁移函数与 deterministic serialization。
- [ ] **Verify GREEN**：全部 lockfile 测试通过。
- [ ] **Commit**：`git add -A && git commit -m "feat(wjsm-pm): wjsm-lock.toml（instance/source locator）+ 迁移读取"`

## 任务 3.2：install 编排 + CasVfs/PnpOverlay

Files:
- 创建 `crates/wjsm-pm/src/store/{vfs,overlay}.rs`
- 修改 `crates/wjsm-pm/src/lib.rs`（`install` / `ensure_materialized` 公共 API）

Why: install 编排 = 读 package.json → package spec 分类 → async 预取 → solve → materialize → 写 CAS → 写 lockfile。CasVfs/PnpOverlay 实现 module 侧 trait，把 lockfile instance 图呈现为虚拟树供编译器读。

Impact/Compatibility: 纯新增。CasVfs 读路径全程零 node_modules 物化；虚拟路径编码 `InstanceId`，内容读取再映射到 `source_id`。

Verification: `cargo nextest run -p wjsm-pm -E 'test(install) | test(cas_vfs) | test(pnp_overlay)'`

Steps:

- [ ] **写失败测试**。`store/vfs.rs`：
  - 虚拟路径 `<vroot>/<encoded_instance_id>/<rel...>`；`encode_instance_dir(instance_id)` percent-encode `%`、`/`、`@`、`#` 等分隔字符，保证单路径组件。
  - `CasVfs` 持 `Arc<Store>`、`vroot`、`instance_to_source: HashMap<InstanceId, SourceId>`；`split(path) -> (instance_id, rel)`；`read_to_string`/`is_file`/`is_dir`/`read_package_json` 均经 `Store::read_package_file(source_id, rel)`。
  - `RoutingVfs` 按 `vroot` 前缀分派虚拟 CAS 与真实 FS。
  - 测试：scoped 包、同 `name@version` 两实例不同边但同 source 内容、真实入口 + 虚拟依赖混合读取。
- [ ] **写失败测试**。`store/overlay.rs`：
  - `PnpOverlay` 从 `WjsmLock` 构造 `root_deps: dep_name -> instance_id`、`edges: owner_instance_id -> dep_name -> target_instance_id`、`local_members: instance_id/name -> real_path`。
  - `owner_of(referrer)` 从虚拟路径拿 `instance_id`；项目真实路径走 root_deps；workspace 本地成员返回真实路径。
  - 测试：root edge、package internal edge、子路径 specifier、duplicate `name@version` 不串边、workspace local resolves to disk。
- [ ] **写失败测试**。install 编排：
  - package spec 分类：registry range/tag/alias、workspace/file link、unsupported git/remote tarball 诊断。
  - workspace：每个 member 是合成 local package instance，root 依赖 member instances；**不把所有成员外部 deps 并成单一 `$root`**，否则丢 dependent 上下文。
  - async 预取 packument 用 `JoinSet` + `Semaphore`，只拉元数据。
  - 同步 solve 生成 `ResolvedGraph`。
  - materialize：对每个 registry `PackageLocator` 先由 registry/resolved/integrity_or_shasum 计算 `source_id` 并检查 `store.has_package(source_id)`；缺失则 fetch tarball→verify integrity/shasum→extract→（若 lifecycle 授权则隔离执行并重扫产物）→`add_package_from_dir`，写入时计算并记录 `manifest_hash`。
  - `ensure_materialized(project_dir, store)`：有 lockfile 时按 lockfile resolved/integrity/shasum/source_id/manifest_hash 补齐缺失 store；无 lockfile 但 package.json 有 deps 时执行 lazy install 并写 lockfile。
- [ ] **Verify RED**：`cargo nextest run -p wjsm-pm -E 'test(cas_vfs) | test(pnp_overlay) | test(install)'`。
- [ ] **完整实现**：完成 install / ensure_materialized / CasVfs / PnpOverlay。
- [ ] **Verify GREEN**：测试通过。
- [ ] **Commit**：`git add -A && git commit -m "feat(wjsm-pm): install 编排 + instance PnP overlay + lazy materialization"`

## 任务 3.3：CLI 子命令 install/add/remove

Files:
- 修改 `crates/wjsm-cli/src/cli_args.rs`（新增子命令）
- 修改 `crates/wjsm-cli/src/lib.rs`（dispatch）
- 创建 `crates/wjsm-cli/src/pm_commands.rs`
- 修改 `crates/wjsm-cli/Cargo.toml`（加 `wjsm-pm` 依赖）

Why: 暴露 `wjsm install/add/remove`，承接 `npm install`；同时在 CLI 首次依赖 `wjsm-pm` 后接入 P1.5b 的 `cache gc` store 维护入口。

Impact/Compatibility: 新增子命令，不改现有命令。`wjsm-cli` 首次依赖 `wjsm-pm`。

Verification: `cargo build -p wjsm-cli && cargo run -- install --help && cargo run -- cache gc --help`

Steps:

- [ ] **写失败测试**。`pm_commands.rs`：
  ```rust
  // 包管理 CLI 命令：install/add/remove
  use anyhow::Result;
  use std::path::Path;

  pub fn cmd_install(project_dir: &Path, allow_scripts: &[String]) -> Result<()> {
      let store_root = default_store_root()?;
      let store = wjsm_pm::store::Store::open(&store_root)?;
      // 与现有 CLI 一致用 Builder::new_multi_thread（见 lib.rs:306/2153），非裸 Runtime::new。
      let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
      let opts = wjsm_pm::InstallOptions { allow_scripts: allow_scripts.to_vec() };
      let lock = rt.block_on(wjsm_pm::install_with_options(project_dir, &store, opts))?;
      std::fs::write(project_dir.join("wjsm-lock.toml"), lock.to_toml())?;
      println!("已安装 {} 个包，无 node_modules", lock.packages.len());
      Ok(())
  }

  fn store_root_from_env(get_env: impl Fn(&str) -> Option<String>) -> Result<std::path::PathBuf> {
      if let Some(dir) = get_env("WJSM_STORE_DIR") {
          return Ok(dir.into());
      }
      let home = get_env("HOME").ok_or_else(|| anyhow::anyhow!("无 HOME"))?;
      Ok(std::path::Path::new(&home).join(".wjsm").join("store"))
  }

  pub fn default_store_root() -> Result<std::path::PathBuf> {
      store_root_from_env(|k| std::env::var(k).ok())
  }

  #[cfg(test)]
  mod tests {
      use super::*;
      #[test]
      fn pm_store_root_respects_env_accessor() {
          let got = store_root_from_env(|k| (k == "WJSM_STORE_DIR").then(|| "/tmp/wjsm_test_store".to_string())).unwrap();
          assert_eq!(got, std::path::PathBuf::from("/tmp/wjsm_test_store"));
      }
  }
  ```
  `cli_args.rs` `Commands` enum 追加：
  ```rust
  /// Install dependencies from package.json (npm install 等价)
  Install {
      /// Project directory
      #[arg(default_value = ".")]
      dir: std::path::PathBuf,
      /// Allow dependency lifecycle scripts for comma-separated packages or `all`
      #[arg(long = "allow-scripts", value_delimiter = ',')]
      allow_scripts: Vec<String>,
  },
  /// Add a dependency (npm install <pkg> 等价)
  Add { pkg: String, #[arg(default_value = ".")] dir: std::path::PathBuf, #[arg(long = "allow-scripts", value_delimiter = ',')] allow_scripts: Vec<String> },
  /// Remove a dependency (npm uninstall 等价)
  Remove { pkg: String, #[arg(default_value = ".")] dir: std::path::PathBuf },
  ```
  `lib.rs` dispatch（**`execute`（L411）`match cli.command` 内**，**不是** `main_entry`）追加。**每臂必须返回 `Result<ExitCode>`**（与既有臂 `Commands::Cache { ref command } => cmd_cache(command)` 一致；`execute` 签名 `-> Result<ExitCode>`，末尾统一由 `main_entry` 的 `match execute(cli)` 收敛错误到退出码）——故**不得**写成 `.map(...).unwrap_or(ExitCode::FAILURE)` 返回裸 `ExitCode`（类型不符、且吞掉错误信息，`main_entry` 的 `Err(e) => eprintln!("Error: {:#}", e)` 是唯一错误打印点）：
  ```rust
  Commands::Install { ref dir, ref allow_scripts } => pm_commands::cmd_install(dir, allow_scripts).map(|()| ExitCode::from(EXIT_SUCCESS)),
  Commands::Add { ref pkg, ref dir, ref allow_scripts } => pm_commands::cmd_add(pkg, dir, allow_scripts).map(|()| ExitCode::from(EXIT_SUCCESS)),
  Commands::Remove { ref pkg, ref dir } => pm_commands::cmd_remove(pkg, dir).map(|()| ExitCode::from(EXIT_SUCCESS)),
  ```
  （`cmd_install`/`cmd_add`/`cmd_remove` 返回 `Result<()>`，`.map(|()| ExitCode::from(EXIT_SUCCESS))` 提升为 `Result<ExitCode>`；错误经 `?`/`Result` 冒泡到 `main_entry` 统一打印。`EXIT_SUCCESS` 是 lib.rs:38 既有常量。）
  `lib.rs` 顶部 `mod pm_commands;`；`Cargo.toml` 加 `wjsm-pm = { path = "../wjsm-pm" }`。同一任务把 `CacheCommand::Gc {}` / `cmd_cache_gc()` 接入现有 `Cache` dispatch，调用 `wjsm_pm::store::gc(&store)` 并打印 reclaimed 统计。
- [ ] **Verify RED**：`cargo build -p wjsm-cli` 预期先因 `cmd_add`/`cmd_remove`/`cmd_cache_gc` 未定义失败。
- [ ] **完整实现**：补 `cmd_add` / `cmd_remove` / `cmd_cache_gc`。**`cmd_add` 的版本来源必须明确**（spec 行 373 `wjsm add <pkg>[@range]`）——解析 `pkg` 参数为 `(name, requested_range)`：
  - **`pkg` 含 `@<range>` 后缀**（如 `lodash@^4`、`@babel/core@^7`；scoped 包首字符 `@` 不算分隔符——用「最后一个 `@` 且其前非空且不在首位」判定 range 分隔）→ 直接用该 range 写入 package.json `dependencies`。
  - **`pkg` 无版本**（如 `lodash`）→ `fetch_packument(name)` 取 `dist_tags.latest`（经上一步新增的 `Packument::latest_version()`），写入 package.json `dependencies` 为 `^<latest>`（npm `save-prefix` 默认 `^`；无 `dist-tags.latest` 时改取 `versions` 中最高**非预发布**版本，仍无则报错）。
  写回 package.json（保序、`serde_json` 处理 `dependencies` 对象，无则新建）后调 `cmd_install(dir, allow_scripts)` 触发 solve+下载+写 lockfile。`cmd_remove`：从 package.json `dependencies`/`devDependencies`/`optionalDependencies`/`peerDependencies` 全部删除 `pkg` 键（四处均不存在则报错），写回后调 `cmd_install(dir, &[])` 重解析（移除包不再被根依赖引用 → 新 lockfile 不含它；其 CAS blob 由 `store gc` 回收，install 不主动删 store）。新增测试：`pm_cmd_add_parses_pkg_range`（覆盖 `lodash`/`lodash@^4`/`@scope/x`/`@scope/x@^2` 四形态的 `(name, range)` 拆分）、`pm_cmd_allow_scripts_parses_pkg_and_all`、`pm_cmd_remove_deletes_all_dependency_sections`、`pm_cache_gc_dispatches_store_gc`。
- [ ] **Verify GREEN**：`cargo build -p wjsm-cli && cargo run -- install --help && cargo run -- install --allow-scripts=all --help && cargo run -- cache gc --help && cargo nextest run -p wjsm-cli -E 'test(pm_)'`。
- [ ] **Commit**：`git add -A && git commit -m "feat(cli): wjsm install/add/remove 子命令"`

## 任务 3.4：编译器与 runtime loader 接入 CAS（run/build 惰性补齐）+ 集成测试

Files:
- 修改 `crates/wjsm-cli/src/lib.rs`（run/build 命令注入 RoutingVfs/PnpOverlay/Boundary；新增 provider-aware in-process 测试 helper 或让测试走 CLI `execute` 路径）
- 修改 `crates/wjsm-cli/src/runtime_loader.rs`（#312 runtime loader 持有 Vfs/Overlay/Boundary，源码读取、格式探测、package boundary 探测全部走 Vfs）
- 创建 `crates/wjsm-cli/tests/pm_run_from_cas.rs`（CLI 集成测试）

**命名约定澄清**：pm 场景不走 `tests/fixture_runner.rs` 的 `.expected` 快照 harness；端到端一律用 crate 内 `#[test]` 集成测试，测试函数名统一以 `pm_` 前缀。

Why: 让 `wjsm run/build` 在 lockfile 存在或 package.json 有依赖时，从 CAS 编译执行依赖；同时让 computed `require()`、dynamic `import()`、`require.resolve()` 的运行期加载也能使用同一解析覆盖层。

Impact/Compatibility: 无依赖项目仍走默认 FS 路径。依赖项目在 run/build 前调用 `ensure_materialized`；仅当确认存在依赖图时注入 CAS providers。

Verification: `cargo nextest run -p wjsm-cli -E 'test(pm_run_from_cas) | test(pm_runtime_loader)'`

Steps:

- [ ] **写失败测试**。`pm_run_from_cas`：mock registry + 临时项目，`wjsm install` 后删除 store 中对应 blob，只保留 `wjsm-lock.toml`，再通过 CLI `Run` 执行路径（`execute(Cli { command: Run, ... })` 测试 helper 或 `CARGO_BIN_EXE_wjsm` 子进程，显式传入临时 `WJSM_STORE_DIR`）运行；断言 run/build 触发 lazy 补齐、stdout 正确、项目无 `node_modules`。**不使用**现有 `run_file_in_process`，因为它当前固定 `ResolutionOptions::default()`，无法注入 store/Vfs/Overlay。
- [ ] **写失败测试**。`pm_run_without_lockfile_lazy_installs`：仅有 package.json dependencies，无 lockfile，首次 `wjsm run` 自动 solve/materialize/write lockfile 后运行成功。
- [ ] **写失败测试**。`pm_runtime_loader_uses_cas_for_dynamic_require`：依赖包内部使用 computed `require(name)` / `import(expr)` / `require.resolve()`，确认 runtime loader 不是直读真实 FS。
- [ ] **写失败测试**。`pm_runtime_loader_detects_ambiguous_cjs_from_cas`：CAS 依赖包提供无真实磁盘 package.json 的 ambiguous `.js` CommonJS 文件；`detect_runtime_file_format` 与 `has_nearest_package_manifest` 必须通过 Vfs/Boundary 判定格式，不能因 `Path::is_file`/`std::fs::read_to_string` 失败而误判 ESM。
- [ ] **Verify RED**：`cargo nextest run -p wjsm-cli -E 'test(pm_run_from_cas) | test(pm_runtime_loader)'` 失败。
- [ ] **完整实现**：
  - `cmd_run`/`cmd_build` 前置调用 `wjsm_pm::ensure_materialized(project_dir, store)`；返回 lockfile + provider bundle。
  - 构造 `RoutingVfs`、`PnpOverlay`、`ResolutionBoundary(project_dir + vroot + workspace member dirs)`，经 `ModuleBundler::with_providers(..., boundary)` 注入。
  - `runtime_loader.rs` 持 `Arc<dyn Vfs>`、overlay、boundary；读取 runtime module source 走 `vfs.read_to_string`；runtime specifier resolution 调 `resolve_runtime_specifier_with_providers`。
  - `detect_runtime_file_format` 改为接收/持有 Vfs，用 `vfs.read_to_string` 做源码形态探测；`has_nearest_package_manifest` 改用 provider-aware `read_package_json`/`is_file`，并受 `ResolutionBoundary` 限制。
  - 测试入口若仍需 in-process，新增 `run_file_in_process_with_pm_options(input, store_root, project_dir, registry)`；否则所有 pm 集成测试走 CLI execute/子进程路径，现有 fixture helper 保持 FS 默认路径。
- [ ] **Verify GREEN**：上述测试通过，项目目录无 `node_modules`。
- [ ] **Commit**：`git add -A && git commit -m "feat(cli): run/build 与 runtime loader 惰性接入 CAS"`

---

# 阶段 P4：task / x / workspaces

## 任务 4.1：wjsm task（npm scripts shell + virtual bin PATH）

Files:
- 创建 `crates/wjsm-pm/src/scripts/mod.rs`
- 修改 `crates/wjsm-cli/src/cli_args.rs` / `lib.rs` / `pm_commands.rs`

Why: `wjsm task <name>` 承接 `npm run <name>`：scripts 是 shell 命令串，且依赖 bin 必须从 CAS/lockfile 暴露到 PATH。

Impact/Compatibility: 新增子命令。`run` 遇 script 名且文件不存在时提示 `did you mean 'wjsm task <name>'?`，不改行为。

Verification: `cargo nextest run -p wjsm-pm -E 'test(scripts)' && cargo nextest run -p wjsm-cli -E 'test(pm_task)'`

Steps:

- [ ] **写失败测试**。`scripts/mod.rs`：`resolve_script_sequence(pkg_json, name)` 返回 `[pre<name>, <name>, post<name>]`；`script_shell_command(cmd)` 在 Unix 生成 `sh -c <cmd>`，Windows 生成 `cmd /C <cmd>`；测试 `echo pre && echo build` 这类 shell 语法不能被当作 executable 直跑。
- [ ] **写失败测试**。`make_virtual_bin_dir(lock, store, project_context)`：为 lockfile 中依赖 `bin` 字段生成临时 shim 目录并 prepend PATH。shim 调用 wjsm 内部 runner（或 `wjsm run-bin --instance <id> --bin <name>`）从 CAS 虚拟目标执行 JS bin；测试 script 中 `demo-bin --flag` 可解析到 CAS 依赖 bin。
- [ ] **写失败测试**。dependency lifecycle 授权：默认跳过 `preinstall/install/postinstall/prepare` 并输出 warning，lockfile 标记 `has_install_script=true,scripts_trusted=false`；`trustedDependencies` 或 `--allow-scripts=<pkg|all>` 时在隔离临时目录执行，执行后重扫产物并写入 CAS；未授权包不得静默执行脚本。测试覆盖默认跳过、授权执行、lockfile 标记三态。
- [ ] **Verify RED**：`cargo nextest run -p wjsm-pm -E 'test(scripts)'`。
- [ ] **完整实现**：CLI `Task { name, dir }` 调用 `ensure_materialized`，创建 virtual bin dir，设置 `PATH`、`npm_lifecycle_event`、必要 `npm_package_*` 环境变量，按 shell 顺序执行 pre/main/post；失败返回对应 exit status。
- [ ] **Verify GREEN**：pm_task 集成测试通过 + `cargo run -- task --help`。
- [ ] **Commit**：`git add -A && git commit -m "feat: wjsm task（shell scripts + CAS virtual bin PATH）"`

## 任务 4.2：wjsm x（npx 等价）+ workspaces

Files:
- 修改 `crates/wjsm-pm/src/scripts/mod.rs`（bin 解析 + virtual runner 复用）
- 创建 `crates/wjsm-pm/src/workspace.rs`
- 修改 CLI（`X` 子命令 + workspace 发现）

Why: `wjsm x <pkg>` 临时拉取执行包 bin；workspaces 支持 monorepo 本地包链接 + 根 lockfile，且保留每个 member 的依赖上下文。

Impact/Compatibility: 新增。workspace 本地包不进 CAS，以成员真实磁盘目录经 `PnpOverlay.local_members` 接入解析覆盖层。

Verification: `cargo nextest run -p wjsm-pm -E 'test(workspace) | test(bin)' && cargo nextest run -p wjsm-cli -E 'test(pm_x)'`

Steps:

- [ ] **写失败测试**。`workspace.rs`：支持 `workspaces` 数组与 `{ packages }` 形式，至少覆盖 `packages/*` 与显式路径；`workspace_link_map` 读取每个 member package.json `name`，返回 name→真实路径；拒绝重复 member name。
- [ ] **写失败测试**。workspace solve：每个 member 建模为 `$ws:<name>` local instance，root 依赖这些 member；每个 member 保留自身 dependencies/devDependencies/peer context。测试两个 member 分别依赖 `c@^1` 与 `c@^2`，lockfile 产生两个外部 instances，不能把外部 deps 并成单一 root 约束。
- [ ] **写失败测试**。`resolve_package_bin` 处理 string/object bin、缺 bin、多个 bin 需显式选择；`cmd_x` 先生成临时 lock/context，materialize 目标包依赖闭包，再通过与 task 相同的 virtual bin runner 执行，bin 内 import 依赖从 CAS 解析。
- [ ] **Verify RED**：`cargo nextest run -p wjsm-pm -E 'test(workspace) | test(bin) | test(pnp_overlay_workspace)'`。
- [ ] **完整实现**：补 workspace discovery/link map、workspace-aware install、`X { pkg, args }`、临时 lock + virtual bin runner。
- [ ] **Verify GREEN**：上述测试通过。
- [ ] **Commit**：`git add -A && git commit -m "feat: wjsm x（virtual bin runner）+ workspaces（member instance 图）"`

# 阶段 P5：分层编译产物缓存（前置 #312）

> 前置检查：确认 issue #312 已合并（验证 `crates/wjsm-cli/src/runtime_loader.rs` 与 runtime module loading 设计/计划中定义的分离编译 loader 已存在，且相关测试通过）。若未合并，暂停 P5，先完成 #312。

## 任务 5.1：可重定位 IR — 单包 lower + 重定位表

Files:
- 创建 `crates/wjsm-semantic/src/relocatable/{mod,lower_one,relocate}.rs`
- 修改 `crates/wjsm-semantic/src/lib.rs`（导出）

Why: L1 跨项目复用的命门是 scope id 项目无关化。单独 lower 一个包 → scope id 局部化的模块 IR 片段 + 重定位表（scope 基址、常量偏移、字符串偏移、未解析 import 符号）。

**关键更正（已逐行核对 lowerer_modules.rs / scope.rs）**：现有 `lower_modules` 的 scope 布局**不是**「模块 scope 从 0 起」，且**不是**「BFS 交错 push」——已核实真实机制是**两趟顺序分配**：
- 根作用域 `$0`（`ScopeTree::new` 硬编码 id=0 的 Function 根）被全局对象 `$0.$global`（`emit_global_constants`，**L386**；`$0.$global` 的 `StoreVar` 在其内 L452）与 hoisted var 占用。
- **趟 1 = 预声明**：`predeclare_module_exports`（L144）**顺序** `for module in modules` 循环——对每个模块 `push_scope(Block)`（L157）得模块顶层 Block scope、`predeclare_stmts` 递归为嵌套词法块/函数 push 子 scope，末尾 `pop_scope`。模块**顺序**由调用方传入的 `Vec<ModuleLoweringInput>`（`wjsm-module` 图按拓扑/BFS 排）决定，但 predeclare 本身是直线循环、**非** BFS 交错。
- **趟 2 = 降级**：`lower_module_bodies`（L643）再次**顺序**遍历全部模块，`enter_scope`（scope.rs L94，复用已存在 id、不分配）回到该模块顶层 scope，再在**走查中** push **全新**的函数/块 scope（`lowerer_core.rs:221` 等）。
- `pop_scope`（scope.rs L99）**只移动 `current` 到 parent，不截断 `arenas`**——scope id 单调递增、永不复用。

**对重定位的致命后果**：因两趟都遍历**全部**模块，单个模块的全程 scope id 被拆成**两段不连续簇**（该模块趟-1 的预声明 id 段 + 趟-2 的走查 id 段），中间夹着其他模块的 id。故**当模块数 N≥2 时，「整体加单一 `base-1` 偏移」把局部 IR（id 从 1 连续）映射回全程 id 的模型机械上不成立**——单偏移只在 N=1（L2-a 单包）成立。因此原计划断言 `min_scope_id() == 0` 与现状冲突（0 是全局根），且「单偏移重定位」需订正（见任务 5.2 的 remap 表方案）。修正：`lower_one` 产出的模块局部 scope 以**约定基址 `LOCAL_SCOPE_BASE`（=1，避开全局根 0；标准库 `ScopeTree` 根即 0，模块首 Block 自然落 1）**起始；`Relocations.scope_refs` 记录**每处** `${scope_id}.{name}` 引用的局部 id（**逐引用重映射**，非单偏移），链接期由 remap 表映射到全程 id。`Relocations` 另显式区分「指向本模块 scope 的引用」（重映射）与「指向全局根 `$0.$global` 的引用」（链接期固定映射到全局 0，不参与重映射）。

Impact/Compatibility: 新增路径；现有 `lower_modules` 整体路径不变。产出必须能经 `link`（任务 5.2）重定位回等价全局 IR。

Verification: `cargo nextest run -p wjsm-semantic -E 'test(relocatable_ir)'`

Steps:

- [ ] **Spike 首步：核对 scope 与引用种类清单**。读 `lowerer_modules.rs` 的 `predeclare_module_exports`/`emit_global_constants`/`create_namespace_objects`/`process_import_aliases`，列出一个模块局部 IR 会出现的**全部位置相关引用种类**（写进 `relocate.rs` 顶部注释作为重定位表 schema 的依据）：① `${scope_id}.{name}` 变量名中的 scope id；② 全局常量池索引 `cN`；③ DataSection 字符串偏移（`USER_STRING_START` 之后）；④ 跨模块 import 绑定符号（`export_map` 里 `(module_id, name) → ir_name`）；⑤ 对全局根 `$0.$global`/namespace object 的引用（**不重定位**，链接期固定）。
- [ ] **写失败测试**。`relocatable/mod.rs` 定义 `RelocatableModule { local_program: Program, relocations: Relocations, scope_layout: ScopeLayout }` 与 `pub const LOCAL_SCOPE_BASE: usize = 1;` 及 `lower_one(ast: swc_ast::Module, metadata: ModuleMetadata) -> Result<RelocatableModule, LoweringError>`。`Relocations` 含 `scope_refs: Vec<ScopeRef>`（每项：IR 位置 + 局部 scope id）、`const_refs`/`string_refs`/`import_refs`。`ScopeLayout` 记录本片段 **predeclare 趟与 walk 趟各自的 scope push 序列（顺序 + `ScopeKind` + parent 局部 id）**——供任务 5.2 `link` 用空 `ScopeTree` 重放 `lower_modules` 的两趟分配序、逐 id 建 `local_id → global_id` 映射（因单偏移无法复现两段不连续区间，见任务 5.2）。
  ```rust
  #[cfg(test)]
  mod tests {
      use super::*;
      fn test_metadata() -> crate::ModuleMetadata {
          crate::ModuleMetadata {
              filename: "a.js".into(), dirname: ".".into(),
              url: "file:///a.js".into(), kind: crate::ModuleKind::Esm,
          }
      }
      #[test]
      fn relocatable_ir_local_scope_starts_at_base() {
          let src = "export const x = 1; function f() { const y = 2; return y; }";
          let ast = wjsm_parser::parse_module(src).unwrap();
          let m = lower_one(ast, test_metadata()).unwrap();
          // 模块局部 scope 从 LOCAL_SCOPE_BASE 起（避开全局根 0），项目无关。
          assert!(m.min_scope_id() >= LOCAL_SCOPE_BASE);
          // 重定位表记录了 scope 引用（x/y/f 的 ${scope}.name）。
          assert!(!m.relocations.scope_refs.is_empty());
          // 常量 1、2 记录进 const_refs。
          assert!(!m.relocations.const_refs.is_empty());
      }
  }
  ```
- [ ] **Verify RED**：`cargo nextest run -p wjsm-semantic -E 'test(relocatable_ir)'`。
- [ ] **完整实现**：实现 `lower_one`——复用现有单模块 lower 逻辑，但 `ScopeTree` 以 `LOCAL_SCOPE_BASE` 为根偏移起始；扫描产出 IR 按 Spike 清单收集①–④进 `Relocations`，⑤类引用打标为「全局固定」不入重定位表。
- [ ] **Verify GREEN**：测试通过。
- [ ] **Commit**：`git add -A && git commit -m "feat(wjsm-semantic): 可重定位 IR 单包 lower + 重定位表"`

## 任务 5.2：链接阶段 — 分级逐指令等价性

Files:
- 创建 `crates/wjsm-semantic/src/relocatable/link.rs`
- 修改 `crates/wjsm-semantic/src/relocatable/mod.rs`

Why: 把多个局部 IR 片段按 bundle 位置重定位合并为全局 Program，产出须与现有 `lower_modules` 整体路径**逐指令等价**——这是 L1 正确性的命门。**风险与分级**：`lower_modules` 存在跨模块耦合（`$0.$global`、entry-block 顺序发射的全局常量 `emit_global_constants`、`predeclare_module_exports`/`lower_module_bodies` 对全部模块的**两趟顺序** scope 分配（predeclare 全部 → walk 全部，单模块 id 因此分两段不连续、需逐 id 重映射而非单偏移）、`shared_env_stack`/live-binding 依赖 `binding_owner_function_scope == current_function_scope_id`）。一次性对任意模块图达到逐指令等价是研究级里程碑，不能假设一步到位。故本任务**分三级验收，逐级放宽输入**，每级独立 commit，前一级绿了才做下一级：
- **L2-a**：单个无 import/无跨模块引用的叶子包（只有本地 const/function/字符串）。
- **L2-b**：两个模块，一条 `import { x } from './a'` 边（覆盖 import 符号重定位 + 命名空间）。
- **L2-c**：三个模块含 re-export、live-binding、共享 env（覆盖 `$global`/`shared_env` 交互）。

Impact/Compatibility: 新增。等价性是硬验收（重定位偏差是静默错误代码，非崩溃）。L2-c 若暂不能达成逐指令等价，该场景必须显式进入 §8.4 L2-bundle 整包路径（整 bundle 编译，不分离），并用测试证明 per-package L1/L2 key 不生成、整包缓存 key 生成；不允许忽略失败测试或静默发布错误粒度的包片段缓存。

Verification: `cargo nextest run -p wjsm-semantic -E 'test(relocatable_ir_equivalence)'`

Steps:

- [ ] **写失败测试（可编译、无空洞）**。`link.rs` 测试。**注意**：`lower_modules` 需 6 个 map 入参，测试用一个 `build_bundle` 辅助从源码构造它们（在 semantic 测试内**手工构造精确 map**——**不得**反向引用 `wjsm-module::analyze_module_links`，见「完整实现」的依赖方向说明），并同时喂给两条路径，保证入参一致：
  ```rust
  #[cfg(test)]
  mod tests {
      use super::*;
      use crate::relocatable::lower_one;

      // 从 (module_id, source) 列表构造 lower_modules 的全部入参 + RelocatableModule 列表。
      // link_inputs 是链接元数据（模块顺序、各模块 import 边、export 名），两条路径共用。
      struct Bundle {
          inputs: Vec<crate::ModuleLoweringInput>,
          import_map: std::collections::HashMap<wjsm_ir::ModuleId, Vec<wjsm_ir::ImportBinding>>,
          dyn_targets: std::collections::HashMap<wjsm_ir::ModuleId, Vec<wjsm_ir::ModuleId>>,
          export_names: std::collections::HashMap<wjsm_ir::ModuleId, std::collections::BTreeSet<String>>,
          dyn_specs: std::collections::HashMap<wjsm_ir::ModuleId, Vec<(String, wjsm_ir::ModuleId)>>,
          re_exports: std::collections::HashMap<wjsm_ir::ModuleId, Vec<wjsm_ir::ReExportBinding>>,
      }
      // 解析每源码为 AST + 构造 6 个 map。L2-a（无 import）所有 map 为空；
      // L2-b/L2-c 按 import/re-export 边填充 import_map/export_names/re_exports。
      fn build_bundle(mods: &[(u32, &str)]) -> Bundle {
          let inputs = mods.iter().map(|(id, src)| crate::ModuleLoweringInput {
              id: wjsm_ir::ModuleId(*id),
              ast: wjsm_parser::parse_module(src).unwrap(),
              metadata: crate::ModuleMetadata {
                  filename: format!("m{id}.js"), dirname: ".".into(),
                  url: format!("file:///m{id}.js"), kind: crate::ModuleKind::Esm,
              },
          }).collect();
          // 各 map 默认空；按测试级别填充对应边（见 fill_link_edges）。
          Bundle {
              inputs,
              import_map: Default::default(), dyn_targets: Default::default(),
              export_names: Default::default(), dyn_specs: Default::default(),
              re_exports: Default::default(),
          }
      }

      fn whole_program(b: &Bundle) -> wjsm_ir::Program {
          crate::lower_modules(b.inputs.clone(), &b.import_map, &b.dyn_targets,
              &b.export_names, &b.dyn_specs, &b.re_exports).unwrap()
      }
      fn linked_program(b: &Bundle) -> wjsm_ir::Program {
          let parts: Vec<_> = b.inputs.iter()
              .map(|m| lower_one(m.ast.clone(), m.metadata.clone()).unwrap()).collect();
          link(parts, &LinkMeta::from_bundle(b)).unwrap()
      }

      #[test]
      fn relocatable_ir_equivalence_leaf_only() {           // L2-a
          let b = build_bundle(&[(0, "const x = 1; function f(){ return 'hi'; }")]);
          assert_eq!(format!("{}", whole_program(&b)), format!("{}", linked_program(&b)));
      }

      #[test]
      fn relocatable_ir_equivalence_single_import_edge() {  // L2-b
          let b = build_bundle(&[(0, "export const x = 1;"), (1, "import { x } from './a'; console.log(x);")]);
          assert_eq!(format!("{}", whole_program(&b)), format!("{}", linked_program(&b)));
      }

      #[test]
      fn relocatable_ir_equivalence_reexport_and_shared_env() { // L2-c
          let b = build_bundle(&[
              (0, "export const v = 1;"),
              (1, "export { v as w } from './a';"),
              (2, "import { w } from './b'; console.log(w);"),
          ]);
          assert_eq!(format!("{}", whole_program(&b)), format!("{}", linked_program(&b)));
      }
  }
  ```
- [ ] **Verify RED**：运行预期失败（`build_bundle`/`link`/`LinkMeta` 未实现）。
- [ ] **完整实现（分级实现）**：
  - 实现 `build_bundle`：对每个 `(id, src)` `wjsm_parser::parse_module` + 构造 `ModuleMetadata`；用 `import`/`export` 语法静态提取构造 6 个 map。L2-a 叶子包各 map 为空；L2-b/L2-c **必须**填 `import_map`（`ImportBinding { source_module, names: Vec<(local, imported)>, specifier }`，对齐 `wjsm-ir` 实际字段——核对 `crates/wjsm-ir/src/lib.rs:789`）、`export_names`、`re_export_map`（`ReExportBinding { source_module, local_name: Option<String>, exported_name: Option<String> }`，核对 `lib.rs:803`）——否则两条路径入参不一致、等价性测试无意义。**注意依赖方向**：`wjsm-module` 已 normal-depend `wjsm-semantic`（`bundler.rs:5`），故 semantic 测试**不得**反向引用 `wjsm-module::analyze_module_links`（会成 dev-dep 环，且 `analyze_module_links` 需真实 `ModuleGraph`（由磁盘文件 BFS 构建，`graph.rs:35`）无法 stub）。测试**自包含**：直接手工构造 `ImportBinding`/`ReExportBinding` 填 map，与 `whole_program`/`linked_program` 两条路径共用同一 `Bundle`。
  - 实现 `LinkMeta::from_bundle`：记录模块顺序、每模块的 **scope-id 重映射表**（`local_id → global_id`，见下）、import 边、export 名到全局符号的解析。
  - 实现 `link`：**scope-id 分配不是单一偏移，而是逐 id 重映射**（已核对 `lowerer_modules.rs` + `scope.rs`，是本任务地基）。事实：① `predeclare_module_exports`（L144）是**顺序** `for module in modules` 循环（push Block → predeclare → `pop_scope`），非「BFS 交错」；② scope 分配是**对所有模块的两趟**——先 predeclare 全部模块（L144 循环各分配「模块 Block + 其内嵌套 let/const/fn 预声明 scope」），再 `lower_module_bodies`（L643）**重新** `enter_scope(module_scope)` 并在 walk 中 push **新的**函数/块 scope；③ `pop_scope`（scope.rs:99）**不截断** `arenas`，id 单调递增、永不复用。**推论**：对 N≥2 模块，单个模块的全局 scope-id 是**两段不连续区间**（predeclare 段 + walk 段），中间交错其他模块的 id——`base-1` 单偏移**只对 N=1（L2-a）成立**，L2-b/L2-c 必然错位。故 `link` 的正确模型是：以一个**空 `ScopeTree` 重放 `lower_modules` 的确切两趟分配序**（predeclare 全部片段 → walk 全部片段），为每个模块建 `local_id → global_id` 映射表；重定位时 `scope_refs` 逐项经该表改写（非加偏移），`const_refs` 加 const_base、`string_refs` 加 data_base、`import_refs` 解析到目标模块全局名。任务 5.1 的 `RelocatableModule` 须额外产出「本片段各 scope 的 push 序列 + kind」供 `link` 重放。
  - **迭代顺序**：先让 L2-a 绿（无 import/无 $global 交互）→ commit；再 L2-b（import 边 + 命名空间对象顺序）→ commit；再 L2-c（re-export/shared_env）→ commit。若 L2-c 等价性仍未达成，必须新增 `relocatable_ir_l2c_uses_bundle_path` 测试，证明该图不会生成 per-package L1/L2 artifact，而是走整包 L2-bundle key，并在 ADR 记录边界。
- [ ] **Verify GREEN**：L2-a/L2-b 必过；L2-c 必须是「逐指令等价测试通过」或「整包路径测试通过且 per-package artifact 被拒绝」，不允许忽略测试。
- [ ] **Commit**（分级）：
  - `git add -A && git commit -m "feat(wjsm-semantic): 可重定位 IR 链接（L2-a 叶子包逐指令等价）"`
  - `git add -A && git commit -m "feat(wjsm-semantic): 可重定位 IR 链接（L2-b import 边逐指令等价）"`
  - `git add -A && git commit -m "feat(wjsm-semantic): 可重定位 IR 链接（L2-c re-export/shared-env 等价或整包路径说明）"`

## 任务 5.3：L1/L2 编译产物缓存接入 store

Files:
- 创建 `crates/wjsm-pm/src/store/artifact.rs`
- 修改 `crates/wjsm-pm/src/store/index.rs` / `store/mod.rs`
- 修改 `crates/wjsm-cli/src/lib.rs`（build 走 L1/L2 缓存）+ `cmd_cache`（展示/清理 L1/L2）

Why: L1 缓存可重定位 IR，必须保持**包内容级**跨项目复用；peer-set/instance/import 解析上下文属于 link 阶段，不得污染 L1 key。L2 若缓存 wasmtime cwasm，则必须绑定 exact linked wasm/engine config 边界，继承 `runtime_startup.rs` 的安全失效语义。

Impact/Compatibility: 新增。L1 key 只含 package-local IR 会受影响的输入；L2 key 含 linked IR/wasm 与 wasmtime/engine/config 边界。L2-bundle 仍作为入口/整包缓存路径，但不掩盖 per-package cache 命中测试。

Verification: `cargo nextest run -p wjsm-pm -E 'test(artifact)'`

Steps:

- [ ] **写失败测试**。`store/artifact.rs`：
  ```rust
  // L1/L2 编译产物缓存：L1 包内容级复用，L2 绑定 linked wasm + engine 边界。
  use crate::store::blob::hash_content;

  #[derive(Debug, Clone)]
  pub struct PackageIrKeyInput<'a> {
      pub manifest_hash: &'a [u8; 32],
      /// package.json type / module-kind map / lowering feature flags 中会改变 package-local IR 的摘要。
      pub source_shape_hash: &'a [u8; 32],
      pub semantic_version: &'a str,
      pub lowering_flags: &'a str,
  }

  /// L1 key = blake3(manifest_hash ‖ source_shape_hash ‖ semantic_version ‖ lowering_flags)
  pub fn l1_key(input: PackageIrKeyInput<'_>) -> [u8; 32] {
      let mut buf = Vec::new();
      buf.extend_from_slice(input.manifest_hash);
      buf.extend_from_slice(input.source_shape_hash);
      buf.extend_from_slice(input.semantic_version.as_bytes());
      buf.extend_from_slice(input.lowering_flags.as_bytes());
      hash_content(&buf)
  }

  #[derive(Debug, Clone)]
  pub struct CwasmKeyInput<'a> {
      pub l1_key: &'a [u8; 32],
      pub linked_ir_hash: &'a [u8; 32],
      pub wasm_bytes_hash: &'a [u8; 32],
      pub backend_abi_hash: u64,
      pub gc_flavor: &'a str,
      pub wasmtime_version: &'a str,
      pub engine_config_hash: &'a [u8; 32],
      pub target_triple: &'a str,
      pub opt_level: &'a str,
      pub cwasm_format_version: u32,
  }

  /// L2 key = blake3(l1_key ‖ linked_ir_hash ‖ wasm_bytes_hash ‖ backend_abi_hash ‖ gc_flavor ‖ wasmtime_version ‖ engine_config_hash ‖ target/opt ‖ cwasm_format_version)
  pub fn l2_key(input: CwasmKeyInput<'_>) -> [u8; 32] {
      let mut buf = Vec::new();
      buf.extend_from_slice(input.l1_key);
      buf.extend_from_slice(input.linked_ir_hash);
      buf.extend_from_slice(input.wasm_bytes_hash);
      buf.extend_from_slice(&input.backend_abi_hash.to_le_bytes());
      buf.extend_from_slice(input.gc_flavor.as_bytes());
      buf.extend_from_slice(input.wasmtime_version.as_bytes());
      buf.extend_from_slice(input.engine_config_hash);
      buf.extend_from_slice(input.target_triple.as_bytes());
      buf.extend_from_slice(input.opt_level.as_bytes());
      buf.extend_from_slice(&input.cwasm_format_version.to_le_bytes());
      hash_content(&buf)
  }

  #[cfg(test)]
  mod tests {
      use super::*;
      fn ir_input<'a>(mh: &'a [u8; 32], shape: &'a [u8; 32], sem: &'a str) -> PackageIrKeyInput<'a> {
          PackageIrKeyInput { manifest_hash: mh, source_shape_hash: shape, semantic_version: sem, lowering_flags: "" }
      }

      #[test]
      fn l1_key_reuses_same_package_across_instances() {
          let mh = [2u8; 32];
          let shape = [3u8; 32];
          let a = l1_key(ir_input(&mh, &shape, "0.1.0"));
          let b = l1_key(ir_input(&mh, &shape, "0.1.0"));
          assert_eq!(a, b, "L1 不绑定 instance_id/peer_set，保证同包跨项目 parse/lower 复用");
          assert_ne!(a, l1_key(ir_input(&mh, &shape, "0.2.0")), "semantic 版本变更 L1 key 应失效");
      }

      #[test]
      fn l2_key_invalidates_on_wasm_engine_and_gc() {
          let l1 = [1u8; 32];
          let linked = [2u8; 32];
          let wasm = [3u8; 32];
          let engine = [4u8; 32];
          let base = CwasmKeyInput { l1_key: &l1, linked_ir_hash: &linked, wasm_bytes_hash: &wasm, backend_abi_hash: 100, gc_flavor: "mark-sweep", wasmtime_version: "43.0.0", engine_config_hash: &engine, target_triple: "wasm32-wasip1", opt_level: "default", cwasm_format_version: 1 };
          let a = l2_key(base.clone());
          let b = l2_key(CwasmKeyInput { wasmtime_version: "44.0.0", ..base.clone() });
          assert_ne!(a, b, "wasmtime 版本变更 L2 key 应失效");
          let c = l2_key(CwasmKeyInput { gc_flavor: "zgc", ..base });
          assert_ne!(a, c, "GC flavor 变更 L2 key 应失效");
      }
  }
  ```
- [ ] **写失败测试**。SQLite `artifacts` 表沿用任务 1.4 最终 schema：`tier/key_hash/source_id/instance_id/compiler_version/abi_hash/gc_flavor/wasmtime_version/engine_config_hash/target_triple/opt_level/pack_id/offset/clen/ulen/metadata`，唯一键 `(tier,key_hash)`；artifact bytes 写入 packfile 并由 `Store::put_artifact_txn` 原子登记，GC mark 阶段保留 live artifacts。
- [ ] **Verify RED**：`cargo nextest run -p wjsm-pm -E 'test(artifact)'`。
- [ ] **完整实现**：artifact key/types + store artifact 事务读写 + `cmd_cache` 统计 + build 接入；L1 命中跳过 package lower；L2 命中先尝试 wasmtime deserialize，失败则删除该 L2 row 并重新编译/写入。
- [ ] **Verify GREEN**：测试通过 + 冒烟：同一 package manifest 在两个项目 install+build，第二次 L1 命中；同一 tarball 在不同 peer-set 下仍命中同一 L1；改变 linked graph / `WJSM_GC` / wasmtime version / engine config 后 L2 失效。
- [ ] **Commit**：`git add -A && git commit -m "feat(wjsm-pm): L1/L2 编译产物缓存跨项目复用"`

## 任务 5.4：全量回归 + pm 端到端集成测试收尾

Files:
- 创建 `crates/wjsm-pm/tests/pm_end_to_end.rs`（多场景集成测试，非 fixture 快照）

Why: 端到端验收 spec §13 的场景集，确认无 node_modules、去重、多版本、task/x 全链路。**沿用任务 3.4 的命名约定**——pm 场景走 crate 内 `#[test]` 集成测试（`pm_` 前缀），不走 `fixtures/*` `.expected` harness（该 harness 只识别 happy/errors/modules 三 suite 且无法表达多步编排）。

Impact/Compatibility: 纯新增测试。

Verification: `cargo nextest run --workspace`

Steps:

- [ ] **写集成测试**（各用内置 mock registry + 临时项目 + 临时 store）：`pm_install_basic`（无依赖包，断言项目目录无 `node_modules`）、`pm_install_dedup`（两项目共享 blob，断言 `index.db` 中相同内容单 blob）、`pm_install_multi_version`（instance-splitting 共存，断言两版本均可 read_package_file）、`pm_task_scripts`（pre/post 序列执行）、`pm_x_bin`（拉包→解析 bin→执行）。
- [ ] **Verify RED**：新测试未接入前 `cargo nextest run -E 'test(pm_)'` 失败。
- [ ] **完整实现**：补齐各集成测试的编排辅助（复用 `tests/mock_registry.rs`）。
- [ ] **Verify GREEN**：`cargo nextest run --workspace` 全绿；`cargo build` 零警告。
- [ ] **Commit**：`git add -A && git commit -m "test(wjsm-pm): pm 端到端集成测试验收"`

---

## Risks

- **可重定位 IR 逐指令等价**（最高风险，研究级里程碑）：重定位偏差是静默错误代码，非崩溃。`lower_modules` 存在 `$0.$global`、entry-block 顺序常量、**两趟顺序 scope 分配（单模块 id 拆成不连续两段，须逐 id 重映射而非单偏移）**、shared-env/live-binding 路由等跨模块耦合，「一次成型逐指令等价」不现实。缓解：任务 5.2 **分级验收**——L2-a（单包无 import）→ L2-b（跨模块 import 边）→ L2-c（re-export/`$global`/live-binding）逐级引入，每级用逐指令等价快照硬验收；L2-c 未达前，含 re-export/live-binding 的包 L1 缓存**不启用**（切换到 L2-bundle 整体路径），不影响 P1–P4 与不含该模式的包。不通过不合并。
- **pubgrub API 形态**：版本已锁定 `0.4`（web 调研确认 0.3=0.4 `DependencyProvider` 一致：`prioritize`+`choose_version`+`get_dependencies` + 关联类型 `P`/`V`/`VS`/`M`/`Priority`/`Err`；MSRV 1.92 / edition 2024 与本 workspace 一致）。残余不确定仅在 `M`/`Priority` 的具体类型选择——任务 2.3 首步 spike 用本地 `cargo doc` 复核并抄进 `provider.rs` 注释。若 0.4 某关联类型经 spike 证明无法承载 npm 语义，则直接记录证据并改为手写 CDCL；评估概率极低。
- **VersionSet 建模 npm 预发布规则**（求解正确性）：pubgrub `VS` 需 `complement`/`intersection`，而 npm"预发布仅同 tuple + comparator 自带预发布才纳入"规则无法直接用纯区间代数表达（uv 对 Python 预发布同样靠专门建模）。缓解：任务 2.3 spike 先定建模（候选 `Ranges<SemVer>` 编码预发布排除 / 自定义 `VersionSet` on `SemVer`），以 task 2.1 `prerelease_inclusion_rule` 转换后逐一等价为硬验收；两条路都保留 comparator-`Range::matches` 为最终语义基准。选定前不落 provider。
- **instance-splitting / peer 正确性**：可满足场景须复现 npm 多版本，真冲突须给解释。缓解：任务 2.3 四测试覆盖去重/分裂/peer 冲突/optional 跳过四态；peer 约束翻译（`peer(react ^17)` → 对宿主环境已选 react 版本的 comparator）明确定义；真实 npm 树对照纳入任务 2.3 验收扩展。
- **前置 #312 未合并**：P5 阻塞。缓解：P1–P4 完全独立可先交付；P5 首步前置检查。
- **全局 store 并发写（跨进程）/ 中断原子性**：`~/.wjsm/store` 全局共享，多个 `wjsm install`（不同项目）可并发写同一 packfile + index.db。**原计划硬伤**：`PackWriter` 缓存 `offset` 并以之记录 `BlobLoc`，但 `.append(true)` 下 OS 写到真实 EOF——两并发写者各自缓存同一 `offset`，B 的 `BlobLoc.offset` 会指向 A 的字节，`read_blob` 静默返回错数据（内容损坏）。缓解（任务 1.2/1.5）：① `add_package_from_dir` 全程持 **store 级独占写锁**（flock `.write.lock`，跨进程串行化 append+sync+commit 临界区）；② `PackWriter::append` **不信任缓存 offset**，写前 `seek(End(0))` 取真实偏移记录进 `BlobLoc`；③ index 三表写入包在单 SQLite 事务（WAL），中断回滚；④ 孤儿尾字节由 `store gc`（任务 1.5b，同一 flock）标记-复制回收。任务 1.5 加 `store_integration_concurrent_writers` 测试：多线程/多 `Store` 实例并发 `add_package_from_dir` 后逐包 `read_package_file` 校验内容正确。
- **默认 Vfs 破坏现有行为**（关键）：resolver 全部 fs 谓词（含 12 处 `canonicalize`）改经 Vfs，`FsVfs` 须与原 `std::fs`/`Path` 调用语义逐处等价。缓解：任务 1.6 硬验收 `cargo nextest run --workspace` 全绿 + `FsVfs::canonicalize` 与 `Path::canonicalize` 对照单测。
- **tarball 解包路径逃逸 / 链接逃逸**（安全）：解包 registry tarball 时，恶意作者可发布 integrity 合法却含 `../` 逃逸路径或符号/硬链接的 tarball（node-tar CVE 系列 CVE-2021-32803/CVE-2026-24842、RUSTSEC-2026-0067）——SSRI **不覆盖**此面。缓解：任务 2.2 `extract_tgz` 三重守卫（逐组件拒 `..`/绝对/盘符前缀 + 拒链接与特殊条目 + 落盘前复核前缀），配 `registry_extract_rejects_path_traversal` / `registry_extract_rejects_symlink_entry` 两测试硬验收；`tar` 依赖锁 `>=0.4.45`（含 RUSTSEC-2026-0067 修复）。**不**用手写 `dest.join` 裸循环（原计划硬伤）。

## Retirement

- 本计划为 new-capability，不删除现有主路径：FS 模式解析、`lower_modules` 整体路径均保留为默认。
- 退出条件（falsifier）：`wjsm install` 后无 node_modules、`wjsm run` 从 CAS 编译成功、同包跨项目 L2 缓存命中零重复编译 → 证明 CAS + 分离编译主路径成立。
- L2-bundle 整包路径长期保留（入口模块 + 无法分离编译场景），非临时。

## ADR 信号（executing 完成后补 ADR）

1. 全局内容寻址存储 `~/.wjsm/store` 作为新持久化 source-of-truth（blob 内容寻址 + lockfile 解析结果分离）。
2. `wjsm-module` 引入 `Vfs`/`ResolutionOverlay` trait——跨 crate 契约变更（module↔pm 边界）。
3. PubGrub 内核 + npm instance-splitting 求解语义。
4. 可重定位 IR / 分离编译（scope id 项目无关化 + 重定位 + 链接），与 #312 地基及 startup snapshot relocatable heap（ADR 0003）同源。

## 自审记录

- Spec 覆盖：CAS 存储(P1)/求解(P2)/install+lockfile+CLI(P3)/task+x+workspaces(P4)/编译产物缓存(P5) 各有任务；spec §13 测试策略逐项映射到任务验收。
- 完整性：无待办标记或未解决空洞；每个任务含明确接口、测试名、实现要点与命令；P5 等价性测试的入参已展开为可编译代码（`build_bundle` helper 构造 `Bundle`，两条路径共用），无 `/* ... */` 空洞。
- 类型一致：`BlobHash=[u8;32]`、`BlobLoc`、`Manifest`、`SemVer`/`Range`/`Comparator`、`WjsmLock`（含 `root_deps`）、`encode_instance_dir`、`RelocatableModule`/`LOCAL_SCOPE_BASE` 跨任务签名一致。
- 兼容：默认 FsVfs/NoOverlay 零破坏、module 不依赖 pm 均标为硬验收；`FsVfs` 每方法与被替换的 `std::fs`/`Path` 调用语义逐处等价。
- 复杂度：主逻辑进新 crate/新文件；resolver.rs 只做 wiring（全部 fs 谓词路由进 `self.vfs`，不新增包管理逻辑），不因触点数量增负。
- 验证：每任务有精确 nextest/cargo 命令；pm 端到端场景走 `crates/wjsm-cli/tests/*.rs` 集成测试（fixture runner suite 仅 happy/errors/modules，不含 pm），命名统一 `test(pm_*)`。
- 双轨/ADR：new-capability，ADR 信号已保留待 executing 后补。

### 二轮审查修正（本次收敛）

针对首轮计划的以下缺口已逐项修正：

1. **「3 处磁盘接缝」证伪** → 任务 1.6 重写为「resolver 实际路由的全部 fs 谓词（`read_to_string`×1 / `canonicalize`×12 / `is_file`×7 / `is_dir`×6，共 26 处）路由进 `Vfs`」，`Vfs` trait 提供 `canonicalize`/`is_file`/`is_dir`/`read_to_string`/`read_package_json`，另加 `exists`（resolver 不调用，仅供 `CasVfs` 内部前缀判定 + trait 完备性）；`CasVfs::canonicalize` 对虚拟路径做词法归一化（`normalize_virtual`）。这是 P2–P4 地基。
2. **npm_semver 部分实现** → 任务 2.1 重写为 comparator 结构模型，覆盖 `^`/`~`/x-range/hyphen/比较运算符/`||`/**预发布包含规则**（修正原 `(lo,hi)` 元组模型对 `2.0.0-alpha` 的误判硬伤），符合 hard rule「No partial implementations」。
3. **P5 逐指令等价 + 测试可编译性 + scope 分配模型** → 任务 5.1/5.2 修正 `LOCAL_SCOPE_BASE`（避开全局根 `$0.$global`）；**逐行核对 `lowerer_modules.rs`+`scope.rs` 后订正两处硬伤**：(a) `predeclare_module_exports` 是顺序循环而非「BFS 交错」；(b) scope 分配是「对全部模块的 predeclare 趟 + walk 趟」两趟，单模块 id 拆成不连续两段，故重定位须**逐 id 重映射（空 `ScopeTree` 重放两趟分配序）**而非「整体加单一 `base-1` 偏移」（后者只对 N=1 成立）。等价性测试展开为可编译代码，分三级验收（L2-a 单包 → L2-b import 边 → L2-c re-export/`$global`/live-binding），承认这是研究级里程碑。
4. **Store 非事务 + GC/pack 轮转缺失** → 任务 1.4/1.5 加 `packs` 表 + `with_txn(|tx| …)` 整包原子写（配套 `txn_put_blob`/`txn_put_manifest_raw`/`txn_put_package` 事务作用域自由函数，避免闭包内二次 `lock` 死锁）+ 回滚测试；新增任务 1.5b 标记-复制 gc；`PackWriter` 支持 pack 轮转。
5. **CasVfs scoped 包破损 + PnpOverlay 真空** → 任务 3.2 用 `encode_instance_dir`（percent-encode `%`、`/`、`@`、`#` 等分隔字符）保证 instance 目录单组件、scoped 包安全；`is_dir` 用 `manifest_has_prefix` 支持中间目录；补全 `PnpOverlay` 实现（边表 + root_deps）。
6. **peer 求解 hand-wave** → 任务 2.3 明确 peer→PubGrub 约束翻译、`MockIndex` API、optionalDependencies 跳过语义 + 对应测试。
7. **fixture 命名不一致** → 统一 pm 端到端为 CLI 集成测试 `test(pm_*)`，移除不存在的 `fixtures/pm` suite（fixture runner 仅 happy/errors/modules）。
8. **schema 偏离设计 §6.3** → 任务 1.4 显式记录偏离（`manifests(hash,body)` vs 规范化 `manifest_entries`）并加回 `packages.meta` 列对齐设计。
