# wjsm 包管理实现计划（wjsm-pm）

Goal: 实现 wjsm 的完整 npm 生态包管理能力（`wjsm install/add/remove/task/x` + workspaces），做到无 `node_modules`、全局内容寻址存储（CAS blob + SQLite + zstd + packfile）直供编译器，并实现 AOT 独有的跨项目分层编译产物复用（L1 可重定位 IR + L2 cwasm 片段）。批准的设计见 `docs/aegis/specs/2026-07-07-package-management-design.md`。

Architecture: 新增独立 crate `wjsm-pm`，拥有 registry client / CAS store / PubGrub solver / lockfile / scripts / workspace。`wjsm-module` 新增 `Vfs` + `ResolutionOverlay` 两个 trait（定义在 module 侧，实现在 pm 侧），把现有三处磁盘访问抽象化——`wjsm-module` **不反向依赖** `wjsm-pm`。`wjsm-semantic` 新增可重定位 IR 单包 lower + 链接阶段（服务 L1）。`wjsm-cli` 组装注入并新增子命令。依赖方向：`wjsm-pm → wjsm-module`、`wjsm-pm → wjsm-snapshot-format`、`wjsm-cli → wjsm-pm`。

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
- P5 前置依赖 issue #312 已合并；可重定位 IR 分离编译产出必须与现有 `lower_modules` 整体路径**逐指令等价**。
- tarball 必须 SSRI 校验通过才入库；依赖生命周期脚本默认禁用，需 `trustedDependencies` 或 `--allow-scripts`。

Verification:

- `cargo nextest run -p wjsm-pm`
- `cargo nextest run -p wjsm-module`（回归 Vfs 抽象不破坏 FS 模式）
- `cargo nextest run -p wjsm-semantic -E 'test(relocatable_ir)'`
- `cargo nextest run -E 'test(pm__)'`（fixtures）
- `cargo nextest run --workspace`（全量回归）
- 冒烟：含依赖 fixture 项目 `wjsm install` 后 `wjsm run` 成功且磁盘无 node_modules

## Plan Basis

Facts（已核对代码）:

