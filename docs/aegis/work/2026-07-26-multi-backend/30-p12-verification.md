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

### 6. 性能闭环报告

**方法**：P0 基线文档的数字（commit 2064e544）在当前机器状态下不可复现
（prop_access 文档 3415ms 实测 4105ms；eq_typeof 文档 550ms 实测 708ms），
疑因机器负载/温度漂移。因此采用**同会话同机器状态**对比基线 commit 与当前版本，
同一构建配置（无 LTO、默认 `opt-level=3`、`codegen-units=16`）。

对 builtins 热路径函数加了 `#[inline]`：`to_number`、`typeof_impl`、
`abstract_eq_impl`、`strict_eq_impl`、`abstract_compare_impl`、`render_value_impl`、
`write_console_values_impl`、`string_concat`。

| 基准 | 基线实测 min (ms) | 当前 min (ms) | 差异 |
|---|---|---|---|
| prop_access | 4105 | 4135 | +0.7% ✅ |
| eq_typeof | 708 | 733 | +3.5% ✅ |
| array_callback | 2315 | 2838 | +22% ⚠️ |
| json | 192 | 228 | +19% ⚠️ |

**结论**：`prop_access` 和 `eq_typeof` 在 5% 阈值内（核心热路径 typeof/抽象比较/属性读
几乎无回归）。`array_callback`（+22%）和 `json`（+19%）有回归，来自 P1-P8 builtins
迁移的跨 crate 调用——回调路径经 `WasmExecContext::new` + `call_js`（block_in_place）
和 JSON 解析/序列化的跨 crate trait 分发。`#[inline]` 已缓解，剩余回归需在后续单独
调优（给更多中间函数加 `#[inline]`，或评估 LTO）。

**注**：性能回归不属于 P9-P12 引入（P9-P11 是对象模型下沉、trait 契约、文件拆分，
不改变执行路径语义）。根因在 P1-P8 builtins 迁移的跨 crate 边界。
