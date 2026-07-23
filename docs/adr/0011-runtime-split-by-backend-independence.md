# ADR 0011: Runtime 拆分为后端无关架构

## 状态

提议中（2026-07-24）

## 背景

### 当前问题

1. **编译时间过长**：wjsm-runtime 编译耗时 41.35s（首次），主要因为依赖整个编译管线（parser + semantic + backend → swc_core 280+ 依赖）
2. **强耦合 wasmtime**：runtime 与 wasmtime 强耦合（Caller、Linker、WasmEnv），无法支持其他后端
3. **职责混乱**：runtime 同时包含执行引擎、host 能力、动态编译、后端绑定

### 依赖分析

- wjsm-runtime: 945 个传递依赖
- eval/vm 相关代码：~3400 行（占总代码量 18%）
- 依赖编译管线的文件：runtime_eval.rs、runtime_node_vm.rs、runtime_builtins.rs 部分

### 未来需求

多后端支持：
```
IR → wjsm-backend-wasm     → WASM (wasmtime)
   → wjsm-backend-native   → LLVM IR → native
   → wjsm-backend-c        → C source
   → wjsm-backend-cranelift → native (direct)
```

## 决策

采用**按后端无关性拆分**架构：

```
wjsm-host          # 后端无关的 host 能力抽象
  ├─ HostRuntime trait（console.log、GC、async 等）
  ├─ Value boxing/unboxing 定义
  ├─ GC 接口抽象（heap、handles）
  ├─ Builtins 语义（不依赖具体后端）
  └─ 不依赖 wasmtime 或任何具体后端

wjsm-host-wasm     # wasmtime 后端实现
  ├─ impl HostRuntime for WasmtimeHost
  ├─ WasmEnv、Caller、Linker
  ├─ 当前 runtime 的大部分代码（19000 行）
  └─ 依赖 wasmtime + wjsm-host

wjsm-dyncode       # 动态代码编译（后端无关）
  ├─ eval()、Function 构造器、vm.runInContext 等
  ├─ 编译到 IR（不依赖 backend）
  ├─ 依赖 parser + semantic（~3400 行）
  └─ 后端由调用者选择

wjsm-runtime       # Facade（向后兼容）
  └─ pub use wjsm_host_wasm::* + wjsm_dyncode::*
```

## 命名理由

- **wjsm-host**：贴合 ECMAScript 术语（host environment），表示"宿主环境"
- **wjsm-host-wasm**：清晰表示这是 WASM 后端的 host 实现
- **wjsm-dyncode**：dynamic code，清晰表达职责（动态代码编译服务）
- **wjsm-runtime**：保持现有名字作为 facade，向后兼容

## 预期收益

### 编译时间

| 场景 | 当前 | 拆分后 |
|------|------|--------|
| 纯执行预编译 WASM | 41.35s | ~12s（wjsm-host-wasm） |
| 需要 eval/vm | 41.35s | ~35s（host-wasm + dyncode） |
| 抽象层开发 | N/A | ~5s（wjsm-host） |

### 架构收益

1. **后端无关**：新后端只需实现 `HostRuntime` trait
2. **职责清晰**：host = 运行时能力，dyncode = 编译服务，backend = 输出格式
3. **按需编译**：不需要 eval 的场景（如 serverless）可以只依赖 wjsm-host-wasm（节省 30s）
4. **向后兼容**：wjsm-runtime 作为 facade 保持现有 API 不变

## 实施计划

### Phase 1：抽象层设计（1-2 周）

1. 创建 `wjsm-host` crate
2. 定义 `HostRuntime` trait：
   ```rust
   pub trait HostRuntime {
       fn console_log(&mut self, args: &[Value]) -> Result<()>;
       fn alloc_object(&mut self) -> Handle;
       fn gc_collect(&mut self) -> GcStats;
       fn async_hook_init(&mut self, ...);
       // ... 约 50+ 方法
   }
   ```
3. 定义后端无关的 Value、Handle、GC 类型
4. 编写设计文档和示例

### Phase 2：重构现有 runtime（2-3 周）

1. 创建 `wjsm-host-wasm` crate
2. 将 wjsm-runtime 代码移入（除 eval/vm）
3. 实现 `impl HostRuntime for WasmtimeHost`
4. 验证所有测试通过

### Phase 3：拆分 dyncode（1 周）

1. 创建 `wjsm-dyncode` crate
2. 移入 runtime_eval.rs、runtime_node_vm.rs
3. 清理编译管线依赖
4. 定义 dyncode 与 host 的接口

### Phase 4：Facade 和兼容性（1 周）

1. 重构 `wjsm-runtime` 为 facade crate
2. pub use wjsm_host_wasm + wjsm_dyncode
3. 确保所有现有代码零改动
4. 更新 CLI 依赖（可选优化）

### Phase 5：文档和验证（1 周）

1. 更新 AGENTS.md
2. 编写"何时用 host-wasm vs runtime"指南
3. Benchmark 验证编译时间
4. 编写新后端实现教程

**总计**：约 6-8 周

## 风险和缓解

### 风险 1：HostRuntime trait 设计复杂

**影响**：trait 方法过多（50+），难以维护

**缓解**：
- 按职责拆分 sub-trait（ConsoleHost、GcHost、AsyncHost 等）
- 提供默认实现减少样板代码
- 充分原型验证后再大规模重构

### 风险 2：性能回归

**影响**：trait 动态分发可能带来性能损失

**缓解**：
- trait 方法用 `#[inline]` 提示优化
- 关键路径考虑泛型单态化
- benchmark 对比验证

### 风险 3：重构工作量大

**影响**：6-8 周工作量，可能阻塞其他功能

**缓解**：
- 渐进式：先拆 dyncode（解决编译时间），再抽象 host
- 保持 wjsm-runtime facade（零破坏性）
- 分阶段验证，每个 phase 都可独立交付

## 替代方案

### 备选 1：最小化拆分（仅 dyncode）

```
wjsm-runtime  # 保持不变
wjsm-dyncode  # 拆出 eval/vm
```

**优点**：工作量小（1-2 周），立即解决编译时间  
**缺点**：不为多后端做准备，未来仍需重构

**决策**：短期可行，但不如方案 1 长远

### 备选 2：按层次拆分

```
wjsm-runtime-core
wjsm-runtime-wasm
wjsm-runtime-dyncode
```

**优点**：保持现有命名风格  
**缺点**：名字太长（wjsm-runtime-wasm 是默认选择）

**决策**：命名不够简洁

## 后续工作

1. **Phase 1 完成后**：征求社区反馈（如果开源）
2. **Phase 4 完成后**：开始 wjsm-host-native 原型
3. **长期**：考虑将 wjsm-host 提取为独立仓库（通用 JS runtime 抽象）

## 参考

- [编译时间分析报告](../compile-time-analysis.md)
- [多后端方案对比](/tmp/runtime_split_multibackend.md)
- ECMAScript 规范：Host Environment 章节
- V8 架构：https://v8.dev/docs/embed
