# Build-Time Embedded Runtime 实施计划

**Goal**：把 wjsm 出厂后不变的全部 runtime 制品在 `cargo build` 期固化进二进制：startup snapshot 字节、共享 wasm helper 模块、内部 builtin JS 扩展。运行时再不为这些"分发后稳定"的内容付任何编译/初始化代价；用户 JS 编译产物完全不缓存。

**Architecture**：

```
crates/wjsm-runtime-snapshot/   # 新 crate，build.rs 产 snapshot 字节，pub static include_bytes!
crates/wjsm-runtime-support/    # 新 crate，build.rs 产 support.cwasm（wasmtime precompiled）
crates/wjsm-runtime/            # 新增 builtin_js/*.js + install API；原 startup_snapshot 保留为 fallback
crates/wjsm-backend-wasm/       # 用户 wasm 改成 import memory/globals/table/helpers，不再内联 helpers
crates/wjsm-module/             # resolver 不变（不引入 wjsm:* 用户命名空间）
crates/wjsm-cli/                # main_entry 启动时 install_embedded_snapshot + install_embedded_support
```

三类 embedded 制品共享同一个 ABI hash 边界：snapshot/support/builtin-js 任一变更都使 embedded snapshot 失效，统一走 cold rebuild。Snapshot ABI hash 输入新增 support module wasm hash + builtin JS bundle source hash。

**Tech Stack**：
- Rust 2024，cargo workspace 新增三个 crate，build.rs 生成 OUT_DIR 制品
- `wasmtime::Engine::precompile_module` 产 `.cwasm`，运行时 `Module::deserialize` 加载
- `wasm-encoder` 在 backend 重写 user module 的 import 段
- `include_bytes!(concat!(env!("OUT_DIR"), …))` 嵌入字节
- 共享 wasmtime memory/table/global：runtime 用 `Memory::new` / `Table::new` / `Global::new` 在 instantiate 前创建，然后通过 Linker 提供给 support 与 user 两个 instance
- wjsm-runtime 新加 `install_embedded_snapshot(&'static [u8])` / `install_embedded_support(&'static [u8])` 注入 API；运行时通过 `OnceLock` 持有

**Baseline / Authority Refs**：
- `docs/adr/0003-startup-snapshot-boundary.md` — snapshot 边界、当前 capture/restore owner、ABI hash 输入
- `docs/adr/0002-runtimestate-stays-flat.md` — 不允许借机重组 RuntimeState
- `docs/async-scheduler.md` — Store/wasm memory 仅 scheduler owner 访问
- `AGENTS.md` Startup snapshot / Function-property handle layout / WASM contract 段
- 当前源码证据：
  - `crates/wjsm-runtime/src/startup_snapshot.rs` — capture/restore owner
  - `crates/wjsm-runtime/src/startup_snapshot_format.rs` — format + abi_hash
  - `crates/wjsm-runtime/src/startup_snapshot_cache.rs` — runtime cache
  - `crates/wjsm-runtime/src/lib.rs:911-1390` — execute path / instantiate bundle
  - `crates/wjsm-backend-wasm/src/compiler_module.rs:243-340,780-940` — 当前 helper 函数索引、bootstrap/init_function_props 阶段函数
  - `crates/wjsm-backend-wasm/src/compiler_helpers.rs:1-1538` — 当前所有 wasm-emitted helper bodies
  - `crates/wjsm-backend-wasm/src/host_import_registry.rs` — host 导入 spec
  - `crates/wjsm-runtime/src/wasm_env.rs` — 当前 export memory/globals/table 的 contract
- 外部参考：
  - Deno `cli/snapshot/build.rs` + `cli/snapshot/lib.rs` — `CLI_SNAPSHOT.bin` 通过独立 wrapper crate include_bytes! 的范式
  - Deno `runtime/snapshot.rs::create_runtime_snapshot` — extension JS 在 snapshot 期注册的范式
  - V8 startup snapshots — heap state 不能含外部世界状态

**Compatibility Boundary**：
- 现有 fixture `.expected` 输出不变
- `wjsm-runtime` public API 保持：`execute / execute_with_writer` 签名和返回值不变；新增 `install_embedded_snapshot` / `install_embedded_support` 是可选注入
- `RuntimeState` 字段仍扁平
- Snapshot 仍只覆盖 pristine runtime startup heap；用户对象、promise/timer/microtask/fetch/stream 活动状态、`SharedRuntimeState`、`eval_cache`、scheduler 状态都不能被任何 embedded 制品捕获
- 用户 JS 模块解析行为不变；不引入 `wjsm:` 命名空间
- 旧 per-module helper 内联 codegen 必须**完全退役**；旧 user module 自己 export memory 的 contract 必须**完全退役**
- ABI hash 不一致一律 cold rebuild，不静默运行
- 三个新 crate 都加 `embedded` cargo feature，默认开启；`--no-default-features` 时 fallback 到当前运行时 capture/runtime cache 路径

**Verification**：

每个子阶段对应 crate 定向测试，加节点 bench：

```bash
cargo nextest run -p wjsm-backend-wasm
cargo nextest run -p wjsm-runtime -E 'test(startup_snapshot) or test(async_scheduler) or test(async_reentry)'
cargo nextest run -E 'test(happy__) or test(modules__) or test(semantic__)'
cargo nextest run --workspace
cargo test -p wjsm-runtime --release --lib --no-run \
  && target/release/deps/wjsm_runtime-*[0-9a-f] bench_execute_phases --ignored --nocapture
```

每个阶段必须输出对比数据，特别是：
- P1 完成：first-run（无磁盘 cache）`full execute` ≤ 旧 cold path
- P2 完成：`module_only`（wasmtime compile）≤ 旧值的 60%
- P3 完成：embedded snapshot 包含 builtin_js 注入后的 globals，且 snapshot/restore on/off 输出一致

**ADR Signal**：新增持久 build-time embedded runtime 边界、user wasm import 形态变更、support module wasm ABI、builtin JS 扩展 contract。完成后写 `docs/adr/0004-build-time-embedded-runtime.md`，并将 `docs/adr/0003-startup-snapshot-boundary.md` 标 superseded-by-0004。

---

## ABI Hash 输入（最终版）

`startup_snapshot_format::abi_hash()` 在原有 6 项基础上追加：

```text
+ support_module_wasm_hash: SHA-256(support.wasm bytes)
+ builtin_js_bundle_hash:   SHA-256(sorted concat of crates/wjsm-runtime/builtin_js/*.js)
+ env_table_layout_version: u32（imported memory/table/globals 的 module-level layout，每次 ABI 变更 +1）
```

Embedded snapshot bytes 中 header.abi_hash 必须等于运行时 `abi_hash()`，不一致一律 evict + cold rebuild。`wjsm-runtime-snapshot/build.rs` 与 `wjsm-runtime-support/build.rs` 都要用 `cargo:rerun-if-changed=` 把对应输入纳入重建链。

---

## Decision Hygiene Review

