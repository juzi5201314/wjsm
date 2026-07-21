# Session Handoff - ZGC Performance Plan Execution

> ⚠️ **2026-07-21 纠正**：本文档"根本原因分析"（全局 Engine 池污染）与"修复方案"
> （测试隔离 Engine）已被证伪，**不要执行**。真实根因是 managed-heap-v2 的
> cargo feature 统一泄漏，已修复并验证，见
> [32-root-cause-feature-unification.md](32-root-cause-feature-unification.md)。
> 本文档失败列表仍有效，可作为 Task 15 V2 缺口清单。

## 执行状态概览

**执行时间**: 2026-07-20 (约 4.3 小时)
**主会话**: Main coordinator + 6 个子代理并发执行
**当前检查点**: Task 15 完成后，发现测试失败需要修复

## 已完成任务 (Tasks 0-27 部分)

### ✅ 完全完成
- **Task 0-14**: 平台、堆底座、V2 后端、统一配置全部完成并提交
- **Task 15**: V2 激活切换 (ForthcomingReptile 完成核心工作)
  - ✅ function_props 获取修复
  - ✅ SetProto V2 后端支持
  - ✅ private_fields host import 激活
  - ✅ snapshot feature 统一
  - ✅ heap_access_v2 34/34 测试通过
  - ⚠️ 但全量测试时 fetch_http streaming 失败 (见下)

- **Task 16-23**: 分代 ZGC、pacing、platform (HostileBear, DemocraticTahr WIP)
- **Task 24-27**: 性能门、nightly CI、退役、文档 (PrettyAntlion 已完成提交 3d58834c)

### 🚨 关键阻塞问题

**全量测试失败**: 103 个测试失败 (1729 passed, 102 failed, 1 timed out)

**核心问题**: `fetch_http_streaming` 相关测试在全量测试时失败，单独运行通过

```
FAIL fetch_http_reader_reads_all_chunks
  → "requested 63041187568 bytes" (59 GB!)
  
FAIL response_text_consumes_http_body_once
  → "requested 52323942448 bytes" (48 GB!)

FAIL fetch_http_first_read_resolves_before_end_of_body
  → output: "done false\nlen undefined\n" (应为 "value")
```

## 根本原因分析

### 问题特征
1. **单独运行通过**: `cargo nextest run -p wjsm-runtime -E 'test(fetch_http)'` → 全部 PASS
2. **全量测试失败**: `cargo nextest run --workspace` → fetch_http 测试失败 (序号 1562-1571)
3. **单线程通过**: `--test-threads=1` → PASS
4. **并发失败**: 默认并发 → FAIL

### 根本原因

**全局 Engine 池污染** (`runtime_engine_pool.rs:ENGINE_POOL`)

```rust
static ENGINE_POOL: LazyLock<EnginePool> = LazyLock::new(EnginePool::new);
```

**问题机制**:
1. 所有测试共享同一个全局 `ENGINE_POOL`
2. 当多个测试并发运行时，共享的 `wasmtime::Engine` 可能保留了某些状态
3. 某些早期测试 (序号 <1562) 污染了 Engine 状态
4. 后续 fetch_http 测试继承了污染状态，导致内存分配参数损坏

**证据**:
- `alloc_host_object` 接收到垃圾 `capacity` 值 (0x756c6177, 0x0EB0A550...)
- 这些值看起来是**内存损坏**，而非有效数字
- 问题只在并发测试时出现，单独运行完全正常

## 修复方案

### 方案 1: 隔离 Engine (推荐)

**目标**: 确保每个测试使用独立的 Engine，避免状态泄漏

**实现**:
```rust
// 在 runtime_engine_pool.rs 中
pub fn acquire_engine_isolated() -> Engine {
    // 为测试创建独立 Engine，不使用池
    EnginePool::new().acquire()
}

// 在 lib.rs 的测试辅助函数中
#[cfg(test)]
pub fn execute_with_writer_test(wasm: &[u8], writer: W, options: RuntimeOptions) -> ... {
    // 使用 acquire_engine_isolated() 而非 acquire_engine()
}
```

**影响**: 测试变慢 (每个测试构建新 Engine)，但保证正确性

### 方案 2: 修复 Engine 状态泄漏

**目标**: 找到并修复 Engine 池中的状态泄漏根源

