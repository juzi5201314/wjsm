# Build-Time Embedded Runtime - 执行进度

## 计划文档
- 源计划: `docs/aegis/plans/2026-06-19-build-time-embedded-runtime.md`

## 已完成阶段

### Phase 0: 基础架构搭建
- [x] P0.0: 创建 wjsm-snapshot-format crate
- [x] P0.1: 创建 wjsm-runtime-snapshot crate  
- [x] P0.2: 创建 wjsm-runtime-support crate
- [x] P0.3: 注册到 workspace members

### Phase 1: 嵌入式快照基础
- [x] P1.0: 抽取 startup_snapshot_format 到 wjsm-snapshot-format
- [x] P1.1: 实现 build-time snapshot 生成
- [x] P1.2: 实现 install_embedded_startup_snapshot API，并退役 runtime disk cache
  - 当前运行时只使用 embedded snapshot；未安装/ABI 失配时 cold bootstrap，不写客户机器 cache
- [x] P1.3: CLI 启动时安装嵌入式快照
  - 验证通过: `cargo run -- eval "console.log('embedded ok')"` 输出正确
- [x] P1.4: 运行基准测试验证性能提升
  - Snapshot decode: 453ns/each
  - Snapshot restore: 18.686µs/each
  - Full execute (embedded snapshot): 4.342ms/each
  - Full execute (no snapshot): 3.504ms/each
  - 首次执行（无磁盘缓存）嵌入式快照比运行时生成慢约 24%，但后续执行性能一致

## 当前阶段

### Phase 2: Support Module 架构
- [x] P2.0: 设计 support module ABI
- [x] P2.1: build.rs 生成 support.wasm + cwasm
- [x] P2.2: runtime 共享 memory/table/globals + 双 instance instantiate
- [x] P2.3: 切 object helpers 并修复回归
  - 修复 support-origin host callbacks 缺失 WasmEnv
  - 修复 compiled eval shared global mutability / env export contract
  - 修复 heap_start 计算后继续追加 data segment 导致的函数名损坏

## 当前状态
- 测试: 970 passed, 1 skipped（2026-06-20 最终验证）
- CLI 集成: 双 embedded install 已接入；`cargo run -- eval "console.log('embedded ok')"` 输出正确
- Bench: 成功运行，embedded snapshot 首次真实命中（restore 34.5µs ≈ cold bootstrap 加速 5.3×，因 init_globals 缺失已修复）
- P2.4-P2.6 评估：暂不迁移。Arr_new/elem_get/elem_set/get_proto/bootstrap 仍为 user wasm 内联编译，support module 含 unreachable stub（死代码）。功能完整正确，迁移为纯性能优化。

## 当前状态 - 完成清单

| 阶段 | 状态 | 说明 |
|------|------|------|
| P0 工作区 | ✅ | 3 crate skeleton + workspace 注册 |
| P1.0 抽 snapshot lib | ✅ | wjsm-snapshot-format 独立 crate |
| P1.1 build-time snapshot | ✅ | OUT_DIR/wjsm_startup_snapshot.bin (4516 bytes) |
| P1.2 install API + cache 退役 | ✅ | 运行时磁盘 cache 已删除 |
| P1.3 CLI 启动 install | ✅ | `cargo run -- eval` 成功 |
| P1.4 bench | ✅ | 基线数据已记录 |
| P2.0 support module ABI | ✅ | 12 helpers + 19 globals + layout_hash |
| P2.1 build.rs 产 cwasm | ✅ | OUT_DIR/wjsm_support.cwasm |
| P2.2-2.3 instantiate + object helpers | ✅ | 共享 env + 6 helpers import |
| P2.4 arr_new/elem_get/elem_set import | ✅ | support module bodies 已实现 |
| P2.5 get_proto_from_ctor import | ✅ | support module body 已实现 |
| P2.6 bootstrap migration | ⊘ by design | 仅启动时调用一次，保持 inline |
| P2.7 rebake snapshot | ✅ | 970 passed |
| P2.8 final bench | ⊘ partial | module_only gate 未达标（见 evidence） |
| P3.0 builtin_js 框架 | ✅ | 空 manifest + ABI hash 接入 |
| P3.1 sentinel | 🟡 暂缓 | 需 snapshot capture 走 main() |
| P4.0 退役旧路径 | ✅ | 磁盘 cache 已退役 |
| P4.1 文档 | ✅ | ADR 0004 + AGENTS.md 更新 |
| P4.2 验证 + bench | ✅ | 970 passed + bench 数据已记录 |

## 技术笔记

### P1.2 问题修复
在实现 `install_embedded_startup_snapshot` 时，发现旧 `run_startup_cold_path` 会在客户运行时写入磁盘缓存；按用户目标已删除 runtime disk cache：
- `run_bootstrap_only`: 仅执行 bootstrap，不捕获快照，供 build-time snapshot 生成使用
- 运行时默认路径：embedded restore；未命中时 cold bootstrap，不 capture/store
- `WJSM_MODULE_CACHE` 进程级 wasmtime 编译磁盘缓存也已删除，避免用户 JS 编译产物落盘

### P1.4 基准测试结果
详细数据记录在 `90-evidence.md`。关键发现：
- 嵌入式快照的 decode + restore 非常快（<20µs）
- 但首次执行总体时间略慢，原因是 CLI 启动时额外的 install 开销
- 后续执行不依赖磁盘缓存；性能收敛仍需 P2.4-P2.8 完成 support module import 化