```text
First-principles invariants:
- Non-negotiable goal: 三类 ship-time-stable 制品在 cargo build 后已固化进二进制，运行时不重做编译/初始化；用户 JS 不被缓存
- Non-negotiable constraints: scheduler/RuntimeState 扁平不变；user-facing JS 语义不变；snapshot 不含外部世界状态；ABI hash 校验严格
- Historical assumptions to delete:
  - 用户 wasm 必须 export memory（改为 import）
  - 用户 wasm 必须自己内联 obj_new/obj_get/.../bootstrap_once（改为 import 共享 support）
  - startup snapshot 必然在 first-run 才生成（改为 build-time 已就绪）

Owner / retirement matrix:
- New canonical owner:
  - wjsm-runtime-snapshot/build.rs: snapshot 字节产出
  - wjsm-runtime-support/build.rs + src: support wasm + cwasm
  - wjsm-runtime/builtin_js/: 内置扩展 JS 源
  - wjsm-runtime/src/lib.rs: install_embedded_* 入口
  - wjsm-cli/src/lib.rs: 进程启动时安装
  - wjsm-backend-wasm/src/compiler_module.rs: 用户 wasm import 形态
- Old owner: 当前 per-module 内联 helpers + 当前 first-run capture-on-demand 路径
- Compat-only carrier: runtime cache（OnceLock + 磁盘）保留为 fallback，仅当 `embedded` feature 关闭或 install_* 未被调用时启用
- Delete-first / retirement trigger: 一旦 P2 切换完成，旧 inline helper codegen 全部删除；P1 切换后 capture-on-demand 路径在 default feature 下不再触发

Falsification matrix:
- Dependency-removal test: 关闭 embedded feature 后，所有 fixture 仍通过；任意一项 ABI 输入修改后 embedded 字节必失效
- Counterexample scenario: builtin JS 不小心修改了 promise/timer 等活动状态字段，capture-time 断言必须命中
- Must fail / degrade / remain correct cases: ABI hash mismatch 必须 cold rebuild；deserialize cwasm 失败必须 fallback；安装错误字节必须 install API panic 或拒绝

Verdict:
- Adopt: 三阶段 build-time embedded + 单一 ABI hash + opt-out feature
- Blocking gaps: support module 的 imported memory/table/globals contract 当前未实现；用户 wasm 改成 import 形态需 backend 大改；builtin JS 扩展期评估未实现
- Next evidence: P2.1 设计任务的 ABI 文档完成，再开始 P2.2 实现
```

---

## Plan Pressure Test

```text
- Owner / contract / retirement:
  - Owner: 三个新 crate 各自单一 owner；wjsm-runtime 仅消费 + install
  - Contract: imported env.{memory,__table,globals} 是 user/support/runtime 共享面；helper exports 是稳定 ABI
  - Retirement: 旧 helper inline codegen + 旧 export memory contract 一并退役
- Architecture integrity / higher-level path:
  - 一份 support module 服务所有用户 instance，避免 per-module 重复编译
  - 一份 snapshot 服务所有 first-run，避免运行时 capture
  - 一份 builtin JS bundle，统一在 snapshot 期注入，不引入运行时 lazy 装载
- Verification scope:
  - Unit: snapshot format/abi_hash、support module instantiate、builtin_js eval、resolver 不变
  - Integration: snapshot on/off/embedded/runtime cache/embedded ABI mismatch；support module deserialize/降级；builtin_js 注入后 globals 在用户代码可见
  - Performance: P1 first-run / P2 module compile / P3 startup eval 三段计时
- Task executability:
  - P1（独立、最小、最先验证 build.rs 模式）→ P2（最大改动）→ P3（依附 P2 后的 instance 形态）
  - 每个 P2 子阶段独立 commit，独立测试
- Pressure result: proceed
```

---

## Plan-Time Complexity Check

```text
Complexity Budget:
- Artifact class: workspace 新增 3 crate；wjsm-runtime/lib.rs 启动路径再加 install/装载逻辑；wjsm-backend-wasm 大重构
- Target files / artifacts:
  - 新增: crates/wjsm-runtime-snapshot/{Cargo.toml, build.rs, src/lib.rs}
  - 新增: crates/wjsm-runtime-support/{Cargo.toml, build.rs, src/lib.rs, src/abi.rs, src/codegen.rs}
  - 新增: crates/wjsm-runtime/builtin_js/{manifest.rs, *.js}
  - 修改: crates/wjsm-runtime/src/{lib.rs, startup_snapshot.rs, startup_snapshot_format.rs, startup_snapshot_cache.rs, wasm_env.rs}
  - 修改: crates/wjsm-backend-wasm/src/{compiler_module.rs, compiler_core.rs, compiler_helpers.rs, compiler_array_helpers.rs, compiler_data.rs, lib.rs}
  - 修改: crates/wjsm-cli/src/lib.rs
  - 修改: AGENTS.md, docs/adr/0003-…(supersede), docs/adr/0004-build-time-embedded-runtime.md, docs/aegis/INDEX.md
- Current pressure: lib.rs 2000+ 行；compiler_helpers.rs 1538 行；compiler_module.rs 1250+ 行
- Projected post-change pressure: compiler_helpers.rs 大幅缩水（helper bodies 移到 support crate）；compiler_module.rs 持平（contract 改 import 化但删除内联生成）；lib.rs 持平（install 走单独 helper 模块）
- Budget result: at-risk
- Planned governance: 每子阶段独立 commit；P2 内部强制按 4 步切换，避免 ABI 一次性翻天

Plan-Time Complexity Check:
- Better file boundary: snapshot/support 各自 owner crate；builtin JS 走 wjsm-runtime/builtin_js 子目录；backend 改动按 helper 类别分批
- Recommendation: split task per phase
```

---

## Tasks 总览