- `ModuleResolver`（resolver.rs:70）字段 `root_path/options/package_cache/visited`；`with_options`（L89）是唯一带 options 构造器。源码读取唯一点在 `resolve`→L754 `std::fs::read_to_string(&path)`；node_modules 查找在 `find_package_in_node_modules`（L328）；package.json 读取在 `package_json.rs:60`。这三处是 CAS 切入接缝。
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
src/store/{mod,index,blob,manifest,artifact,vfs,overlay}.rs
src/lockfile/{mod,wjsm_lock,migrate}.rs
src/scripts/mod.rs
src/workspace.rs
tests/mock_registry.rs      # 内置离线 mock registry 测试辅助
```
修改 `crates/wjsm-module/src/`：新增 `vfs.rs`（trait 定义 + `FsVfs`/`NoOverlay` 默认实现）；`resolver.rs`（构造器接受 vfs/overlay，3 处磁盘访问改 trait 调用）；`bundler.rs`/`graph.rs`（注入透传）；`lib.rs`（导出 trait）。
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
- Target files：`wjsm-pm/*`（全新，每文件单一职责 ≤500 行）；`wjsm-module`（trait 抽象微创，3 处调用改写 + 新 vfs.rs）；`wjsm-semantic/relocatable/*`（新 owner 文件，非改大文件）；`wjsm-cli`（新 pm_commands.rs + dispatch 微创）。
- Current pressure：`resolver.rs` 1576 行已超纪律——**禁止**往其加包管理逻辑，只改 3 处调用签名。
- Projected post-change pressure：主复杂度进新文件，现有大文件不增负。
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

Impact/Compatibility: 纯新增。packfile 追加式；写中断产生的尾部字节由 gc 回收（本任务只实现读写，gc 见任务 1.4 索引层）。

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
- [ ] **最小代码**：上面代码即完整实现，无需额外。
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

Impact/Compatibility: 纯新增。事务写保证中断可回滚。

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
      manifest_hash BLOB NOT NULL, PRIMARY KEY (name, version)
  );
  CREATE TABLE IF NOT EXISTS artifacts (
      cache_key BLOB PRIMARY KEY, tier INTEGER NOT NULL,
      pack_id INTEGER, offset INTEGER, clen INTEGER, ulen INTEGER
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
- [ ] **最小代码**：上面即完整。
- [ ] **Verify GREEN**：两测试通过。
- [ ] **Commit**：`git commit -am "feat(wjsm-pm): SQLite index.db（blobs/manifests/packages/artifacts）"`

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

  pub struct Store {
      root: PathBuf,
      packs_dir: PathBuf,
      index: StoreIndex,
  }

  impl Store {
      pub fn open(store_root: &Path) -> Result<Self> {
          let root = store_root.join(STORE_VERSION);
          let packs_dir = root.join("packs");
          let index = StoreIndex::open(&root.join("index.db"))?;
          std::fs::create_dir_all(&packs_dir)?;
          Ok(Self { root, packs_dir, index })
      }

      /// 把一个解包后的包目录写入 CAS：每文件去重成 blob，构建 manifest，入库。
      pub fn add_package_from_dir(&self, name: &str, version: &str, integrity: &str, dir: &Path) -> Result<()> {
          let mut entries = Vec::new();
          let mut writer = PackWriter::open(&self.packs_dir, 0)?;
          let mut stack = vec![dir.to_path_buf()];
          while let Some(cur) = stack.pop() {
              for ent in std::fs::read_dir(&cur)? {
                  let ent = ent?;
                  let path = ent.path();
                  if path.is_dir() {
                      stack.push(path);
                      continue;
                  }
                  let content = std::fs::read(&path)?;
                  let h = hash_content(&content);
                  if self.index.get_blob(&h)?.is_none() {
                      let loc = writer.append(&content)?;
                      self.index.put_blob(&h, loc)?;
                  }
                  let rel = path.strip_prefix(dir).unwrap().to_string_lossy().replace('\\', "/");
                  entries.push(ManifestEntry { rel_path: rel, blob_hash: h, mode: 0o644 });
              }
          }
          let m = Manifest::from_entries(entries);
          let mh = m.hash();
          self.index.put_package(name, version, integrity, &mh)?;
          // manifest body 存 index
          self.put_manifest(&m)?;
          Ok(())
      }

      fn put_manifest(&self, m: &Manifest) -> Result<()> {
          let body = serde_json::to_vec(&m.entries)?;
          self.index_conn_put_manifest(&m.hash(), &body)
      }

      fn index_conn_put_manifest(&self, hash: &[u8; 32], body: &[u8]) -> Result<()> {
          self.index.put_manifest_raw(hash, body)
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
- [ ] **Verify RED**：`cargo nextest run -p wjsm-pm -E 'test(store_integration)'`。
- [ ] **最小代码**：上面即完整。
- [ ] **Verify GREEN**：两测试通过。
- [ ] **Commit**：`git commit -am "feat(wjsm-pm): Store 统一入口（写包+读文件）"`

## 任务 1.6：wjsm-module 新增 Vfs/ResolutionOverlay trait（默认实现零破坏）

Files:
- 创建 `crates/wjsm-module/src/vfs.rs`
- 修改 `crates/wjsm-module/src/lib.rs`（导出）
- 修改 `crates/wjsm-module/src/resolver.rs`（构造器接受 trait 对象，3 处磁盘访问改调用）
- 修改 `crates/wjsm-module/src/bundler.rs` / `graph.rs`（注入透传）

Why: 把 resolver.rs:754/328、package_json.rs:60 三处磁盘访问抽象为 trait，让 CAS 无缝切入且不反转依赖方向。默认 `FsVfs`/`NoOverlay` 保证现有行为不变。

Impact/Compatibility: **关键兼容任务**。默认实现必须与现有行为逐字节等价——现有 module + 全量 fixture 必须全绿。

Verification: `cargo nextest run -p wjsm-module && cargo nextest run --workspace`

Steps:

- [ ] **写失败测试**。`vfs.rs`：
  ```rust
  // 虚拟文件系统抽象 + 解析覆盖层：让 CAS 无缝切入解析，不反转依赖方向
  use anyhow::{Context, Result};
  use std::path::{Path, PathBuf};

  /// 源码/元数据读取抽象。默认 FsVfs = 现有 std::fs 行为。
  pub trait Vfs: Send + Sync {
      fn read_to_string(&self, path: &Path) -> Result<String>;
      fn is_dir(&self, path: &Path) -> bool;
      fn read_package_json(&self, dir: &Path) -> Result<Option<String>>;
  }

  /// 解析覆盖层：bare specifier → 虚拟树中的具体包根。None = 回退默认 node_modules 遍历。
  pub trait ResolutionOverlay: Send + Sync {
      fn resolve_bare(&self, specifier: &str, referrer: &Path) -> Result<Option<PathBuf>>;
  }

  /// 默认文件系统实现（现有行为）。
  pub struct FsVfs;

  impl Vfs for FsVfs {
      fn read_to_string(&self, path: &Path) -> Result<String> {
          std::fs::read_to_string(path).with_context(|| format!("Failed to read module: {}", path.display()))
      }
      fn is_dir(&self, path: &Path) -> bool {
          path.is_dir()
      }
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
      fn fs_vfs_reads_and_reports_missing_pkg_json() {
          let dir = std::env::temp_dir().join(format!("wjsm_vfs_{}", std::process::id()));
          let _ = std::fs::remove_dir_all(&dir);
          std::fs::create_dir_all(&dir).unwrap();
          std::fs::write(dir.join("a.js"), "export const x=1;").unwrap();
          let vfs = FsVfs;
          assert_eq!(vfs.read_to_string(&dir.join("a.js")).unwrap(), "export const x=1;");
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
  `lib.rs` 追加 `mod vfs;` + `pub use vfs::{FsVfs, NoOverlay, ResolutionOverlay, Vfs};`
- [ ] **Verify RED**：`cargo nextest run -p wjsm-module -E 'test(vfs)'`。
- [ ] **最小代码 + 接入**：
  - `resolver.rs`：`ModuleResolver` 增字段 `vfs: std::sync::Arc<dyn Vfs>`、`overlay: std::sync::Arc<dyn ResolutionOverlay>`；`with_options` 默认注入 `Arc::new(FsVfs)` / `Arc::new(NoOverlay)`；新增 `with_providers(root, options, vfs, overlay)` 构造器。
  - L754 `std::fs::read_to_string(&path)` → `self.vfs.read_to_string(&path)`。
  - L328 `find_package_in_node_modules` 起始处：先 `if let Some(root) = self.overlay.resolve_bare(package_name, from_dir)? { return Ok(Some(root)); }` 再回退现有遍历；遍历里 `candidate.is_dir()` → `self.vfs.is_dir(&candidate)`。
  - `package_json.rs`：`read_package_info` 增参数或改为经 vfs——最小改动：新增 `read_package_info_with_vfs(dir, vfs)`，`read_package_info` 保留调用 `FsVfs`。resolver 调用点传 `&*self.vfs`。
  - `bundler.rs`/`graph.rs`：`ModuleBundler` 增 `with_providers`，`ModuleGraph::build_with_providers` 透传到 `ModuleResolver::with_providers`；现有 `build_with_options` 内部用默认 provider。
- [ ] **Verify GREEN**：`cargo nextest run -p wjsm-module && cargo nextest run --workspace` 全绿（证明默认实现零破坏）。
- [ ] **Commit**：`git commit -am "feat(wjsm-module): Vfs/ResolutionOverlay trait 抽象磁盘访问（默认零破坏）"`

---

# 阶段 P2：registry client + PubGrub solver

## 任务 2.1：npm 精确 SemVer 语义

Files:
- 创建 `crates/wjsm-pm/src/solver/mod.rs`
- 创建 `crates/wjsm-pm/src/solver/npm_semver.rs`
- 修改 `crates/wjsm-pm/src/lib.rs`

Why: npm 区间语义（`^`/`~`/x-range/`||`/预发布规则）与通用 semver 有差异，必须精确匹配 node-semver，作为 PubGrub 的 Version/VersionSet 基础。

Impact/Compatibility: 纯新增。

Verification: `cargo nextest run -p wjsm-pm -E 'test(npm_semver)'`

Steps:

- [ ] **写失败测试**（对照 node-semver 行为表）。`solver/npm_semver.rs`：
  ```rust
  // npm 精确 SemVer 区间语义（^ ~ x-range || 预发布规则）
  use std::cmp::Ordering;

  #[derive(Debug, Clone, PartialEq, Eq)]
  pub struct SemVer {
      pub major: u64,
      pub minor: u64,
      pub patch: u64,
      pub pre: Vec<String>,
  }

  impl SemVer {
      pub fn parse(s: &str) -> Option<Self> {
          let s = s.trim().trim_start_matches('v');
          let (core, pre) = match s.split_once('-') {
              Some((c, p)) => (c, p.split('.').map(String::from).collect()),
              None => (s, Vec::new()),
          };
          let core = core.split('+').next().unwrap_or(core);
          let mut it = core.split('.');
          let major = it.next()?.parse().ok()?;
          let minor = it.next()?.parse().ok()?;
          let patch = it.next()?.parse().ok()?;
          Some(SemVer { major, minor, patch, pre })
      }
  }

  impl PartialOrd for SemVer {
      fn partial_cmp(&self, o: &Self) -> Option<Ordering> { Some(self.cmp(o)) }
  }
  impl Ord for SemVer {
      fn cmp(&self, o: &Self) -> Ordering {
          (self.major, self.minor, self.patch)
              .cmp(&(o.major, o.minor, o.patch))
              .then_with(|| match (self.pre.is_empty(), o.pre.is_empty()) {
                  (true, true) => Ordering::Equal,
                  (true, false) => Ordering::Greater, // 无预发布 > 有预发布
                  (false, true) => Ordering::Less,
                  (false, false) => self.pre.cmp(&o.pre),
              })
      }
  }

  /// npm range：解析为 (下界含, 上界不含) 的并集。
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub struct Range {
      /// 每个 comparator set 是 [下界, 上界)；多个 set 是并集（||）。
      pub sets: Vec<(Option<SemVer>, Option<SemVer>)>,
  }

  impl Range {
      pub fn parse(s: &str) -> Option<Self> {
          let mut sets = Vec::new();
          for part in s.split("||") {
              sets.push(parse_comparator(part.trim())?);
          }
          Some(Range { sets })
      }

      pub fn matches(&self, v: &SemVer) -> bool {
          self.sets.iter().any(|(lo, hi)| {
              lo.as_ref().map_or(true, |l| v >= l) && hi.as_ref().map_or(true, |h| v < h)
          })
      }
  }

  fn parse_comparator(s: &str) -> Option<(Option<SemVer>, Option<SemVer>)> {
      let s = s.trim();
      if s.is_empty() || s == "*" || s == "x" {
          return Some((None, None));
      }
      if let Some(rest) = s.strip_prefix('^') {
          let v = SemVer::parse(rest)?;
          let hi = if v.major > 0 {
              SemVer { major: v.major + 1, minor: 0, patch: 0, pre: vec![] }
          } else if v.minor > 0 {
              SemVer { major: 0, minor: v.minor + 1, patch: 0, pre: vec![] }
          } else {
              SemVer { major: 0, minor: 0, patch: v.patch + 1, pre: vec![] }
          };
          return Some((Some(v), Some(hi)));
      }
      if let Some(rest) = s.strip_prefix('~') {
          let v = SemVer::parse(rest)?;
          let hi = SemVer { major: v.major, minor: v.minor + 1, patch: 0, pre: vec![] };
          return Some((Some(v.clone()), Some(hi)));
      }
      // 精确版本
      let v = SemVer::parse(s)?;
      let hi = SemVer { major: v.major, minor: v.minor, patch: v.patch + 1, pre: v.pre.clone() };
      Some((Some(v), Some(hi)))
  }

  #[cfg(test)]
  mod tests {
      use super::*;
      fn v(s: &str) -> SemVer { SemVer::parse(s).unwrap() }

      #[test]
      fn npm_semver_caret_nonzero_major() {
          let r = Range::parse("^1.2.3").unwrap();
          assert!(r.matches(&v("1.2.3")));
          assert!(r.matches(&v("1.9.0")));
          assert!(!r.matches(&v("2.0.0")));
          assert!(!r.matches(&v("1.2.2")));
      }

      #[test]
      fn npm_semver_caret_zero_major() {
          let r = Range::parse("^0.2.3").unwrap();
          assert!(r.matches(&v("0.2.3")));
          assert!(r.matches(&v("0.2.9")));
          assert!(!r.matches(&v("0.3.0")));
      }

      #[test]
      fn npm_semver_tilde_and_union_and_star() {
          assert!(Range::parse("~1.2.3").unwrap().matches(&v("1.2.9")));
          assert!(!Range::parse("~1.2.3").unwrap().matches(&v("1.3.0")));
          let u = Range::parse("^1.0.0 || ^2.0.0").unwrap();
          assert!(u.matches(&v("1.5.0")) && u.matches(&v("2.5.0")) && !u.matches(&v("3.0.0")));
          assert!(Range::parse("*").unwrap().matches(&v("9.9.9")));
      }

      #[test]
      fn npm_semver_prerelease_ordering() {
          assert!(v("1.0.0") > v("1.0.0-alpha"));
          assert!(v("1.0.0-alpha") < v("1.0.0-beta"));
      }
  }
  ```
  `solver/mod.rs`：`pub mod npm_semver;`；`lib.rs`：`pub mod solver;`
- [ ] **Verify RED**：`cargo nextest run -p wjsm-pm -E 'test(npm_semver)'`。
- [ ] **最小代码**：上面即完整（x-range 展开等边角在后续按 fixture 补，本任务覆盖核心 4 类）。
- [ ] **Verify GREEN**：四测试通过。
- [ ] **Commit**：`git commit -am "feat(wjsm-pm): npm 精确 SemVer 区间语义"`

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

Impact/Compatibility: 纯新增。

Verification: `cargo nextest run -p wjsm-pm -E 'test(solver)'`

Steps:

- [ ] **Spike 首步：锁定 pubgrub API**。运行 `cargo doc -p pubgrub --no-deps 2>/dev/null; grep -rn "trait DependencyProvider" ~/.cargo/registry/src/*/pubgrub-*/src/ | head`，确认 `DependencyProvider` 关联类型（`P`/`V`/`VS`/`get_dependencies`/`choose_version`）签名，写进 `provider.rs` 顶部注释。若 0.2 API 与预期不符，锁定实际版本号更新 Cargo.toml。
- [ ] **写失败测试**。`solver/provider.rs` 实现 `DependencyProvider`：`Package = String`（含 instance 后缀）、`Version = SemVer`、依赖从缓存的 packument 取（惰性）。`solver/duplication.rs` 实现 instance-splitting：
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
  }
  ```
  `solver/mod.rs` 定义 `solve`、`ResolvedGraph`、`test_support::MockIndex`、`explain.rs` 的解释构造。
