# wjsm 包管理实现计划（wjsm-pm）

Goal: 实现 wjsm 的完整 npm 生态包管理能力（`wjsm install/add/remove/task/x` + workspaces），做到无 `node_modules`、全局内容寻址存储（CAS blob + SQLite + zstd + packfile）直供编译器，并实现 AOT 独有的跨项目分层编译产物复用（L1 可重定位 IR + L2 cwasm 片段）。批准的设计见 `docs/aegis/specs/2026-07-07-package-management-design.md`。

Architecture: 新增独立 crate `wjsm-pm`，拥有 registry client / CAS store / PubGrub solver / lockfile / scripts / workspace。`wjsm-module` 新增 `Vfs` + `ResolutionOverlay` 两个 trait（定义在 module 侧，实现在 pm 侧）；`Vfs` 抽象 `ModuleResolver` 解析算法实际路由的**全部**文件系统谓词（`read_to_string`×1 / `canonicalize`×12 / `is_file`×7 / `is_dir`×6，resolver.rs 内共 **26 处**）+ `package_json.rs` 的 `read_package_json`（合并原 `fs::metadata`+`read_to_string`），而非仅三处读取；trait 另提供 `exists`（`FsVfs`=`Path::exists`）作抽象完整性、供 `CasVfs` 内部前缀判定复用——**resolver 自身不调用 `exists`**（已 grep 核实 `wjsm-module` 无 `.exists()` 触点）。`wjsm-module` **不反向依赖** `wjsm-pm`。`wjsm-semantic` 新增可重定位 IR 单包 lower + 链接阶段（服务 L1）。`wjsm-cli` 组装注入并新增子命令。依赖方向：`wjsm-pm → wjsm-module`、`wjsm-pm → wjsm-snapshot-format`、`wjsm-cli → wjsm-pm`。

Tech Stack: Rust 2024；`rusqlite`（bundled SQLite，WAL）；`zstd`；`blake3`（内容哈希）；`tar` + `flate2`（tgz 解包）；`sha2` + base64（npm SSRI 完整性校验）；`reqwest`（workspace 已有，async/tokio）；`tokio`（workspace 已有，`spawn_blocking` 隔离 SQLite 同步写）；`pubgrub` crate（版本求解）；`toml`（workspace 已有，lockfile）；`serde_json`（packument、迁移读取）；`serde_yaml`（pnpm-lock 迁移）；现有 fixture runner + nextest。

Baseline/Authority Refs:

- `docs/aegis/specs/2026-07-07-package-management-design.md`（本计划的批准设计）
- `AGENTS.md` / `CLAUDE.md`：AOT pipeline、crate 依赖方向、Rust 2024、注释中文、文件 ≤500 行/函数 ≤30 行体量纪律、ECMAScript/npm spec 兼容 hard rules、临时文件禁入项目树
- `docs/aegis/specs/2026-07-07-runtime-module-loading-design.md` + `docs/aegis/plans/2026-07-07-runtime-module-loading.md`（issue #312，P5 前置依赖：可重定位 IR / 分离编译地基）
- `docs/adr/0003-startup-snapshot-boundary.md`（relocatable heap 同源思路）
- `crates/wjsm-module/src/resolver.rs`（`ModuleResolver` struct L70、`with_options` L89、`find_package_in_node_modules` L328、源码读取 L754）
- `crates/wjsm-module/src/bundler.rs`（`ModuleBundler` L12、`with_resolution_options` L23）
- `crates/wjsm-module/src/graph.rs`（`ModuleGraph::build_with_options` L39）
- `crates/wjsm-module/src/package_json.rs`（`read_package_info` L48、`fs::read_to_string` L60）
- `crates/wjsm-module/src/resolution_options.rs`（`ResolutionKind` / `ResolutionOptions`）
- `crates/wjsm-semantic/src/lowerer_modules.rs`（`lower_modules` L37、`ModuleLoweringInput` L9、`ModuleMetadata` L16、`ModuleKind` L24）
- `crates/wjsm-semantic/src/scope.rs`（`ScopeTree` L43、`push_scope` L61 全局递增 arena；IR 名 `${scope_id}.{name}`）
- `crates/wjsm-cli/src/cli_args.rs`（`Commands` enum L150、`CacheCommand` L411）
- `crates/wjsm-cli/src/lib.rs`（`main_entry` L335、命令 dispatch L365、`cmd_cache` L1407、`run_file_in_process` L2074）
- `crates/wjsm-runtime/src/runtime_startup.rs`（`compile_or_load_cached` L55、`precompile_module` L85）
- `crates/wjsm-snapshot-format`（`abi_hash`、`register_abi_hash_external_input`）
- `tests/fixture_runner.rs`（E2E harness）

Compatibility Boundary:

- 无依赖 / 纯本地相对导入的现有项目行为不变：`FsVfs` + `NoOverlay` 为默认，CAS 覆盖层仅在有 lockfile/依赖时由 CLI 注入。
- 所有现有 fixture、`wjsm run file.js` 语义不变。
- `wjsm-module` 不依赖 `wjsm-pm`；trait 定义在 module 侧。
- blob 内容寻址身份、lockfile 解析结果身份分离；store 版本目录 `~/.wjsm/store/v1`。
- 已知代价：首版无物化 node_modules，外部 Node 工具链看不到依赖（wjsm 自有 check/lint/fmt 走 CAS 不受影响）；`--node-modules-dir` 逃生舱不在本计划。
- 迁移不删除原生态 lockfile（除非 `--prune`）。
- P5 前置依赖 issue #312 已合并；可重定位 IR 分离编译产出与现有 `lower_modules` 整体路径**分级逐指令等价**（L2-a 叶子包 / L2-b import 边必过；L2-c re-export/shared-env 等价或明确降级到 L2-bundle）。
- tarball 必须 SSRI 校验通过才入库；依赖生命周期脚本默认禁用，需 `trustedDependencies` 或 `--allow-scripts`。

Verification:

- `cargo nextest run -p wjsm-pm`
- `cargo nextest run -p wjsm-module`（回归 Vfs 抽象不破坏 FS 模式）
- `cargo nextest run -p wjsm-semantic -E 'test(relocatable_ir)'`
- `cargo nextest run -p wjsm-cli -E 'test(pm_)'`（CLI 集成测试：install→run，含无 node_modules 断言。pm 场景需 store 预置 + install 前置步骤，标准 fixture_runner（仅 happy/errors/modules 三 suite、纯 run 比对）无法表达，故用 `crates/wjsm-cli/tests/` 下自定义集成测试，不注册 fixtures/pm suite）
- `cargo nextest run --workspace`（全量回归）
- 冒烟：含依赖 fixture 项目 `wjsm install` 后 `wjsm run` 成功且磁盘无 node_modules

## Plan Basis

Facts（已核对代码）:

- `ModuleResolver`（resolver.rs:70）字段 `root_path/options/package_cache/visited`；`with_options`（L89）是唯一带 options 构造器。**文件系统触点远不止三处**（已逐行核对 resolver.rs）：
  - `std::fs::read_to_string`：L754（源码读取，唯一读点）。
  - `Path::canonicalize`：L91、334、343、359、380、454、469、593、600、609、631、667（12 处，遍布 `resolve_file_or_directory`/`resolve_existing_module_path`/`resolve_package_target_path`/`resolve_directory_index`/`find_nearest_package`/`read_package_info`/`canonical_entry_path`/`find_package_in_node_modules`）。
  - `Path::is_file`：L453、468、592、599、608、630、663（7 处）。
  - `Path::is_dir`：L342、354、456、474、602、614（6 处）。
  - node_modules 遍历：`find_package_in_node_modules`（L328）；bare specifier 入口 `resolve_bare_specifier`（L242）先查 `find_nearest_package` 再遍历 node_modules（L265）。
  - package.json 读取：`package_json.rs:read_package_info`（L48，`fs::metadata` + `read_package_info_manifest`→L60 `fs::read_to_string`）。
  - **决定性事实**：`std::fs::canonicalize` 要求路径在真实磁盘存在。CAS 虚拟路径 `<vroot>/<name>@<ver>/…` 永不落盘，直接 canonicalize 必然失败。因此 CAS 切入**不是**"改三处读取"，而是"把 resolver 的全部 fs 谓词路由进 `Vfs`，并让 `Vfs::canonicalize` 对虚拟路径做恒等归一化"。这是 resolver 级重构（任务 1.6 承载），是 P2–P4 的地基。
- `ModuleBundler`（bundler.rs:12）持 `root_path/options`，`lower_bundle`/`parse_entry_ast`/`bundle` 均经 `ModuleGraph::build_with_options`（graph.rs:39）。注入点在 bundler + graph + resolver 构造链。
- `lower_modules`（lowerer_modules.rs:37）接收 `Vec<ModuleLoweringInput>` + 各种 `HashMap<ModuleId, _>`，所有模块共用一棵 `ScopeTree`，scope id 全局递增（scope.rs:61 `idx = self.arenas.len()`）并写进 IR 名 `${scope_id}.{name}`。这是 L1 跨项目复用的命门——必须模块局部化 + 重定位。
- CLI dispatch 在 `main_entry`（lib.rs:365）`match cli.command`；`Commands` enum 在 cli_args.rs:150；现有 `Cache` 子命令 dispatch 到 `cmd_cache`（L1407）。新子命令加在这两处。
- workspace 已有 `reqwest`（rustls-tls+stream）、`tokio`、`toml`、`serde_json`、`serde`。**新增依赖**：`rusqlite`、`zstd`、`blake3`、`tar`、`flate2`、`sha2`、`base64`、`pubgrub`、`serde_yaml`。
- `runtime_startup.rs:55` `compile_or_load_cached` 用 wasmtime `precompile_module`（L85）+ `deserialize_file` 做 cwasm 缓存，按 wasm bytes 哈希 key——L2 复用此机制思路扩展到包粒度。
- fixture runner（tests/fixture_runner.rs）in-process 跑 `run_file_in_process`（lib.rs:2074），比对 exit+stdout+stderr。

Assumptions:

- issue #312 在 P5 开工时已合并，提供分离编译 loader 与 multi-instance shared-env（P5 任务假设其存在；若未合并，P5 阻塞，P1–P4 不受影响）。
- `pubgrub` crate 版本支持自定义 `Version`/`VersionSet`（用于 npm SemVer 语义）。任务 2.2 首步验证其 trait 形态并锁定版本。

Unknowns（计划内解决）:

- pubgrub crate 的确切 API 形态（`DependencyProvider` trait 签名）→ 任务 2.2 首步 spike 锁定。
- 可重定位 IR 重定位表需覆盖的引用种类完整清单 → 任务 5.1 首步用等价性快照测试驱动发现。

## BaselineUsageDraft

- Required baseline refs：spec 全文、AGENTS.md、resolver.rs/bundler.rs/graph.rs/package_json.rs、lowerer_modules.rs/scope.rs、cli_args.rs/lib.rs、runtime_startup.rs、snapshot-format、fixture_runner.rs、issue #312 spec/plan。
- Delivered context refs：pnpm/yarn/bun/deno 机制研究（DeepWiki，已在 spec §1）。
- Acknowledged before plan refs：以上全部已在写计划前读取核对（Facts 段为证）。
- Cited in plan refs：见各任务 Files/Why。
- Missing refs：pubgrub crate API（任务 2.2 spike）、可重定位引用完整清单（任务 5.1 快照驱动）。
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
修改 `crates/wjsm-module/src/`：新增 `vfs.rs`（trait 定义 + `FsVfs`/`NoOverlay` 默认实现）；`resolver.rs`（构造器接受 vfs/overlay，**全部 26 处 fs 谓词**——`read_to_string`×1/`canonicalize`×12/`is_file`×7/`is_dir`×6——改 trait 调用；resolver 无 `exists`/独立 `metadata` 触点）；`package_json.rs`（`read_package_info` 经 `Vfs::read_package_json`，合并原 `fs::metadata`+`fs::read_to_string`）；`bundler.rs`/`graph.rs`（注入透传）；`lib.rs`（导出 trait）。
修改 `crates/wjsm-semantic/src/`：新增 `relocatable/{mod,lower_one,relocate,link}.rs`；`lib.rs` 导出。
修改 `crates/wjsm-cli/src/`：`cli_args.rs`（新增 `Install/Add/Remove/Task/X` 子命令）；`lib.rs`（dispatch + `cmd_install` 等）；新增 `pm_commands.rs`。
修改根 `Cargo.toml`：workspace members + 新增依赖。

## Compatibility

- 不变式：`wjsm-module` 无 `wjsm-pm` 依赖；FS 模式默认；现有 fixture 全绿；`wjsm run file.js` 语义不变；blob 内容寻址；lockfile 分离。
- 非目标（不实现）：物化 node_modules、`wjsm publish`、原生 postinstall 编译、git+/远程 tarball 依赖源、HMR、私有 registry 完整企业 auth。
- 稳定接口：`Vfs`/`ResolutionOverlay` trait 一经定义即为 module↔pm 契约（ADR 信号 2）。

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
- Task executability：任务含完整代码与命令；pubgrub/可重定位两处 unknown 用 spike/快照驱动首步收敛。
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

Why: 建立 pm crate 与依赖，后续所有 pm 逻辑的容器。

Impact/Compatibility: 纯新增；不触碰现有 crate。

Verification: `cargo build -p wjsm-pm`

Steps:

- [ ] **写占位测试**。创建 `crates/wjsm-pm/src/lib.rs`：
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
  创建 `crates/wjsm-pm/src/store/mod.rs`：`// CAS 存储引擎入口`（空模块占位）。