| 阶段 | 任务 | 验收 |
|---|---|---|
| P0 | 工作区准备：3 crate skeleton + Cargo workspace member 注册 | `cargo build --workspace` 通过，新 crate 空骨架编译 |
| P1.0 | 抽 snapshot lib：把 `startup_snapshot_format` 拆成独立 `wjsm-snapshot-format` crate（pure，无 wasmtime） | crate 单独编译；wjsm-runtime 仍正常 |
| P1.1 | `wjsm-runtime-snapshot` build.rs：编译空 seed JS、capture、写 OUT_DIR/snapshot.bin | OUT_DIR/snapshot.bin 存在，header.abi_hash 等于运行时 |
| P1.2 | wjsm-runtime `install_embedded_snapshot` 入口 + 与 cache get_cached 优先级 | 单测：install 后 cache lookup 命中 embedded |
| P1.3 | wjsm-cli 启动时 install | `wjsm run hello.js` 走 embedded 路径，输出不变 |
| P1.4 | bench：embedded first-run vs runtime first-run 对比 | embedded first-run 不付 cold bootstrap |
| P2.0 | 设计 support module ABI：列出 imported env.memory/env.__table/env.globals/host imports + exports；写 `wjsm-runtime-support/src/abi.rs` 常量与 ABI hash 输入 | abi.rs 单元测试通过，ABI hash 稳定 |
| P2.1 | `wjsm-runtime-support/src/codegen.rs` + build.rs：复用 `wjsm-backend-wasm` 现有 helper emit 逻辑产 support.wasm；用 `Engine::precompile_module` 写 OUT_DIR/support.cwasm | OUT_DIR/support.cwasm 存在；wasmtime deserialize 成功；helper exports 完整 |
| P2.2 | wjsm-runtime instantiate path：共享 memory/table/globals 创建，先 instantiate support，再 instantiate user | 单测：empty user wasm + support 双 instance 不报错；helper 调用穿透 |
| P2.3 | 切 object helpers：obj_new/obj_get/obj_set/obj_delete 改为 user wasm import；删除 backend 内联生成 | 全 fixture 中涉及对象的 happy/errors 通过 |
| P2.4 | 切 array/elem helpers：arr_new/elem_get/elem_set | array 相关 fixture 通过 |
| P2.5 | 切 utility helpers：string_eq/to_int32/get_proto_from_ctor | 全部 string/typeof/proto 相关 fixture 通过 |
| P2.6 | 切 bootstrap：`__wjsm_bootstrap_once` / `__wjsm_init_function_props` 改为从 support import；user main 不再调用 inline | startup_snapshot 全部测试 + happy 通过 |
| P2.7 | 重新 bake P1 snapshot（ABI 已变）；clean 后 first-run 仍命中 embedded | embedded snapshot abi_hash 与运行时一致；workspace 全绿 |
| P2.8 | bench：support module 接入后 module_only 时间 | 期望下降至旧值 60% 以下 |
| P3.0 | 框架：crates/wjsm-runtime/builtin_js/ + manifest.rs 列出 ordered JS 文件；snapshot build.rs 在 capture 前依次执行每个 JS | builtin_js 为空时 snapshot 与今日字节级一致 |
| P3.1 | ABI hash 输入纳入 builtin_js bundle SHA-256；任何 .js 修改触发 OUT_DIR/snapshot.bin 重建 | 单测：改一个 JS 文件后 abi_hash 变 |
| P3.2 | 添加占位 builtin JS 文件（仅 sentinel global）+ runtime 验证 | snapshot 命中后用户代码可读到 sentinel global |
| P4.0 | 旧路径退役：删除 backend 内联 helper bodies、删除旧 export memory contract、删除旧 capture-on-first-run 默认路径（feature 关闭时仍存在） | 代码搜索零旧 owner 残留 |
| P4.1 | 文档：写 ADR 0004，标 ADR 0003 superseded-by-0004；更新 AGENTS.md、INDEX.md | docs 自审通过 |
| P4.2 | 全工作区验证 + bench 三段证据 | 见 Verification |

---

# P0：工作区准备

**Why**：三个新 crate 必须先注册到 workspace，先验证 cargo dep 没有 cycle，再开始实现。

**Files**：
- modify: `Cargo.toml`（workspace.members）
- create: `crates/wjsm-runtime-snapshot/Cargo.toml`
- create: `crates/wjsm-runtime-snapshot/src/lib.rs`
- create: `crates/wjsm-runtime-snapshot/build.rs`
- create: `crates/wjsm-runtime-support/Cargo.toml`
- create: `crates/wjsm-runtime-support/src/lib.rs`
- create: `crates/wjsm-runtime-support/build.rs`

**Steps**：

- [ ] 在 `Cargo.toml` workspace.members 末尾追加：
  ```toml
      "crates/wjsm-runtime-snapshot",
      "crates/wjsm-runtime-support",
  ```
- [ ] 创建 `crates/wjsm-runtime-snapshot/Cargo.toml`：
  ```toml
  [package]
  name = "wjsm-runtime-snapshot"
  version.workspace = true
  edition.workspace = true

  [features]
  default = ["embedded"]
  embedded = []

  [dependencies]

  [build-dependencies]
  anyhow = { workspace = true }
  ```
  暂不依赖任何 wjsm-* crate；P1.1 才补 build-deps。
- [ ] 创建 `crates/wjsm-runtime-snapshot/src/lib.rs`：
  ```rust
  //! Build-time embedded startup snapshot bytes.
  //!
  //! See docs/adr/0004-build-time-embedded-runtime.md.

  #[cfg(feature = "embedded")]
  pub static EMBEDDED_STARTUP_SNAPSHOT: Option<&[u8]> = Some(include_bytes!(concat!(
      env!("OUT_DIR"),
      "/wjsm_startup_snapshot.bin"
  )));

  #[cfg(not(feature = "embedded"))]
  pub static EMBEDDED_STARTUP_SNAPSHOT: Option<&[u8]> = None;
  ```
- [ ] 创建 `crates/wjsm-runtime-snapshot/build.rs` 占位：
  ```rust
  fn main() {
      let out_dir = std::env::var_os("OUT_DIR").expect("OUT_DIR not set");
      let path = std::path::PathBuf::from(out_dir).join("wjsm_startup_snapshot.bin");
      // P0 占位：写空字节，P1.1 真正生成。embedded feature 关闭时 lib.rs 不会读它，所以空字节不影响。
      if !path.exists() {
          std::fs::write(&path, b"").expect("write placeholder snapshot");
      }
  }
  ```
- [ ] 创建 `crates/wjsm-runtime-support/Cargo.toml`：
  ```toml
  [package]
  name = "wjsm-runtime-support"
  version.workspace = true
  edition.workspace = true

  [features]
  default = ["embedded"]
  embedded = []

  [dependencies]

  [build-dependencies]
  anyhow = { workspace = true }
  ```
- [ ] 创建 `crates/wjsm-runtime-support/src/lib.rs`：
  ```rust
  //! Build-time embedded shared support module (precompiled wasmtime artifact).

  #[cfg(feature = "embedded")]
  pub static EMBEDDED_SUPPORT_CWASM: Option<&[u8]> = Some(include_bytes!(concat!(
      env!("OUT_DIR"),
      "/wjsm_support.cwasm"
  )));

  #[cfg(not(feature = "embedded"))]
  pub static EMBEDDED_SUPPORT_CWASM: Option<&[u8]> = None;
  ```
- [ ] 创建 `crates/wjsm-runtime-support/build.rs` 占位（结构同 snapshot）：
  ```rust
  fn main() {
      let out_dir = std::env::var_os("OUT_DIR").expect("OUT_DIR not set");
      let path = std::path::PathBuf::from(out_dir).join("wjsm_support.cwasm");
      if !path.exists() {
          std::fs::write(&path, b"").expect("write placeholder cwasm");
      }
  }
  ```
- [ ] 验证：
  ```bash
  cargo build -p wjsm-runtime-snapshot -p wjsm-runtime-support
  cargo nextest run --workspace
  ```
  期望：两个新 crate 编译通过；既有测试不变。
- [ ] 提交：`feat(workspace): add wjsm-runtime-snapshot and wjsm-runtime-support skeletons`

---

# P1：Embedded startup snapshot

**Why**：让 first-run 不再付 cold bootstrap。snapshot 字节在构建期生成、`include_bytes!` 进二进制；运行时优先用 embedded，runtime cache 退化为 opt-out fallback。

## P1.0 抽 snapshot 公共 lib：`wjsm-snapshot-format` crate