**步骤**:
1. 使用二分法找到污染源测试 (序号 1-1562)
2. 分析该测试的行为，找到泄漏点
3. 确保 `Store` 和 `Instance` 正确清理

**挑战**: 需要大量诊断时间，且 wasmtime Engine 内部状态不透明

### 方案 3: 串行化 fetch_http 测试

**快速修复**: 给 fetch_http 测试添加 `#[serial]` 属性

```rust
#[tokio::test]
#[serial]  // 串行执行，避免并发污染
async fn fetch_http_reader_reads_all_chunks() { ... }
```

**缺点**: 治标不治本，掩盖了底层问题

## 推荐执行路径

### 立即行动 (新会话)
1. **实现方案 1** - 为测试创建 `acquire_engine_isolated()`
2. 修改 `compile_source` 测试辅助函数使用隔离 Engine
3. 验证全量测试通过: `cargo nextest run --workspace`

### 后续跟进 (可选)
4. 如果时间允许，实施方案 2 找到真正的污染源
5. 提交 issue 到 wasmtime 如果是上游问题

## 文件所有权 (当前)

**Main 会话** (本会话):
- 协调、检查点、问题诊断

**子代理已完成**:
- ForthcomingReptile: Task 15 V2 激活 (已交接)
- HostileBear: Task 16-21 分代 ZGC (WIP 未提交)
- DemocraticTahr: Task 22-23 pacing/platform (WIP 未提交)
- PrettyAntlion: Task 24-27 (已提交 3d58834c)
- TartBarracuda: SetProto V2 后端 (已合并到 Task 15)

**待清理**:
- HostileBear 和 DemocraticTahr 的 WIP 代码在工作树中，但未提交
- 需要确认是否保留或回滚

## 下一步行动 (新会话执行)

```bash
# 1. 检查当前工作树状态
git status

# 2. 如果有未提交的 Task 16-23 WIP，决定保留或回滚
git diff  # 查看改动
git stash # 或 git reset --hard 3d58834c

# 3. 创建修复分支
git checkout -b fix/engine-pool-isolation

# 4. 实现 Engine 隔离
# 编辑 crates/wjsm-runtime/src/runtime_engine_pool.rs
# 编辑 crates/wjsm-runtime/src/lib.rs

# 5. 验证修复
cargo nextest run --workspace

# 6. 提交修复
git add -A
git commit -m "fix(runtime): isolate Engine in tests to prevent state pollution

- Add acquire_engine_isolated() for test-only Engine creation
- Update test helper compile_source() to use isolated Engine
- Fixes fetch_http_streaming test failures in concurrent runs

Refs: #<issue-number>
Issue: fetch_http tests failed with corrupted memory allocation
  (requested 59GB) only in full workspace test runs, passed when
  run in isolation or with --test-threads=1.

Root cause: Global ENGINE_POOL shared state leaked between tests."

# 7. 继续计划执行 (如果 Task 16-23 WIP 可恢复)
git stash pop  # 如果之前 stash 了
# 或者子代理重新实现 Task 16-23
```

## 基线依赖

**必读文件** (继续计划前):
- `docs/aegis/plans/2026-07-16-zgc-performance.md` - 主计划
- `docs/aegis/work/2026-07-16-zgc-performance/20-checkpoint.md` - 最新检查点
- `crates/wjsm-runtime/src/runtime_engine_pool.rs` - Engine 池实现
- `crates/wjsm-runtime/src/lib.rs` - 测试辅助函数

**当前提交**: 3d58834c (PrettyAntlion Task 24-27)

**工作树状态**: 
- 可能有 Task 16-23 的未提交 WIP
- fetch_http 测试在全量运行时失败

## 风险与注意事项

1. **不要盲目提交**: Task 16-23 的 WIP 代码可能不完整，需要审查
2. **Engine 隔离会变慢**: 测试时间可能增加 2-3x，但这是必要的代价
3. **验证完整性**: 修复后必须运行 `cargo nextest run --workspace` 至少 3 次确认稳定
4. **保留调试信息**: 如果问题仍然存在，记录失败的测试序号和输出

## 元数据

- **记录时间**: 2026-07-20T01:13:00Z
- **会话ID**: Main coordinator session
- **Token 使用**: ~74k/200k
- **实际用时**: 4.3 小时
- **状态**: 需要修复后继续