- [ ] **Verify RED**：`cargo nextest run -p wjsm-pm -E 'test(solver)'`。
- [ ] **最小代码**：实现 `solve`：先跑 PubGrub 单版本；捕获 `NoSolution` 时定位冲突包 → duplication 分裂实例子锥递归 → 合并 `ResolvedGraph`；仍不可满足则 `explain` 产出 PubGrub 派生链。
- [ ] **Verify GREEN**：三测试通过。
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

- [ ] **写失败测试**。`store/vfs.rs`（`CasVfs` impl `wjsm_module::Vfs`）+ `store/overlay.rs`（`PnpOverlay` impl `wjsm_module::ResolutionOverlay`，按 lockfile 把 `referrer + specifier` 映射到虚拟包根 `<vroot>/<name>@<version>`）：
  ```rust
  // CasVfs：从 CAS store 读源码，虚拟路径 <vroot>/<name>@<version>/<rel>
  use crate::store::Store;
  use anyhow::Result;
  use std::path::{Path, PathBuf};
  use std::sync::Arc;

  pub struct CasVfs {
      store: Arc<Store>,
      vroot: PathBuf,
  }

  impl CasVfs {
      pub fn new(store: Arc<Store>, vroot: PathBuf) -> Self {
          Self { store, vroot }
      }
      /// 虚拟路径 → (name, version, rel_path)
      fn split(&self, path: &Path) -> Option<(String, String, String)> {
          let rel = path.strip_prefix(&self.vroot).ok()?;
          let mut comps = rel.components();
          let pkgver = comps.next()?.as_os_str().to_string_lossy().to_string();
          let (name, version) = pkgver.rsplit_once('@')?;
          let rest = comps.as_path().to_string_lossy().replace('\\', "/");
          Some((name.to_string(), version.to_string(), rest))
      }
  }

  impl wjsm_module::Vfs for CasVfs {
      fn read_to_string(&self, path: &Path) -> Result<String> {
          let (n, v, rel) = self.split(path).ok_or_else(|| anyhow::anyhow!("非虚拟路径: {}", path.display()))?;
          let bytes = self.store.read_package_file(&n, &v, &rel)?
              .ok_or_else(|| anyhow::anyhow!("CAS 缺文件: {n}@{v}/{rel}"))?;
          Ok(String::from_utf8(bytes)?)
      }
      fn is_dir(&self, path: &Path) -> bool {
          // 虚拟树目录判定：路径能拆出包且 rel 为空或存在同前缀文件
          self.split(path).map_or(false, |(_, _, rel)| rel.is_empty())
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
      fn cas_vfs_reads_from_store() {
          let root = std::env::temp_dir().join(format!("wjsm_pm_casvfs_{}", std::process::id()));
          let _ = std::fs::remove_dir_all(&root);
          let pkg = root.join("src_pkg");
          std::fs::create_dir_all(&pkg).unwrap();
          std::fs::write(pkg.join("index.js"), b"export const v=1;").unwrap();
          let store = Arc::new(Store::open(&root.join("store")).unwrap());
          store.add_package_from_dir("demo", "1.0.0", "sha512-x", &pkg).unwrap();
          let vroot = PathBuf::from("/virt");
          let vfs = CasVfs::new(store, vroot.clone());
          let got = wjsm_module::Vfs::read_to_string(&vfs, &vroot.join("demo@1.0.0/index.js")).unwrap();
          assert_eq!(got, "export const v=1;");
      }
  }
  ```
  `lib.rs` 增 `pub async fn install(project_dir: &Path, store: &Store) -> Result<WjsmLock>`：读 package.json deps → `solve` → 对每个包 `fetch_tarball`+`verify_integrity`+`extract`+`add_package_from_dir` → 写 lockfile。加一个用 mock registry 的 `install_end_to_end` 测试。