**Why**：`build.rs` 不能直接依赖 `wjsm-runtime`（会触发 cargo build dep cycle）。把纯字节格式 + abi_hash 输入抽出来，放到 wjsm-runtime 与 wjsm-runtime-snapshot 都能正常依赖的中立 crate 里。capture/restore 仍留在 wjsm-runtime（它需要 `RuntimeState`）。

**Files**：
- create: `crates/wjsm-snapshot-format/Cargo.toml`
- create: `crates/wjsm-snapshot-format/src/lib.rs`（迁移自 `crates/wjsm-runtime/src/startup_snapshot_format.rs`）
- modify: `Cargo.toml`（workspace.members 加 `crates/wjsm-snapshot-format`）
- modify: `crates/wjsm-runtime/Cargo.toml`（加 `wjsm-snapshot-format = { path = "..." }`）
- modify: `crates/wjsm-runtime/src/startup_snapshot_format.rs` → 删除，全部 re-export 自新 crate
- modify: `crates/wjsm-runtime/src/{lib.rs, startup_snapshot.rs, startup_snapshot_cache.rs}`：所有 `startup_snapshot_format::*` 导入改为 `wjsm_snapshot_format::*`

**Steps**：

- [ ] 创建 `crates/wjsm-snapshot-format/Cargo.toml`：
  ```toml
  [package]
  name = "wjsm-snapshot-format"
  version.workspace = true
  edition.workspace = true

  [dependencies]
  anyhow = { workspace = true }
  ```
- [ ] 把 `crates/wjsm-runtime/src/startup_snapshot_format.rs` 整文件 `git mv` 到 `crates/wjsm-snapshot-format/src/lib.rs`，调整 `pub(crate)` 为 `pub`：
  ```rust
  // 原 pub(crate) struct StartupSnapshotHeader -> pub
  // 原 pub(crate) struct StartupSnapshotOwned -> pub
  // 原 pub(crate) struct StartupSnapshotView<'a> -> pub
  // 原 pub(crate) enum SnapshotNativeCallable -> pub
  // 原 pub(crate) fn encode_snapshot / decode_snapshot / abi_hash -> pub
  ```
- [ ] 在 `crates/wjsm-runtime/src/lib.rs` 删除 `mod startup_snapshot_format;`，改为 `use wjsm_snapshot_format as startup_snapshot_format;` 顶层别名（保持现有路径调用最少改动）。
- [ ] 在 `crates/wjsm-runtime/Cargo.toml` 加：
  ```toml
  wjsm-snapshot-format = { path = "../wjsm-snapshot-format" }
  ```
- [ ] 验证：
  ```bash
  cargo nextest run -p wjsm-runtime -E 'test(startup_snapshot)'
  cargo nextest run --workspace
  ```
  全绿。
- [ ] 提交：`refactor(snapshot): extract format/abi_hash into wjsm-snapshot-format crate`

## P1.1 build-time 生成 snapshot 字节

**Why**：snapshot 必须在 cargo build 期就生成，使 first-run 直接 restore。但 capture 流程依赖 `wjsm-runtime` 的 `RuntimeState` + 全套 host imports，又不能在 wjsm-runtime-snapshot 的 `[build-dependencies]` 里反向依赖 wjsm-runtime（会引发 cycle）。结论：在 `wjsm-runtime` 自身公开一个**纯字节产出**入口 `pub fn build_embedded_startup_snapshot_bytes() -> Result<Vec<u8>>`，由 wjsm-runtime-snapshot/build.rs 调用——build.rs 用 `[build-dependencies]` 引 wjsm-runtime，cargo 把它当外部依赖编译，不构成 cycle（因为 wjsm-runtime-snapshot 自身的 `[dependencies]` **不**反向依赖 wjsm-runtime）。

**Contract**：
- wjsm-runtime 暴露：
  ```rust
  pub fn build_embedded_startup_snapshot_bytes() -> Result<Vec<u8>>;
  ```
  实现：编译空 JS 源 → wasm bytes → 内部 instantiate + run host post-bootstrap + run `__wjsm_bootstrap_once` → `capture_startup_snapshot` → `encode_snapshot` → 返回 bytes。
- wjsm-runtime-snapshot/build.rs 调用并写到 `OUT_DIR/wjsm_startup_snapshot.bin`。
- wjsm-runtime-snapshot 不在 `[dependencies]` 引 wjsm-runtime；只在 `[build-dependencies]` 引。
- 字节内容 ABI hash 校验在 P1.2 install 路径强制。

**Files**：
- modify: `crates/wjsm-runtime/src/lib.rs`（新增 `build_embedded_startup_snapshot_bytes`，调用 `instantiate_execute_bundle` 单 instance + `run_startup_cold_path` 一次 + `capture_startup_snapshot` + `encode_snapshot`）
- modify: `crates/wjsm-runtime/Cargo.toml`：把 `wjsm-parser` / `wjsm-semantic` / `wjsm-backend-wasm` 从 `[dependencies]` 复制一份到 `[dev-dependencies]`（已是 deps，无需重复）。**确认**当前 wjsm-runtime 的 `[dependencies]` 已含 parser/semantic/backend，可直接复用；不必动 Cargo.toml
- modify: `crates/wjsm-runtime-snapshot/Cargo.toml`：
  ```toml
  [build-dependencies]
  anyhow = { workspace = true }
  wjsm-runtime = { path = "../wjsm-runtime" }
  ```
- modify: `crates/wjsm-runtime-snapshot/build.rs`：
  ```rust
  fn main() {
      let out_dir = std::env::var_os("OUT_DIR").expect("OUT_DIR not set");
      let path = std::path::PathBuf::from(out_dir).join("wjsm_startup_snapshot.bin");
      let bytes = wjsm_runtime::build_embedded_startup_snapshot_bytes()
          .expect("generate embedded startup snapshot");
      std::fs::write(&path, &bytes).expect("write embedded snapshot");
      println!("cargo:rerun-if-changed=build.rs");
      // Snapshot bytes 由 abi_hash 输入决定；以下覆盖所有可能的 ABI 输入触发器：
      println!("cargo:rerun-if-changed=../wjsm-runtime/src");
      println!("cargo:rerun-if-changed=../wjsm-backend-wasm/src");
      println!("cargo:rerun-if-changed=../wjsm-snapshot-format/src");
  }
  ```

**Steps**：

- [ ] 在 `crates/wjsm-runtime/src/lib.rs` 新增公开函数 `build_embedded_startup_snapshot_bytes`，使用现有 `compile_source`（如果存在；否则在内联实现：parse → lower → compile）+ 现有 `instantiate_execute_bundle` + `run_startup_cold_path`（已经会 capture + cache，但这里要直接拿 bytes，不进 disk cache）。改造 cold path 抽出 `capture_and_encode(store, wasm_env) -> Vec<u8>` 共用 helper。
- [ ] 在 `wjsm-runtime/tests/embedded_snapshot_build.rs` 加单测：
  ```rust
  #[test]
  fn build_embedded_startup_snapshot_bytes_returns_valid_view() {
      let bytes = wjsm_runtime::build_embedded_startup_snapshot_bytes().unwrap();
      let view = wjsm_snapshot_format::decode_snapshot(&bytes).unwrap();
      assert_eq!(view.header.abi_hash, wjsm_snapshot_format::abi_hash());
  }
  ```
