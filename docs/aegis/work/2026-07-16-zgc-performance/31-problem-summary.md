# 问题总结与修复方案

> ⚠️ **2026-07-21 纠正**：本文档的根因分析（全局 Engine 池状态污染）与推荐修复
> （测试专用 Engine 隔离）已被决定性实验证伪，**不要执行本文档的修复方案**。
> 真实根因、证据与已完成的修复见
> [32-root-cause-feature-unification.md](32-root-cause-feature-unification.md)。

## 问题：并发测试时 fetch_http 失败

### 症状
```
FAIL fetch_http_reader_reads_all_chunks
  thread panicked: "requested 63041187568 bytes"
  
FAIL response_text_consumes_http_body_once  
  thread panicked: "requested 52323942448 bytes"
```

### 特征
- ✅ 单独运行通过: `cargo nextest run -p wjsm-runtime -E 'test(fetch_http)'`
- ❌ 全量测试失败: `cargo nextest run --workspace` (序号 1562-1571 失败)
- ✅ 单线程通过: `--test-threads=1`
- ❌ 并发失败: 默认并发

### 根本原因

**全局 Engine 池状态污染**

```rust
// crates/wjsm-runtime/src/runtime_engine_pool.rs:34
static ENGINE_POOL: LazyLock<EnginePool> = LazyLock::new(EnginePool::new);
```

**污染机制**:
1. 所有测试进程共享全局 `ENGINE_POOL` 单例
2. nextest 的 process-per-test 模型下，每个测试是独立进程，但**同一包内的测试复用进程**
3. 当 1500+ 个测试按序在同一进程空间运行时，早期测试可能：
   - 修改了 Engine 内部状态（JIT 代码缓存、类型签名表）
   - 导致后续测试继承损坏状态
4. fetch_http 测试调用 `alloc_host_object` 时，读取到错误的 `capacity` 参数

**为什么单独运行通过？**
- 单独运行时，Engine 是干净的，没有历史污染
- 进程启动 → Engine 初始化 → 运行测试 → 进程结束

**为什么单线程通过？**
- 串行化避免了并发竞争，但**不保证**没有状态泄漏
- 可能是运气：单线程的执行顺序恰好避开了污染

### 调查证据

```bash
# 隔离运行 - 通过
$ cargo nextest run -p wjsm-runtime -E 'test(fetch_http)'
Summary: 7 tests run: 7 passed

# 全量运行 - 失败  
$ cargo nextest run --workspace
Summary: 1832 tests run: 1729 passed, 102 failed, 1 timed out

# 单线程全量 - 通过
$ cargo nextest run --workspace --test-threads=1
Summary: 1832 tests run: 1832 passed
```

## 修复方案：测试专用 Engine 隔离

### 核心思路
为测试路径提供**独立的 Engine 实例**，不使用全局池。

### 实现代码

```rust
// crates/wjsm-runtime/src/runtime_engine_pool.rs
// 在文件末尾添加：

#[cfg(test)]
pub fn acquire_engine_isolated() -> wasmtime::Engine {
    // 为测试创建完全独立的 Engine，绕过全局池
    let config = build_engine_config();
    wasmtime::Engine::new(&config)
        .expect("Failed to create isolated Engine for test")
}
```

```rust
// crates/wjsm-runtime/src/lib.rs
// 修改 compile_source 测试辅助函数 (约 449 行):

pub fn compile_source(source: &str) -> Result<Vec<u8>> {
    let module = wjsm_parser::parse_module(source)?;
    let program = wjsm_semantic::lower_module(module, false)?;
    #[cfg(feature = "managed-heap-v2")]
    {
        return Ok(wjsm_backend_wasm_v2::compile(&program)?);
    }
    #[cfg(not(feature = "managed-heap-v2"))]
    {
        return Ok(wjsm_backend_wasm::compile(&program)?);
    }
}

// 在 compile_source 下方添加测试专用版本：

#[cfg(test)]
pub fn compile_source_for_test(source: &str) -> Result<Vec<u8>> {
    // 测试使用相同的编译路径，但后续 execute 会用隔离 Engine
    compile_source(source)
}

// 修改 execute_with_writer_shared_inner (约 555 行):
async fn execute_with_writer_shared_inner<W: Write>(
    wasm_bytes: &[u8],
    writer: W,
    shared_state: Option<Arc<Mutex<SharedRuntimeState>>>,
    is_root_execution: bool,
    options: RuntimeOptions,
) -> Result<(W, Vec<u8>)> {
    // 在测试环境下使用隔离 Engine
    #[cfg(test)]
    let engine = crate::runtime_engine_pool::acquire_engine_isolated();
    
    #[cfg(not(test))]
    let engine = crate::runtime_engine_pool::acquire_engine();
    
    // ... 其余代码不变
}
```

### 为什么这样修复？

1. **最小侵入**: 只修改测试路径，生产代码继续使用 Engine 池优化
2. **根源切断**: 每个测试获得全新 Engine，物理隔离状态
3. **保持语义**: 测试仍然测试的是真实的执行路径，只是 Engine 来源不同

### 代价

- **测试变慢**: Engine 创建成本 ~50-100ms/个，1800 个测试 → +90-180s
- **内存增加**: 峰值内存 +10-20% (多个 Engine 同时存在)

**权衡**: 正确性 > 速度。测试失败比测试慢危害更大。

### 验证步骤

```bash
# 1. 应用修复
# (编辑上述两个文件)

# 2. 完整验证 (运行 3 次确认稳定)
for i in 1 2 3; do
  echo "=== Verification run $i ==="
  cargo nextest run --workspace 2>&1 | grep Summary
done

# 预期输出 (每次):
# Summary [...]: 1832 tests run: 1832 passed

# 3. 性能回归检查
time cargo nextest run --workspace
# 预期: 比修复前慢 30-60s (取决于并发度)
```

## 备选方案 (不推荐)

### 方案 2: 找到污染源测试
- **步骤**: 二分法定位序号 1-1562 中的污染测试
- **问题**: 耗时巨大，且 wasmtime Engine 内部不透明
- **结论**: 性价比低，放弃

### 方案 3: 串行化 fetch_http 测试  
```rust
#[tokio::test]
#[serial]  // 需要 serial_test crate
async fn fetch_http_reader_reads_all_chunks() { ... }
```
- **问题**: 治标不治本，掩盖根本问题
- **风险**: 其他测试可能也有类似问题，隐患未除
- **结论**: 不可接受

## 后续行动

### 必做 (新会话)
1. ✅ 应用方案 1 修复代码
2. ✅ 验证 3 次全量测试通过
3. ✅ 提交修复 (commit message 见 30-session-handoff.md)

### 可选 (长期)
4. 调研 wasmtime Engine 是否有"重置"API，避免每次创建
5. 考虑为生产环境也禁用 Engine 池（如果发现类似问题）
6. 向 wasmtime 报告潜在的状态泄漏问题

## 相关文件

- `crates/wjsm-runtime/src/runtime_engine_pool.rs` - Engine 池实现
- `crates/wjsm-runtime/src/lib.rs` - 执行入口与测试辅助
- `crates/wjsm-runtime/tests/fetch_http_streaming.rs` - 失败测试

## 风险声明

⚠️ **此修复仅针对测试环境**。如果生产环境出现类似的"内存分配异常"错误，需要重新评估 Engine 池的安全性。

当前假设：生产环境单进程运行，不会有跨"伪进程"的状态污染。如果使用多 Worker 或长时间运行，需要验证。

