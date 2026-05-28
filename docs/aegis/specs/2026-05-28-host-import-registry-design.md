# Design Spec: Host Import Registry Consolidation

**Date:** 2026-05-28
**Goal:** 消除 `wjsm-backend-wasm` 中宿主导入索引漂移的根因，把宿主导入的顺序、名字、类型、Builtin 绑定和特殊索引查询收敛到单一 owner，避免新增/修改 import 时继续出现排序错误、过时注释和硬编码 `func_idx` 修复链。

**Architecture:** AOT JS/TS → IR → WASM；本设计只调整 `wjsm-backend-wasm` 的 host import owner 与索引派生方式，不改变 JS 语义，不改变 runtime 的按名字链接机制。

**Tech Stack:** Rust 2024、`wasm-encoder`、`wjsm-ir::Builtin`、`wasmtime` runtime（名字链接，非索引链接）。

**Baseline/Authority Refs:**
- `crates/wjsm-backend-wasm/src/lib.rs`
- `crates/wjsm-backend-wasm/src/compiler_core.rs`
- `crates/wjsm-backend-wasm/src/compiler_builtins.rs`
- `crates/wjsm-backend-wasm/src/compiler_instructions.rs`
- `crates/wjsm-backend-wasm/src/compiler_control.rs`
- `crates/wjsm-backend-wasm/src/compiler_module.rs`
- `crates/wjsm-ir/src/builtin.rs`
- `crates/wjsm-runtime/src/lib.rs`
- `crates/wjsm-runtime/src/host_imports/mod.rs`

**Compatibility Boundary:**
- **Must NOT break** 现有生成 WASM 对已有 runtime host functions 的名字与签名契约。
- **Must NOT change** runtime 通过 `linker.define("env", name, ...)` 按名字链接的机制。
- **Must NOT introduce** 新共享 crate、build.rs 代码生成链、或 runtime → backend 依赖。
- **Must remove** backend 内部所有依赖“人工记住某个 import 现在是第几个”的调用路径。
- **Must preserve** 当前 host import 的声明顺序，除非后续单独批准“重新排序并同步快照/验证”的设计；本次目标是收敛 owner，不是顺手洗牌。

**Verification:**
- backend 单元/集成验证能够证明：新增/修改 host import 后，不再需要同步编辑多个索引真相源。
- 变更后不存在对 imported host function 的数字字面量 `Call(N)`、`unwrap_or(N)` 或手写区间 `50..=76` 这类索引依赖。
- 受影响的 backend 测试通过；如需要新增测试，优先验证 registry 派生和关键 call site 行为，而不是断言具体数字索引。

---

## 1. Problem Statement

当前实现把同一件事分散在多处维护：

1. `HOST_IMPORT_NAMES` 维护名字与隐式顺序。
2. `compiler_core.rs` 手写 `imports.import("env", ..., EntityType::Function(...))`，再次维护顺序与类型。
3. `wjsm-ir::Builtin::import_name()` 与 `ALL_BUILTINS` 再维护一份 builtin → import name 关系。
4. `Compiler` 初始化和若干 codegen 路径中保存或直接写入硬编码 `func_idx`，例如：
   - `Call(16)` / `Call(17)`
   - `unwrap_or(313)`、`unwrap_or(95)`、`unwrap_or(76)` 等
   - `gc_collect_func_idx = 22`、`closure_create_func_idx = 35`、`proxy_trap_get_func_idx = 320`、`obj_get_by_index_func_idx = 385`
   - `for import_idx in 50u32..=76u32`
5. runtime 侧还保留带 index 含义的注释，但 runtime 实际按名字链接，这些数字注释不是执行真相，却会误导后续修改。

结果是：任何新增、删除、插入 host import 的变更，都可能让这些真相源彼此漂移。真正触发 WASM validation / 运行错误的，不是 runtime 的名字链接，而是 backend 还在用过期数字索引发 `call`。

---

## 2. First-Principles Invariants

### First Principle
新增或修改一个宿主导入时，工程师必须只改一个权威 owner；其余顺序、索引、导出名、Builtin 映射、分组列表都应从这个 owner 派生。

### Non-negotiables
- imported host function 的调用索引必须来自命名查询，而不是记忆数字。
- host import 的名字与函数签名必须在同一条声明里出现，避免“名字在 A，类型在 B”。
- backend 可以有针对 `Builtin` 和少量特殊导入的 typed lookup，但 lookup 的底层来源必须是统一 registry。
- runtime 的编号注释不能继续伪装成 authority。