- [ ] 跑 RED：测试当前应通过编译失败（函数尚未实现）。
- [ ] 实现 `build_embedded_startup_snapshot_bytes`。
- [ ] 跑 GREEN：`cargo nextest run -p wjsm-runtime -E 'test(build_embedded_startup_snapshot_bytes)'`
- [ ] 修改 `crates/wjsm-runtime-snapshot/Cargo.toml` + `build.rs` 如上。
- [ ] 验证 OUT_DIR 中 snapshot.bin 真实存在：
  ```bash
  cargo build -p wjsm-runtime-snapshot
  find target -name 'wjsm_startup_snapshot.bin' -exec ls -l {} +
  ```
  字节大小 > 100。
- [ ] 提交：`feat(snapshot): generate embedded startup snapshot bytes at build time`

## P1.2 install_embedded_snapshot 入口 + cache 优先级

**Why**：让 wjsm-runtime 在执行路径上优先用 embedded bytes，runtime cache 仅当未安装或 ABI 不匹配时启用。

**Files**：
- modify: `crates/wjsm-runtime/src/lib.rs`：
  - 新增 `static EMBEDDED_STARTUP_SNAPSHOT: OnceLock<&'static [u8]> = OnceLock::new();`
  - 新增公开 `pub fn install_embedded_startup_snapshot(bytes: &'static [u8])`
  - 修改 `execute_with_writer_shared_inner` 中 `snapshot_bytes` 的取值顺序：先 embedded → 然后 runtime cache（既有 `startup_snapshot_cache::get_cached`）
  - embedded ABI 不匹配时 `eprintln!`（debug feature gated），fallback 到 runtime cache 路径
- modify: `crates/wjsm-runtime/tests/startup_snapshot.rs`：增加 `embedded_snapshot_install_then_cache_lookup_returns_embedded` 单测

**Steps**：

- [ ] 加 `install_embedded_startup_snapshot`：
  ```rust
  static EMBEDDED_STARTUP_SNAPSHOT: OnceLock<&'static [u8]> = OnceLock::new();
  pub fn install_embedded_startup_snapshot(bytes: &'static [u8]) {
      let _ = EMBEDDED_STARTUP_SNAPSHOT.set(bytes);
  }
  fn embedded_startup_snapshot_view() -> Option<&'static [u8]> {
      let bytes = (*EMBEDDED_STARTUP_SNAPSHOT.get()?).clone();
      let view = wjsm_snapshot_format::decode_snapshot(bytes).ok()?;
      if view.header.abi_hash != wjsm_snapshot_format::abi_hash() {
          if startup_snapshot_debug_enabled() {
              eprintln!("embedded snapshot abi hash mismatch; falling back to runtime cache/cold");
          }
          return None;
      }
      Some(bytes)
  }
  ```
- [ ] 修改 `execute_with_writer_shared_inner`：
  ```rust
  let snapshot_bytes = if startup_snapshot_enabled() {
      match embedded_startup_snapshot_view() {
          Some(bytes) => Some(Arc::from(bytes)),
          None => startup_snapshot_cache::get_cached().await,
      }
  } else {
      None
  };
  ```
  注意：`Arc::from(&'static [u8])` 不直接可用，需要 `Arc<[u8]>::from(bytes.to_vec().into_boxed_slice())` 或者改 `try_restore_snapshot` 接受 `&[u8]` 视图。建议：把 `try_restore_snapshot` 改为接受 `&[u8]`，避免拷贝。
- [ ] 加单测：
  ```rust
  #[test]
  fn embedded_snapshot_install_makes_first_run_skip_cold() {
      let bytes = wjsm_runtime::build_embedded_startup_snapshot_bytes().unwrap();
      // Leak for &'static
      let leaked: &'static [u8] = Box::leak(bytes.into_boxed_slice());
      wjsm_runtime::install_embedded_startup_snapshot(leaked);
      // 不预 cache，run 一次后 cache 中应**仍**为空（embedded 路径不写 disk cache）
      let _ = run("console.log(1)").unwrap();
      // 后续断言略；具体在测试中实现
  }
  ```
- [ ] 跑 RED → 实现 → GREEN：
  ```bash
  cargo nextest run -p wjsm-runtime -E 'test(embedded_snapshot)'
  ```
- [ ] 提交：`feat(runtime): install_embedded_startup_snapshot + cache priority`

## P1.3 wjsm-cli 启动时 install

**Files**：
- modify: `crates/wjsm-cli/Cargo.toml`：加
  ```toml
  wjsm-runtime-snapshot = { path = "../wjsm-runtime-snapshot" }
  ```
- modify: `crates/wjsm-cli/src/lib.rs`：在 `main_entry` 顶部插入：
  ```rust
  if let Some(bytes) = wjsm_runtime_snapshot::EMBEDDED_STARTUP_SNAPSHOT {
      wjsm_runtime::install_embedded_startup_snapshot(bytes);
  }
  ```

**Steps**：
- [ ] 加 cargo dep + install 调用。
- [ ] 验证：
  ```bash
  cargo run -- eval "console.log('embedded ok')"
  ```
  输出 `embedded ok`。
- [ ] 提交：`feat(cli): install embedded startup snapshot at startup`

## P1.4 P1 收尾验证 + bench

**Steps**：

- [ ] 跑：
  ```bash
  cargo nextest run -p wjsm-runtime -E 'test(startup_snapshot)'
  cargo nextest run --workspace
  cargo test -p wjsm-runtime --release --lib --no-run
  target/release/deps/wjsm_runtime-*[0-9a-f] bench_execute_phases --ignored --nocapture
  ```
- [ ] 在 bench 输出中确认 first-run（无任何 disk cache、无 process memory cache）已经走 embedded restore，时间 ≈ runtime warm restore（~15µs），而不是 cold bootstrap（~25µs）。
- [ ] 在 `docs/aegis/work/2026-06-19-build-time-embedded-runtime/` 新建工作目录，记录 bench 数据到 `90-evidence.md`。
- [ ] 提交：`docs(work): record P1 verification evidence`

---

# P2：Runtime support module（共享 helper wasm）

**Why**：用户 wasm 当前每个模块内联了所有 helper bodies（compiler_helpers.rs 1538 行），这是 wasmtime compile 时间（实测 release ~3ms）的主因。把 helpers 提到一份共享、build-time 预编译的 `support.cwasm` 里，每次 `Module::deserialize` 跳过 wasmtime compile，用户 wasm 体积下降。

**P2 ABI 关键约束**（在 P2.0 设计任务中固化）：
- 用户 wasm 与 support module 共享 `env.memory`、`env.__table`、`env.<global...>` 一组导入
- runtime 在 instantiate 之前用 wasmtime API 创建 memory/table/globals，通过 Linker `define` 给两个 instance
- support module 的 element section 在 table 起始 `[0..K)` 区间登记 helper 函数
- 用户 wasm 的 element section 在 `[K..K+M)` 区间登记其用户函数；K 是 backend codegen-time 常量（来自 `wjsm-runtime-support/src/abi.rs::SUPPORT_TABLE_RESERVED_LEN`）
- 所有 helper export 名字以 `wjsm_support_` 开头，用户 wasm 通过 `import "wjsm_support" "obj_new"` 等引用