- [ ] **Verify RED/编译失败**：此时 `store` 模块文件不完整会编译失败，先建最小 `store/mod.rs` 内容为空注释即可通过。运行 `cargo build -p wjsm-pm` 预期报 members 未注册错误。
- [ ] **最小代码**。根 `Cargo.toml` `members` 追加 `"crates/wjsm-pm"`；`[workspace.dependencies]` 追加：
  ```toml
  rusqlite = { version = "0.32", features = ["bundled"] }
  zstd = "0.13"
  blake3 = "1"
  tar = "0.4"
  flate2 = "1"
  sha2 = "0.10"
  base64 = "0.22"
  pubgrub = "0.2"
  serde_yaml = "0.9"
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
  base64 = { workspace = true }
  pubgrub = { workspace = true }
  reqwest = { workspace = true }
  tokio = { workspace = true }
  toml = { workspace = true }
  serde = { workspace = true }
  serde_json = { workspace = true }
  serde_yaml = { workspace = true }
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
      pub fn append(&mut self, content: &[u8]) -> Result<BlobLoc> {
          let compressed = zstd::encode_all(content, 19).context("zstd 压缩 blob")?;
          let offset = self.offset;
          self.file.write_all(&compressed).context("追加 blob 到 packfile")?;
          self.offset += compressed.len() as u64;
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
- [ ] **最小代码**：上面 `PackWriter`（含 `len`/`sync`/`pack_id`）即完整。轮转决策**不在** blob 层——`PackWriter` 只对单一 `pack_id` 负责；「活跃 pack 选择 + 超软上限换新 pack」是 `Store::active_writer`（任务 1.5）经 `index.active_pack_id()`/`bump_pack()` 决定，pack 元数据落 `packs` 表（任务 1.4）。此分层避免 blob 层扫描目录、避免两处各存一份"当前 pack"状态。
- [ ] **Verify GREEN**：`cargo nextest run -p wjsm-pm -E 'test(blob)'` 两个测试通过。
- [ ] **Commit**：`git commit -am "feat(wjsm-pm): blob 层 zstd+packfile 内容寻址"`

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
- [ ] **最小代码**：上面即完整。
- [ ] **Verify GREEN**：两个测试通过。
- [ ] **Commit**：`git commit -am "feat(wjsm-pm): 包文件清单 manifest 内容寻址"`

## 任务 1.4：SQLite 索引（index.db）

Files:
- 创建 `crates/wjsm-pm/src/store/index.rs`
- 修改 `crates/wjsm-pm/src/store/mod.rs`

Why: SQLite index.db（WAL）统管 packages/manifests/blobs/artifacts 映射，取代海量 JSON 小文件元数据。

Impact/Compatibility: 纯新增。事务写保证中断可回滚——本任务提供 `with_txn(|tx| …)` 入口（内部锁 `Mutex<Connection>` 后开 `rusqlite::Transaction`，成功 commit / 失败 rollback；**不暴露**裸 `transaction(&mut self)`，因 `Store` 经 `Arc` 共享、`conn` 在 `Mutex` 内），任务 1.5 的 `add_package_from_dir` **必须**经 `with_txn` 在单事务内写完 blobs+manifest+package，中断则整体回滚。

**与批准设计 §6.3 的偏离（显式记录）**：设计 §6.3 用规范化三表 `manifests(id) + manifest_entries(manifest_id, rel_path, blob_hash, mode)` + `packages.meta BLOB(MessagePack)`。本计划首版**仅**塌缩 manifest 存储为 `manifests(hash, body BLOB)`（manifest 整体 JSON 存 body），**保留** `packages.meta` 列（与设计对齐，本任务建列、写入见任务 3.2）。理由：首版读取路径只需"包→manifest→rel_path→blob"整体加载，不需按 rel_path 做 SQL 查询。**代价**：无法用 SQL 直接查"哪些包含某 rel_path"；`manifest_entries` 规范化（可查询性）推迟到需要 `wjsm store ls`/按文件反查时再实现（届时递增 store 版本 `v2`）。`packages.meta` 不属此偏离——它照设计保留。此偏离已在 executing 后补 ADR 时记录。

Verification: `cargo nextest run -p wjsm-pm -E 'test(index)'`

Steps:

- [ ] **写失败测试**。`store/index.rs`：
  ```rust
  // SQLite 索引：packages/manifests/blobs/artifacts（WAL 模式，单写多读）
  use crate::store::blob::{BlobHash, BlobLoc};
  use anyhow::{Context, Result};
  use rusqlite::{params, Connection};
  use std::path::Path;

  pub struct StoreIndex {
      conn: Connection,
  }

  const SCHEMA: &str = r#"
  CREATE TABLE IF NOT EXISTS blobs (
      hash BLOB PRIMARY KEY, pack_id INTEGER NOT NULL, offset INTEGER NOT NULL,
      clen INTEGER NOT NULL, ulen INTEGER NOT NULL
  );
  CREATE TABLE IF NOT EXISTS manifests (
      hash BLOB PRIMARY KEY, body BLOB NOT NULL
  );
  CREATE TABLE IF NOT EXISTS packages (
      name TEXT NOT NULL, version TEXT NOT NULL, integrity TEXT NOT NULL,
      manifest_hash BLOB NOT NULL,
      meta BLOB,  -- package.json 关键字段 MessagePack 快照（对齐设计 §6.3，供 solver 免二次拉取）；本任务建列，写入见任务 3.2
      PRIMARY KEY (name, version)
  );
  CREATE TABLE IF NOT EXISTS artifacts (
      cache_key BLOB PRIMARY KEY, tier INTEGER NOT NULL,
      pack_id INTEGER, offset INTEGER, clen INTEGER, ulen INTEGER
  );
  -- packfile 元数据：当前活动 pack 与已封存字节（gc 计算孤儿字节的基准）
  CREATE TABLE IF NOT EXISTS packs (
      pack_id INTEGER PRIMARY KEY, committed_len INTEGER NOT NULL
  );
  "#;

  impl StoreIndex {
      pub fn open(db_path: &Path) -> Result<Self> {
          if let Some(parent) = db_path.parent() {
              std::fs::create_dir_all(parent)?;
          }
          let conn = Connection::open(db_path).context("打开 index.db")?;
          conn.pragma_update(None, "journal_mode", "WAL")?;
          conn.execute_batch(SCHEMA)?;
          Ok(Self { conn })
      }

      pub fn get_blob(&self, hash: &BlobHash) -> Result<Option<BlobLoc>> {
          let mut stmt = self.conn.prepare("SELECT pack_id,offset,clen,ulen FROM blobs WHERE hash=?1")?;
          let r = stmt.query_row(params![hash.as_slice()], |row| {
              Ok(BlobLoc {
                  pack_id: row.get::<_, i64>(0)? as u32,
                  offset: row.get::<_, i64>(1)? as u64,
                  clen: row.get::<_, i64>(2)? as u32,
                  ulen: row.get::<_, i64>(3)? as u32,
              })
          });
          match r {
              Ok(loc) => Ok(Some(loc)),
              Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
              Err(e) => Err(e.into()),
          }
      }

      pub fn put_blob(&self, hash: &BlobHash, loc: BlobLoc) -> Result<()> {
          self.conn.execute(
              "INSERT OR IGNORE INTO blobs(hash,pack_id,offset,clen,ulen) VALUES(?1,?2,?3,?4,?5)",
              params![hash.as_slice(), loc.pack_id as i64, loc.offset as i64, loc.clen as i64, loc.ulen as i64],
          )?;
          Ok(())
      }

      pub fn put_package(&self, name: &str, version: &str, integrity: &str, manifest_hash: &BlobHash) -> Result<()> {
          self.conn.execute(
              "INSERT OR REPLACE INTO packages(name,version,integrity,manifest_hash) VALUES(?1,?2,?3,?4)",
              params![name, version, integrity, manifest_hash.as_slice()],
          )?;
          Ok(())
      }

      pub fn get_package_manifest(&self, name: &str, version: &str) -> Result<Option<BlobHash>> {
          let mut stmt = self.conn.prepare("SELECT manifest_hash FROM packages WHERE name=?1 AND version=?2")?;
          let r = stmt.query_row(params![name, version], |row| {
              let v: Vec<u8> = row.get(0)?;
              Ok(v)
          });
          match r {
              Ok(v) => {
                  let mut h = [0u8; 32];
                  h.copy_from_slice(&v);
                  Ok(Some(h))
              }
              Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
              Err(e) => Err(e.into()),
          }
      }
  }

  #[cfg(test)]
  mod tests {
      use super::*;

      fn temp_db(name: &str) -> std::path::PathBuf {
          let d = std::env::temp_dir().join(format!("wjsm_pm_idx_{name}_{}", std::process::id()));
          let _ = std::fs::remove_dir_all(&d);
          d.join("index.db")
      }

      #[test]
      fn blob_put_get() {
          let idx = StoreIndex::open(&temp_db("blob")).unwrap();
          let h = [7u8; 32];
          assert!(idx.get_blob(&h).unwrap().is_none());
          let loc = BlobLoc { pack_id: 0, offset: 10, clen: 5, ulen: 20 };
          idx.put_blob(&h, loc).unwrap();
          assert_eq!(idx.get_blob(&h).unwrap().unwrap(), loc);
      }

      #[test]
      fn package_put_get() {
          let idx = StoreIndex::open(&temp_db("pkg")).unwrap();
          let mh = [3u8; 32];
          idx.put_package("lodash", "4.17.21", "sha512-abc", &mh).unwrap();
          assert_eq!(idx.get_package_manifest("lodash", "4.17.21").unwrap().unwrap(), mh);
          assert!(idx.get_package_manifest("lodash", "0.0.0").unwrap().is_none());
      }
  }
  ```
  `store/mod.rs` 追加 `pub mod index;`
- [ ] **Verify RED**：`cargo nextest run -p wjsm-pm -E 'test(index)'`。
- [ ] **最小代码**：**先把上面代码里的 `conn: Connection` 改为 `conn: std::sync::Mutex<Connection>`**（`Store` 经 `Arc` 共享，事务需内部可变；所有 `self.conn.execute`/`prepare` 改为先 `self.conn.lock().unwrap()` 借出 guard 再调用）。再补齐下列（供任务 1.5 / GC / 求解使用），并各加一个 `index_*` 测试：
  - `put_manifest_raw(hash, body)` / `get_manifest_raw(hash)`（manifest body 存取，任务 1.5 用）。
  - `with_txn<R>(&self, f: impl FnOnce(&Transaction) -> Result<R>) -> Result<R>`：锁 `Mutex<Connection>` 后开事务、执行闭包、成功 `commit`/失败 `rollback`。任务 1.5 用它把「写全部 blob + manifest + package」包进单事务，中断整体回滚。（不暴露裸 `transaction(&mut self)`，因 `Store` 经 `Arc` 共享、`conn` 在 `Mutex` 内。）
  - **事务作用域自由函数**（模块级 `fn`，非 `&self` 方法）：`txn_put_blob(tx: &Transaction, hash, loc)` / `txn_put_manifest_raw(tx, hash, body)` / `txn_put_package(tx, name, version, integrity, manifest_hash)`。它们直接在传入的 `&Transaction` 上 `execute`，**不触碰 `Mutex`**——因为 `with_txn` 的闭包运行时锁已被 `with_txn` 持有，闭包内若再调 `&self` 的 `put_*` 方法会二次 `lock()` 同一 `Mutex` **死锁**。任务 1.5 的 `add_package_from_dir` 只能经这三个 `txn_*` 自由函数写入。`&self` 的 `put_blob`/`put_manifest_raw`/`put_package` 保留供事务外单发写入（各自 `lock()` 一次），与 `txn_*` 共享同一 SQL（可让 `&self` 方法内部 `lock` 后转调 `txn_*` 复用 SQL，避免两份语句）。
  - `active_pack_id()` / `bump_pack() -> Result<u32>`：读写 `packs` 表当前活跃 pack（任务 1.2 pack 轮转的索引侧）。
  - `reachable_blob_hashes() -> HashSet<BlobHash>`：`packages`→`manifests`(body 反序列化 entries) 收集全部可达 blob；`gc` 用它标记，未被引用的 blobs 行 + packfile 尾部孤儿字节为可回收（GC 实现见任务 1.5b 的 `store::gc`）。
  - `packages.meta` 列建列即可，写入 package.json 关键字段 MessagePack 快照在任务 3.2 install 编排补（对齐设计 §6.3，供 solver 免二次拉取）。
- [ ] **Verify GREEN**：全部 `index_*` 测试通过。
- [ ] **Commit**：`git commit -am "feat(wjsm-pm): SQLite index.db（blobs/manifests/packages/artifacts/packs + 事务 + GC 支持）"`

## 任务 1.5：Store 统一入口（写包 + 读文件事务）

Files:
- 修改 `crates/wjsm-pm/src/store/mod.rs`

Why: 把 blob/manifest/index 组装成 `Store`——`add_package_from_dir`（解包目录→blob→manifest→事务入库）与 `read_package_file`（包+相对路径→源码）。这是编译器直供的读取路径。

Impact/Compatibility: 纯新增。

Verification: `cargo nextest run -p wjsm-pm -E 'test(store_integration)'`

Steps:

- [ ] **写失败测试**。`store/mod.rs`（替换占位内容）：
  ```rust
  // CAS 存储引擎：blob + manifest + index 组装
  pub mod blob;
  pub mod index;
  pub mod manifest;

  use anyhow::{Context, Result};
  use blob::{hash_content, read_blob, PackWriter};
  use index::StoreIndex;
  use manifest::{Manifest, ManifestEntry};
  use std::path::{Path, PathBuf};

  pub const STORE_VERSION: &str = "v1";

  /// 单 packfile 软上限（超过则轮转到下一 pack，避免单文件无限膨胀）。
  const PACK_ROTATE_BYTES: u64 = 512 * 1024 * 1024;

  pub struct Store {
      root: PathBuf,
      packs_dir: PathBuf,
      pub(crate) index: StoreIndex,
  }

  impl Store {
      pub fn open(store_root: &Path) -> Result<Self> {
          let root = store_root.join(STORE_VERSION);
          let packs_dir = root.join("packs");
          let index = StoreIndex::open(&root.join("index.db"))?;
          std::fs::create_dir_all(&packs_dir)?;
          Ok(Self { root, packs_dir, index })
      }

      /// 选取活跃 packfile；超过软上限则轮转到下一 pack_id（index.packs 表记录）。
      fn active_writer(&self) -> Result<PackWriter> {
          let mut pack_id = self.index.active_pack_id()?;
          let mut writer = PackWriter::open(&self.packs_dir, pack_id)?;
          if writer.len() >= PACK_ROTATE_BYTES {
              pack_id = self.index.bump_pack()?; // 原子递增活跃 pack_id
              writer = PackWriter::open(&self.packs_dir, pack_id)?;
          }
          Ok(writer)
      }

      /// 把一个解包后的包目录写入 CAS：每文件去重成 blob，构建 manifest，**整包在单事务内入库**。
      ///
      /// 原子性：blob 字节先追加进 packfile（追加式，中断只留孤儿尾字节，由 gc 回收），
      /// 但 index 的 blobs/manifests/packages 三表写入包裹在**单个 SQLite 事务**中——
      /// 中断则整体回滚，index 永不出现"包已登记但 manifest/blob 缺失"的半写状态。
      pub fn add_package_from_dir(&self, name: &str, version: &str, integrity: &str, dir: &Path) -> Result<()> {
          let mut writer = self.active_writer()?;
          // 阶段一：blob 落 packfile，收集 (hash, loc, 是否新) + manifest entries（不写 index）。
          let mut new_blobs: Vec<(BlobHash, BlobLoc)> = Vec::new();
          let mut entries = Vec::new();
          let mut stack = vec![dir.to_path_buf()];
          while let Some(cur) = stack.pop() {
              for ent in std::fs::read_dir(&cur)? {
                  let ent = ent?;
                  let path = ent.path();
                  let meta = ent.metadata()?;
                  if meta.is_dir() {
                      stack.push(path);
                      continue;
                  }
                  let content = std::fs::read(&path)?;
                  let h = hash_content(&content);
                  if self.index.get_blob(&h)?.is_none()
                      && !new_blobs.iter().any(|(bh, _)| bh == &h)
                  {
                      let loc = writer.append(&content)?;
                      new_blobs.push((h, loc));
                  }
                  let rel = path.strip_prefix(dir).unwrap().to_string_lossy().replace('\\', "/");
                  // mode：保留可执行位（bin 文件），其余归一化 0o644。
                  let mode = normalize_mode(&meta);
                  entries.push(ManifestEntry { rel_path: rel, blob_hash: h, mode });
              }
          }
          writer.sync()?; // fsync packfile，确保 blob 字节先于 index 落盘
          let m = Manifest::from_entries(entries);
          let body = serde_json::to_vec(&m.entries)?;
          let mh = m.hash();
          // 阶段二：单事务写 index（blobs + manifest + package 原子提交，失败回滚）。
          self.index.with_txn(|tx| {
              for (h, loc) in &new_blobs {
                  index::txn_put_blob(tx, h, *loc)?;
              }
              index::txn_put_manifest_raw(tx, &mh, &body)?;
              index::txn_put_package(tx, name, version, integrity, &mh)?;
              Ok(())
          })
      }

      fn load_manifest(&self, hash: &[u8; 32]) -> Result<Manifest> {
          let body = self.index.get_manifest_raw(hash)?.context("manifest 不存在")?;
          let entries: Vec<ManifestEntry> = serde_json::from_slice(&body)?;
          Ok(Manifest { entries })
      }

      /// 读取包内某文件源码（编译器直供路径）。
      pub fn read_package_file(&self, name: &str, version: &str, rel_path: &str) -> Result<Option<Vec<u8>>> {
          let Some(mh) = self.index.get_package_manifest(name, version)? else {
              return Ok(None);
          };
          let m = self.load_manifest(&mh)?;
          let Some(entry) = m.lookup(rel_path) else {
              return Ok(None);
          };
          let loc = self.index.get_blob(&entry.blob_hash)?.context("blob 索引缺失")?;
          Ok(Some(read_blob(&self.packs_dir, loc)?))
      }
  }

  #[cfg(test)]
  mod tests {
      use super::*;

      fn temp(name: &str) -> PathBuf {
          let d = std::env::temp_dir().join(format!("wjsm_pm_store_{name}_{}", std::process::id()));
          let _ = std::fs::remove_dir_all(&d);
          d
      }

      #[test]
      fn store_integration_add_and_read() {
          let root = temp("root");
          let pkg = temp("pkg");
          std::fs::create_dir_all(pkg.join("lib")).unwrap();
          std::fs::write(pkg.join("index.js"), b"export const v = 1;\n").unwrap();
          std::fs::write(pkg.join("lib/util.js"), b"export const u = 2;\n").unwrap();
          let store = Store::open(&root).unwrap();
          store.add_package_from_dir("demo", "1.0.0", "sha512-x", &pkg).unwrap();
          let got = store.read_package_file("demo", "1.0.0", "index.js").unwrap().unwrap();
          assert_eq!(got, b"export const v = 1;\n");
          let got2 = store.read_package_file("demo", "1.0.0", "lib/util.js").unwrap().unwrap();
          assert_eq!(got2, b"export const u = 2;\n");
          assert!(store.read_package_file("demo", "1.0.0", "missing.js").unwrap().is_none());
      }

      #[test]
      fn store_integration_dedup_shared_file() {
          let root = temp("dedup");
          let store = Store::open(&root).unwrap();
          for (n, v) in [("a", "1.0.0"), ("b", "1.0.0")] {
              let pkg = temp(&format!("pkg_{n}"));
              std::fs::create_dir_all(&pkg).unwrap();
              std::fs::write(pkg.join("LICENSE"), b"MIT SAME CONTENT").unwrap();
              store.add_package_from_dir(n, v, "sha512-y", &pkg).unwrap();
          }
          // 相同 LICENSE 内容 → 同一 blob（去重）
          let h = hash_content(b"MIT SAME CONTENT");
          assert!(store.index.get_blob(&h).unwrap().is_some());
      }
  }
  ```
  在 `store/index.rs` 补 `put_manifest_raw` / `get_manifest_raw`：
  ```rust
  impl StoreIndex {
      pub fn put_manifest_raw(&self, hash: &[u8; 32], body: &[u8]) -> anyhow::Result<()> {
          self.conn.execute(
              "INSERT OR IGNORE INTO manifests(hash,body) VALUES(?1,?2)",
              rusqlite::params![hash.as_slice(), body],
          )?;
          Ok(())
      }
      pub fn get_manifest_raw(&self, hash: &[u8; 32]) -> anyhow::Result<Option<Vec<u8>>> {
          let mut stmt = self.conn.prepare("SELECT body FROM manifests WHERE hash=?1")?;
          let r = stmt.query_row(rusqlite::params![hash.as_slice()], |row| row.get::<_, Vec<u8>>(0));
          match r {
              Ok(b) => Ok(Some(b)),
              Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
              Err(e) => Err(e.into()),
          }
      }
  }
  ```
  （`index` 字段 test 访问需 `pub(crate) index`——把 `Store.index` 改 `pub(crate)`。）
  **可变性**：`Store` 经 `Arc<Store>` 共享（CasVfs 持有），故 `add_package_from_dir(&self, …)` 是 `&self`；但 rusqlite `Connection::transaction()` 要求 `&mut Connection`。因此 `StoreIndex.conn` 用 `Mutex<Connection>` 包裹（`StoreIndex { conn: Mutex<Connection> }`），`with_txn()` 内 `lock()` 后 `conn.transaction()`——写路径串行化（与 WAL 单写模型一致），读路径同样经 `lock()`。这也满足 `Vfs: Send + Sync`（`CasVfs` 内 `Arc<Store>` 需 `Sync`）。`StoreIndex` 提供 `with_txn(&self, f)`（不暴露裸 `transaction(&mut self)`，因 `conn` 在 `Mutex` 内且 `Store` 经 `Arc` 共享）供 `add_package_from_dir` 包裹整包写入，把 blob/manifest/package 三类写入收进单个事务，保证「要么整包入库、要么整包回滚」，兑现 Impact 声明的原子性。
  `Store` 另需 `manifest_has_prefix(name, version, rel) -> Result<bool>`（供 CasVfs::is_dir 判定目录）：加载包 manifest，返回是否存在任一 entry 的 `rel_path` 以 `rel/` 为前缀（或等于 `rel` 的父目录链）。
  `normalize_mode(meta: &std::fs::Metadata) -> u32` 辅助：跨平台归一化文件模式——Unix 下保留可执行位（`0o755` 若 owner-exec 置位，否则 `0o644`）；非 Unix 恒 `0o644`。保证同内容文件在不同平台 manifest 一致（blob 去重不受 mode 影响，mode 只随 manifest entry）。
- [ ] **Verify RED**：`cargo nextest run -p wjsm-pm -E 'test(store_integration)'`。
- [ ] **最小代码**：上面即完整。新增第三个测试 `store_integration_atomic_rollback`：构造一个中途 `add_package_from_dir` 失败（如注入一个不可读文件），断言失败后 `read_package_file` 返回 `None`（package 行未提交），证明事务回滚。
- [ ] **Verify GREEN**：三测试通过。
- [ ] **Commit**：`git commit -am "feat(wjsm-pm): Store 统一入口（写包+读文件，整包事务+pack 轮转）"`

## 任务 1.5b：store gc（回收孤儿 blob + packfile 重写）

Files:
- 创建 `crates/wjsm-pm/src/store/gc.rs`
- 修改 `crates/wjsm-pm/src/store/mod.rs`
- 修改 `crates/wjsm-cli/src/cli_args.rs`（`cache gc` 子命令，或复用现有 `Cache`）

Why: packfile 是追加式；写中断的孤儿字节、以及被 `--prune` 移除的包，其 blob 会成为不可达垃圾。设计 §6.2/§6.3 明示由 `wjsm store gc` 回收。前序任务（1.2/1.4/1.5）已把 gc 引为「后续任务实现」——此任务兑现，不留悬空能力。

Impact/Compatibility: 纯新增。gc 走「标记-复制」：`reachable_blob_hashes`（index 已提供）标记可达 blob，把可达 blob 复制进新 packfile，原子替换 `packs/` 与 blobs 表位置，删除旧 pack。gc 期间加 store 级文件锁串行化。

Verification: `cargo nextest run -p wjsm-pm -E 'test(gc)'`

Steps:

- [ ] **写失败测试**。`store/gc.rs`：`gc(store) -> GcStats { reclaimed_blobs, reclaimed_bytes }`。测试 `gc_reclaims_orphan_blob`：向 store 写一个包 A，直接经 `PackWriter` 追加一个不被任何 manifest 引用的孤儿 blob（模拟中断残留），运行 `gc`，断言孤儿 blob 被回收（`reclaimed_blobs >= 1`）且 A 的文件仍可 `read_package_file` 读出（可达 blob 保留、位置已重映射）。
- [ ] **Verify RED**：`cargo nextest run -p wjsm-pm -E 'test(gc)'`。
- [ ] **最小代码**：实现标记-复制 gc + store 级 flock（`fs2` 或 `std` advisory lock；workspace 若无 `fs2`，用 `O_EXCL` lock 文件）。CLI `cache gc` 调用 `wjsm_pm::store::gc`。
- [ ] **Verify GREEN**：测试通过。
- [ ] **Commit**：`git commit -am "feat(wjsm-pm): store gc 回收孤儿 blob（标记-复制 + 文件锁）"`

## 任务 1.6：wjsm-module 新增 Vfs/ResolutionOverlay trait（全量 fs 谓词抽象，默认零破坏）

Files:
- 创建 `crates/wjsm-module/src/vfs.rs`
- 修改 `crates/wjsm-module/src/lib.rs`（导出）
- 修改 `crates/wjsm-module/src/resolver.rs`（构造器接受 trait 对象；resolver 实际调用的 fs 谓词——`read_to_string`×1/`canonicalize`×12/`is_file`×7/`is_dir`×6——改 trait 调用。注：resolver 自身**不调用** `Path::exists`，`Vfs::exists` 仅为 `CasVfs` 内部 `is_file`/`is_dir` 复用与 trait 完备性而定义）
- 修改 `crates/wjsm-module/src/package_json.rs`（`read_package_info` 经 Vfs）
- 修改 `crates/wjsm-module/src/bundler.rs` / `graph.rs`（注入透传）

Why: 让 CAS 无缝切入解析且不反转依赖方向。**关键更正**：resolver.rs 的 fs 触点远不止三处（见 Plan Basis Facts）——`canonicalize`（12 处）、`is_file`（7 处）、`is_dir`（6 处）遍布整个解析算法。其中 `std::fs::canonicalize` 要求路径在真实磁盘存在，CAS 虚拟路径 `<vroot>/<name>@<ver>/…` 永不落盘，直接 canonicalize 必然失败。因此 CAS 切入**不是**"改三处读取"，而是"把 resolver 全部 fs 谓词路由进 `Vfs`，并让 `Vfs::canonicalize` 对虚拟路径做恒等归一化（去 `.`/`..`，不触碰磁盘）"。这是 resolver 级重构，是 P2–P4 的地基。

Impact/Compatibility: **最关键兼容任务**。`FsVfs` 的每个方法必须与被替换的 `std::fs`/`Path` 调用**语义逐字节等价**（含 `canonicalize` 的符号链接解析与错误信息）——现有 module 单测 + 全量 fixture 必须全绿。回归失败即视为等价性破坏，不得放行。

Verification: `cargo nextest run -p wjsm-module && cargo nextest run --workspace`

Steps:

- [ ] **写失败测试**。`vfs.rs`：
  ```rust
  // 虚拟文件系统抽象 + 解析覆盖层：让 CAS 无缝切入解析，不反转依赖方向。
  // Vfs 覆盖 ModuleResolver 解析算法的全部 fs 谓词——不止读取，还包括
  // canonicalize/is_file/is_dir/exists，因为 CAS 虚拟路径无法走真实磁盘 canonicalize。
  use anyhow::{Context, Result};
  use std::path::{Path, PathBuf};

  /// 源码/元数据读取 + 路径谓词抽象。默认 FsVfs = 现有 std::fs / Path 行为。
  pub trait Vfs: Send + Sync {
      fn read_to_string(&self, path: &Path) -> Result<String>;
      /// 归一化路径为规范绝对路径。FsVfs 走 std::fs::canonicalize（解析符号链接、要求存在）；
      /// CasVfs 对虚拟路径做纯词法归一化（合并 `.`/`..`，不触碰磁盘）。
      fn canonicalize(&self, path: &Path) -> Result<PathBuf>;
      fn is_file(&self, path: &Path) -> bool;
      fn is_dir(&self, path: &Path) -> bool;
      fn exists(&self, path: &Path) -> bool;
      fn read_package_json(&self, dir: &Path) -> Result<Option<String>>;
  }

  /// 解析覆盖层：(referrer 上下文 + bare specifier) → 虚拟树中的具体包根。
  /// None = 回退默认 node_modules 遍历。
  pub trait ResolutionOverlay: Send + Sync {
      fn resolve_bare(&self, specifier: &str, referrer: &Path) -> Result<Option<PathBuf>>;
  }

  /// 默认文件系统实现（现有行为，逐调用等价）。
  pub struct FsVfs;

  impl Vfs for FsVfs {
      fn read_to_string(&self, path: &Path) -> Result<String> {
          std::fs::read_to_string(path)
              .with_context(|| format!("Failed to read module: {}", path.display()))
      }
      fn canonicalize(&self, path: &Path) -> Result<PathBuf> {
          path.canonicalize()
              .with_context(|| format!("canonicalize {}", path.display()))
      }
      fn is_file(&self, path: &Path) -> bool { path.is_file() }
      fn is_dir(&self, path: &Path) -> bool { path.is_dir() }
      fn exists(&self, path: &Path) -> bool { path.exists() }
      fn read_package_json(&self, dir: &Path) -> Result<Option<String>> {
          let path = dir.join("package.json");
          match std::fs::read_to_string(&path) {
              Ok(s) => Ok(Some(s)),
              Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
              Err(e) => Err(e).with_context(|| format!("read package.json at {}", path.display())),
          }
      }
  }

  /// 默认无覆盖（回退现有 node_modules 遍历）。
  pub struct NoOverlay;

  impl ResolutionOverlay for NoOverlay {
      fn resolve_bare(&self, _specifier: &str, _referrer: &Path) -> Result<Option<PathBuf>> {
          Ok(None)
      }
  }

  #[cfg(test)]
  mod tests {
      use super::*;

      #[test]
      fn fs_vfs_predicates_match_std() {
          let dir = std::env::temp_dir().join(format!("wjsm_vfs_{}", std::process::id()));
          let _ = std::fs::remove_dir_all(&dir);
          std::fs::create_dir_all(&dir).unwrap();
          std::fs::write(dir.join("a.js"), "export const x=1;").unwrap();
          let vfs = FsVfs;
          assert_eq!(vfs.read_to_string(&dir.join("a.js")).unwrap(), "export const x=1;");
          assert!(vfs.is_file(&dir.join("a.js")));
          assert!(vfs.is_dir(&dir));
          assert!(vfs.exists(&dir.join("a.js")));
          assert!(!vfs.exists(&dir.join("missing.js")));
          // canonicalize 与 std 一致（要求存在）
          assert_eq!(vfs.canonicalize(&dir.join("a.js")).unwrap(),
                     dir.join("a.js").canonicalize().unwrap());
          assert!(vfs.canonicalize(&dir.join("missing.js")).is_err());
          assert!(vfs.read_package_json(&dir).unwrap().is_none());
          std::fs::write(dir.join("package.json"), r#"{"name":"x"}"#).unwrap();
          assert!(vfs.read_package_json(&dir).unwrap().is_some());
      }

      #[test]
      fn no_overlay_returns_none() {
          assert!(NoOverlay.resolve_bare("lodash", Path::new("/x")).unwrap().is_none());
      }
  }
  ```
  `vfs.rs` 追加纯词法归一化辅助（供 CasVfs::canonicalize 复用，供虚拟路径去 `.`/`..`，不触碰磁盘）：
  ```rust
  /// 纯词法路径归一化：合并 `.`/`..` 组件，保留其余，不解析符号链接、不访问磁盘。
  /// 用于 CasVfs 的虚拟路径（虚拟树无符号链接，无需真实 canonicalize）。
  pub fn normalize_virtual(path: &Path) -> PathBuf {
      use std::path::Component;
      let mut out = PathBuf::new();
      for comp in path.components() {
          match comp {
              Component::ParentDir => { out.pop(); }
              Component::CurDir => {}
              other => out.push(other.as_os_str()),
          }
      }
      out
  }
  ```
  `lib.rs` 追加 `mod vfs;` + `pub use vfs::{normalize_virtual, FsVfs, NoOverlay, ResolutionOverlay, Vfs};`
- [ ] **Verify RED**：`cargo nextest run -p wjsm-module -E 'test(vfs)'`。
- [ ] **最小代码 + 全量接入**：
  - `resolver.rs`：`ModuleResolver` 增字段 `vfs: Arc<dyn Vfs>`、`overlay: Arc<dyn ResolutionOverlay>`；`with_options` 默认注入 `Arc::new(FsVfs)` / `Arc::new(NoOverlay)`；新增 `with_providers(root, options, vfs, overlay)` 构造器。构造器里 `root_path.canonicalize()`（L91）改 `vfs.canonicalize`。
  - **全部 fs 谓词改经 `self.vfs`**（这是本任务核心，逐处改写，不遗漏）：
    - `canonicalize`：L334、343、359、380、454、469、593、600、609、631、667 全部 → `self.vfs.canonicalize(...)`。注意其中若干是关联函数（`Self::resolve_package_target_path`/`resolve_directory_index`/`canonical_entry_path` 为 `fn`（无 `&self`）），需改签名接受 `&dyn Vfs` 参数或提升为方法——按调用链最小改动提升为 `&self` 方法。
    - `is_file`：L453、468、592、599、608、630、663 → `self.vfs.is_file(...)`。
    - `is_dir`：L342、354、456、474、602、614 → `self.vfs.is_dir(...)`。
    - L754 `std::fs::read_to_string(&path)` → `self.vfs.read_to_string(&path)`。
  - `find_package_in_node_modules`（L328）起始处：先 `if let Some(root) = self.overlay.resolve_bare(package_name, from_dir)? { return Ok(Some(root)); }` 再回退现有遍历（遍历内 `candidate.is_dir()`/`canonicalize` 已按上条改经 vfs）。**注意**：`resolve_bare_specifier`（L242）在遍历前先查 `find_nearest_package`（L249）——overlay 命中路径须在 node_modules 遍历前生效，故 overlay 钩子放在 `find_package_in_node_modules` 入口即可覆盖 bare specifier 主路径。
  - `package_json.rs`：`read_package_info` 改签名 `read_package_info(dir, vfs: &dyn Vfs)`，内部 `fs::metadata`+`fs::read_to_string` → `vfs.read_package_json(dir)`（合并为一次调用，语义等价：Some→解析、None→Ok(None)）。resolver 的 `read_package_info`（L378）调用点传 `&*self.vfs`。
  - `bundler.rs`/`graph.rs`：`ModuleBundler` 增 `with_providers`，`ModuleGraph::build_with_providers` 透传到 `ModuleResolver::with_providers`；现有 `build_with_options`/`with_resolution_options` 内部用默认 provider（`Arc::new(FsVfs)`/`Arc::new(NoOverlay)`）。
- [ ] **Verify GREEN**：`cargo nextest run -p wjsm-module && cargo nextest run --workspace` 全绿（证明默认 FsVfs 与原 std 调用逐处等价、零破坏）。
- [ ] **Commit**：`git commit -am "feat(wjsm-module): Vfs/ResolutionOverlay trait 抽象全部 fs 谓词（默认零破坏）"`

---

# 阶段 P2：registry client + PubGrub solver

## 任务 2.1：npm 精确 SemVer 语义

Files:
- 创建 `crates/wjsm-pm/src/solver/mod.rs`
- 创建 `crates/wjsm-pm/src/solver/npm_semver.rs`
- 修改 `crates/wjsm-pm/src/lib.rs`

Why: npm 区间语义（`^`/`~`/x-range/hyphen/比较运算符/`||`/预发布包含规则）与通用 semver 有差异，必须精确匹配 node-semver。设计 §7.3 要求**精确匹配**——按项目 hard rule「No partial implementations」，本任务一次性覆盖 node-semver 全部区间形态，不留「后续按 fixture 补」的缺口。

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

  // 详见「最小代码」：split_hyphen / expand_lower_bound / expand_upper_bound / parse_single
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
- [ ] **最小代码**：补齐上面注释处的四个展开函数（严格按 node-semver）：
  - `split_hyphen(s)`：识别 ` - ` 分隔（两侧各是一个 partial），返回 `(lo_str, hi_str)`。
  - `expand_lower_bound(partial)`：`X`/缺省段 → `>=0.0.0`；`1` → `>=1.0.0`；`1.2` → `>=1.2.0`；`1.2.3` → `>=1.2.3`。
  - `expand_upper_bound(partial)`：`1` → `<2.0.0`；`1.2` → `<1.3.0`；`1.2.3` → `<=1.2.3`；含 `X` 段同理向上补齐。
  - `parse_single(tok)`：依次匹配前缀
    - `^`：`^1.2.3`→`>=1.2.3 <2.0.0`；`^0.2.3`→`>=0.2.3 <0.3.0`；`^0.0.3`→`>=0.0.3 <0.0.4`；`^1`/`^1.x`→`>=1.0.0 <2.0.0`；`^0`→`>=0.0.0 <1.0.0`；`^0.0`→`>=0.0.0 <0.1.0`。含 `x` 段的 `^` 按"缺省段视为 0、上界由最高非通配段决定"。
    - `~`：`~1.2.3`/`~1.2`→`>=… <(minor+1).0.0` 修正为 `<major.(minor+1).0`；`~1`→`>=1.0.0 <2.0.0`。
    - `>=`/`>`/`<=`/`<`/`=`：解析运算符后对 partial 补齐为具体 SemVer（缺省段补 0），生成单个 `Comparator`。
    - x-range / 精确：`1.2.x`→`>=1.2.0 <1.3.0`；`1.x`→`>=1.0.0 <2.0.0`；`1.2.3`→`=1.2.3`（`Op::Eq`）。
  - 所有展开产出的 comparator 中，**上界 `<X.Y.Z`（无预发布）保持无预发布**，从而 `set_matches` 的预发布包含规则正确排除跨版本预发布。
- [ ] **Verify GREEN**：全部 `npm_semver_*` 测试通过（含 `prerelease_inclusion_rule`）。
- [ ] **Commit**：`git commit -am "feat(wjsm-pm): npm 精确 SemVer 区间语义（node-semver 全形态 + 预发布包含规则）"`

## 任务 2.2：registry client（packument + SSRI + tarball）

Files:
- 创建 `crates/wjsm-pm/src/registry/{mod,packument,tarball,npmrc}.rs`
- 创建 `crates/wjsm-pm/tests/mock_registry.rs`
- 修改 `crates/wjsm-pm/src/lib.rs`

Why: 从 registry 拉 packument 元数据、按 SSRI 校验 tarball 完整性、解包 tgz。内置离线 mock registry 保证测试确定。

Impact/Compatibility: 纯新增。SSRI 校验失败必须拒绝入库（安全边界）。

Verification: `cargo nextest run -p wjsm-pm -E 'test(registry) | test(mock_registry)'`

Steps:

- [ ] **写失败测试**。`registry/tarball.rs`（SSRI + 解包，先做纯函数最易测）：
  ```rust
  // tarball：SSRI 完整性校验 + tgz 流式解包
  use anyhow::{bail, Result};
  use base64::Engine;
  use sha2::{Digest, Sha512};
  use std::io::Read;
  use std::path::Path;

  /// 校验 tarball 字节匹配 npm SSRI（形如 "sha512-<base64>"）。
  pub fn verify_integrity(bytes: &[u8], ssri: &str) -> Result<()> {
      let Some(b64) = ssri.strip_prefix("sha512-") else {
          bail!("仅支持 sha512 SSRI: {ssri}");
      };
      let expected = base64::engine::general_purpose::STANDARD.decode(b64)?;
      let actual = Sha512::digest(bytes);
      if actual.as_slice() != expected.as_slice() {
          bail!("tarball 完整性校验失败");
      }
      Ok(())
  }

  /// 解包 tgz 到目标目录，剥离顶层 "package/" 前缀（npm 约定）。
  pub fn extract_tgz(bytes: &[u8], dest: &Path) -> Result<()> {
      let gz = flate2::read::GzDecoder::new(bytes);
      let mut ar = tar::Archive::new(gz);
      for entry in ar.entries()? {
          let mut entry = entry?;
          let path = entry.path()?.to_path_buf();
          let rel = path.strip_prefix("package").unwrap_or(&path);
          if rel.as_os_str().is_empty() {
              continue;
          }
          let out = dest.join(rel);
          if let Some(parent) = out.parent() {
              std::fs::create_dir_all(parent)?;
          }
          let mut buf = Vec::new();
          entry.read_to_end(&mut buf)?;
          std::fs::write(&out, buf)?;
      }
      Ok(())
  }

  #[cfg(test)]
  mod tests {
      use super::*;

      #[test]
      fn registry_ssri_verify() {
          let data = b"hello tarball";
          let ssri = format!("sha512-{}", base64::engine::general_purpose::STANDARD.encode(Sha512::digest(data)));
          assert!(verify_integrity(data, &ssri).is_ok());
          assert!(verify_integrity(b"tampered", &ssri).is_err());
      }

      #[test]
      fn registry_extract_strips_package_prefix() {
          // 构造一个内存 tgz：package/index.js
          let mut tar_buf = Vec::new();
          {
              let mut b = tar::Builder::new(&mut tar_buf);
              let content = b"export const x=1;";
              let mut header = tar::Header::new_gnu();
              header.set_size(content.len() as u64);
              header.set_cksum();
              b.append_data(&mut header, "package/index.js", &content[..]).unwrap();
              b.finish().unwrap();
          }
          let mut gz = Vec::new();
          {
              use std::io::Write;
              let mut enc = flate2::write::GzEncoder::new(&mut gz, flate2::Compression::default());
              enc.write_all(&tar_buf).unwrap();
              enc.finish().unwrap();
          }
          let dest = std::env::temp_dir().join(format!("wjsm_pm_tgz_{}", std::process::id()));
          let _ = std::fs::remove_dir_all(&dest);
          extract_tgz(&gz, &dest).unwrap();
          assert_eq!(std::fs::read(dest.join("index.js")).unwrap(), b"export const x=1;");
      }
  }
  ```
  `registry/packument.rs`（元数据结构 + 解析）：
  ```rust
  // packument：registry GET /<pkg> 的元数据解析
  use crate::solver::npm_semver::SemVer;
  use anyhow::Result;
  use serde::Deserialize;
  use std::collections::BTreeMap;

  #[derive(Debug, Deserialize)]
  pub struct Packument {
      #[serde(default)]
      pub versions: BTreeMap<String, VersionMeta>,
  }

  #[derive(Debug, Deserialize, Clone)]
  pub struct VersionMeta {
      pub version: String,
      #[serde(default)]
      pub dependencies: BTreeMap<String, String>,
      #[serde(default, rename = "peerDependencies")]
      pub peer_dependencies: BTreeMap<String, String>,
      #[serde(default, rename = "optionalDependencies")]
      pub optional_dependencies: BTreeMap<String, String>,
      pub dist: Dist,
  }

  #[derive(Debug, Deserialize, Clone)]
  pub struct Dist {
      pub tarball: String,
      pub integrity: String,
  }

  impl Packument {
      pub fn parse(json: &str) -> Result<Self> {
          Ok(serde_json::from_str(json)?)
      }
      /// 满足 range 的最高版本。
      pub fn best_match(&self, range: &crate::solver::npm_semver::Range) -> Option<VersionMeta> {
          self.versions
              .values()
              .filter(|m| SemVer::parse(&m.version).map_or(false, |v| range.matches(&v)))
              .max_by(|a, b| SemVer::parse(&a.version).unwrap().cmp(&SemVer::parse(&b.version).unwrap()))
              .cloned()
      }
  }

  #[cfg(test)]
  mod tests {
      use super::*;
      use crate::solver::npm_semver::Range;

      #[test]
      fn registry_packument_best_match() {
          let json = r#"{"versions":{
            "1.0.0":{"version":"1.0.0","dist":{"tarball":"http://x/1.0.0.tgz","integrity":"sha512-a"}},
            "1.2.0":{"version":"1.2.0","dist":{"tarball":"http://x/1.2.0.tgz","integrity":"sha512-b"}},
            "2.0.0":{"version":"2.0.0","dist":{"tarball":"http://x/2.0.0.tgz","integrity":"sha512-c"}}
          }}"#;
          let p = Packument::parse(json).unwrap();
          let m = p.best_match(&Range::parse("^1.0.0").unwrap()).unwrap();
          assert_eq!(m.version, "1.2.0");
      }
  }
  ```
  `registry/npmrc.rs`（最小 registry/token 解析）：
  ```rust
  // .npmrc：registry URL / scope / _authToken 解析
  use std::collections::HashMap;

  #[derive(Debug, Default, Clone)]
  pub struct NpmrcConfig {
      pub default_registry: Option<String>,
      pub scope_registry: HashMap<String, String>,
      pub auth_tokens: HashMap<String, String>,
  }

  impl NpmrcConfig {
      pub fn parse(text: &str) -> Self {
          let mut cfg = NpmrcConfig::default();
          for line in text.lines() {
              let line = line.trim();
              if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                  continue;
              }
              if let Some((k, val)) = line.split_once('=') {
                  let (k, val) = (k.trim(), val.trim());
                  if k == "registry" {
                      cfg.default_registry = Some(val.to_string());
                  } else if let Some(scope) = k.strip_suffix(":registry") {
                      cfg.scope_registry.insert(scope.to_string(), val.to_string());
                  } else if let Some(host) = k.strip_suffix(":_authToken") {
                      cfg.auth_tokens.insert(host.trim_start_matches("//").to_string(), val.to_string());
                  }
              }
          }
          cfg
      }
  }

  #[cfg(test)]
  mod tests {
      use super::*;
      #[test]
      fn registry_npmrc_parse() {
          let cfg = NpmrcConfig::parse("registry=https://r.example\n@my:registry=https://my.example\n//my.example/:_authToken=TOK\n");
          assert_eq!(cfg.default_registry.as_deref(), Some("https://r.example"));
          assert_eq!(cfg.scope_registry.get("@my").unwrap(), "https://my.example");
          assert_eq!(cfg.auth_tokens.get("my.example/").unwrap(), "TOK");
      }
  }
  ```
  `registry/mod.rs`：`pub mod npmrc; pub mod packument; pub mod tarball;` + async `fetch_packument`/`fetch_tarball`（reqwest，本任务只写签名 + 一个 `#[ignore]` 联网测试；离线覆盖靠上面纯函数 + mock_registry.rs）。
  `tests/mock_registry.rs`：起本地 `tokio` HTTP server（用 reqwest 对端或简单 `std::net::TcpListener` + 固定响应）返回 packument JSON + 内存 tgz，端到端验证 `fetch_packument`→`best_match`→`fetch_tarball`→`verify_integrity`→`extract_tgz`→`Store::add_package_from_dir`。
  `lib.rs`：`pub mod registry;`
