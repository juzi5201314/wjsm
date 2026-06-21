---
artifact: TaskIntentDraft
work: 2026-06-19-build-time-embedded-runtime
created: 2026-06-19
---

# Task Intent Draft

## Requested Outcome

实现 `docs/aegis/plans/2026-06-19-build-time-embedded-runtime.md` 计划，将 wjsm 运行时三类稳定制品（startup snapshot 字节、共享 wasm helper 模块、内部 builtin JS 扩展）在 `cargo build` 期固化进二进制。

## Scope

- **In scope**:
  - P0: 工作区准备（3 个新 crate skeleton + workspace 注册）
  - P1: Embedded startup snapshot（format 抽取、build.rs 生成、install API、cli 集成）
  - P2: Runtime support module（ABI 设计、build.rs、共享 instantiate、helpers 分批切换、bootstrap 切换）
  - P3: Builtin JS 扩展框架（manifest、ABI hash、sentinel 验证）
  - P4: 收尾（文档、ADR、全工作区验证）

- **Out of scope**:
  - 不在本计划范围内的其他功能开发
  - 不引入 `wjsm:` 用户命名空间
  - 不重组 RuntimeState（保持扁平）

## Non-Goals

- 用户 JS 模块解析行为不变
- 用户 JS 编译产物不缓存
- Scheduler 状态不进入 snapshot
- 不引入新的 cargo feature flag（除 `embedded` 外）

## Risk Hints

1. **Snapshot 边界漂移**: builtin JS 注入后 heap 状态可能破坏 snapshot 纯净性
2. **Wasmtime API 稳定性**: `Module::precompile` 和 `deserialize` 在不同版本间行为可能不一致
3. **wasm-encoder 兼容性**: 用户 module 改写 import 段时可能破坏现有验证
4. **Memory 共享语义**: imported memory 与 exported memory 的 identity 在 wasmtime 中的行为需要验证

## Success Evidence

1. P1 完成：first-run 不写客户机器磁盘 cache，`full execute` ≤ 旧 cold path
2. P2 完成：`module_only`（wasmtime compile）≤ 旧值的 60%
3. P3 完成：embedded snapshot 包含 builtin_js 注入后的 globals，且 snapshot/restore on/off 输出一致
4. 所有 fixture `.expected` 输出不变
5. 所有现有测试通过

## Stop Conditions

- `done`: 所有 P0-P4 任务完成，验证通过，文档更新
- `blocked`: 遇到需要用户决策的问题（如 API 设计选择、性能阈值调整）
- `needs-verification`: 某个阶段的验证结果需要进一步分析
- `scope-exceeded`: 发现需要修改计划本身的问题

## Baseline Read Set Hint

### Required Baseline Refs (计划中引用)

1. `docs/adr/0003-startup-snapshot-boundary.md`
2. `docs/adr/0002-runtimestate-stays-flat.md`
3. `docs/async-scheduler.md`
4. `AGENTS.md` (Startup snapshot / Function-property handle layout / WASM contract 段)

### Current Source Evidence (计划中引用)

1. `crates/wjsm-runtime/src/startup_snapshot.rs`
2. `crates/wjsm-snapshot-format/src/lib.rs`
3. `crates/wjsm-runtime/src/lib.rs:911-1390`
4. `crates/wjsm-backend-wasm/src/compiler_module.rs:243-340,780-940`
5. `crates/wjsm-backend-wasm/src/compiler_helpers.rs:1-1538`
6. `crates/wjsm-backend-wasm/src/host_import_registry.rs`
7. `crates/wjsm-runtime/src/wasm_env.rs`
8. `crates/wjsm-semantic/src/lib.rs:621`
9. `crates/wjsm-ir/src/lib.rs:15`
10. `crates/wjsm-runtime/src/runtime_eval.rs:45`

### Acknowledged Before Plan Refs

- 已读取：`docs/aegis/plans/2026-06-19-build-time-embedded-runtime.md`（完整计划）
- 已读取：`AGENTS.md`（项目约束）

### Cited in Plan Refs

- 外部参考：Deno snapshot 范式、V8 startup snapshots

### Missing Refs

- 无（所有必要文档均可访问）

## Baseline Usage Draft

- `wjsm-runtime` 当前提供 `execute / execute_with_writer` API，签名和返回值保持不变
- `RuntimeState` 字段保持扁平
- Snapshot 只覆盖 pristine runtime startup heap
- 用户 JS 模块解析行为不变
- 不引入 `wjsm:` 命名空间

## Impact Statement Draft

- **新增 3 个 crate**: `wjsm-runtime-snapshot`, `wjsm-runtime-support`, `wjsm-snapshot-format`
- **修改 `wjsm-runtime`**: 新增 `install_embedded_snapshot` / `install_embedded_support` API
- **修改 `wjsm-backend-wasm`**: 用户 module 改为 import memory/globals/table/helpers
- **修改 `wjsm-cli`**: 启动时自动 install embedded snapshot 和 support
- **退役**: 旧 per-module helper 内联 codegen、旧 user module export memory contract、旧 capture-on-first-run 默认路径

## Initial Checkpoint

### Current Todo

- [ ] P0: 工作区准备
- [ ] P1.0: 抽 snapshot lib
- [ ] P1.1: build-time 生成 snapshot 字节
- [ ] P1.2: install_embedded_snapshot 入口
- [ ] P1.3: wjsm-cli 启动时 install
- [ ] P1.4: bench
- [ ] P2.0: 设计 support module ABI
- [ ] P2.1: build.rs 生成 support.wasm + cwasm
- [ ] P2.2: runtime instantiate 共享 memory/table/globals
- [ ] P2.3: 切 object helpers
- [ ] P2.4: 切 array/elem helpers
- [ ] P2.5: 切 utility helpers
- [ ] P2.6: 切 bootstrap 阶段函数
- [ ] P2.7: 重新 bake P1 snapshot
- [ ] P2.8: 删除旧路径 + bench
- [ ] P3.0: builtin_js 框架 + manifest
- [ ] P3.1: ABI hash 纳入 builtin_js bundle + sentinel
- [ ] P4.0: 文档 ADR 0004 + 更新 AGENTS.md
- [ ] P4.1: 全工作区验证 + bench
- [ ] P4.2: 提测

### Active Task

P0: 工作区准备（尚未开始）

### Completed Tasks

无

### Evidence Refs

无

### Blockers

无

### Next Step

开始执行 P0: 工作区准备