## P2.0 设计 support module ABI

**Files**：
- create: `crates/wjsm-runtime-support/src/abi.rs`：常量 + ABI hash 输入

**Steps**：

- [ ] 在 `abi.rs` 写：
  ```rust
  pub const SUPPORT_MODULE_NAME: &str = "wjsm_support";
  pub const SUPPORT_VERSION: u32 = 1;
  pub const SUPPORT_TABLE_RESERVED_LEN: u32 = /* 由 helper 数 + Array.prototype 表长 + 4 富裕度 */ 64;
  // 列出所有 imported globals 与它们的 ValType + Mutability
  pub const ENV_GLOBALS: &[EnvGlobal] = &[
      EnvGlobal { name: "__shadow_sp", ty: ValTy::I32, mutable: true },
      EnvGlobal { name: "__heap_ptr", ty: ValTy::I32, mutable: true },
      EnvGlobal { name: "__obj_table_ptr", ty: ValTy::I32, mutable: true },
      EnvGlobal { name: "__obj_table_count", ty: ValTy::I32, mutable: true },
      EnvGlobal { name: "__object_proto_handle", ty: ValTy::I64, mutable: true },
      EnvGlobal { name: "__array_proto_handle", ty: ValTy::I64, mutable: true },
      EnvGlobal { name: "__object_heap_start", ty: ValTy::I32, mutable: true },
      EnvGlobal { name: "__bootstrap_done", ty: ValTy::I32, mutable: true },
      EnvGlobal { name: "__function_props_done", ty: ValTy::I32, mutable: true },
      EnvGlobal { name: "__function_props_base", ty: ValTy::I32, mutable: true },
      EnvGlobal { name: "__num_ir_functions", ty: ValTy::I32, mutable: true },
      EnvGlobal { name: "__arr_proto_table_base", ty: ValTy::I32, mutable: true },
      EnvGlobal { name: "__arr_proto_table_len", ty: ValTy::I32, mutable: true },
      EnvGlobal { name: "__arr_proto_table_hash", ty: ValTy::I64, mutable: true },
  ];
  pub const SUPPORT_EXPORTS: &[&str] = &[
      "obj_new", "obj_get", "obj_set", "obj_delete",
      "arr_new", "elem_get", "elem_set",
      "string_eq", "to_int32", "get_proto_from_ctor",
      "wjsm_bootstrap_once", "wjsm_init_function_props",
  ];
  pub fn support_module_layout_hash() -> u64 { /* hash of all const above */ }
  ```
- [ ] 单测 `support_module_layout_hash_is_stable`：把 hash 写死，任何字段变化导致测试失败 → 强制 ABI 升级。
- [ ] 把 `support_module_layout_hash` 加入 `wjsm-snapshot-format::abi_hash` 输入：
  - `wjsm-snapshot-format` 暴露 `pub const fn support_module_layout_hash_input(...) -> u64`，由 wjsm-runtime 在 abi_hash 计算时合并；或者直接让 abi_hash 接受外部 u64 参数
  - 实现细节：让 abi_hash 通过 `OnceLock<u64>` 接受 `register_external_input(u64)` 一次性合并
- [ ] 提交：`feat(support): support module ABI constants and layout hash`

## P2.1 build.rs：生成 support.wasm + support.cwasm

**Why**：把当前 backend 的 helper codegen 复用，仅产出"helper-only"的 wasm 模块；再用 wasmtime 预编译。

**Files**：
- create: `crates/wjsm-runtime-support/src/codegen.rs`：调用 wjsm-backend-wasm 的 helper emit API（需要 backend 暴露一个 `pub fn emit_support_module(abi: &SupportModuleAbi) -> Result<Vec<u8>>`）
- modify: `crates/wjsm-backend-wasm/src/lib.rs`：暴露 `pub fn emit_support_module(...)`，内部走和现有 user-module 几乎一致的 helper emit 路径，但：
  - 不写 user functions
  - 不写 `main`/`__eval_entry`
  - import memory/table/globals from "env"
  - export helpers as 上述名字
  - reserve table[0..SUPPORT_TABLE_RESERVED_LEN] 给 helpers 自己的间接调用
- modify: `crates/wjsm-runtime-support/Cargo.toml`：
  ```toml
  [build-dependencies]
  anyhow = { workspace = true }
  wjsm-backend-wasm = { path = "../wjsm-backend-wasm" }
  wasmtime = { workspace = true }
  ```
- modify: `crates/wjsm-runtime-support/build.rs`：
  ```rust
  fn main() -> anyhow::Result<()> {
      let out_dir = std::env::var_os("OUT_DIR").expect("OUT_DIR");
      let support_wasm = wjsm_backend_wasm::emit_support_module()?;

      let support_wasm_path = std::path::PathBuf::from(&out_dir).join("wjsm_support.wasm");
      std::fs::write(&support_wasm_path, &support_wasm)?;

      let cwasm_path = std::path::PathBuf::from(&out_dir).join("wjsm_support.cwasm");
      let mut cfg = wasmtime::Config::new();
      cfg.async_support(true);
      let engine = wasmtime::Engine::new(&cfg)?;
      let cwasm_bytes = engine.precompile_module(&support_wasm)?;
      std::fs::write(&cwasm_path, &cwasm_bytes)?;

      println!("cargo:rerun-if-changed=build.rs");
      println!("cargo:rerun-if-changed=../wjsm-backend-wasm/src");
      println!("cargo:rerun-if-changed=src/abi.rs");
      Ok(())
  }
  ```

**Steps**：

- [ ] 在 `wjsm-backend-wasm` 加 `emit_support_module()`：基于现有 `Compiler` 类型，新增 `CompileMode::SupportModule`，复用 `compile_object_helpers` / `compile_array_helpers` / `compile_*_proto_wrappers` / `compile_bootstrap_once_function` / `compile_init_function_props_function`，但跳过 user function 编译。Memory/globals/table 改为 import。
- [ ] 单测 `wjsm-backend-wasm/tests/support_module_emit.rs`：
  ```rust
  #[test]
  fn emit_support_module_produces_helper_exports() {
      let bytes = wjsm_backend_wasm::emit_support_module().unwrap();
      let parser = wasmparser::Parser::new(0);
      let exports = collect_exports(parser, &bytes);
      for name in wjsm_runtime_support::abi::SUPPORT_EXPORTS {
          assert!(exports.contains(*name), "missing export: {name}");
      }
  }
  ```
- [ ] 跑 RED → 实现 → GREEN
- [ ] 验证 OUT_DIR 真实产 cwasm：
  ```bash
  cargo build -p wjsm-runtime-support
  find target -name 'wjsm_support.cwasm' -exec ls -l {} +
  ```
- [ ] 提交：`feat(support): emit + precompile shared support module`

## P2.2 runtime instantiate 路径：共享 memory/table/globals + support 双 instance

**Why**：让 user wasm 与 support module 共用同一个线性内存与函数表。