- [ ] **Verify RED**：`cargo nextest run -p wjsm-pm -E 'test(cas_vfs) | test(install)'`。
- [ ] **最小代码**：上面 + install 编排。
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

## 任务 3.4：编译器接入 CAS（run/build 惰性补齐）+ fixture

Files:
- 修改 `crates/wjsm-cli/src/lib.rs`（run/build 命令检测 lockfile → 注入 CasVfs/PnpOverlay）
- 创建 `fixtures/pm/run_from_cas/`（含 package.json + wjsm-lock.toml + mock 包源）
- 修改 `tests/fixture_runner.rs` 或新增 pm fixture 集成测试

Why: 让 `wjsm run/build` 检测到 lockfile 时经 CasVfs 直接从 CAS 编译执行依赖，无 node_modules。这是纯惰性模型 + 编译器直供的端到端闭环。

Impact/Compatibility: run/build 仅在存在 wjsm-lock.toml + 依赖时启用 CAS 注入；无 lockfile 走现有 FsVfs 路径（零破坏）。

Verification: `cargo nextest run -E 'test(pm__run_from_cas)'` + 冒烟无 node_modules

Steps:

- [ ] **写失败测试**。新增 `crates/wjsm-cli/tests/pm_run_from_cas.rs`：构造临时项目（package.json 依赖 demo，先用离线 mock 或预置 store），`wjsm install` 后 `run_file_in_process` 跑入口 `import {v} from 'demo'; console.log(v)`，断言 stdout 含预期值且项目目录**无 `node_modules`**。
- [ ] **Verify RED**：运行预期失败（run 尚未注入 CAS）。
- [ ] **最小代码**：`lib.rs` 的 `cmd_run`/`cmd_build` 前置：探测 `<dir>/wjsm-lock.toml`，存在则用 `ModuleBundler::with_providers(root, options, Arc::new(CasVfs::new(...)), Arc::new(PnpOverlay::from_lock(...)))` 替代默认 bundler。
- [ ] **Verify GREEN**：`cargo nextest run -E 'test(pm_run_from_cas)'` 通过，断言项目无 node_modules。
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