- [ ] **Verify RED**：`cargo nextest run -p wjsm-pm -E 'test(registry)'`。
- [ ] **最小代码**：上面纯函数即完整；`registry/mod.rs` 的 async fetch 用 reqwest 直连。
- [ ] **Verify GREEN**：`cargo nextest run -p wjsm-pm -E 'test(registry) | test(mock_registry)'` 通过。
- [ ] **Commit**：`git commit -am "feat(wjsm-pm): registry client（packument/SSRI/tarball/npmrc）+ mock registry"`

## 任务 2.3：PubGrub DependencyProvider + instance-splitting

Files:
- 创建 `crates/wjsm-pm/src/solver/{provider,duplication,explain}.rs`
- 修改 `crates/wjsm-pm/src/solver/mod.rs`

Why: PubGrub 内核求"去重最大化"解；instance-splitting 在单版本不可满足时分裂实例，复现 npm 嵌套多版本共存语义。这是本设计相对纯 PubGrub 工具的 npm 适配核心。

**求解语义（明确定义，不 hand-wave）**：
- **依赖类型处理**：`dependencies` 为硬约束（参与求解，不可满足即失败）；`peerDependencies` 翻译为对**求解全局**的约束——包 `a@1.0.0` 的 peer `react@^17` 表示"最终图中被 `a` 看见的 `react` 必须满足 `^17`"，在 PubGrub 中建模为 `a@1.0.0` 依赖 `react`（区间 `^17`）的一条边，与普通 dependency 同参与求解，但**不触发 instance-splitting**（peer 冲突是真失败，见下）；`optionalDependencies` 求解失败**跳过**（不判全局失败），实现为在 `get_dependencies` 中对 optional 边捕获 `NoVersion` 后忽略该边。
- **instance-splitting 触发条件**：仅当**普通 dependency** 的同名包无单版本满足全部 dependent 的区间交集时触发——按 dependent 子树把该包分裂为多个实例（`c` → `c#a`、`c#b`），各实例独立求版本。peer 冲突**不分裂**（peer 的语义是"共享同一实例"，分裂会违反 peer 契约）。
- **peer 冲突 = 真失败**：两个包对同一 peer 要求不相交区间（`^17` vs `^18`）→ 无法共享单实例、又不可分裂 → `NoSolution`，`explain` 输出派生链，必须提及冲突的 peer 名。