### Assumptions to Delete
- “`HOST_IMPORT_NAMES` 只要人工同步就够了”。
- “`unwrap_or(313)` 这种 fallback 只是临时容错”。
- “array prototype import 恰好是 `50..=76`，记住就行”。
- “runtime 注释上的 index 对实现有约束力”。

---

## 3. Selected Approach

采用**单一声明表 / 注册表**，owner 放在 `wjsm-backend-wasm` 内的新模块中，推荐文件：

- `crates/wjsm-backend-wasm/src/host_import_registry.rs`

该模块成为 backend 内唯一的 host import authority。

### 3.1 Registry Data Model

每个 host import 以一条结构化声明描述，至少包含：

- `name: &'static str`
- `type_idx: u32`
- `builtin: Option<Builtin>` — 仅当该 import 对应某个 IR builtin 时填写
- `special: Option<SpecialHostImport>` — 仅当 backend 其他路径需要直接索引它时填写
- `group: Option<HostImportGroup>` — 仅当需要按组收集（例如 Array prototype method table）时填写

`SpecialHostImport` 只覆盖 backend 里确实存在直接索引需求的导入，例如：

- `StringConcat`
- `StringConcatVa`
- `GcCollect`
- `CreateException`
- `ClosureCreate`
- `ClosureGetFunc`
- `ClosureGetEnv`
- `ObjSpread`
- `ProxyTrapGet` / `ProxyTrapSet` / `ProxyTrapDelete`
- `GetBuiltinGlobal`
- `NewTarget`
- `NewTargetSet`
- `CreateUnmappedArgumentsObject`
- `CreateMappedArgumentsObject`
- `ArrayFrom`
- `ObjGetByIndex`
- `TypedArraySetByIndex`
- 以及任何仍需直接索引的非-builtin import

`HostImportGroup` 只为真实存在的组语义服务，例如 `ArrayPrototypeMethod`，用于替代 `50..=76` 这样的区间魔数。

### 3.2 Derived Outputs

以下内容全部从 registry 派生，不再各自手写：

1. `ImportSection` 中的 `imports.import(...)` 顺序与名字
2. 导出区对 imported host functions 的 re-export 名字
3. function import count
4. `builtin_func_indices`
5. `special_host_import_indices`
6. 需要按组推入 function table 的 import index 列表

`HOST_IMPORT_NAMES` 不再作为单独权威常量存在；如果某处仍需要只读名字列表，必须从 registry 视图派生，而不是再维护一个平行数组。

### 3.3 Lookup Contract

backend 内所有 imported host function 的索引获取，统一改为以下两类命名查询：

- `registry.builtin_index(Builtin::X)`
- `registry.special_index(SpecialHostImport::Y)`

需要按组收集时使用：

- `registry.group_indices(HostImportGroup::ArrayPrototypeMethod)`

缺失条目必须立即报错，不允许再有 `unwrap_or(N)` 这种“看似容错、实则继续把错误索引带下去”的写法。

---

## 4. Canonical Owner Boundaries

### New Canonical Owner
`crates/wjsm-backend-wasm/src/host_import_registry.rs`

它拥有：
- host import 名字
- host import 类型索引
- builtin / special / group 标签
- 名字到 index 的唯一派生逻辑

### Old Owners to Retire
以下内容不再拥有 host import 顺序真相：
- `crates/wjsm-backend-wasm/src/lib.rs` 中的 `HOST_IMPORT_NAMES`
- `crates/wjsm-ir/src/builtin.rs` 中仅为 backend 映射服务的 `Builtin::import_name()` / `ALL_BUILTINS` 路径（若切换后无其他用途，应删除）
- `compiler_core.rs`、`compiler_builtins.rs`、`compiler_instructions.rs`、`compiler_control.rs`、`compiler_module.rs` 中的数字 import 索引与 fallback 数字
- runtime 侧任何带编号 authority 暗示的注释

### Compat-only Carriers
无。目标是 clean cutover，不保留“旧数组 + 新注册表”双轨长期共存。

### Delete-first / Retirement Trigger
一旦 registry 接管所有派生与查询：
- 删除 `HOST_IMPORT_NAMES`
- 删除所有 host import 数字 fallback
- 删除或去编号化 runtime 的索引注释
- 删除 backend 中仅为旧映射服务的死代码

---

## 5. Detailed Design

### 5.1 Backend Initialization
`Compiler::new_with_data_base` 不再：
- 先手写 `imports.import(...)`
- 再从 `HOST_IMPORT_NAMES` 生成 `name_to_idx`
- 再从 `Builtin::import_name()` 反查 index

而是改为：
1. 从 registry 迭代生成 `imports.import(...)`
2. 直接构建 typed lookup（builtin / special / group）
3. 将 lookup 结果写入 `Compiler` 的命名字段或映射中

