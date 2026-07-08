# Node 高级内置模块实现计划（issue #311）

Goal: 实现 issue #311 的依赖无关交付面：Node `stream`、`http/https` 客户端、`zlib`、`child_process`、package scripts 与基础 `wjsm install`。

Architecture: 采用 JS builtin + Rust host bridge。`wjsm-module` 注册 builtin；`wjsm-runtime` 暴露 `__wjsm_node_zlib` / `__wjsm_node_child_process` 并同步 `NativeCallable` snapshot ABI；`wjsm-cli` 新增 scripts/install owner 模块。`http.Server` 真 TCP 留给 #313，当前 API 给出明确错误。

Tech Stack: Rust 2024，现有 `NativeCallable` host bridge，`flate2` / `brotli` / `tar`，现有 `reqwest`，现有 fixture runner 与 nextest。

Baseline/Authority Refs:

- `docs/aegis/specs/2026-07-08-node-advanced-modules-issue311-design.md`
- issue #311
- `AGENTS.md`：Node/ECMAScript 语义、注释中文、文件体量、fixture 规则
- `crates/wjsm-module/src/builtin_modules.rs`
- `crates/wjsm-module/builtin_js/node_events.js`
- `crates/wjsm-runtime/src/runtime_node_fs.rs`
- `crates/wjsm-runtime/src/runtime_node_crypto.rs`
- `crates/wjsm-runtime/src/startup_snapshot_native_bridge.rs`
- `crates/wjsm-snapshot-format/src/lib.rs`
- `crates/wjsm-cli/src/lib.rs`

Compatibility Boundary:

- 现有 builtin、runtime module loading、package resolution 行为保持。
- 新 builtin 使用 canonical 名称，支持裸 specifier 与 `node:` 前缀。
- child_process 默认 sandbox；只有 allowlist 允许执行。
- `wjsm test` 仅在当前 package 有 `scripts.test` 时改为 package script；否则保持测试文件发现。
- `wjsm run <existing-file>` 保持文件执行；`wjsm run <script>` 只在输入不是既有文件且 package script 存在时触发。

Verification:

- `cargo nextest run -E 'test(modules__node_builtin_stream) | test(modules__node_builtin_http) | test(modules__node_builtin_zlib) | test(modules__node_builtin_child_process)'`
- `cargo nextest run -p wjsm-runtime -E 'test(zlib) | test(child_process) | test(snapshot)'`
- `cargo nextest run -p wjsm-cli -E 'test(package_script) | test(install)'`
- `cargo check -p wjsm-runtime -p wjsm-module -p wjsm-cli -p wjsm-snapshot-format`

## Plan Basis

Facts:

- Builtin modules are ESM files under `crates/wjsm-module/builtin_js/` registered by `BUILTIN_MODULES`.
- Existing fs/crypto host bridges install `__wjsm_node_*` globals and dispatch through `NativeCallable`.
- Startup snapshot captures stateless native callables; new stateless variants require snapshot enum/ABI updates.
- CLI command dispatch is centralized in `execute()`; `cmd_run` and `cmd_test` are existing entry points.

Assumptions:

- #313 owns TCP/net/tls substrate, so issue #311 can only expose server APIs as explicit unsupported errors.
- Basic `wjsm install` follows issue #311 node_modules layout even though later package manager design may replace it.

Unknowns:

- crate versions for new compression/extraction dependencies will be resolved by Cargo during verification.

## Files

Create:

- `crates/wjsm-module/builtin_js/node_stream.js`
- `crates/wjsm-module/builtin_js/node_http.js`
- `crates/wjsm-module/builtin_js/node_https.js`
- `crates/wjsm-module/builtin_js/node_zlib.js`
- `crates/wjsm-module/builtin_js/node_child_process.js`
- `crates/wjsm-runtime/src/runtime_node_data.rs`
- `crates/wjsm-runtime/src/runtime_node_zlib.rs`
- `crates/wjsm-runtime/src/runtime_node_child_process.rs`
- `crates/wjsm-cli/src/cli_scripts.rs`
- `crates/wjsm-cli/src/cli_install.rs`
- `fixtures/modules/node_builtin_stream/main.js` + `.expected`
- `fixtures/modules/node_builtin_http/main.js` + `.expected`
- `fixtures/modules/node_builtin_zlib/main.js` + `.expected`
- `fixtures/modules/node_builtin_child_process/main.js` + `.expected`

Modify:

- `Cargo.toml`
- `crates/wjsm-runtime/Cargo.toml`
- `crates/wjsm-cli/Cargo.toml`
- `crates/wjsm-module/src/builtin_modules.rs`
- `crates/wjsm-runtime/src/lib.rs`
- `crates/wjsm-runtime/src/runtime_node_globals.rs`
- `crates/wjsm-runtime/src/runtime_builtins.rs`
- `crates/wjsm-runtime/src/runtime_process.rs`
- `crates/wjsm-runtime/src/types.rs`
- `crates/wjsm-runtime/src/startup_snapshot_native_bridge.rs`
- `crates/wjsm-snapshot-format/src/lib.rs`
- `crates/wjsm-cli/src/cli_args.rs`
- `crates/wjsm-cli/src/lib.rs`
- `docs/aegis/INDEX.md`

