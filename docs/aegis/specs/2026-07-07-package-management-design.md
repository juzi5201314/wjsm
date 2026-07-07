# wjsm 包管理设计（wjsm-pm）

- 日期：2026-07-07
- 状态：待用户审阅
- 关联：CLAUDE.md AOT 架构约束、issue #311（Phase 4 生态/CLI tooling）、**issue #312（runtime module loading，本计划前置依赖：分离编译 loader / multi-instance shared-env 需先合并入主干）**、issue #309/#310（package resolution / node globals 现有代码）
- 输入：pnpm CAFS + SQLite index.db、yarn Berry PnP + ZipFS、bun 全局 store + clonefile/hardlink、deno managed content-addressable cache 的机制研究；wjsm 现有 `wjsm-module` resolver/graph/bundler、`runtime_startup.rs` cwasm 缓存、`wjsm-snapshot-format` ABI hash。

## 1. 背景与核心洞察

目标：让 wjsm 具备完整的 npm 生态包管理能力，做到 `npm install` / `npm run` / `npx` 可被 wjsm 无缝替换，且彻底解决 `node_modules` 的两大顽疾——**海量小文件（inode 爆炸）** 与 **跨项目重复占用磁盘**。

### 1.1 四个工具的答案（研究结论）

| 工具 | 存储 | node_modules | 去重 | 索引 | 压缩 |
|---|---|---|---|---|---|
| pnpm | 全局 CAFS 内容寻址（文件哈希 2 字符前缀分片） | 有，`.pnpm` 虚拟 store + symlink 非扁平 | 文件级硬链接/reflink，跨项目共享 | **SQLite `index.db`（MessagePack）** 取代海量 JSON 元数据小文件 | tarball 传输压缩，落盘解压 |
| yarn PnP | 每包 1 个 zip 存 `.yarn/cache` | **无**，`.pnp.cjs` 映射 + ZipFS 虚拟读 zip | 包级（zip 收成 1 inode） | `.pnp.cjs` 序列化解析图 | zip 可配 `compressionLevel` |
| bun | 全局 `~/.bun/install/cache`，`name@version` | isolated 模式 `.bun/pkg@ver` + symlink | clonefile(macOS)/hardlink(Linux) | 二进制 lockfile | tarball |
| deno | 全局内容寻址缓存 | 默认无，managed 模式直接从缓存解析 `npm:` | 内容哈希 | 内部 | — |

关键规律：**小文件灾难的根源是把成千上万个包文件物化散落到磁盘。** yarn PnP 和 deno 的答案是"根本不物化"——用一张解析映射表 + 虚拟读取直接从归档/缓存取内容。

### 1.2 wjsm 的独有优势

wjsm 不是 Node 进程宿主，它是 **AOT 编译器**：拥有自己的 `wjsm-module` resolver，编译期把依赖 resolve → parse → lower → 编译进自包含 WASM。它比 yarn/deno 更进一步——**编译器可以直接从内容寻址存储读源码喂给 parser，运行时产物零 `node_modules`、零小文件**。

更进一步：因为 wjsm 拥有编译器，它能做到 npm/pnpm/bun 都做不到的事——**跨项目缓存"包 → 编译产物"**。同一个 `lodash@4.17.21` 在整台机器上只需被 parse/lower/编译一次，任何新项目 install 后直接复用编译产物。这是 AOT 包管理器相对所有 JS 包管理器的结构性差异化优势，也是本计划的核心目标（§8）。实现它需要"可重定位 IR / 分离编译"，故本计划**前置依赖 issue #312**（其已引入分离编译地基）先合并入主干。

## 2. 决策摘要（已与用户确认）

| 维度 | 决策 |
|---|---|
| 物理布局 | **无 node_modules，全局 CAS 直供编译器**（不物化依赖到磁盘） |
| 存储引擎 | **内容寻址 blob + SQLite 索引 + zstd 压缩** |
| 去重粒度 | **混合：包级归档（file_hash 清单）+ 文件级内容寻址 blob** |
| 下载时机 | **纯惰性**（deno 式：`run`/`build` 时按需解析下载缓存） |
| 求解算法 | **PubGrub 内核 + npm 嵌套重复语义叠加** |
| 编译产物缓存 | **做，本计划核心目标：L1 可重定位 IR + L2 cwasm 片段（真分离编译，跨项目复用）** |
| lockfile | **自有格式 + 读取现有 lockfile 迁移** |
| 脚本运行器 | **`wjsm task <name>`**（deno 式，与 `wjsm run <file>` 正交） |
| npx 等价物 | **`wjsm x <pkg>`**（bun 式最短别名） |
| CLI 范围 | install / add / remove / task / x / workspaces |
| 架构落点 | **独立新 crate `wjsm-pm`** |

## 3. 目标与非目标

### 目标