Impact/Compatibility: 纯新增。

Verification: `cargo nextest run -p wjsm-pm -E 'test(solver)'`

Steps:

- [ ] **Spike 首步：锁定 pubgrub API**。运行 `cargo doc -p pubgrub --no-deps 2>/dev/null; grep -rn "trait DependencyProvider" ~/.cargo/registry/src/*/pubgrub-*/src/ | head`，确认 `DependencyProvider` 关联类型（`P`/`V`/`VS`/`get_dependencies`/`choose_version`）签名，写进 `provider.rs` 顶部注释。若 0.2 API 与预期不符，锁定实际版本号更新 Cargo.toml。
- [ ] **定义 `MockIndex` 与 `ResolvedGraph` 契约**（测试地基，先写清签名）。`solver/mod.rs`：
  - `MockIndex::new()` → builder；`.pkg(name, version, deps: &[(name, range)])` 追加一个版本及其普通依赖；`.peer(name, version, peers: &[(name, range)])` 为已存在的 `(name,version)` 追加 peer 约束；`.optional(name, version, opts)` 追加 optional 边。builder 实现 `provider::PackageIndex`（`versions_of(name)` / `deps_of(name, version)` / `peers_of` / `optionals_of`）。
  - `ResolvedGraph { instances: Vec<ResolvedInstance> }`；`ResolvedInstance { name, version, deps: Vec<(String, InstanceId)> }`；`instances_of(name) -> Vec<&ResolvedInstance>`。
  - `solve(idx, root_name, root_version) -> Result<ResolvedGraph, SolveError>`；`SolveError::explanation() -> String`。