## Tasks

### Task 1 — Register new builtin modules

Files: `builtin_modules.rs`, new `node_*.js` stubs with real exports in later tasks.

Steps:
1. Add `stream`, `http`, `https`, `zlib`, `child_process` entries to `BUILTIN_MODULES`.
2. Create JS builtin files following existing default + named export pattern.
3. Verify parser accepts builtin sources with `cargo check -p wjsm-module`.

### Task 2 — Implement Node stream builtin in JS

Files: `node_stream.js`, stream fixture.

Steps:
1. Implement constructors using `EventEmitter.call(this)` and prototype methods.
2. Implement readable/writable buffering, `pipe`, `unpipe`, `pause`, `resume`, `destroy`, `finished`, `pipeline`.
3. Implement `Transform` / `PassThrough`, `Readable.from`, `toWeb/fromWeb` compatibility adapters.
4. Add fixture covering CJS require, ESM import, data/end/finish/drain, pipeline and objectMode.
5. Run the stream fixture.

### Task 3 — Implement zlib host bridge and JS API

Files: runtime node data/zlib modules, globals, builtins dispatch, types, snapshot format/bridge, `node_zlib.js`, zlib fixture.

Steps:
1. Extract shared Node data helpers for Buffer/string bytes.
2. Add `ZlibMethodKind`, host object creation, sync compression/decompression methods.
3. Add `NativeCallable::ZlibMethod` dispatch and snapshot ABI entries.
4. Implement JS sync functions and create* `Transform` factories that compress accumulated chunks on flush.
5. Add fixture covering gzip/gunzip, deflate/inflate, raw and brotli.
6. Run zlib fixture plus runtime snapshot-related checks.

### Task 4 — Implement child_process host bridge and JS API

Files: runtime child_process module, RuntimeOptions/ProcessState sandbox fields, globals, dispatch, snapshot bridge, `node_child_process.js`, child_process fixture.

Steps:
1. Add allowlist parsing from CLI env (`WJSM_CHILD_PROCESS_ALLOW`) into RuntimeOptions.
2. Implement host `spawnSync` / `execSync` using `std::process::Command`, cwd/env/timeout/maxBuffer/encoding/shell options.
3. Return Node-shaped result objects with Buffer stdout/stderr, status and signal.
4. Implement JS `ChildProcess`, `spawn`, `exec`, `spawnSync`, `execSync` wrappers and captured stdout/stderr streams.
5. Add fixture that proves default denial and allowlisted command success.
6. Run child_process fixture and CLI in-process option test.

### Task 5 — Implement http/https client builtins

Files: `node_http.js`, `node_https.js`, http fixture.

Steps:
1. Implement `ClientRequest` as Writable collecting body chunks.
2. On `end`, call global `fetch`, build `IncomingMessage` Readable from response body text/arrayBuffer, set `statusCode`, `headers`, `method`, `url`.
3. Implement `request`, `get`, `Agent` placeholder-free data object, `createServer` explicit #313 error.
4. Make `https` re-export http client behavior.
5. Add fixture using a data URL or Response-compatible local path if fetch supports it; otherwise test object/API shape without network.
6. Run http fixture.

### Task 6 — Implement CLI scripts and basic install

Files: `cli_args.rs`, `lib.rs`, new `cli_scripts.rs`, new `cli_install.rs`, CLI tests.

Steps:
1. Add `Install` command.
2. Add package script runner: package.json discovery, PATH injection, shell execution, pre/post lifecycle for named scripts.
3. Wire `run <script>` when input is not an existing path and script exists.
4. Wire `test` to package script when no explicit input/eval and `scripts.test` exists; otherwise preserve test-file discovery.
5. Implement registry metadata fetch, tarball download, tgz extraction to `node_modules/<pkg>`, package.json dependencies update.
6. Add targeted CLI tests for script PATH injection and install using a mocked local tarball/registry boundary where possible.
7. Run CLI targeted tests.

### Task 7 — Verify integration and clean up

Files: all changed files.

Steps:
1. Run targeted module fixtures.
2. Run package-level cargo checks.
3. Fix warnings and behavioral failures at the owner.
4. Update `docs/aegis/INDEX.md` with spec and plan rows.
5. Record final evidence.

## Risks

- Snapshot ABI drift: handled by explicit enum/discriminant update and tests.
- child_process security: default deny; tests set allowlist.
- http fixture network flakiness: use API shape or deterministic local data path, not external network.
- package install registry tests: prefer unit-level tar extraction helpers if remote registry would be flaky.

## Retirement

- No old builtin owner is retained for the new modules.
- No server fake path is introduced; #313 remains the canonical owner for TCP/server substrate.