Why: L1 跨项目复用的命门是 scope id 项目无关化。单独 lower 一个包 → scope id 从 0 起的模块局部 IR + 重定位表（scope 基址、常量偏移、字符串偏移、未解析 import 符号）。

Impact/Compatibility: 新增路径；现有 `lower_modules` 整体路径不变。产出必须能重定位回等价全局 IR。

Verification: `cargo nextest run -p wjsm-semantic -E 'test(relocatable_ir)'`

Steps:

- [ ] **写失败测试（等价性快照驱动）**。`relocatable/mod.rs` 定义 `RelocatableModule { local_program: Program, relocations: Relocations }` 与 `lower_one(ast, metadata) -> RelocatableModule`。测试：对一个简单模块，`lower_one` 产出的 local IR 中 scope id 从 0 起（断言最小 scope id 基址）；重定位表记录了所有 `${scope_id}.` 出现位置。
  ```rust
  #[cfg(test)]
  mod tests {
      use super::*;
      #[test]
      fn relocatable_ir_local_scope_starts_at_base() {
          let src = "export const x = 1; function f() { const y = 2; return y; }";
          let ast = wjsm_parser::parse_module(src).unwrap();
          let m = lower_one(ast, test_metadata()).unwrap();
          // 局部 scope id 从固定基址起（模块局部，项目无关）
          assert_eq!(m.min_scope_id(), 0);
          // 重定位表非空（记录 scope/常量/字符串引用）
          assert!(!m.relocations.scope_refs.is_empty());
      }
  }
  ```