1. `wjsm install`：按 `package.json` 解析依赖闭包、PubGrub 求解版本、下载、写入 CAS、生成自有 lockfile。
2. `wjsm add <pkg>` / `wjsm remove <pkg>`：增删依赖并更新 manifest + lockfile。
3. 纯惰性：`wjsm run` / `wjsm build` 时若 lockfile 存在但 store 缺 blob，自动补齐；无 lockfile 时按 `package.json` 惰性解析。
4. `wjsm task <name>`：执行 `package.json` `scripts.<name>`，支持 `pre`/`post` 生命周期与 `wjsm` 二进制注入 PATH。
5. `wjsm x <pkg>`：临时拉取并执行包的 bin（npx/dlx 等价物）。
6. workspaces：`package.json` `workspaces` 字段、本地包链接、单一根 lockfile。
7. 内容寻址存储：文件级 blob 去重 + zstd 压缩 + packfile 聚合 + SQLite 索引，消灭 inode 爆炸与跨项目重复。
8. 编译器直供：`wjsm-module` 通过虚拟解析覆盖层 + Vfs 从 CAS 读源码，不物化 node_modules。
9. 分层编译产物缓存（**本计划核心目标**）：L1 可重定位 IR + L2 cwasm 片段跨项目复用，同一包全机器只 parse/lower/编译一次。**前置依赖 issue #312 的分离编译地基**。
10. 生态互操作：读取 `package-lock.json` / `pnpm-lock.yaml` / `yarn.lock` / `bun.lock` 迁移已有项目。
11. 安全边界：tarball 完整性校验；依赖生命周期脚本（postinstall 等）默认禁用，需 `trustedDependencies` 允许清单。

### 非目标

- 物化真实 `node_modules` 目录（首版不做；`--node-modules-dir` 逃生舱留作后续扩展，见 §12 兼容边界）。
- 发布能力（`wjsm publish` / registry 写入）。
- 依赖原生编译 postinstall（node-gyp / prebuild）；原生插件走既有 N-API ADR 路径，不在包管理首版。
- Git / tarball URL / `file:` 以外的非 registry 依赖源首版仅支持 registry + `file:` workspace 链接；`git+` / 远程 tarball 依赖延后。
- HMR、watch 安装。
- 私有 registry 的完整企业级 auth 矩阵（首版支持 `.npmrc` registry / scope / `_authToken`）。
- 运行时 TypeScript 编译语义变更（沿用现有管线）。

## 4. First-principles / Architecture Integrity Lens

First-principles invariants：

- Non-negotiable goal：给定 `package.json`，产出确定性、可复现、与 npm 语义兼容的依赖解析结果，并让 wjsm 编译器无需 node_modules 即可读取任意依赖源码。
- Non-negotiable constraints：
  - `wjsm-module` 不得反向依赖 `wjsm-pm`；解析算法（exports/imports/main/条件）继续归 `wjsm-module`。
  - CAS 是内容寻址 source-of-truth；lockfile 是解析结果 source-of-truth；两者分离。
  - blob 身份 = 文件内容哈希；包身份 = `name@version`；解析身份 = （referrer 包上下文, specifier）→ 具体 `name@version` + subpath。
- Historical assumptions to delete：`import 'pkg'` 必须走真实 `node_modules` 目录查找；依赖必须物化到磁盘；每个项目各自 parse/lower 相同的包。

Architecture Integrity Lens：

- Invariant：解析算法、下载/存储、版本求解、编译器接入四类 owner 分离；`wjsm-module` 只通过 trait 请求源码与解析覆盖，不感知 CAS/registry 细节。
- Canonical owner / contract：
  - `wjsm-pm` 拥有 registry client、CAS store、PubGrub solver、lockfile、scripts、workspace。
  - `wjsm-module` 新增 `Vfs` + `ResolutionOverlay` 两个 trait（定义于此，实现于 `wjsm-pm`），把现有 `fs::read_to_string`（resolver.rs:754）、`find_package_in_node_modules`（resolver.rs:328）、`package_json.rs:60` 三处磁盘访问抽象为 trait 调用。
  - `wjsm-cli` 组装：构造 `wjsm-pm` 的 CAS-backed provider，注入 `wjsm-module` bundler。
- Responsibility overlap：`wjsm-pm` 不重复实现 exports/imports/main 解析；它把 CAS 里的包呈现为"虚拟包树"，`wjsm-module` 现有解析逻辑在虚拟树上原样工作。
- Higher-level simplification：把"node_modules 目录查找"统一抽象为"解析覆盖层 + Vfs"，同一抽象同时服务 FS 模式（现状兼容）与 CAS 模式（PnP 式精确解析），避免两套解析代码。
- Retirement / falsifier：当 `wjsm install` 后磁盘无 node_modules、`wjsm run` 直接从 CAS 编译成功、`du -sh` 存储显著小于等价 node_modules、同一包跨项目零重复编译时，旧"物化 node_modules"假设退出主路径。
- Verdict：proceed，采用"独立 wjsm-pm crate + wjsm-module Vfs/覆盖层注入 + CLI 组装"架构。

Anti-Entropy Declaration：