- [ ] **写失败测试**。`solver/provider.rs` 实现 `DependencyProvider`：`Package = String`（含 instance 后缀 `name#owner`）、`Version = SemVer`、依赖从 `PackageIndex` 取（惰性）。`solver/duplication.rs` 实现 instance-splitting：
  ```rust
  // instance-splitting：PubGrub 单版本不可满足时，按 dependent 子树分裂实例，复现 npm 嵌套多版本
  // （测试驱动的核心场景）
  #[cfg(test)]
  mod tests {
      use crate::solver::{solve, ResolvedGraph};
      use crate::solver::test_support::MockIndex;

      #[test]
      fn solver_single_version_dedup() {
          // root → a@^1, b@^1；a → c@^1；b → c@^1 → c 收敛单版本
          let idx = MockIndex::new()
              .pkg("root", "1.0.0", &[("a", "^1"), ("b", "^1")])
              .pkg("a", "1.0.0", &[("c", "^1")])
              .pkg("b", "1.0.0", &[("c", "^1")])
              .pkg("c", "1.5.0", &[]);
          let g: ResolvedGraph = solve(&idx, "root", "1.0.0").unwrap();
          assert_eq!(g.instances_of("c").len(), 1, "可去重时 c 应单版本");
      }

      #[test]
      fn solver_instance_split_multi_version() {
          // a → c@^1；b → c@^2；c 无单版本解 → 分裂两实例（npm 能装）
          let idx = MockIndex::new()
              .pkg("root", "1.0.0", &[("a", "^1"), ("b", "^1")])
              .pkg("a", "1.0.0", &[("c", "^1")])
              .pkg("b", "1.0.0", &[("c", "^2")])
              .pkg("c", "1.9.0", &[])
              .pkg("c", "2.1.0", &[]);
          let g = solve(&idx, "root", "1.0.0").unwrap();
          let versions: Vec<_> = g.instances_of("c").iter().map(|i| i.version.clone()).collect();
          assert!(versions.contains(&"1.9.0".to_string()) && versions.contains(&"2.1.0".to_string()),
                  "冲突时 c 应分裂为 1.x 与 2.x 两实例");
      }

      #[test]
      fn solver_peer_conflict_explains() {
          // peer 硬冲突 → 真失败 + 解释
          let idx = MockIndex::new()
              .pkg("root", "1.0.0", &[("a", "^1"), ("b", "^1")])
              .pkg("a", "1.0.0", &[]).peer("a", "1.0.0", &[("react", "^17")])
              .pkg("b", "1.0.0", &[]).peer("b", "1.0.0", &[("react", "^18")])
              .pkg("react", "17.0.0", &[]).pkg("react", "18.0.0", &[]);
          let err = solve(&idx, "root", "1.0.0").unwrap_err();
          assert!(err.explanation().contains("react"), "解释应指出 react peer 冲突");
      }

      #[test]
      fn solver_optional_dep_missing_is_skipped() {
          // a 的 optional 依赖 fsevents 在 index 中不存在 → 不判全局失败，图中无 fsevents
          let idx = MockIndex::new()
              .pkg("root", "1.0.0", &[("a", "^1")])
              .pkg("a", "1.0.0", &[])
              .optional("a", "1.0.0", &[("fsevents", "^2")]);
          let g = solve(&idx, "root", "1.0.0").unwrap();
          assert!(g.instances_of("fsevents").is_empty(), "缺失的 optional 依赖应被跳过");
          assert_eq!(g.instances_of("a").len(), 1);
      }
  }
  ```
  `solver/mod.rs` 定义 `solve`、`ResolvedGraph`、`MockIndex`、`explain.rs` 的解释构造（签名见上一步）。
- [ ] **Verify RED**：`cargo nextest run -p wjsm-pm -E 'test(solver)'`。
- [ ] **最小代码**：实现 `solve`：
  - 先跑 PubGrub 单版本（普通 dependency + peer 均作硬边，optional 边在 `get_dependencies` 中对缺失版本捕获并忽略）。
  - 捕获 `NoSolution` 时区分成因：若冲突源是**普通 dependency** 的同名包区间不交 → 触发 duplication，按 dependent 子树把该包分裂为实例（`c#a`/`c#b`）递归求解各子锥，合并 `ResolvedGraph`；若冲突源含 **peer** 约束 → 不分裂，直接 `explain` 产出 PubGrub 派生链并返回 `SolveError`。
  - `explain` 从 PubGrub 的 `DerivationTree` 提取涉及的包名，保证冲突 peer 名出现在输出。
- [ ] **Verify GREEN**：四测试通过（去重 / 分裂 / peer 冲突 / optional 跳过）。
- [ ] **Commit**：`git commit -am "feat(wjsm-pm): PubGrub solver + instance-splitting（npm 嵌套多版本）"`

---

# 阶段 P3：install / lockfile / CLI + 编译器接入

## 任务 3.1：自有 lockfile（wjsm-lock.toml）+ 迁移读取

Files:
- 创建 `crates/wjsm-pm/src/lockfile/{mod,wjsm_lock,migrate}.rs`
- 修改 `crates/wjsm-pm/src/lib.rs`

Why: 确定性 lockfile 记录解析图 + integrity + 实例身份；迁移读取 package-lock/pnpm-lock/yarn.lock/bun.lock 无缝接管存量项目。

Impact/Compatibility: 纯新增。不删除原生态 lockfile（除非 `--prune`）。

Verification: `cargo nextest run -p wjsm-pm -E 'test(lockfile)'`

Steps:

- [ ] **写失败测试**。`lockfile/wjsm_lock.rs`（确定性往返）+ `lockfile/migrate.rs`（读 package-lock v3）：
  ```rust
  // wjsm-lock.toml：解析图 + integrity + 实例身份，确定性序列化
  use serde::{Deserialize, Serialize};

  #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
  pub struct WjsmLock {
      pub compiler_version: String,
      /// 顶层项目直接依赖：dep_name → 解析后的具体 version（PnpOverlay 根依赖表）。
      #[serde(default)]
      pub root_deps: Vec<(String, String)>,
      /// 稳定排序的解析条目。
      pub packages: Vec<LockedPackage>,
  }

  #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
  pub struct LockedPackage {
      pub name: String,
      pub version: String,
      pub integrity: String,
      /// 解析后的依赖边：dep_name → 具体 version。
      pub deps: Vec<(String, String)>,
  }

  impl WjsmLock {
      pub fn to_toml(&self) -> String {
          let mut sorted = self.clone();
          sorted.packages.sort_by(|a, b| (&a.name, &a.version).cmp(&(&b.name, &b.version)));
          sorted.root_deps.sort();
          for p in &mut sorted.packages {
              p.deps.sort();
          }
          toml::to_string_pretty(&sorted).expect("lockfile 序列化")
      }
      pub fn from_toml(s: &str) -> anyhow::Result<Self> {
          Ok(toml::from_str(s)?)
      }
  }

  #[cfg(test)]
  mod tests {
      use super::*;
      #[test]
      fn lockfile_deterministic_roundtrip() {
          let lock = WjsmLock {
              compiler_version: "0.1.0".into(),
              root_deps: vec![("a".into(), "1.0.0".into())],
              packages: vec![
                  LockedPackage { name: "b".into(), version: "1.0.0".into(), integrity: "sha512-b".into(), deps: vec![] },
                  LockedPackage { name: "a".into(), version: "1.0.0".into(), integrity: "sha512-a".into(), deps: vec![("b".into(), "1.0.0".into())] },
              ],
          };
          let s1 = lock.to_toml();
          let round = WjsmLock::from_toml(&s1).unwrap();
          // 排序稳定：a 在 b 前
          assert!(s1.find("\"a\"").unwrap() < s1.find("\"b\"").unwrap());
          assert_eq!(round.to_toml(), s1);
      }
  }
  ```
  `lockfile/migrate.rs`：
  ```rust
  // 迁移读取：package-lock.json v3 → 已固定版本作为求解提示
  use std::collections::BTreeMap;

  pub fn read_package_lock_v3(json: &str) -> anyhow::Result<BTreeMap<String, String>> {
      let v: serde_json::Value = serde_json::from_str(json)?;
      let mut pinned = BTreeMap::new();
      if let Some(pkgs) = v.get("packages").and_then(|p| p.as_object()) {
          for (path, meta) in pkgs {
              if path.is_empty() { continue; } // 根包跳过
              let name = path.rsplit("node_modules/").next().unwrap_or(path).to_string();
              if let Some(ver) = meta.get("version").and_then(|x| x.as_str()) {
                  pinned.insert(name, ver.to_string());
              }
          }
      }
      Ok(pinned)
  }

  #[cfg(test)]
  mod tests {
      use super::*;
      #[test]
      fn lockfile_migrate_package_lock_v3() {
          let json = r#"{"lockfileVersion":3,"packages":{
            "":{"name":"root"},
            "node_modules/lodash":{"version":"4.17.21"},
            "node_modules/react":{"version":"18.2.0"}
          }}"#;
          let pinned = read_package_lock_v3(json).unwrap();
          assert_eq!(pinned.get("lodash").unwrap(), "4.17.21");
          assert_eq!(pinned.get("react").unwrap(), "18.2.0");
      }
  }
  ```
  `lockfile/mod.rs`：`pub mod migrate; pub mod wjsm_lock;`（pnpm-lock/yarn.lock/bun.lock 迁移各补一函数 + 一测试，结构同上，本任务先落 package-lock v3 + 骨架，其余三格式在同任务追加对应 `read_pnpm_lock`/`read_yarn_lock`/`read_bun_lock` + 各一测试）。`lib.rs`：`pub mod lockfile;`
- [ ] **Verify RED**：`cargo nextest run -p wjsm-pm -E 'test(lockfile)'`。
- [ ] **最小代码**：上面即完整；补齐三格式迁移函数。
- [ ] **Verify GREEN**：全部 lockfile 测试通过。
- [ ] **Commit**：`git commit -am "feat(wjsm-pm): wjsm-lock.toml + 迁移读取（package-lock/pnpm/yarn/bun）"`

## 任务 3.2：install 编排 + CasVfs/PnpOverlay

Files:
- 创建 `crates/wjsm-pm/src/store/{vfs,overlay}.rs`
- 修改 `crates/wjsm-pm/src/lib.rs`（`install` 公共 API）

Why: install 编排 = 读 package.json → solve → 下载 → 写 CAS → 写 lockfile。CasVfs/PnpOverlay 实现 module 侧 trait，把 CAS 包呈现为虚拟树供编译器读。

Impact/Compatibility: 纯新增。CasVfs 读路径全程零文件系统物化。

Verification: `cargo nextest run -p wjsm-pm -E 'test(install) | test(cas_vfs)'`

Steps:

- [ ] **写失败测试**。`store/vfs.rs`（`CasVfs` impl `wjsm_module::Vfs`——**必须实现 trait 全部方法**：`read_to_string`/`is_file`/`is_dir`/`exists`/`canonicalize`/`read_package_json`）。**关键更正**：包目录段是 `<name>@<version>`，而 scoped 包名 `@babel/core` 含 `/`（两个路径组件），且 `rsplit_once('@')` 对 `@babel/core@7.0.0` 会误拆。虚拟根编码统一为**单段 percent-encode**：把包目录段编码为 `<name>@<version>`，其中 `name` 内的 `/` 与 `@` 用 `%2F`/`%40` 转义（`@babel%2Fcore@7.0.0`），保证包目录恒为**单个路径组件**、拆分无歧义；rel 为其后所有组件。
  ```rust
  // CasVfs：从 CAS store 读源码。虚拟路径 <vroot>/<encoded_pkg@ver>/<rel...>
  // encoded_pkg 把 name 中的 / → %2F、@ → %40，保证包目录为单一路径组件（scoped 包安全）。
  use crate::store::Store;
  use anyhow::Result;
  use std::path::{Path, PathBuf};
  use std::sync::Arc;

  pub struct CasVfs {
      store: Arc<Store>,
      vroot: PathBuf,
  }

  /// 把 name@version 编码为单一路径组件（scoped 包 / 与 @ 转义）。
  pub fn encode_pkg_dir(name: &str, version: &str) -> String {
      let enc = name.replace('%', "%25").replace('/', "%2F").replace('@', "%40");
      format!("{enc}@{version}")
  }
  fn decode_pkg_dir(seg: &str) -> Option<(String, String)> {
      // version 无 @，故最后一个 '@' 为分隔符
      let (enc_name, version) = seg.rsplit_once('@')?;
      let name = enc_name.replace("%40", "@").replace("%2F", "/").replace("%25", "%");
      Some((name, version.to_string()))
  }

  impl CasVfs {
      pub fn new(store: Arc<Store>, vroot: PathBuf) -> Self { Self { store, vroot } }
      /// 虚拟路径 → (name, version, rel_path)。非虚拟路径返回 None。
      fn split(&self, path: &Path) -> Option<(String, String, String)> {
          let rel = path.strip_prefix(&self.vroot).ok()?;
          let mut comps = rel.components();
          let pkgver = comps.next()?.as_os_str().to_string_lossy().to_string();
          let (name, version) = decode_pkg_dir(&pkgver)?;
          let rest = comps.as_path().to_string_lossy().replace('\\', "/");
          Some((name, version, rest))
      }
      /// rel 是否为包内已存在文件（用于 is_file / exists）。
      fn has_file(&self, n: &str, v: &str, rel: &str) -> bool {
          self.store.read_package_file(n, v, rel).ok().flatten().is_some()
      }
      /// rel 是否为包内某文件的前缀目录（用于 is_dir）。
      fn is_dir_prefix(&self, n: &str, v: &str, rel: &str) -> bool {
          self.store.manifest_has_prefix(n, v, rel).unwrap_or(false)
      }
  }

  impl wjsm_module::Vfs for CasVfs {
      fn read_to_string(&self, path: &Path) -> Result<String> {
          let (n, v, rel) = self.split(path).ok_or_else(|| anyhow::anyhow!("非虚拟路径: {}", path.display()))?;
          let bytes = self.store.read_package_file(&n, &v, &rel)?
              .ok_or_else(|| anyhow::anyhow!("CAS 缺文件: {n}@{v}/{rel}"))?;
          Ok(String::from_utf8(bytes)?)
      }
      fn is_file(&self, path: &Path) -> bool {
          self.split(path).map_or(false, |(n, v, rel)| !rel.is_empty() && self.has_file(&n, &v, &rel))
      }
      fn is_dir(&self, path: &Path) -> bool {
          // rel 为空 = 包根目录；否则为存在文件的前缀目录
          self.split(path).map_or(false, |(n, v, rel)| rel.is_empty() || self.is_dir_prefix(&n, &v, &rel))
      }
      fn exists(&self, path: &Path) -> bool { self.is_file(path) || self.is_dir(path) }
      fn canonicalize(&self, path: &Path) -> Result<PathBuf> {
          // 虚拟路径恒等归一化：去 . / ..，不触碰磁盘（虚拟树无符号链接）。
          Ok(wjsm_module::normalize_virtual(path))
      }
      fn read_package_json(&self, dir: &Path) -> Result<Option<String>> {
          let Some((n, v, rel)) = self.split(dir) else { return Ok(None) };
          let key = if rel.is_empty() { "package.json".to_string() } else { format!("{rel}/package.json") };
          Ok(self.store.read_package_file(&n, &v, &key)?.map(|b| String::from_utf8_lossy(&b).into_owned()))
      }
  }

  #[cfg(test)]
  mod tests {
      use super::*;

      #[test]
      fn cas_vfs_reads_scoped_and_dirs() {
          let root = std::env::temp_dir().join(format!("wjsm_pm_casvfs_{}", std::process::id()));
          let _ = std::fs::remove_dir_all(&root);
          let pkg = root.join("src_pkg");
          std::fs::create_dir_all(pkg.join("lib")).unwrap();
          std::fs::write(pkg.join("index.js"), b"export const v=1;").unwrap();
          std::fs::write(pkg.join("lib/util.js"), b"export const u=2;").unwrap();
          let store = Arc::new(Store::open(&root.join("store")).unwrap());
          store.add_package_from_dir("@babel/core", "7.0.0", "sha512-x", &pkg).unwrap();
          let vroot = PathBuf::from("/virt");
          let vfs = CasVfs::new(store, vroot.clone());
          let dir = vroot.join(encode_pkg_dir("@babel/core", "7.0.0"));
          use wjsm_module::Vfs;
          assert_eq!(vfs.read_to_string(&dir.join("index.js")).unwrap(), "export const v=1;");
          assert!(vfs.is_file(&dir.join("index.js")));
          assert!(vfs.is_dir(&dir));              // 包根
          assert!(vfs.is_dir(&dir.join("lib")));  // 前缀目录
          assert!(!vfs.is_file(&dir.join("lib"))); // 目录不是文件
          assert!(!vfs.exists(&dir.join("missing.js")));
      }
  }
  ```
  `store/overlay.rs`（`PnpOverlay`）：按 lockfile 把 `(referrer 所属包上下文, bare specifier)` 映射到虚拟包根。referrer 是虚拟路径 → 用 `CasVfs::split`/`decode_pkg_dir` 反查其所属包 → 在 lockfile 中查该包对该 specifier 解析到的具体 `(name, version)` → 返回 `vroot.join(encode_pkg_dir(name, version))`。顶层项目（referrer 在项目目录、非 vroot 下）用 lockfile 的根依赖表。
  ```rust
  // PnpOverlay：bare specifier → 虚拟包根，边解析严格按 lockfile（复现 npm 嵌套多版本）
  use crate::lockfile::WjsmLock;
  use crate::store::vfs::{decode_pkg_dir, encode_pkg_dir};
  use anyhow::Result;
  use std::collections::HashMap;
  use std::path::{Path, PathBuf};

  pub struct PnpOverlay {
      vroot: PathBuf,
      project_dir: PathBuf,
      /// (dependent_name, dependent_version) → (dep_name → resolved_version)
      edges: HashMap<(String, String), HashMap<String, String>>,
      /// 顶层项目直接依赖：dep_name → resolved_version
      root_deps: HashMap<String, String>,
  }

  impl PnpOverlay {
      pub fn from_lock(lock: &WjsmLock, vroot: PathBuf, project_dir: PathBuf) -> Self {
          let mut edges = HashMap::new();
          for p in &lock.packages {
              let map: HashMap<_, _> = p.deps.iter().cloned().collect();
              edges.insert((p.name.clone(), p.version.clone()), map);
          }
          let root_deps = lock.root_deps.iter().cloned().collect();
          Self { vroot, project_dir, edges, root_deps }
      }
      /// referrer 所属包 (name, version)：虚拟路径 → 解码；项目路径 → None（走 root_deps）。
      fn owner_of(&self, referrer: &Path) -> Option<(String, String)> {
          let rel = referrer.strip_prefix(&self.vroot).ok()?;
          let seg = rel.components().next()?.as_os_str().to_string_lossy();
          decode_pkg_dir(&seg)
      }
  }

  impl wjsm_module::ResolutionOverlay for PnpOverlay {
      fn resolve_bare(&self, specifier: &str, referrer: &Path) -> Result<Option<PathBuf>> {
          // 取包名（去子路径）：@scope/name/... → @scope/name；name/... → name
          let pkg = split_pkg_name(specifier);
          let resolved = match self.owner_of(referrer) {
              Some(owner) => self.edges.get(&owner).and_then(|m| m.get(&pkg)),
              None if referrer.starts_with(&self.project_dir) => self.root_deps.get(&pkg),
              None => None,
          };
          Ok(resolved.map(|ver| self.vroot.join(encode_pkg_dir(&pkg, ver))))
      }
  }

  fn split_pkg_name(specifier: &str) -> String {
      if let Some(rest) = specifier.strip_prefix('@') {
          let mut it = rest.splitn(3, '/');
          match (it.next(), it.next()) { (Some(s), Some(n)) => format!("@{s}/{n}"), _ => specifier.to_string() }
      } else {
          specifier.split('/').next().unwrap_or(specifier).to_string()
      }
  }

  #[cfg(test)]
  mod tests {
      use super::*;
      use crate::lockfile::{LockedPackage, WjsmLock};

      #[test]
      fn pnp_overlay_maps_edge_and_root() {
          let lock = WjsmLock {
              compiler_version: "0".into(),
              root_deps: vec![("a".into(), "1.0.0".into())],
              packages: vec![LockedPackage {
                  name: "a".into(), version: "1.0.0".into(), integrity: "sha512-a".into(),
                  deps: vec![("c".into(), "2.0.0".into())],
              }],
          };
          let vroot = PathBuf::from("/virt");
          let proj = PathBuf::from("/proj");
          let ov = PnpOverlay::from_lock(&lock, vroot.clone(), proj.clone());
          use wjsm_module::ResolutionOverlay;
          // 顶层项目 → root_deps
          assert_eq!(ov.resolve_bare("a", &proj.join("main.js")).unwrap(),
                     Some(vroot.join(encode_pkg_dir("a", "1.0.0"))));
          // 包 a 内部 import 'c' → 边表解析到 c@2.0.0
          let a_ref = vroot.join(encode_pkg_dir("a", "1.0.0")).join("index.js");
          assert_eq!(ov.resolve_bare("c", &a_ref).unwrap(),
                     Some(vroot.join(encode_pkg_dir("c", "2.0.0"))));
          // 子路径 specifier 取包名
          assert_eq!(ov.resolve_bare("c/sub/x.js", &a_ref).unwrap(),
                     Some(vroot.join(encode_pkg_dir("c", "2.0.0"))));
      }
  }
  ```
  `store/mod.rs` 补 `manifest_has_prefix(name, version, rel_prefix)`（load_manifest 后判断是否存在以 `rel_prefix + "/"` 起始或等于的 entry）。`lockfile/wjsm_lock.rs` 的 `WjsmLock` 增 `root_deps: Vec<(String,String)>` 字段（顶层直接依赖，任务 3.1 一并加）。`lib.rs` 增 `pub async fn install(project_dir: &Path, store: &Store) -> Result<WjsmLock>`：读 package.json deps → `solve` → 对每个包 `fetch_tarball`+`verify_integrity`+`extract`+`add_package_from_dir` → 组装含 `root_deps` + 每包 `deps` 边的 `WjsmLock` → 返回。加一个用 mock registry 的 `install_end_to_end` 测试。
- [ ] **Verify RED**：`cargo nextest run -p wjsm-pm -E 'test(cas_vfs) | test(pnp_overlay) | test(install)'`。
- [ ] **最小代码**：上面 + `manifest_has_prefix` + install 编排。
- [ ] **Verify GREEN**：测试通过。
- [ ] **Commit**：`git commit -am "feat(wjsm-pm): install 编排 + CasVfs/PnpOverlay 编译器直供"`

## 任务 3.3：CLI 子命令 install/add/remove

Files:
- 修改 `crates/wjsm-cli/src/cli_args.rs`（新增子命令）
- 修改 `crates/wjsm-cli/src/lib.rs`（dispatch）
- 创建 `crates/wjsm-cli/src/pm_commands.rs`
- 修改 `crates/wjsm-cli/Cargo.toml`（加 `wjsm-pm` 依赖）

Why: 暴露 `wjsm install/add/remove`，承接 `npm install`。这是无缝替换 npm 的入口。

Impact/Compatibility: 新增子命令，不改现有命令。`wjsm-cli` 首次依赖 `wjsm-pm`。

Verification: `cargo build -p wjsm-cli && cargo run -- install --help`

Steps:

- [ ] **写失败测试**。`pm_commands.rs`：
  ```rust
  // 包管理 CLI 命令：install/add/remove
  use anyhow::Result;
  use std::path::Path;

  pub fn cmd_install(project_dir: &Path) -> Result<()> {
      let store_root = default_store_root()?;
      let store = wjsm_pm::store::Store::open(&store_root)?;
      let rt = tokio::runtime::Runtime::new()?;
      let lock = rt.block_on(wjsm_pm::install(project_dir, &store))?;
      std::fs::write(project_dir.join("wjsm-lock.toml"), lock.to_toml())?;
      println!("已安装 {} 个包，无 node_modules", lock.packages.len());
      Ok(())
  }

  pub fn default_store_root() -> Result<std::path::PathBuf> {
      if let Ok(dir) = std::env::var("WJSM_STORE_DIR") {
          return Ok(dir.into());
      }
      let home = std::env::var("HOME").map_err(|_| anyhow::anyhow!("无 HOME"))?;
      Ok(std::path::Path::new(&home).join(".wjsm").join("store"))
  }

  #[cfg(test)]
  mod tests {
      use super::*;
      #[test]
      fn pm_store_root_respects_env() {
          // SAFETY: 测试内串行设置
          unsafe { std::env::set_var("WJSM_STORE_DIR", "/tmp/wjsm_test_store"); }
          assert_eq!(default_store_root().unwrap(), std::path::PathBuf::from("/tmp/wjsm_test_store"));
          unsafe { std::env::remove_var("WJSM_STORE_DIR"); }
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
  },
  /// Add a dependency (npm install <pkg> 等价)
  Add { pkg: String, #[arg(default_value = ".")] dir: std::path::PathBuf },
  /// Remove a dependency (npm uninstall 等价)
  Remove { pkg: String, #[arg(default_value = ".")] dir: std::path::PathBuf },
  ```
  `lib.rs` dispatch（L365 match 内）追加：
  ```rust
  Commands::Install { ref dir } => pm_commands::cmd_install(dir).map(|_| ExitCode::SUCCESS).unwrap_or(ExitCode::FAILURE),
  Commands::Add { ref pkg, ref dir } => pm_commands::cmd_add(pkg, dir).map(|_| ExitCode::SUCCESS).unwrap_or(ExitCode::FAILURE),
  Commands::Remove { ref pkg, ref dir } => pm_commands::cmd_remove(pkg, dir).map(|_| ExitCode::SUCCESS).unwrap_or(ExitCode::FAILURE),
  ```
  `lib.rs` 顶部 `mod pm_commands;`；`Cargo.toml` 加 `wjsm-pm = { path = "../wjsm-pm" }`。
- [ ] **Verify RED**：`cargo build -p wjsm-cli` 预期先因 `cmd_add`/`cmd_remove` 未定义失败。
- [ ] **最小代码**：补 `cmd_add`（读 package.json → 加 dep → 调 install）、`cmd_remove`（删 dep → 重解析）。
- [ ] **Verify GREEN**：`cargo build -p wjsm-cli && cargo run -- install --help && cargo nextest run -p wjsm-cli -E 'test(pm_)'`。
- [ ] **Commit**：`git commit -am "feat(cli): wjsm install/add/remove 子命令"`

## 任务 3.4：编译器接入 CAS（run/build 惰性补齐）+ 集成测试

Files:
- 修改 `crates/wjsm-cli/src/lib.rs`（run/build 命令检测 lockfile → 注入 CasVfs/PnpOverlay）
- 创建 `crates/wjsm-cli/tests/pm_run_from_cas.rs`（CLI 集成测试）