### 5.2 Codegen Call Sites
以下模式必须全部退役：
- `WasmInstruction::Call(<数字字面量>)`，当目标是 imported host function 时
- `...get(...).copied().unwrap_or(<数字>)`
- 构造函数中 `xxx_func_idx = <数字>`
- 任何依赖“这一组 import 连续落在某个闭区间”的逻辑

替代原则：
- string 拼接、异常构造、proxy trap、closure helper、obj_get_by_index 等都通过命名键查询
- `Array.prototype` method table 改成从 `HostImportGroup::ArrayPrototypeMethod` 派生的 index 列表推入

### 5.3 Runtime Comments Policy
runtime 继续按名字定义 host functions；本次不试图把 runtime 也改成依赖 backend registry，也不引入共享 crate。

但必须退休误导性编号注释：
- 可以保留“这一组函数是什么”的分组注释
- 不再手写“Import 323”“index 358”这类会漂移的数字注释
- 若未来确实需要编号文档，只能由权威源自动派生，而不是人工维护

### 5.4 Failure Mode Handling
registry 初始化或 lookup 缺失时，必须在编译期/构建路径中立即失败，并带明确错误信息，例如：
- 缺少某个 builtin 对应的 registry entry
- 某个 `SpecialHostImport` 未注册
- 重复 `builtin` / `special` / `name`

目标是“尽早、确定性失败”，而不是“带着错误索引继续生成 WASM”。

---

## 6. Non-goals

本设计**不**包含：
- 新共享 crate 或跨 crate registry 基础设施
- runtime host import 实现的语义重构
- host import 顺序的重新洗牌
- 与本问题无关的 backend helper 重构
- 自动代码生成或 build.rs 模板系统

---

## 7. Acceptance Criteria

设计落地后，必须满足：

1. 新增一个 host import 时，backend 只需要修改 registry owner，而不是同步编辑数组、导入顺序、Builtin 名字映射、special 索引字段和注释。
2. backend 中 imported host function 的 call site 不再依赖数字字面量或 fallback 数字。
3. `Array.prototype` import table 不再依赖 `50..=76` 这类区间魔数。
4. runtime 中不再保留会误导维护者的 host import index 注释。
5. 若 registry 与 call site 的命名键不一致，失败表现为明确错误，而不是生成错误索引的 WASM。

---

## 8. Risks and Trade-offs

### Why This Is Preferred
- 修的是根因，不是增加更早报警的补丁。
- 不引入 build 生成链，不增加调试面。
- owner 清晰，后续 agent/人类都更不容易在多个文件间漏改。

### Accepted Trade-offs
- 需要一次性清理多个 backend 文件的数字索引引用。
- registry 模块会比较大，但这是有意把分散复杂度收拢到单一 owner，而不是让复杂度继续泄漏到多个 call site。

### Rejected Alternatives
- **仅加断言/测试：** 只能更早报错，不能消灭硬编码索引。
- **build.rs / 外部 manifest 生成：** 同样可行，但当前仓库没有必要为这个问题引入更重的生成基础设施。
- **把 registry 提到共享 crate：** 会增加依赖方向和 owner 复杂度；当前根因完全可以在 backend 内解决。

---

## 9. TaskIntentDraft / BaselineReadSetHint / ImpactStatementDraft

### TaskIntentDraft
- **Outcome:** backend host import 索引改为单一注册表派生
- **Success evidence:** 无数字 import call/fallback；新增 import 只改一个 owner；相关验证通过
- **Stop condition:** 设计与计划都已写好，执行时有明确 owner、兼容边界和验证路径
- **Non-goals:** 不做 runtime 语义重构，不引入代码生成

### BaselineReadSetHint
- `compiler_core.rs`：当前 import section 构造与 `builtin_func_indices`
- `lib.rs`：`HOST_IMPORT_NAMES` 与 Compiler 字段
- `compiler_*`：所有数字 call site / fallback / range magic
- `builtin.rs`：`Builtin::import_name()` / `ALL_BUILTINS`
- runtime `lib.rs` 与 `host_imports/`：确认 runtime 只按名字链接，编号注释可退役

### ImpactStatementDraft
- **Affected layers:** backend-wasm 主体；runtime 仅注释与可读性治理
- **Owners:** 新 registry 模块成为 canonical owner；旧数组/旧映射退休
- **Invariants:** 名字与签名契约不变，JS 语义不变，runtime 链接机制不变
- **Compatibility risk:** 中等；因为会触碰多个 backend call site，但行为面集中、可验证