- Deletion Class：new-capability（新增能力，不删除现有 FS 解析主路径）。
- Old Path/Object：无删除；FS 模式解析保持默认，CAS 模式为新增注入路径。
- Expected Preserved Behavior：无依赖或本地相对导入的现有项目、所有现有 fixture、`wjsm run file.js` 语义不变。
- External Boundary Touched：yes，新增用户可见 CLI 子命令与全局存储目录 `~/.wjsm/store`。
- Source-of-Truth Data Risk：新增持久化存储（全局 store + 项目 lockfile）；`wjsm cache clear` 等删除操作需谨慎，不触碰用户源码。
- User Confirmation Required：no（新增能力）。

## 5. crate 架构与依赖方向

新增 `crates/wjsm-pm`，workspace 依赖方向：

```
parser → semantic → ir ← backend-wasm → runtime → cli
                              wjsm-module ↗            ↑
                                    ↑                  |
                              wjsm-pm ─────────────────┘
                        (依赖 wjsm-module 的 trait 定义 + wjsm-snapshot-format 的 ABI hash)
```

- `wjsm-pm` 依赖 `wjsm-module`（用其 `Vfs`/`ResolutionOverlay` trait 定义）、`wjsm-snapshot-format`（编译产物缓存 ABI key）。
- `wjsm-module` **不依赖** `wjsm-pm`（trait 定义在 module 侧，实现在 pm 侧）。
- `wjsm-cli` 依赖 `wjsm-pm` + `wjsm-module`，负责组装注入。

### 5.1 wjsm-pm 模块划分（遵循项目文件体量纪律，每文件单一职责）

```
crates/wjsm-pm/src/
  lib.rs                 # 公共 API：install/add/remove/resolve/link_provider
  solver/
    mod.rs               # 求解编排
    npm_semver.rs        # npm 精确 SemVer 区间语义（^ ~ x-range || 预发布规则）
    provider.rs          # PubGrub DependencyProvider（惰性拉 packument）
    duplication.rs       # npm 嵌套重复：instance-splitting 递归求解
    explain.rs           # PubGrub 冲突派生 → 人类可读解释
  registry/
    mod.rs               # registry client 编排
    packument.rs         # GET /<pkg> 元数据、etag 重验证、缓存
    tarball.rs           # tgz 流式解包 + SSRI 完整性校验
    npmrc.rs             # .npmrc registry/scope/_authToken/proxy
  store/
    mod.rs               # CAS store 公共入口 + STORE_VERSION
    index.rs             # SQLite schema + 查询（rusqlite）
    blob.rs              # zstd blob 读写、packfile 追加与 mmap 读取
    manifest.rs          # 包文件清单（tree 对象：path→blob_hash+mode）
    artifact.rs          # L1/L2 编译产物缓存
    vfs.rs               # CAS-backed Vfs 实现（impl wjsm_module::Vfs）
    overlay.rs           # PnP 式解析覆盖层（impl wjsm_module::ResolutionOverlay）
  lockfile/
    mod.rs               # 自有 lockfile 读写
    wjsm_lock.rs         # 格式定义 + 序列化
    migrate.rs           # package-lock/pnpm-lock/yarn-lock/bun.lock 读取迁移
  scripts/
    mod.rs               # task runner + 生命周期脚本 + PATH 注入
  workspace.rs           # workspaces 发现、本地链接、根 lockfile
```

### 5.2 wjsm-module 侧改动（新增 trait，抽象三处磁盘访问）

```rust
// wjsm-module 新增：虚拟文件系统抽象
pub trait Vfs: Send + Sync {
    fn read_to_string(&self, path: &Path) -> Result<String>;
    fn exists(&self, path: &Path) -> bool;
    fn is_dir(&self, path: &Path) -> bool;
    fn read_package_json(&self, dir: &Path) -> Result<Option<String>>;
}

// wjsm-module 新增：解析覆盖层（bare specifier → 具体包/版本/子路径）
pub trait ResolutionOverlay: Send + Sync {
    /// referrer 所属包上下文 + bare specifier → 该依赖被 lockfile 固定到的
    /// 具体包版本在虚拟树中的根路径。None 表示回退到默认 node_modules 遍历。
    fn resolve_bare(&self, specifier: &str, referrer: &Path) -> Result<Option<PathBuf>>;
}
```

- 默认实现 `FsVfs`（现有行为，`std::fs`）+ `NoOverlay`（现有 node_modules 向上遍历），保证零破坏。
- CAS 模式由 `wjsm-pm` 提供 `CasVfs` + `PnpOverlay`，注入到 `ModuleResolver` / `ModuleBundler`。
- resolver.rs:754 → `vfs.read_to_string`；resolver.rs:328 的 `find_package_in_node_modules` → 先问 `overlay.resolve_bare`，命中即用；package_json.rs → `vfs.read_package_json`。

### 5.3 wjsm-semantic 侧改动（可重定位 IR，服务 L1）

L1 编译产物缓存要求 semantic 能单独 lower 一个包并产出位置无关 IR（详见 §8）。owner 落在 `wjsm-semantic`，**不在 `wjsm-pm`**——`wjsm-pm` 只消费缓存产物：