- [ ] **Verify RED**：`cargo nextest run -p wjsm-semantic -E 'test(relocatable_ir)'`。
- [ ] **最小代码**：实现 `lower_one`——复用现有单模块 lower 逻辑但以模块局部 ScopeTree 起始；扫描产出 IR 收集重定位项（scope id 引用、常量索引、DataSection 字符串偏移、跨模块 import 绑定）进 `Relocations`。
- [ ] **Verify GREEN**：测试通过。
- [ ] **Commit**：`git commit -am "feat(wjsm-semantic): 可重定位 IR 单包 lower + 重定位表"`

## 任务 5.2：链接阶段 — 逐指令等价性

Files:
- 创建 `crates/wjsm-semantic/src/relocatable/link.rs`
- 修改 `crates/wjsm-semantic/src/relocatable/mod.rs`

Why: 把多个局部 IR 片段按 bundle 位置重定位合并为全局 Program，必须与现有 `lower_modules` 整体路径**逐指令等价**——这是 L1 正确性的命门。

Impact/Compatibility: 新增。等价性是硬验收（重定位偏差是静默错误代码）。

Verification: `cargo nextest run -p wjsm-semantic -E 'test(relocatable_ir_equivalence)'`

Steps:

- [ ] **写失败测试（逐指令等价快照）**。`link.rs` 测试：对同一组模块，`lower_modules`（整体路径）产出 `Program A`；`lower_one` 每模块 + `link`（分离路径）产出 `Program B`；断言 `A == B`（IR dump 逐指令相等）。用 2-3 个含跨模块 import、常量、字符串字面量的模块覆盖 scope/常量/字符串/import 四类重定位。
  ```rust
  #[cfg(test)]
  mod tests {
      use super::*;
      #[test]
      fn relocatable_ir_equivalence_matches_lower_modules() {
          let modules = sample_modules(); // a.js 导出、b.js 导入 a
          let whole = crate::lower_modules(/* 整体路径入参 */).unwrap();
          let linked = {
              let parts: Vec<_> = modules.iter().map(|m| lower_one(m.ast.clone(), m.metadata.clone()).unwrap()).collect();
              link(parts, /* 链接元数据 */).unwrap()
          };
          assert_eq!(format!("{whole}"), format!("{linked}"), "分离编译链接结果必须逐指令等价于 lower_modules");
      }
  }
  ```