**Files**：
- modify: `crates/wjsm-runtime/src/lib.rs`：
  - 新增 `pub fn install_embedded_support(cwasm: &'static [u8])`
  - 新增 `instantiate_with_support(engine, user_module, store) -> ExecuteInstanceBundle`：
    1. `let memory = wasmtime::Memory::new(&mut store, MemoryType::new(1, None))?;`
    2. 创建所有 env globals（用 `Global::new` + 初值）+ table（`Table::new`）
    3. `Linker.define(&mut store, "env", "memory", memory)?;` 等
    4. 反序列化 support cwasm：`let support_module = unsafe { Module::deserialize(&engine, cwasm) }?;`
    5. instantiate support：`let support_instance = linker.instantiate_async(&mut store, &support_module).await?;`
    6. 把 support_instance 的 exports 通过 linker 注册到 `wjsm_support` namespace
    7. instantiate user：`let user_instance = linker.instantiate_async(&mut store, user_module).await?;`
- modify: `crates/wjsm-runtime/src/wasm_env.rs`：`WasmEnv` 字段从 `instance.get_export` 改为持有 runtime 创建的 memory/globals/table（不再依赖 user instance 导出）

**Steps**：

- [ ] 加 `install_embedded_support` + `OnceLock<&'static [u8]>`。
- [ ] 重构 `instantiate_execute_bundle`：
  - 当 support cwasm 已安装且 user wasm 是新 ABI（imports memory），走新路径
  - 否则走旧路径（兼容期）
- [ ] 单测：仅 support module 自己 instantiate 无错（用 P2.1 的 `wjsm-runtime/tests/support_instantiate.rs`）。
- [ ] 单测：手写最小 user wasm（仅 import env + wjsm_support，body 调一次 obj_new）→ 双 instance 链接 + 调用成功。
- [ ] 提交：`feat(runtime): instantiate user module with shared support module`

## P2.3 切换 object helpers（obj_new/obj_get/obj_set/obj_delete）

**Why**：第一批切换。这些 helper 内部互相调用且与 host imports 紧密绑定，是最大的"hot path"。

**Files**：
- modify: `crates/wjsm-backend-wasm/src/compiler_module.rs`：
  - `obj_new_func_idx` 等 4 个原本走 `self.functions.function(...)` 的位置，改为：在 imports 段加 `import "wjsm_support" "obj_new"`，把 idx 记成 import 索引
- modify: `crates/wjsm-backend-wasm/src/compiler_helpers.rs`：删除 `compile_object_helpers` 中 obj_new/obj_get/obj_set/obj_delete 的内联生成代码
- modify: `crates/wjsm-runtime-support/build.rs` / `wjsm-backend-wasm::emit_support_module`：support module 这一侧仍保留这 4 个 helper 的 emit

**Steps**：

- [ ] backend：用 cargo feature flag 临时控制：`wjsm-backend-wasm` 加 feature `support-module-imports`（default-on），开启则 helpers 走 import；关闭则走旧 inline。这样可以分批切换，每批切完跑测试。
- [ ] 改 compiler_module.rs：
  ```rust
  fn import_or_emit_helper(&mut self, name: &str, type_idx: u32) -> u32 {
      if cfg!(feature = "support-module-imports") {
          // 在 imports 段追加；返回 import 索引
      } else {
          // 旧 self.functions.function(type_idx); 路径
      }
  }
  ```
- [ ] 跑 happy/errors fixture 子集：
  ```bash
  cargo nextest run -E 'test(happy__obj) or test(happy__class) or test(errors__obj)'
  ```
  全绿才能 commit。
- [ ] 提交：`refactor(backend): import obj_* helpers from support module`

## P2.4 切换 array/elem helpers（arr_new/elem_get/elem_set）

同 P2.3 的模式：
- [ ] backend 改 import；删除 inline
- [ ] 跑 array 相关 fixture：`cargo nextest run -E 'test(happy__array) or test(errors__array)'`
- [ ] 提交：`refactor(backend): import array/elem helpers from support module`

## P2.5 切换 utility helpers（string_eq/to_int32/get_proto_from_ctor）

- [ ] 同模式
- [ ] 跑 string/typeof/proto fixture
- [ ] 提交：`refactor(backend): import utility helpers from support module`

## P2.6 切换 bootstrap 阶段函数（__wjsm_bootstrap_once / __wjsm_init_function_props）

**Why**：这两个原本是 user wasm 自己 export 的 startup 阶段。改成 user wasm 从 wjsm_support import；user `main` 入口的 `emit_startup_phase_call` 改成调 import 的 helper。

**注意**：`__wjsm_init_function_props` 内部需要 `function_param_counts` / `function_names` 等 user-specific 数据。这些必须通过参数传给 helper，或者通过 user module 的内联子函数代理。

**设计选择**：
- 选 A：`wjsm_init_function_props` helper 接受 `(num_funcs: i32, name_table_ptr: i32, param_count_ptr: i32)`，user wasm 在 data segment 里布局 name/param 表，传入 helper
- 选 B：`__wjsm_init_function_props` 仍在 user module 生成（很短），只 `__wjsm_bootstrap_once` 走 support
- 选 A 让 user wasm 进一步缩小，但要重构 init 逻辑访问外部表
- 选 B 简单但少省一点

P2.6 的 plan 选 **A**，更彻底。

- [ ] support module export `wjsm_init_function_props(num_funcs, names_ptr, param_counts_ptr) -> i64`
- [ ] backend 在 user module data segment 写入 name/param 表，user main 调用 helper 时传入指针
- [ ] 跑 startup_snapshot + happy fixture
- [ ] 提交：`refactor(backend): import bootstrap stages from support module`

## P2.7 重新 bake P1 snapshot（ABI 已变）

- [ ] `cargo clean -p wjsm-runtime-snapshot`
- [ ] `cargo build -p wjsm-runtime-snapshot`：build.rs 应自动重生成 snapshot
- [ ] 跑 `cargo nextest run -p wjsm-runtime -E 'test(startup_snapshot)'`：embedded snapshot ABI hash 与运行时一致
- [ ] 跑 workspace 全测
- [ ] 提交：`chore(snapshot): rebake embedded snapshot after support module ABI change`

## P2.8 删除旧 inline helper 路径 + bench

- [ ] 删除 `wjsm-backend-wasm` 的 `support-module-imports` feature flag（默认开启即唯一路径）
- [ ] 删除 `compiler_helpers.rs` 中所有已迁移 helpers 的 inline 生成代码
- [ ] 删除 `wasm_env.rs` 中"从 user instance 取 memory"的兜底（统一从 runtime 创建的 env shared 取）
- [ ] 跑 bench：
  ```bash
  cargo test -p wjsm-runtime --release --lib --no-run
  target/release/deps/wjsm_runtime-*[0-9a-f] bench_execute_phases --ignored --nocapture
  ```
  期望 `BENCH module_only` ≤ 旧值的 60%
- [ ] 把 bench 数据写入 `docs/aegis/work/2026-06-19-build-time-embedded-runtime/90-evidence.md`
- [ ] 提交：`refactor(backend): remove inline helper codegen, support module is sole owner`

---

# P3：Builtin JS 扩展框架