- 新增单包 lower 入口：产出 scope id 从 0 起的模块局部 `Program` 片段 + 重定位表（scope 基址、常量偏移、字符串偏移、未解析 import 符号）。
- 新增链接阶段：把多个局部片段按 bundle 位置重定位合并为全局 `Program`，等价于现有 `lower_modules` 的输出。
- 现有 `lower_modules` 整体路径保留为默认（无 L1 缓存时、入口 bundle）；分离编译是新增路径，与现有 startup snapshot relocatable heap 同源（ADR 0003）。
- 此项与 issue #312 的分离编译地基共享同一"可重定位片段 + 链接"机器，故本计划前置依赖 #312 合并。

## 6. 内容寻址存储（核心：解决小文件 + 空间）

### 6.1 存储布局

```
~/.wjsm/store/v1/
  index.db              # SQLite：包、包文件清单、blob 元数据、编译产物索引
  packs/                # 追加式 packfile，聚合独立 zstd 压缩的 blob（少量 inode）
    0000.pack
    0001.pack
  artifacts/            # L1 IR / L2 cwasm 编译产物（大对象，独立文件或 pack）
  tmp/                  # 下载与解包暂存（原子 rename 入库）
```

存储根含版本后缀 `v1`（参照 pnpm `v11`）；格式不兼容变更时递增，旧版本可并存。

### 6.2 双层去重模型（混合归档 + 文件级 CAS）

- **blob 层（文件级内容寻址）**：每个唯一文件内容 = 一个 blob，blake3 内容哈希寻址，独立 zstd 压缩，追加进 packfile。跨所有包/版本/项目共享。相同 `LICENSE`、相同子模块只存一份。
- **manifest 层（包级归档 = 文件哈希清单）**：每个 `name@version` 有一份 manifest（有序 `relative_path → (blob_hash, mode)` 清单，类似 git tree），manifest 本身也内容寻址。这就是"包级归档"——它是逻辑清单，不是物理 tar，引用共享 blob。

**为何两目标都最优**：
- inode 数 ≈ packfile 数（常数级）+ index.db，不随文件总数增长 → 消灭小文件 inode 爆炸。
- 文件级 blob 去重 → 跨包/跨版本/跨项目相同内容零重复。
- zstd 压缩 → 磁盘占用 = 去重后内容的压缩体积。

10 万个包文件（跨多项目）→ 数千个唯一 zstd blob，聚合进个位数 packfile + 一个 SQLite DB。

### 6.3 SQLite schema（index.db）

```sql
-- 包身份 → manifest
CREATE TABLE packages (
  name        TEXT NOT NULL,
  version     TEXT NOT NULL,
  integrity   TEXT NOT NULL,          -- registry SSRI（sha512）
  manifest_id INTEGER NOT NULL,       -- → manifests.id
  meta        BLOB,                   -- MessagePack：package.json 关键字段快照
  PRIMARY KEY (name, version)
);

-- 包文件清单（tree）
CREATE TABLE manifests (
  id   INTEGER PRIMARY KEY,
  hash BLOB NOT NULL UNIQUE           -- manifest 内容哈希
);
CREATE TABLE manifest_entries (
  manifest_id INTEGER NOT NULL,
  rel_path    TEXT NOT NULL,
  blob_hash   BLOB NOT NULL,          -- → blobs.hash
  mode        INTEGER NOT NULL,       -- 可执行位等
  PRIMARY KEY (manifest_id, rel_path)
);

-- blob 元数据：内容哈希 → packfile 位置
CREATE TABLE blobs (
  hash        BLOB PRIMARY KEY,       -- blake3 内容哈希
  pack_id     INTEGER NOT NULL,
  offset      INTEGER NOT NULL,
  clen        INTEGER NOT NULL,       -- 压缩后长度
  ulen        INTEGER NOT NULL        -- 原始长度
);

-- 编译产物缓存（§8）
CREATE TABLE artifacts (
  cache_key   BLOB PRIMARY KEY,       -- blake3(输入 § 8.2)
  tier        INTEGER NOT NULL,       -- 1=IR, 2=cwasm
  pack_id     INTEGER,
  offset      INTEGER,
  clen        INTEGER,
  ulen        INTEGER
);
```

- 采用 SQLite WAL 模式支持并发读 + 单写；元数据 MessagePack 编码（参照 pnpm 从 JSON 小文件迁移到 index.db 的经验，减少 syscall 与空间开销）。
- 写入事务：下载 → 校验完整性 → 解包为 blob → 追加 packfile → 事务性写 blobs/manifests/packages → 原子提交。中断可回滚（packfile 追加内容成为不可达垃圾，由 `wjsm store gc` 回收）。

### 6.4 读取路径（编译器直供）

```
resolver 需要 lodash/index.js 源码
  → overlay.resolve_bare("lodash", referrer) → 虚拟根 <vroot>/lodash@4.17.21
  → wjsm-module 现有 exports/main 解析 → 虚拟路径 <vroot>/lodash@4.17.21/index.js
  → vfs.read_to_string → CasVfs 查 index.db（package→manifest→rel_path→blob_hash）
  → mmap packfile[pack_id] 偏移 offset → zstd 解压 → UTF-8 String
  → wjsm_parser::parse_module
```