**命名约定澄清**（修正原计划前后不一致）：pm 场景**不走** `tests/fixture_runner.rs` 的 `.expected` 快照 harness——该 harness 的 `build.rs` 只识别 `happy`/`errors`/`modules` 三个 suite（已核对 `build.rs:8`），且无法表达「先 install 再 run 再断言无 node_modules」的多步编排。因此 pm 端到端一律用 **crate 内 `#[test]` 集成测试**（`crates/wjsm-cli/tests/*.rs`、`crates/wjsm-pm/tests/*.rs`），测试函数名统一以 `pm_` 前缀命名，过滤器统一为 `test(pm_)`。不新建 `fixtures/pm/` 目录。

Why: 让 `wjsm run/build` 检测到 lockfile 时经 CasVfs 直接从 CAS 编译执行依赖，无 node_modules。这是纯惰性模型 + 编译器直供的端到端闭环。

Impact/Compatibility: run/build 仅在存在 wjsm-lock.toml + 依赖时启用 CAS 注入；无 lockfile 走现有 FsVfs 路径（零破坏）。

Verification: `cargo nextest run -p wjsm-cli -E 'test(pm_run_from_cas)'` + 冒烟无 node_modules

Steps:

- [ ] **写失败测试**。新增 `crates/wjsm-cli/tests/pm_run_from_cas.rs`，测试 `pm_run_from_cas`：构造临时项目（package.json 依赖 demo，用预置 store 或内置 mock registry），`wjsm install` 后 `run_file_in_process` 跑入口 `import {v} from 'demo'; console.log(v)`，断言 stdout 含预期值且项目目录**无 `node_modules`**。
- [ ] **Verify RED**：运行预期失败（run 尚未注入 CAS）。
- [ ] **最小代码**：`lib.rs` 的 `cmd_run`/`cmd_build` 前置：探测 `<dir>/wjsm-lock.toml`，存在则用 `ModuleBundler::with_providers(root, options, Arc::new(CasVfs::new(...)), Arc::new(PnpOverlay::from_lock(...)))` 替代默认 bundler。
- [ ] **Verify GREEN**：`cargo nextest run -p wjsm-cli -E 'test(pm_run_from_cas)'` 通过，断言项目无 node_modules。
- [ ] **Commit**：`git commit -am "feat(cli): run/build 惰性接入 CAS（无 node_modules 直供编译器）"`

---

# 阶段 P4：task / x / workspaces

## 任务 4.1：wjsm task（scripts + pre/post + PATH 注入）

Files:
- 创建 `crates/wjsm-pm/src/scripts/mod.rs`
- 修改 `crates/wjsm-cli/src/cli_args.rs` / `lib.rs` / `pm_commands.rs`

Why: `wjsm task <name>` 承接 `npm run <name>`，执行 package.json scripts + pre/post 生命周期，PATH 注入 wjsm。与 `wjsm run <file>` 语义正交（deno 式）。

Impact/Compatibility: 新增子命令。`run` 遇 script 名且文件不存在时提示 `did you mean 'wjsm task <name>'?`，不改行为。

Verification: `cargo nextest run -p wjsm-pm -E 'test(scripts)'`

Steps:

- [ ] **写失败测试**。`scripts/mod.rs`：
  ```rust
  // task runner：解析 scripts + pre/post 生命周期顺序
  use serde_json::Value;

  /// 返回给定 script 的执行序列：[pre<name>, <name>, post<name>]（存在才含）。
  pub fn resolve_script_sequence(pkg_json: &str, name: &str) -> anyhow::Result<Vec<(String, String)>> {
      let v: Value = serde_json::from_str(pkg_json)?;
      let scripts = v.get("scripts").and_then(|s| s.as_object());
      let Some(scripts) = scripts else { anyhow::bail!("package.json 无 scripts") };
      let mut seq = Vec::new();
      for key in [format!("pre{name}"), name.to_string(), format!("post{name}")] {
          if let Some(cmd) = scripts.get(&key).and_then(|c| c.as_str()) {
              seq.push((key, cmd.to_string()));
          }
      }
      anyhow::ensure!(seq.iter().any(|(k, _)| k == name), "script '{name}' 不存在");
      Ok(seq)
  }

  #[cfg(test)]
  mod tests {
      use super::*;
      #[test]
      fn scripts_pre_post_sequence() {
          let pj = r#"{"scripts":{"prebuild":"echo pre","build":"echo build","postbuild":"echo post","test":"echo t"}}"#;
          let seq = resolve_script_sequence(pj, "build").unwrap();
          assert_eq!(seq.iter().map(|(k,_)| k.as_str()).collect::<Vec<_>>(), vec!["prebuild","build","postbuild"]);
          let seq2 = resolve_script_sequence(pj, "test").unwrap();
          assert_eq!(seq2.len(), 1);
          assert!(resolve_script_sequence(pj, "missing").is_err());
      }
  }
  ```
  CLI：`Task { name: String, #[arg(default_value=".")] dir: PathBuf }` 子命令 + dispatch + `cmd_task`（按序列 `std::process::Command` 执行，env PATH 前置 wjsm 所在目录；`run` 命令补 did-you-mean 提示）。
- [ ] **Verify RED**：`cargo nextest run -p wjsm-pm -E 'test(scripts)'`。
- [ ] **最小代码**：上面 + `cmd_task`。
- [ ] **Verify GREEN**：测试通过 + `cargo run -- task --help`。
- [ ] **Commit**：`git commit -am "feat: wjsm task（scripts+pre/post+PATH 注入）"`

## 任务 4.2：wjsm x（npx 等价）+ workspaces

Files:
- 修改 `crates/wjsm-pm/src/scripts/mod.rs`（bin 解析）
- 创建 `crates/wjsm-pm/src/workspace.rs`
- 修改 CLI（`X` 子命令 + workspace 发现）

Why: `wjsm x <pkg>` 临时拉取执行包 bin（承接 npx）；workspaces 支持 monorepo 本地包链接 + 根 lockfile。

Impact/Compatibility: 新增。workspace 本地包以虚拟链接接入 PnpOverlay。

Verification: `cargo nextest run -p wjsm-pm -E 'test(workspace) | test(bin)'`

Steps:

- [ ] **写失败测试**。`workspace.rs`：
  ```rust
  // workspaces：package.json workspaces 字段 glob 发现本地包
  use serde_json::Value;
  use std::path::{Path, PathBuf};

  pub fn discover_workspace_members(root_pkg_json: &str, root_dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
      let v: Value = serde_json::from_str(root_pkg_json)?;
      let globs = match v.get("workspaces") {
          Some(Value::Array(a)) => a.iter().filter_map(|x| x.as_str().map(String::from)).collect::<Vec<_>>(),
          Some(Value::Object(o)) => o.get("packages").and_then(|p| p.as_array())
              .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default(),
          _ => return Ok(vec![]),
      };
      let mut members = Vec::new();
      for g in globs {
          // 支持 "packages/*" 形式
          if let Some(prefix) = g.strip_suffix("/*") {
              let base = root_dir.join(prefix);
              if base.is_dir() {
                  for ent in std::fs::read_dir(&base)? {
                      let p = ent?.path();
                      if p.join("package.json").exists() { members.push(p); }
                  }
              }
          } else {
              let p = root_dir.join(&g);
              if p.join("package.json").exists() { members.push(p); }
          }
      }
      members.sort();
      Ok(members)
  }

  #[cfg(test)]
  mod tests {
      use super::*;
      #[test]
      fn workspace_discovers_glob_members() {
          let root = std::env::temp_dir().join(format!("wjsm_pm_ws_{}", std::process::id()));
          let _ = std::fs::remove_dir_all(&root);
          for name in ["a", "b"] {
              let d = root.join("packages").join(name);
              std::fs::create_dir_all(&d).unwrap();
              std::fs::write(d.join("package.json"), format!(r#"{{"name":"{name}"}}"#)).unwrap();
          }
          let members = discover_workspace_members(r#"{"workspaces":["packages/*"]}"#, &root).unwrap();
          assert_eq!(members.len(), 2);
      }
  }
  ```
  `scripts/mod.rs` 加 `resolve_package_bin`（读包 package.json `bin` 字段）+ 测试；CLI `X { pkg: String, args: Vec<String> }` + `cmd_x`（拉包→解析 bin→执行）。
- [ ] **Verify RED**：`cargo nextest run -p wjsm-pm -E 'test(workspace) | test(bin)'`。
- [ ] **最小代码**：上面 + cmd_x。
- [ ] **Verify GREEN**：测试通过。
- [ ] **Commit**：`git commit -am "feat: wjsm x（npx 等价）+ workspaces 发现"`

---

# 阶段 P5：分层编译产物缓存（前置 #312）

> 前置检查：确认 issue #312 已合并（`git log --oneline | grep -i "312\|runtime module loading"` 或验证 `wjsm-runtime` 已有分离编译 loader）。若未合并，暂停 P5，先完成 #312。

## 任务 5.1：可重定位 IR — 单包 lower + 重定位表

Files:
- 创建 `crates/wjsm-semantic/src/relocatable/{mod,lower_one,relocate}.rs`
- 修改 `crates/wjsm-semantic/src/lib.rs`（导出）

Why: L1 跨项目复用的命门是 scope id 项目无关化。单独 lower 一个包 → scope id 局部化的模块 IR 片段 + 重定位表（scope 基址、常量偏移、字符串偏移、未解析 import 符号）。

**关键更正（已核对 lowerer_modules.rs）**：现有 `lower_modules` 的 scope 布局**不是**「模块 scope 从 0 起」——根作用域 `$0` 被全局对象 `$0.$global`（emit_global_constants，L452）与 hoisted var 占用；各模块顶层是 `predeclare_module_exports`（L144）按解析图 BFS **交错** push 在 `$0` 之下的 **Block** 作用域。因此原计划断言 `min_scope_id() == 0` 与现状语义冲突（0 是全局根，非模块根）。修正：`lower_one` 产出的模块局部 scope 以**约定基址 `LOCAL_SCOPE_BASE`（≥1，避开全局根 0）**起始，重定位时整体加 bundle 分配的 `scope_base`。`Relocations` 显式区分「指向本模块 scope 的引用」（重定位）与「指向全局根 `$0.$global` 的引用」（链接期固定映射到全局 0，不加基址）。

Impact/Compatibility: 新增路径；现有 `lower_modules` 整体路径不变。产出必须能经 `link`（任务 5.2）重定位回等价全局 IR。

Verification: `cargo nextest run -p wjsm-semantic -E 'test(relocatable_ir)'`

Steps:

- [ ] **Spike 首步：核对 scope 与引用种类清单**。读 `lowerer_modules.rs` 的 `predeclare_module_exports`/`emit_global_constants`/`create_namespace_objects`/`process_import_aliases`，列出一个模块局部 IR 会出现的**全部位置相关引用种类**（写进 `relocate.rs` 顶部注释作为重定位表 schema 的依据）：① `${scope_id}.{name}` 变量名中的 scope id；② 全局常量池索引 `cN`；③ DataSection 字符串偏移（`USER_STRING_START` 之后）；④ 跨模块 import 绑定符号（`export_map` 里 `(module_id, name) → ir_name`）；⑤ 对全局根 `$0.$global`/namespace object 的引用（**不重定位**，链接期固定）。
- [ ] **写失败测试**。`relocatable/mod.rs` 定义 `RelocatableModule { local_program: Program, relocations: Relocations }` 与 `pub const LOCAL_SCOPE_BASE: usize = 1;` 及 `lower_one(ast: swc_ast::Module, metadata: ModuleMetadata) -> Result<RelocatableModule, LoweringError>`。`Relocations` 含 `scope_refs: Vec<ScopeRef>`（每项：IR 位置 + 局部 scope id）、`const_refs`/`string_refs`/`import_refs`。
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
- [ ] **最小代码**：实现 `lower_one`——复用现有单模块 lower 逻辑，但 `ScopeTree` 以 `LOCAL_SCOPE_BASE` 为根偏移起始；扫描产出 IR 按 Spike 清单收集①–④进 `Relocations`，⑤类引用打标为「全局固定」不入重定位表。
- [ ] **Verify GREEN**：测试通过。
- [ ] **Commit**：`git commit -am "feat(wjsm-semantic): 可重定位 IR 单包 lower + 重定位表"`

## 任务 5.2：链接阶段 — 分级逐指令等价性

Files:
- 创建 `crates/wjsm-semantic/src/relocatable/link.rs`
- 修改 `crates/wjsm-semantic/src/relocatable/mod.rs`

Why: 把多个局部 IR 片段按 bundle 位置重定位合并为全局 Program，产出须与现有 `lower_modules` 整体路径**逐指令等价**——这是 L1 正确性的命门。**风险与分级**：`lower_modules` 存在跨模块耦合（`$0.$global`、entry-block 顺序发射的全局常量 `emit_global_constants`、`predeclare_module_exports` 按解析图 BFS 交错分配 scope、`shared_env_stack`/live-binding 依赖 `binding_owner_function_scope == current_function_scope_id`）。一次性对任意模块图达到逐指令等价是研究级里程碑，不能假设一步到位。故本任务**分三级验收，逐级放宽输入**，每级独立 commit，前一级绿了才做下一级：
- **L2-a**：单个无 import/无跨模块引用的叶子包（只有本地 const/function/字符串）。
- **L2-b**：两个模块，一条 `import { x } from './a'` 边（覆盖 import 符号重定位 + 命名空间）。
- **L2-c**：三个模块含 re-export、live-binding、共享 env（覆盖 `$global`/`shared_env` 交互）。

Impact/Compatibility: 新增。等价性是硬验收（重定位偏差是静默错误代码，非崩溃）。L2-c 若在预算内无法达成逐指令等价，**降级路径明确**：该场景走 §8.4 L2-bundle 整包兜底（整 bundle 编译，不分离），不阻塞 L2-a/L2-b 的跨项目复用收益，并在 commit message + ADR 记录未覆盖场景。

Verification: `cargo nextest run -p wjsm-semantic -E 'test(relocatable_ir_equivalence)'`

Steps:

- [ ] **写失败测试（可编译、无占位符）**。`link.rs` 测试。**注意**：`lower_modules` 需 6 个 map 入参，测试用一个 `build_bundle` 辅助从源码构造它们（在 semantic 测试内**手工构造**最小 map——**不得**反向引用 `wjsm-module::analyze_module_links`，见「最小代码」的依赖方向说明），并同时喂给两条路径，保证入参一致：
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
      // L2-b/L2-c 由「最小代码」按 import/re-export 边填充 import_map/export_names/re_exports。
      fn build_bundle(mods: &[(u32, &str)]) -> Bundle {
          let inputs = mods.iter().map(|(id, src)| crate::ModuleLoweringInput {
              id: wjsm_ir::ModuleId(*id),
              ast: wjsm_parser::parse_module(src).unwrap(),
              metadata: crate::ModuleMetadata {
                  filename: format!("m{id}.js"), dirname: ".".into(),
                  url: format!("file:///m{id}.js"), kind: crate::ModuleKind::Esm,
              },
          }).collect();
          // 各 map 默认空；按测试级别在「最小代码」中填充对应边（见 fill_link_edges）。
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
- [ ] **最小代码（分级实现）**：
  - 实现 `build_bundle`：对每个 `(id, src)` `wjsm_parser::parse_module` + 构造 `ModuleMetadata`；用 `import`/`export` 语法静态提取构造 6 个 map。L2-a 叶子包各 map 为空；L2-b/L2-c **必须**填 `import_map`（`ImportBinding { source_module, names: Vec<(local, imported)>, specifier }`，对齐 `wjsm-ir` 实际字段——核对 `crates/wjsm-ir/src/lib.rs:789`）、`export_names`、`re_export_map`（`ReExportBinding { source_module, local_name: Option<String>, exported_name: Option<String> }`，核对 `lib.rs:803`）——否则两条路径入参不一致、等价性测试无意义。**注意依赖方向**：`wjsm-module` 已 normal-depend `wjsm-semantic`（`bundler.rs:5`），故 semantic 测试**不得**反向引用 `wjsm-module::analyze_module_links`（会成 dev-dep 环，且 `analyze_module_links` 需真实 `ModuleGraph`（由磁盘文件 BFS 构建，`graph.rs:35`）无法 stub）。测试**自包含**：直接手工构造 `ImportBinding`/`ReExportBinding` 填 map，与 `whole_program`/`linked_program` 两条路径共用同一 `Bundle`。
  - 实现 `LinkMeta::from_bundle`：记录模块顺序、每模块 scope 基址（下一节）、import 边、export 名到全局符号的解析。
  - 实现 `link`：**scope 基址分配须复现 `lower_modules` 的分配序**——先占用全局根 scope 0（`$0.$global`/全局常量），再对每模块按 bundle 顺序 `push_scope(Block)` 得到该模块基址（见任务 5.1 `min_scope_id` 的语义：局部 IR 从 1 起，链接时整体加 `base-1` 偏移，使模块首 scope 落在 `lower_modules` 分配的同一 id）。对每片段应用重定位：scope id 加偏移、常量索引加 const_base、DataSection 字符串偏移加 data_base、import 符号解析到目标模块全局名。
  - **迭代顺序**：先让 L2-a 绿（无 import/无 $global 交互）→ commit；再 L2-b（import 边 + 命名空间对象顺序）→ commit；再 L2-c（re-export/shared_env）→ commit 或（若不可达）标记降级到 L2-bundle 并 commit 说明。
- [ ] **Verify GREEN**：L2-a/L2-b 必过；L2-c 过或明确降级（`#[ignore]` + 降级说明，且任务 5.3 对该场景走 L2-bundle key）。
- [ ] **Commit**（分级）：
  - `git commit -am "feat(wjsm-semantic): 可重定位 IR 链接（L2-a 叶子包逐指令等价）"`
  - `git commit -am "feat(wjsm-semantic): 可重定位 IR 链接（L2-b import 边逐指令等价）"`
  - `git commit -am "feat(wjsm-semantic): 可重定位 IR 链接（L2-c re-export/shared-env 等价或降级说明）"`

## 任务 5.3：L1/L2 编译产物缓存接入 store

Files:
- 创建 `crates/wjsm-pm/src/store/artifact.rs`
- 修改 `crates/wjsm-pm/src/store/mod.rs`
- 修改 `crates/wjsm-cli/src/lib.rs`（build 走 L1/L2 缓存）+ `cmd_cache`（展示/清理 L1/L2）

Why: L1 缓存可重定位 IR、L2 缓存 cwasm 片段，key = 包内容哈希 + 编译器版本 + abi_hash，实现"同一包全机器编译一次"。

Impact/Compatibility: 新增。key 含 `abi_hash`/`gc_flavor`，编译器/ABI 变更自动失效。L2-bundle 作为入口/降级兜底。

Verification: `cargo nextest run -p wjsm-pm -E 'test(artifact)'`

Steps:

- [ ] **写失败测试**。`store/artifact.rs`：
  ```rust
  // L1/L2 编译产物缓存：内容寻址 key，跨项目复用
  use crate::store::blob::hash_content;

  /// L1 key = blake3(manifest_hash ‖ compiler_version ‖ lowering_flags)
  pub fn l1_key(manifest_hash: &[u8; 32], compiler_version: &str, lowering_flags: &str) -> [u8; 32] {
      let mut buf = Vec::new();
      buf.extend_from_slice(manifest_hash);
      buf.extend_from_slice(compiler_version.as_bytes());
      buf.extend_from_slice(lowering_flags.as_bytes());
      hash_content(&buf)
  }

  /// L2 key = blake3(l1_key ‖ backend_abi_hash ‖ gc_flavor)
  pub fn l2_key(l1: &[u8; 32], abi_hash: u64, gc_flavor: &str) -> [u8; 32] {
      let mut buf = Vec::new();
      buf.extend_from_slice(l1);
      buf.extend_from_slice(&abi_hash.to_le_bytes());
      buf.extend_from_slice(gc_flavor.as_bytes());
      hash_content(&buf)
  }

  #[cfg(test)]
  mod tests {
      use super::*;
      #[test]
      fn artifact_key_invalidates_on_compiler_and_abi() {
          let mh = [1u8; 32];
          let a = l1_key(&mh, "0.1.0", "");
          let b = l1_key(&mh, "0.2.0", "");
          assert_ne!(a, b, "编译器版本变更 L1 key 应失效");
          let l2a = l2_key(&a, 100, "mark-sweep");
          let l2b = l2_key(&a, 200, "mark-sweep");
          assert_ne!(l2a, l2b, "abi_hash 变更 L2 key 应失效");
          // 同包同解析图顺序无关：manifest_hash 相同 → l1 相同
          assert_eq!(l1_key(&mh, "0.1.0", ""), a);
      }
  }
  ```
  `store/mod.rs` 加 `pub mod artifact;` + `Store::get_artifact(key)`/`put_artifact(key, tier, bytes)`（存 artifacts 表 + packfile）。`cmd_cache` 扩展展示 L1/L2 统计。build 命令：编译每包前查 L2 缓存，命中直接 `deserialize`，未命中编译后 `put_artifact`。
- [ ] **Verify RED**：`cargo nextest run -p wjsm-pm -E 'test(artifact)'`。
- [ ] **最小代码**：上面 + store artifact 读写 + build 接入。
- [ ] **Verify GREEN**：测试通过 + 冒烟：同一包在两个项目 install+build，第二次 L2 命中（无重复编译）。
- [ ] **Commit**：`git commit -am "feat(wjsm-pm): L1/L2 编译产物缓存跨项目复用"`

## 任务 5.4：全量回归 + pm 端到端集成测试收尾

Files:
- 创建 `crates/wjsm-pm/tests/pm_end_to_end.rs`（多场景集成测试，非 fixture 快照）

Why: 端到端验收 spec §13 的场景集，确认无 node_modules、去重、多版本、task/x 全链路。**沿用任务 3.4 的命名约定**——pm 场景走 crate 内 `#[test]` 集成测试（`pm_` 前缀），不走 `fixtures/*` `.expected` harness（该 harness 只识别 happy/errors/modules 三 suite 且无法表达多步编排）。

Impact/Compatibility: 纯新增测试。

Verification: `cargo nextest run --workspace`

Steps:

- [ ] **写集成测试**（各用内置 mock registry + 临时项目 + 临时 store）：`pm_install_basic`（无依赖包，断言项目目录无 `node_modules`）、`pm_install_dedup`（两项目共享 blob，断言 `index.db` 中相同内容单 blob）、`pm_install_multi_version`（instance-splitting 共存，断言两版本均可 read_package_file）、`pm_task_scripts`（pre/post 序列执行）、`pm_x_bin`（拉包→解析 bin→执行）。
- [ ] **Verify RED**：新测试未接入前 `cargo nextest run -E 'test(pm_)'` 失败。
- [ ] **最小代码**：补齐各集成测试的编排辅助（复用 `tests/mock_registry.rs`）。
- [ ] **Verify GREEN**：`cargo nextest run --workspace` 全绿；`cargo build` 零警告。
- [ ] **Commit**：`git commit -am "test(wjsm-pm): pm 端到端集成测试验收"`

---

## Risks

- **可重定位 IR 逐指令等价**（最高风险，研究级里程碑）：重定位偏差是静默错误代码，非崩溃。`lower_modules` 存在 `$0.$global`、entry-block 顺序常量、BFS 交错 scope 分配、shared-env/live-binding 路由等跨模块耦合，「一次成型逐指令等价」不现实。缓解：任务 5.2 **分级验收**——L0（单包无 import）→ L1（跨模块 import）→ L2（`$global`/live-binding）逐级引入，每级用逐指令等价快照硬验收；未达 L2 前 L1 缓存对含 live-binding 的包**不启用**（回退 L2-bundle 整体路径），不影响 P1–P4 与不含该模式的包。不通过不合并。
- **pubgrub API 不确定**：任务 2.3 首步 spike 锁定版本与 trait 形态；若 0.2 不满足自定义 Version，回退手写 CDCL 或锁定可用版本。
- **instance-splitting / peer 正确性**：可满足场景须复现 npm 多版本，真冲突须给解释。缓解：任务 2.3 四测试覆盖去重/分裂/peer 冲突/optional 跳过四态；peer 约束翻译（`peer(react ^17)` → 对宿主环境已选 react 版本的 comparator）明确定义；后续可加真实 npm 树对照测试。
- **前置 #312 未合并**：P5 阻塞。缓解：P1–P4 完全独立可先交付；P5 首步前置检查。
- **SQLite 并发写 / 中断原子性**：WAL 单写多读；写路径 CLI 层 `spawn_blocking` 隔离；整包写入包在单事务内（任务 1.5），中断回滚；孤儿 blob 由 `store gc`（任务 1.5b）标记-复制回收。缓解：任务 1.4 用 WAL；install 串行写 + store 级文件锁。
- **默认 Vfs 破坏现有行为**（关键）：resolver 全部 fs 谓词（含 12 处 `canonicalize`）改经 Vfs，`FsVfs` 须与原 `std::fs`/`Path` 调用语义逐处等价。缓解：任务 1.6 硬验收 `cargo nextest run --workspace` 全绿 + `FsVfs::canonicalize` 与 `Path::canonicalize` 对照单测。

## Retirement

- 本计划为 new-capability，不删除现有主路径：FS 模式解析、`lower_modules` 整体路径均保留为默认。
- 退出条件（falsifier）：`wjsm install` 后无 node_modules、`wjsm run` 从 CAS 编译成功、同包跨项目 L2 缓存命中零重复编译 → 证明 CAS + 分离编译主路径成立。
- L2-bundle 兜底路径长期保留（入口模块 + 无法分离编译场景），非临时。

## ADR 信号（executing 完成后补 ADR）

1. 全局内容寻址存储 `~/.wjsm/store` 作为新持久化 source-of-truth（blob 内容寻址 + lockfile 解析结果分离）。
2. `wjsm-module` 引入 `Vfs`/`ResolutionOverlay` trait——跨 crate 契约变更（module↔pm 边界）。
3. PubGrub 内核 + npm instance-splitting 求解语义。
4. 可重定位 IR / 分离编译（scope id 项目无关化 + 重定位 + 链接），与 #312 地基及 startup snapshot relocatable heap（ADR 0003）同源。

## 自审记录

- Spec 覆盖：CAS 存储(P1)/求解(P2)/install+lockfile+CLI(P3)/task+x+workspaces(P4)/编译产物缓存(P5) 各有任务；spec §13 测试策略逐项映射到任务验收。
- Placeholder：无 TBD/TODO；每步含完整可粘贴代码与命令；P5 等价性测试的入参已展开为可编译代码（`build_bundle` helper 构造 `Bundle`，两条路径共用），无 `/* ... */` 占位。
- 类型一致：`BlobHash=[u8;32]`、`BlobLoc`、`Manifest`、`SemVer`/`Range`/`Comparator`、`WjsmLock`（含 `root_deps`）、`encode_pkg_dir`、`RelocatableModule`/`LOCAL_SCOPE_BASE` 跨任务签名一致。
- 兼容：默认 FsVfs/NoOverlay 零破坏、module 不依赖 pm 均标为硬验收；`FsVfs` 每方法与被替换的 `std::fs`/`Path` 调用语义逐处等价。
- 复杂度：主逻辑进新 crate/新文件；resolver.rs 只做 wiring（全部 fs 谓词路由进 `self.vfs`，不新增包管理逻辑），不因触点数量增负。
- 验证：每任务有精确 nextest/cargo 命令；pm 端到端场景走 `crates/wjsm-cli/tests/*.rs` 集成测试（fixture runner suite 仅 happy/errors/modules，不含 pm），命名统一 `test(pm_*)`。
- 双轨/ADR：new-capability，ADR 信号已保留待 executing 后补。

### 二轮审查修正（本次收敛）

针对首轮计划的以下缺口已逐项修正：

1. **「3 处磁盘接缝」证伪** → 任务 1.6 重写为「resolver 实际路由的全部 fs 谓词（`read_to_string`×1 / `canonicalize`×12 / `is_file`×7 / `is_dir`×6，共 26 处）路由进 `Vfs`」，`Vfs` trait 提供 `canonicalize`/`is_file`/`is_dir`/`read_to_string`/`read_package_json`，另加 `exists`（resolver 不调用，仅供 `CasVfs` 内部前缀判定 + trait 完备性）；`CasVfs::canonicalize` 对虚拟路径做词法归一化（`normalize_virtual`）。这是 P2–P4 地基。
2. **npm_semver 部分实现** → 任务 2.1 重写为 comparator 结构模型，覆盖 `^`/`~`/x-range/hyphen/比较运算符/`||`/**预发布包含规则**（修正原 `(lo,hi)` 元组模型对 `2.0.0-alpha` 的误判硬伤），符合 hard rule「No partial implementations」。
3. **P5 逐指令等价 + 占位测试** → 任务 5.1/5.2 修正 `LOCAL_SCOPE_BASE`（避开全局根 `$0.$global`），等价性测试展开为可编译代码，并分四阶段验收（单模块无 import → 常量/字符串 → 跨模块 import → `$global`/live-binding），承认这是研究级里程碑。
4. **Store 非事务 + GC/pack 轮转缺失** → 任务 1.4/1.5 加 `packs` 表 + `with_txn(|tx| …)` 整包原子写（配套 `txn_put_blob`/`txn_put_manifest_raw`/`txn_put_package` 事务作用域自由函数，避免闭包内二次 `lock` 死锁）+ 回滚测试；新增任务 1.5b 标记-复制 gc；`PackWriter` 支持 pack 轮转。
5. **CasVfs scoped 包破损 + PnpOverlay 真空** → 任务 3.2 用 `encode_pkg_dir`（`/`→`%2F`、`@`→`%40`）保证包目录单组件、scoped 包安全；`is_dir` 用 `manifest_has_prefix` 支持中间目录；补全 `PnpOverlay` 实现（边表 + root_deps）。
6. **peer 求解 hand-wave** → 任务 2.3 明确 peer→PubGrub 约束翻译、`MockIndex` API、optionalDependencies 跳过语义 + 对应测试。
7. **fixture 命名不一致** → 统一 pm 端到端为 CLI 集成测试 `test(pm_*)`，移除不存在的 `fixtures/pm` suite（fixture runner 仅 happy/errors/modules）。
8. **schema 偏离设计 §6.3** → 任务 1.4 显式记录偏离（`manifests(hash,body)` vs 规范化 `manifest_entries`）并加回 `packages.meta` 列对齐设计。