**Why**：内部 API（未来想用 JS 而不是 Rust 实现的 Web/Node 兼容 API）的承载位置；snapshot 期评估 → 结果固化进 embedded snapshot。本阶段只做框架 + 占位文件，不引入任何具体 API 实现。

## P3.0 框架：builtin_js 目录 + manifest

**Files**：
- create: `crates/wjsm-runtime/builtin_js/manifest.rs`：列出 ordered builtin JS files
  ```rust
  // build.rs 与 capture 路径都 include!("./manifest.rs")
  pub static BUILTIN_JS_FILES: &[(&str, &str)] = &[
      // ("wjsm_internal/url.js", include_str!("url.js")),
      // 当前为空，留待具体 API
  ];
  ```
- modify: `crates/wjsm-runtime/src/lib.rs::build_embedded_startup_snapshot_bytes`：在 capture 之前依次 `eval` 每个 builtin JS 文件
- modify: `crates/wjsm-snapshot-format/src/lib.rs::abi_hash`：输入加 builtin js bundle SHA-256

**Steps**：

- [ ] 加 `crates/wjsm-runtime/builtin_js/manifest.rs`（空列表）
- [ ] 在 `build_embedded_startup_snapshot_bytes` 流程：
  1. 编译空 user JS → wasm
  2. instantiate
  3. 跑 host post-bootstrap + `__wjsm_bootstrap_once`
  4. **新增**：依次 `eval(BUILTIN_JS_FILES[i].1)`（用现有 `runtime_eval` 路径）
  5. capture
- [ ] ABI hash 输入：`SHA256(concat sorted by path of all BUILTIN_JS_FILES contents)`
- [ ] 单测：
  ```rust
  #[test]
  fn empty_builtin_js_bundle_does_not_crash() {
      let bytes = wjsm_runtime::build_embedded_startup_snapshot_bytes().unwrap();
      assert!(bytes.len() > 0);
  }
  ```
- [ ] 提交：`feat(runtime): builtin JS extension framework with empty manifest`

## P3.1 端到端 sentinel 验证

**Why**：证明 builtin JS 注入的 globals 真的进入 embedded snapshot 并对用户代码可见。

**Files**：
- modify: `crates/wjsm-runtime/builtin_js/manifest.rs`：临时加 `("__wjsm_builtin_sentinel.js", "globalThis.__wjsm_builtin_sentinel = 'ok';")`
- create: `fixtures/happy/builtin_js_sentinel.js`：`console.log(globalThis.__wjsm_builtin_sentinel);`
- create: `fixtures/happy/builtin_js_sentinel.expected`：`ok\n`

**Steps**：

- [ ] 加 sentinel + fixture
- [ ] 跑：
  ```bash
  cargo nextest run -E 'test(happy__builtin_js_sentinel)'
  ```
- [ ] 看到通过后，**删除 sentinel**（保留空 manifest 框架）
- [ ] 跑全 fixture：`cargo nextest run -E 'test(happy__) or test(modules__)'`
- [ ] 提交：`test(builtin_js): end-to-end sentinel verifies framework`，再 `revert: remove sentinel after verification`

---

# P4：收尾

## P4.0 文档

**Files**：
- create: `docs/adr/0004-build-time-embedded-runtime.md`
- modify: `docs/adr/0003-startup-snapshot-boundary.md`：在 Status 行后追加 `Superseded by ADR 0004`
- modify: `AGENTS.md`：更新 Startup snapshot / WASM contract / Function-property handle layout 段，反映新 import-based 形态
- modify: `docs/aegis/INDEX.md`：appendix 新计划与 ADR

ADR 0004 内容大纲：
- Context：三类 ship-time-stable 制品当前散落在 runtime first-run 路径
- Decision：build-time 固化为 embedded snapshot + precompiled support cwasm + builtin JS bundle；ABI hash 统一管理
- Consequences：
  - Positive：first-run 不付 cold；wasmtime compile 时间下降；用户 wasm 体积下降；builtin API JS 实现成为低成本路径
  - Negative：三 crate workspace、build.rs 触发链；任何 ABI 输入修改都触发 snapshot+support 重生
  - Risks：embedded ABI mismatch 必须 ABS 校验；fallback 路径必须保留 feature gate

## P4.1 全工作区验证

```bash
cargo fmt --all
cargo nextest run --workspace
cargo nextest run -p wjsm-runtime -E 'test(startup_snapshot) or test(async_scheduler) or test(async_reentry)'
cargo nextest run -E 'test(happy__) or test(modules__) or test(semantic__)'
cargo test -p wjsm-runtime --release --lib --no-run \
  && target/release/deps/wjsm_runtime-*[0-9a-f] bench_execute_phases --ignored --nocapture
```

预期 bench：
- `BENCH full execute off`（embedded 关闭）：基本不变
- `BENCH full execute on warm`（embedded snapshot 命中 + support cwasm deserialized）：≤ P1 之前 warm 路径 50%
- `BENCH module_only`：≤ 旧值 60%

把数据写到 `docs/aegis/work/2026-06-19-build-time-embedded-runtime/90-evidence.md`。

## P4.2 提测

- [ ] 提测 commit：`docs(adr): 0004 build-time embedded runtime + supersede 0003`
- [ ] 验收：`cargo nextest run --workspace` + bench 三段证据 + ADR 落地
- [ ] 完成

---

## 风险与回退

| 风险 | 触发 | 回退 |
|---|---|---|
| support cwasm wasmtime 版本/feature 配置不匹配 | 升级 wasmtime 后未重 bake | runtime 检测到 deserialize 失败时 fallback 到运行时 capture（feature 关闭路径） |
| embedded ABI hash mismatch 但 install 仍发生 | builder 与 runtime 不同步构建 | install 路径 abi_hash 校验失败 → 静默丢弃，走 runtime cache/cold |
| build.rs 在 docker 等无 wasmtime native deps 环境失败 | CI 环境差异 | feature `embedded` 关闭即可降级；CI 单独跑一遍 `--no-default-features` |
| builtin JS 引入 timer/promise 等动态状态 | 错误的 builtin JS 实现 | capture 期断言（已有）必须命中；测试覆盖 |
| 用户在 wjsm-runtime 作为库使用时未 install embedded | 库使用者忘记调用 install | 默认 fallback 到 runtime cache；不报错 |
| 旧 export memory contract 残留代码导致回归 | P4 删除不彻底 | grep 检查 `__heap_ptr` / `obj_table_ptr` 等导出引用，确保仅 import |

## Retirement

| 旧 owner | 退役动作 | 触发条件 |
|---|---|---|
| `compiler_helpers.rs` 内联 obj_*/arr_*/elem_* helper bodies | 删除（保留 support module 唯一 owner） | P2.8 |
| user wasm `export "memory"`/`export "__table"` contract | 改为 import；删除导出代码 | P2.2 |
| user wasm 内嵌 `__wjsm_bootstrap_once` / `__wjsm_init_function_props` 定义 | 改为 import support exports | P2.6 |
| runtime first-run capture-on-demand 默认路径 | 仅当 install 未发生或 ABI mismatch 时启用 | P1.4 |
| ADR 0003 顶层 status | 标 superseded-by-0004 | P4.0 |