全程零文件系统物化。blob 永不需要落成真实文件。

### 6.5 增强（可选，标注为后续优化）

- **共享 zstd 字典**：JS 源码 token 高度重复，训练一个 store 级 zstd dictionary 可显著提升小文件压缩率。首版用无字典 zstd，字典训练留作 `wjsm store optimize`。
- **reflink 导出**：若未来加 `--node-modules-dir` 逃生舱，从 packfile 解出的 blob 用 reflink/hardlink 物化，复用 pnpm/bun 策略。

## 7. 依赖版本求解（PubGrub 内核 + npm 语义）

### 7.1 为何 PubGrub

PubGrub（uv / dart pub / cargo 新式求解器）用 CDCL 式冲突驱动子句学习，回溯高效、冲突时产出人类可读的完整解释链。Rust 有成熟的 `pubgrub` crate。相比 npm/pnpm 的贪心安装，PubGrub 在 peer dependency 冲突等真实场景给出可诊断的失败原因，而非静默错误或指数回溯。

### 7.2 核心张力与解法：npm 允许同包多版本共存

**关键设计点（ADR 信号）**：PubGrub 假设"每个包在解中至多一个版本"（Python/Rust/Dart 语义）。但 npm 生态**依赖同名包多版本嵌套共存**（两个包各需不同 `lodash` 版本时都能装）。纯 PubGrub 单版本求解会在 npm 能成功的场景**失败**，不满足"完全兼容 npm"。

解法——**PubGrub 内核 + 嵌套重复叠加（instance-splitting）**：

1. 先用 PubGrub 求"去重最大化"解：尽可能让每个包名收敛到单版本（这也是 npm 提升/dedup 的目标）。
2. 当 PubGrub 证明某包名无法用单版本满足所有 dependent 的约束时，**不立即判失败**，而是将该包按 dependent 子树**分裂为多个实例**，对冲突子锥递归 PubGrub 求解。产出 npm 兼容的嵌套多版本结果。
3. 仅当分裂后子锥仍不可满足（如 peer dependency 硬冲突）时，才判定真失败，并用 PubGrub 的派生链给出解释。

这保留了 PubGrub 的诊断能力（真冲突场景），又复现了 npm 的重复共存语义（可满足场景）。这是本设计相对纯 PubGrub 工具（uv/pip）的 npm 适配创新点。

### 7.3 npm SemVer 精确语义

npm 的区间语义与 Cargo `semver` crate 有差异（`x-range`、`||` 并集、`~`/`^` 在 0.x 的特殊行为、预发布版本的包含规则、`*`/空串）。首版实现独立 `npm_semver.rs` 精确匹配 npm/node-semver 行为，不复用 Cargo semver 的默认语义。作为 PubGrub 的 `Version` + `VersionSet` 实现。

### 7.4 惰性 DependencyProvider

`provider.rs` 实现 PubGrub `DependencyProvider`：按需从 registry 拉 packument（只拉解析闭包内的包），缓存进 store。求解与下载解耦——求解只需版本 + 依赖区间元数据，tarball 在求解定稿后惰性补齐。

### 7.5 peer dependencies

peerDependencies 参与约束（作为对 dependent 环境的要求），冲突时进入 §7.2 的真失败分支并给出 PubGrub 解释。optionalDependencies 求解失败可跳过（不判全局失败）。

## 8. 分层编译产物缓存（AOT 杀手锏，本计划核心目标）

**范围决策**：本计划**前置依赖 issue #312**（分离编译 loader / multi-instance shared-env 已合并入主干），并把 **L1 per-package 跨项目编译产物复用作为一等目标**（非延后）。缓存 **L1 可重定位 IR + L2 cwasm 片段两层**。

### 8.1 三层

- **L0 源码 blob**：即 §6 的 CAS blob（归一化 UTF-8 源码）。
- **L1 可重定位 IR**：单个包 parse + lower 后的**位置无关 IR 片段**，按包内容哈希寻址缓存。跨项目复用——避免每个项目重复 parse/lower 相同的包。省 parse/lower 开销。
- **L2 cwasm 片段**：单个包编译后的 WebAssembly 片段，按内容寻址缓存，链接时装配。省最重的 backend 编译开销。复用现有 `runtime_startup.rs` 的 `compile_or_load_cached`（wasmtime `Module::deserialize_file`）机制思路，扩展到包粒度。

### 8.2 命门：scope id 项目无关化（可重定位 IR）

**核心技术障碍（已验证）**：现有 `wjsm-semantic` 中 scope id 是**全局单调递增的 arena 索引**（`scope.rs:61`：`idx = self.arenas.len()`），且被编码进 IR 变量名（`format!("${scope_id}.{name}")`，如 `$7.x`）。`lower_modules` 里所有模块共用同一棵 `ScopeTree`，按解析图 BFS 顺序 push_scope。后果：**同一个 `lodash@4.17.21` 在项目 A 里是 `$7.foo`、在项目 B 里是 `$23.foo`，IR 字节不同**——若 L1 key 用 IR 内容哈希，跨项目 100% miss，L1 形同虚设。

