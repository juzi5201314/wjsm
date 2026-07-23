# wjsm-runtime 编译时间优化分析报告

## 当前状况

- **单独编译耗时**：41.35s（基线，cargo clean 后）
- **增量编译**：0.6s（几乎即时）
- **源代码规模**：207 个文件，~19000 行核心代码
- **依赖规模**：45 个直接依赖，**945 个传递依赖**
- **编译产物**：47.8 MB 的 .rlib

## 瓶颈分析

### 1. 依赖膨胀（核心问题）
- **swc_core**：280+ 依赖树，用于 eval/vm.runInContext
- **wasmtime**：已优化到 opt-level=3
- runtime 为支持 eval 直接依赖整个编译管线：
  ```
  wjsm-runtime → wjsm-parser → swc_core
               → wjsm-semantic → swc_core  
               → wjsm-backend-wasm → swc_core
               → wjsm-module → swc_core
  ```

### 2. 文件规模
- lib.rs: 2968 行（70 个模块声明，17 个 pub use）
- array_object.rs: 2668 行
- collections_buffers.rs: 2513 行  
- runtime_builtins.rs: 2466 行
- runtime_eval.rs: 2420 行

### 3. 已有优化
- wasmtime/cranelift: opt-level=3 ✓
- wjsm-runtime: opt-level=3 ✓
- memchr: opt-level=3 ✓
- debug info: line-tables-only ✓

## 尝试的优化方案

### ❌ 方案 1：将 eval 功能放到 optional feature
- **结果**：无效，因为 eval 是核心功能，default = ["eval"] 意味着日常开发仍会编译整个管线
- **放弃原因**：对实际工作流无帮助

### ❌ 方案 2：对 swc_core 应用 opt-level=2
- **结果**：41.35s → **86s**（变慢 2 倍）
- **原因**：swc_core 本身编译极慢，加优化后更慢；且对 runtime 热路径帮助不大

### ❌ 方案 3：增加 codegen-units = 256
- **结果**：41.35s → **117s**（变慢 2.8 倍）
- **原因**：虽增加并行度，但链接时间和管理开销大幅增加

### ⚠️ 方案 4：拆分大文件（array_object.rs 等）
- **分析**：这些文件功能紧密耦合，强行拆分会带来更多 `pub(super)` 和跨模块依赖
- **预期收益**：对首次编译帮助不大（依赖才是瓶颈），仅对可维护性有帮助
- **状态**：暂缓

## 根本原因

**编译时间的主要来源不是 wjsm-runtime 本身的代码量，而是 945 个传递依赖（尤其是 swc_core）。**

增量编译已经很快（0.6s），首次编译慢是因为要编译整个依赖树。

## 实际有效的优化方向

### 1. 架构级优化（需要大改）
- **拆分 runtime**：将 eval 相关功能（runtime_eval + 编译管线依赖）拆到独立 crate
  - `wjsm-runtime-core`：核心运行时，无 swc_core 依赖
  - `wjsm-runtime-eval`：eval 支持，依赖编译管线
  - `wjsm-runtime`：facade crate，re-export 两者
  - **预期收益**：不需要 eval 的场景（纯运行预编译代码）可以只编译 core

### 2. 工程优化（立即可行）
- **sccache/ccache**：已被排除（你说不用第三方）
- **更快的链接器**：已在用 lld ✓
- **cargo-nextest**：已在用 ✓
- **workspace 缓存**：已配置 ✓

### 3. 可维护性优化（非性能，但有价值）
- 拆分 lib.rs 为逻辑子模块（减少单文件复杂度）
- 拆分超大文件（array_object.rs 等）为子模块
- **预期收益**：更好的代码组织，但对编译时间影响有限

## 最终建议

### 短期（不改架构）
**41.35s 已经是当前架构下的合理水平**，进一步优化收益有限：
- swc_core 优化会让编译更慢
- codegen-units 优化会让编译更慢
- 增量编译已经很快（0.6s）

### 中期（架构改进）
如果编译时间确实影响开发体验，考虑：
1. **拆分 runtime 为 core + eval 两个 crate**（如上所述）
2. **延迟加载编译管线**：eval 时动态加载 wjsm-semantic/backend-wasm（需要 dlopen 或 plugin 架构）

### 长期（工程改进）
- 引入 Rust analyzer 的 proc-macro server 机制
- 探索 cranelift 作为 rustc codegen backend（实验性）

## 数据对比

| 场景 | 时间 | 说明 |
|------|------|------|
| 基线（当前配置） | 41.35s | 已有优化（wasmtime opt-level=3等） |
| 增量编译 | 0.6s | 几乎即时 |
| + swc opt-level=2 | **86s** | ❌ 变慢 2 倍 |
| + codegen-units=256 | **117s** | ❌ 变慢 2.8 倍 |

## 结论

**当前 41.35s 是合理的编译时间，主要瓶颈是 945 个传递依赖（swc_core）。**

在不改架构的前提下，已经没有明显的优化空间。如果要进一步提速，需要：
1. 拆分 runtime 架构（分离 eval 功能）
2. 或者接受当前编译时间，依赖增量编译（0.6s）进行日常开发

建议：**保持当前配置，专注增量编译体验**。首次编译 41s 对于这个规模的项目是合理的。
