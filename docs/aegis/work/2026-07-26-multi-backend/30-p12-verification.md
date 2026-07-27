# P12 最终验证门 — 2026-07-27

## 验证结果

### 1. 全量测试 ✅
```
cargo nextest run --workspace --no-fail-fast → 1805/1805 pass, 2 skip
```

### 2. GC 矩阵（happy fixtures）✅
- `WJSM_TEST_GC=mark-sweep` → 667/667 pass
- `WJSM_TEST_GC=g1` → 667/667 pass
- `WJSM_TEST_GC=zgc` → 667/667 pass

### 3. 快照兼容 ✅
```
cargo nextest run -p wjsm-runtime -E 'test(snapshot)' → 10/10 pass
```

### 4. 耦合断言 ✅
- `rg 'wasmtime|Caller|WasmEnv' crates/wjsm-builtins/src/` → 0
- `rg 'tokio|reqwest|block_in_place' crates/wjsm-builtins/src/` → 0
- `rg 'dyn ExecContext' crates/` → 0
- `rg 'wasmtime' crates/wjsm-gc/src/` (排除注释) → 0

### 5. 端到端行为证明 ✅
- Promise.allSettled → `fulfilled,rejected` ✅
- TransformStream → `42` ✅
- Atomics SAB → `5 8` ✅
- JIT target error → `Error: JIT backend is not implemented yet` ✅
- `cargo run -- eval '1+2'` → `3` ✅

### 6. 性能闭环报告（2026-07-28 复核修正）

**原结论（已推翻）**：本文档此前记录 `array_callback +22%`、`json +19%` 回归，
归因于 P1-P8 builtins 迁移的跨 crate 调用。复核证明该结论错误：原测量为**串行**
采集（先跑基线 commit、再跑当前版本），期间机器负载/温度漂移被误读为代码回归。

**复核方法**：基线 commit `2064e544` 单独 checkout 到 `/tmp/wjsm-baseline` 独立
`target/`，与当前版本二进制**交替（interleaved A/B）**采样，消除时间漂移。

| 基准 | 基线 min (ms) | 重构版 min (ms) | 实际差异 |
|---|---|---|---|
| array_callback | 2900 | 2824 | −2.6%（无回归）|
| json | 276 | 214 | −22%（无回归）|
| prop_access | 4378 | 4227 | −3.4%（无回归）|
| eq_typeof | 791 | 797 | +0.8%（阈值内）|

**结论**：多后端重构**未引入性能回归**，四项基准全部在阈值内或更快。串行采样
是无效方法，后续性能对比必须交替采样。

### 7. 回调路径导出查找优化（2026-07-28）

复核期间 `perf record` 暴露一个与重构无关的既有热点：array_callback 的
**约 70% CPU 周期**耗在 wasmtime 导出名查找（`StringPool::get_atom` 11.7% +
`IndexMap::get` 11.5% + `hash_one::<&str>` 10.9% + `sip::Hasher::write` 10.3% +
`WasmEnv::from_caller` 10.6%）。

根因：`WasmEnv::from_caller` 先尝试 `Caller::get_export` 逐个解析 **28 个**导出名
（每个一次字符串 siphash + IndexMap 查找），仅在失败时才回落
`RuntimeState::cached_wasm_env`；而 `call_wasm_callback_async` 每次回调都调它一次，
`prepare_callback_shadow_stack` 再调一次并额外按名字解析 `__shadow_sp`——
**每次 JS 回调 57 次字符串哈希查找**。

修复（`wasm_env.rs` / `host_helpers_callback.rs`）：
1. `from_caller` **翻转优先级**——先读 `cached_wasm_env`（`WasmEnv: Copy`，一次
   结构体拷贝），未命中才走导出名解析。缓存在 instantiate 后由
   `extract_wasm_env` 写入，且 realm clone 显式置 `None`，回落路径仍覆盖。
2. `prepare_callback_shadow_stack` 改为接收已解析的 `&WasmEnv`，`shadow_sp` 直接
   取自结构体字段（与 `__shadow_sp` 导出同一全局），删除二次 `get_export`。

效果（interleaved A/B，min-of-3，vs 基线 commit）：

| 基准 | 基线 (ms) | 优化后 (ms) | 提升 |
|---|---|---|---|
| array_callback | 3160 | 416 | **7.6×** |
| prop_access | 4754 | 649 | **7.3×** |
| json | 311 | 126 | **2.5×** |
| eq_typeof | 906 | 920 | 持平（纯 wasm 循环，无 host 调用，预期对照组）|

`perf` 复测：导出名查找热点完全消失，采样数 3804 → 153，剩余头部为
`Func::call_async` 本身（13.6%）。

验证：`cargo nextest run --workspace` → 1805/1805 pass, 2 skip；
GC 矩阵 `mark-sweep`/`g1`/`zgc` 各 667/667 pass；四基准 stdout 与基线逐字一致。

**注**：复核中另发现一个与本次改动**无关的既有缺陷**（基线同样复现）：
Proxy `apply` trap 内对 `arguments` 使用 spread 转发时结果错误
（`p(1,2)` 得 `[1, 2]` 而非求和值）；非 spread 的 apply trap 正常。已记录待单独修复。