解法——**可重定位 IR（分离编译）**：

1. `wjsm-semantic` 支持**单独 lower 一个包**，产出 **scope id 从 0 起的模块局部 IR**（类比 `.o` 目标文件的本地符号）。
2. IR 中的**位置相关引用统一标注为可重定位项**：scope id、全局常量索引、DataSection 字符串偏移、跨模块 import 绑定。
3. **链接阶段**把每个包的局部 IR 按其在 bundle 中的位置**重定位到全局空间**（scope id 加基址偏移、常量/字符串偏移重定位、import 绑定解析到目标模块的全局符号）。这与现有 **startup snapshot 的 relocatable heap** 思路同源（相对偏移 + restore 时 remap，见 ADR 0003）。
4. L1 缓存 key 只依赖**包自身内容 + 编译器版本**，与项目无关 → 跨项目命中。

### 8.3 缓存 key

```
L1 key = blake3(package_manifest_hash ‖ semantic_compiler_version ‖ lowering_flags)
L2 key = blake3(L1_key ‖ backend_abi_hash ‖ gc_flavor)
```

- `package_manifest_hash` = §6 包的 manifest 内容哈希（与解析图顺序无关）。
- `backend_abi_hash` / `gc_flavor` 复用 `wjsm-snapshot-format::abi_hash` 与既有 GC 选择常量，任一变更自动失效缓存。
- 编译器版本变更 → key 变更 → 自动失效，不需手动清缓存。

### 8.4 L2-bundle 回退（无 #312 时的降级路径）

在可重定位 IR / 分离编译落地前，或对无法分离编译的入口 bundle，保留**整 bundle 粒度**回退：key = `blake3(lockfile 解析图 ‖ compiler_ver ‖ backend_abi_hash ‖ gc_flavor)`，直接复用现有 cwasm 缓存机制。这不是首版目标粒度，仅作降级与入口模块兜底。

### 8.5 与现有 cache 命令的关系

现有 `wjsm cache`（`CacheCommand::Stats/Clear`）管理 runtime 编译 WASM 缓存。本设计的编译产物缓存并入同一心智，但归 store 管理；`wjsm cache` 扩展为可展示/清理 L1/L2 产物。避免两套并行缓存 owner。

## 9. lockfile 与生态互操作

### 9.1 自有格式 `wjsm-lock.toml`

内容：
- 解析图：每个 `name@version` 的 integrity、依赖边（解析后的具体版本）、instance-splitting 产生的实例身份。
- CAS 引用：manifest_hash（可选，供离线校验）。
- 编译产物提示：L2 cache_key（可选，供 CI 预热判断）。
- 求解器元数据：compiler_version、npm_semver 版本，供确定性复现。

确定性：相同 `package.json` + 相同 registry 状态 → 相同 lockfile（排序稳定）。

### 9.2 迁移读取

`migrate.rs` 读取 `package-lock.json`（v2/v3）、`pnpm-lock.yaml`、`yarn.lock`（v1 + berry）、`bun.lock`，将已固定的版本作为 PubGrub 的**优先解 / 约束提示**，尽量复现原解，减少版本漂移。首次 `wjsm install` 检测到这些文件即可无缝接管存量项目，生成 `wjsm-lock.toml`（不删除原 lockfile，除非用户显式 `--prune`）。

## 10. CLI 设计

| 命令 | 承接 | 语义 |
|---|---|---|
| `wjsm run <file>` | `node file.js` | **不变**，执行文件（现有能力） |
| `wjsm install` | `npm install` | 解析 + 下载 + 写 CAS + 生成 lockfile |
| `wjsm add <pkg>[@range]` | `npm install <pkg>` | 加依赖 + 更新 manifest/lockfile |
| `wjsm remove <pkg>` | `npm uninstall <pkg>` | 删依赖 |
| `wjsm task <name>` | `npm run <name>` | 执行 `scripts.<name>` + pre/post 生命周期 |
| `wjsm x <pkg>` | `npx <pkg>` | 临时拉取执行包 bin |

- `wjsm run` 遇到参数是已声明 script 名且文件不存在时，友好提示 `did you mean 'wjsm task <name>'?`，但**不自动改行为**（保持语义正交，避免 bun 式查找顺序歧义）。
- `wjsm task` 执行环境注入 `wjsm` 到 PATH（使 script 内 `node`/`npm` 调用可被劫持为 wjsm，逐步替换）；script 内 bin 依赖从 CAS 解析。
- workspaces：`wjsm install` 在 workspace 根统一求解，本地包以 `file:`/虚拟链接接入解析覆盖层。

## 11. 安全边界

- **完整性**：tarball 下载后按 packument `dist.integrity`（SSRI sha512）校验；install 时按 lockfile integrity 复校。校验失败拒绝入库。
- **生命周期脚本**：依赖的 `postinstall`/`preinstall`/`install` 脚本**默认禁用**（参照 pnpm `onlyBuiltDependencies` / bun `trustedDependencies`），需 `package.json` `trustedDependencies` 或 `wjsm install --allow-scripts` 显式允许。原生编译 postinstall 不在首版（见非目标）。
- **用户脚本**：`wjsm task` / `wjsm x` 执行用户自己的 script/bin，属正常授权范围。
- **.npmrc auth**：`_authToken` 从 `.npmrc`/env 读取，不落日志、不写 lockfile。
- **文件系统边界**：store 读写限于 `~/.wjsm/store`（可 `WJSM_STORE_DIR` 覆盖）；registry 下载限于配置的 registry host。