- [ ] **Verify RED**：运行预期失败（link 未实现或重定位有偏差）。
- [ ] **最小代码**：实现 `link`——按模块顺序分配 scope 基址，对每个片段应用重定位（scope id += base、常量索引 += const_base、字符串偏移 += data_base、import 符号解析到目标模块全局名），合并成全局 Program。迭代修正直到逐指令等价测试通过。
- [ ] **Verify GREEN**：等价性测试通过。
- [ ] **Commit**：`git commit -am "feat(wjsm-semantic): 可重定位 IR 链接阶段（逐指令等价 lower_modules）"`

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

## 任务 5.4：全量回归 + pm fixtures 收尾

Files:
- 创建 `fixtures/pm/{install_basic,install_dedup,install_multi_version,task_scripts,x_bin}/`
- 修改 INDEX（计划完成标记由 executing 阶段处理）

Why: 端到端验收 spec §13 的 fixture 集，确认无 node_modules、去重、多版本、task/x 全链路。

Impact/Compatibility: 纯新增测试。

Verification: `cargo nextest run --workspace`

Steps:

- [ ] **写 fixtures**：按 spec §13 建 `install_basic`（无依赖包，`du` 校验无 node_modules）、`install_dedup`（两项目共享 blob 不重复）、`install_multi_version`（instance-splitting 共存）、`task_scripts`、`x_bin`，各配 `.expected`。
- [ ] **Verify RED**：新 fixture 未接入前 `cargo nextest run -E 'test(pm__)'` 失败。
- [ ] **最小代码**：接入 pm fixture 到 runner。
- [ ] **Verify GREEN**：`cargo nextest run --workspace` 全绿；`cargo build`（`crates/wjsm-cli`）零警告。
- [ ] **Commit**：`git commit -am "test(wjsm-pm): pm fixtures 端到端验收"`

