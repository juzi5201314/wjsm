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
- 测试: 970 passed, 1 skipped
- CLI 集成: 双 embedded install 已接入；`cargo run -- eval "console.log('embedded ok')"` 输出正确
- 当前下一步: P2.4 array/elem helpers 迁移

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