## 12. 兼容边界与 ADR 信号

兼容边界：

- 无依赖 / 纯本地相对导入的现有项目行为不变（FS 模式默认，CAS 覆盖层仅在有 lockfile/依赖时激活）。
- 所有现有 fixture、`wjsm run file.js` 语义不变。
- **已知代价**：首版无物化 node_modules，第三方 Node 工具链（外部 editor LSP、eslint、tsc）看不到依赖。wjsm 自有 `check`/`lint`/`fmt` 直接走 CAS 不受影响。`--node-modules-dir` 逃生舱（reflink 物化）标注为后续扩展，需独立 ADR。
- 迁移不删除原生态 lockfile（除非 `--prune`）。

ADR 信号（实现后需补 ADR）：

1. 新增全局内容寻址存储 `~/.wjsm/store` 作为新的持久化 source-of-truth（blob 内容寻址 + lockfile 解析结果分离）。
2. `wjsm-module` 引入 `Vfs`/`ResolutionOverlay` trait，把磁盘访问抽象化——跨 crate 契约变更。
3. PubGrub 内核 + npm instance-splitting 的求解语义——为何不用纯贪心、也不用纯单版本 PubGrub。
4. 分层编译产物缓存 + **可重定位 IR / 分离编译**（scope id 项目无关化、重定位表、链接阶段）——为何需要位置无关 IR、与 issue #312 分离编译地基及 startup snapshot relocatable heap（ADR 0003）的同源关系。

## 13. 测试策略与验收线

### 单元测试（wjsm-pm）

- `npm_semver`：`^`/`~`/`x-range`/`||`/预发布规则逐条对照 node-semver 行为表。
- `solver`：单版本收敛、instance-splitting 多版本共存、peer 冲突产出解释、optional 跳过。
- `store`：blob 去重（相同内容单 blob）、zstd 往返、packfile 追加 + mmap 读、SQLite 事务回滚、跨版本共享文件去重计数。
- `registry`：packument 解析、SSRI 校验失败拒绝、etag 重验证、`.npmrc` scope/token 解析。
- `lockfile`：自有格式确定性往返、`package-lock`/`pnpm-lock`/`yarn.lock`/`bun.lock` 迁移读取。
- `artifact`：L1/L2 key 随 compiler_ver/abi 变更失效；同一包 manifest_hash 在不同解析图顺序下 L1 key 稳定（项目无关）。
- `relocatable_ir`（wjsm-semantic）：单包 lower 产出模块局部 IR + 重定位表；链接后与 `lower_modules` 整体路径的全局 IR **逐指令等价**（IR 快照对照）；scope id / 常量偏移 / 字符串偏移 / import 符号重定位正确。

### 集成 / fixtures

新增 `fixtures/pm/` 子集（真实小包或 mock registry）：

- `install_basic`：装一个无依赖包，`du` 校验无 node_modules、store 有 blob。
- `install_dedup`：两项目共享同版本包，store blob 不重复。
- `install_multi_version`：instance-splitting，两包各需不同版本共存。
- `install_conflict`：peer 冲突，输出 PubGrub 解释。
- `run_from_cas`：`wjsm run` 直接从 CAS 编译执行依赖，无 node_modules。
- `task_scripts`：`wjsm task build` 执行 pre/build/post。
- `x_bin`：`wjsm x` 临时执行包 bin。
- `migrate_pnpm`：读 `pnpm-lock.yaml` 生成 `wjsm-lock.toml`。
- `workspace_link`：monorepo 本地包链接解析。

### 验收命令

- `cargo nextest run -p wjsm-pm`
- `cargo nextest run -E 'test(pm__)'`
- `cargo nextest run -p wjsm-module`（回归 Vfs 抽象不破坏 FS 模式）
- 冒烟：`wjsm install`（含依赖的 fixture 项目）后 `wjsm run` 成功且磁盘无 node_modules。

## 14. 复杂度与文件边界

Complexity Budget：

- Artifact class：跨 pm/module/cli 的核心包管理架构 + 新持久化存储。
- Target：新 crate `wjsm-pm`（solver/registry/store/lockfile/scripts/workspace 六大子模块）；`wjsm-module` 新增 `Vfs`/`ResolutionOverlay` trait 并把三处磁盘访问改为 trait 调用；**`wjsm-semantic` 新增可重定位 IR 单包 lower 入口 + 链接阶段（服务 L1，§5.3/§8.2）**；`wjsm-cli` 新增 install/add/remove/task/x 子命令组装。
- Current pressure：`resolver.rs` 已 1576 行、`cjs_transform.rs` 984 行、runtime `lib.rs` 2115 行——已超纪律，禁止再往这些大文件塞包管理逻辑。
- Planned governance：所有包管理逻辑进 `wjsm-pm` 独立 owner 文件（每文件单一职责 ≤500 行）；`wjsm-module` 仅做 trait 抽象的微创手术（3 处 `fs::` 调用改为 trait 调用 + 新增 trait 定义文件），不改解析算法；**`wjsm-semantic` 的可重定位 IR 是实打实的架构演进（非微创），新增单包 lower / 链接 owner 文件，复用 #312 已引入的分离编译机器，不重复造轮子**。
- Budget result：within-budget（新增 crate 承载复杂度，现有大文件不增负）。