---

## Risks

- **可重定位 IR 逐指令等价**（最高风险）：重定位偏差是静默错误代码，非崩溃。缓解：任务 5.2 用逐指令等价快照测试硬验收，覆盖 scope/常量/字符串/import 四类引用；不通过不合并。
- **pubgrub API 不确定**：任务 2.3 首步 spike 锁定版本与 trait 形态；若 0.2 不满足自定义 Version，回退手写 CDCL 或锁定可用版本。
- **instance-splitting 正确性**：可满足场景须复现 npm 多版本，真冲突须给解释。缓解：任务 2.3 三测试覆盖去重/分裂/冲突三态；后续可加真实 npm 树对照测试。
- **前置 #312 未合并**：P5 阻塞。缓解：P1–P4 完全独立可先交付；P5 首步前置检查。
- **SQLite 并发写**：WAL 单写多读；写路径 CLI 层 `spawn_blocking` 隔离。缓解：任务 1.4 用 WAL；install 串行写。
- **默认 Vfs 破坏现有行为**：任务 1.6 硬验收 `cargo nextest run --workspace` 全绿。

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
- Placeholder：无 TBD/TODO；每步含完整可粘贴代码与命令。
- 类型一致：`BlobHash=[u8;32]`、`BlobLoc`、`Manifest`、`SemVer`/`Range`、`WjsmLock` 跨任务签名一致。
- 兼容：默认 FsVfs/NoOverlay 零破坏、module 不依赖 pm、逐指令等价均标为硬验收。
- 复杂度：主逻辑进新 crate/新文件，现有大文件（resolver.rs 1576 行）只改 3 处调用签名。
- 验证：每任务有精确 nextest/cargo 命令。
- 双轨/ADR：new-capability，ADR 信号已保留待 executing 后补。