## 15. 分阶段实施建议（供 writing-plans 细化）

- **P1 存储与解析地基**：`store`（SQLite + blob + zstd + packfile + manifest）+ `wjsm-module` Vfs/Overlay trait + FsVfs 保持默认。验收：现有测试全绿 + store 单测。
- **P2 registry + solver**：`npm_semver` + PubGrub provider + instance-splitting + registry client + SSRI。验收：solver/registry 单测 + 离线 mock registry install。
- **P3 install/lockfile/CLI**：`wjsm install/add/remove` + 自有 lockfile + 迁移读取 + CasVfs/PnpOverlay 接入编译器。验收：`install_*`/`run_from_cas` fixture。
- **P4 task/x/workspaces**：脚本运行器 + npx 等价 + monorepo。验收：`task_scripts`/`x_bin`/`workspace_link`。
- **P5 编译产物缓存（本计划核心，前置 #312 已合并）**：先在 `wjsm-semantic` 落地**可重定位 IR + 链接**（scope id 模块局部化 + 重定位表 + 链接阶段，§8.2），再实现 **L1 可重定位 IR 缓存 + L2 cwasm 片段缓存**（§8.1/§8.3），跨项目包级复用。L2-bundle（§8.4）作为入口/降级兜底。验收：同一包在两个项目 install 后 L1/L2 缓存命中（内容寻址 key 一致）；`wjsm-semantic` 分离编译产出与 `lower_modules` 整体路径等价（IR 快照对照）。

## 附录：工作制品

TaskIntentDraft：

- Requested outcome：完成 wjsm 包管理设计，批准后进 writing-plans。
- Success evidence：spec 覆盖 CAS 存储引擎、PubGrub+npm 求解、惰性下载、自有 lockfile + 迁移、编译器直供、分层编译产物缓存、CLI（install/add/remove/task/x/workspaces）、安全边界、兼容边界、测试与分阶段。
- Stop condition：spec 落盘 + 用户审阅通过。
- Non-goals：见 §3。
- Risks：PubGrub 与 npm 嵌套重复语义的调和正确性；**可重定位 IR / 分离编译的正确性（scope id 重定位、字符串/常量偏移重定位、跨模块 import 符号解析必须与现有 `lower_modules` 整体路径等价）**；本计划前置 #312 合并的时序风险；无 node_modules 对第三方工具链的兼容代价；SQLite/packfile 并发写正确性。

BaselineUsageDraft：

- Required baseline refs：CLAUDE.md AOT/文件纪律；`wjsm-module` resolver.rs:328/754、graph.rs、package_json.rs:60、resolution_options.rs、runtime_resolution.rs；`runtime_startup.rs` cwasm 缓存；`wjsm-snapshot-format` abi_hash；issue #311/#312 spec。
- Delivered context refs：pnpm/yarn/bun/deno 机制研究（DeepWiki）。
- Cited in design refs：§1、§5、§6、§7、§8、§9。
- Missing refs：`wjsm-semantic::lower_modules` 模块间耦合度（L1 分离编译可行性）——planning 阶段验证。
- Decision：continue。

ImpactStatementDraft：

- Affected layers：新增 `wjsm-pm`；`wjsm-module` trait 抽象 + 3 处磁盘访问改造；`wjsm-cli` 子命令；新增全局 store + 项目 lockfile 持久化；fixtures `pm/`。
- Owners：`wjsm-pm` 拥有 store/solver/registry/lockfile/scripts/workspace；`wjsm-module` 拥有解析算法（不变）；CLI 拥有组装注入。
- Invariants：wjsm-module 不依赖 wjsm-pm；blob 内容寻址；lockfile 解析结果分离；FS 模式默认兼容。
- Compatibility：现有项目/fixture/run 语义不变；无 node_modules 的第三方工具链代价明确标注。

Product Risk Lens：

- Value：无缝替换 npm/npx/npm run，彻底解决 node_modules 小文件 + 空间；AOT 独有的跨项目编译产物复用。
- Non-goals：publish、原生 postinstall、git/远程 tarball 依赖、物化 node_modules。
- Trade-offs：无物化 node_modules 换来空间/inode 极致优化，但牺牲第三方 Node 工具可见性（wjsm 自有工具链不受影响）；PubGrub 内核换来诊断能力，但需 instance-splitting 适配 npm 重复语义。
- Decision needed：本 spec 推荐方案已含全部关键决策；求解器 instance-splitting 的正确性边界建议在 planning 阶段深挖并可能单独 ADR。
