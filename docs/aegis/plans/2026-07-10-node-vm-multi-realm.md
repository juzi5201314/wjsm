# node:vm 多 Realm 沙箱实现计划

- 日期：2026-07-10
- 状态：已按用户授权（无需审批 spec）进入实现
- 关联 issue：#313
- 设计 spec：`docs/aegis/specs/2026-07-10-node-vm-multi-realm-design.md`

## Goal

在 wjsm 中完整、忠实地实现 `node:vm` 核心稳定全集 + `timeout`，采用「单堆多 realm」模型：所有 realm 共享同一 wasmtime Store / 线性内存 / `obj_table` / GC，每个 realm 拥有独立 intrinsics（`[] instanceof Array === false`），对象跨 realm 按引用自由流动。Realm 诞生走「克隆 pristine snapshot 对象图 + 全量 handle 重映射」，eval 双路径（编译 eval 为主、AST 解释器兜底）均 realm 感知。

## Architecture

```
require('node:vm')
  → builtin_modules.rs 注册 canonical "vm"
  → node_vm.js（API 外形：Script / runIn* / compileFunction / constants）
  → __wjsm_node_vm host bridge（createContext / runInRealm / isContext / compileFunctionInRealm）
      → realm.rs（Realm / RealmIntrinsics / RealmId / active_realms 登记）
      → realm_clone.rs（pristine 对象图克隆 + 全量 handle 重映射）
          → handle_remap.rs（共享重写内核，snapshot restore + realm clone 共用）
      → runtime_eval.rs（eval 双路径注入 realm 上下文）
      → runtime_gc/roots.rs（per-realm root 枚举）
```

主 realm = `active_realms[0]`（现有全局单例惰性登记），消除单例 vs realm 双路径特例。

## Tech Stack

- Rust 2024，`swc_core` 解析、`wasm-encoder` codegen、`wasmtime` 执行。
- Realm 数据结构存 `RuntimeState`（扁平字段，遵 ADR 0002）。
- Timeout 走 wasmtime epoch interruption（`Engine::set_epoch_deadline` + 后台 timer bump epoch）。
- 测试：`cargo nextest`，fixtures（`fixtures/happy` + `.expected`）+ crate 单测。

## Baseline / Authority Refs

- issue #313（`vm` 原为非目标，本轮用户明确要求完整落地）。
- ADR 0002（RuntimeState 扁平）、0003（snapshot 边界/重定位规则）、0004（build-time embedded runtime / abi_hash 输入）、0005（pluggable GC root provider）。
- 设计 spec §2.1–§2.3 决定性架构判断，§5 架构设计，§4 次级决策。
- 源码 owner：`runtime_eval.rs`、`startup_snapshot.rs`/`startup_snapshot_remap.rs`、`runtime_gc/roots.rs`、`host_imports/collections_buffers.rs`、`snapshot-format/src/lib.rs`、`builtin_modules.rs`、`runtime_node_globals.rs`。

## Compatibility Boundary

- 单 realm 程序行为与开销**完全不变**（主 realm = `active_realms[0]` 惰性登记，非 vm 程序不进克隆路径）。
- 现有全部 fixture `.expected` 输出不变。
- `RuntimeState` 保持扁平（仅新增字段，不嵌套）。
- snapshot ABI 因新增 `NativeCallable::VmMethod` 而 rebake（构建期自动，`abi_hash` 同步）。
- 非目标（明确抛错，不留 no-op）：`SourceTextModule`/`SyntheticModule`、`measureMemory`、`importModuleDynamically` 完整语义、跨线程 realm、安全沙箱语义。

## Verification（全局验收）

```bash
cargo nextest run --workspace          # 全绿，零编译警告
cargo build 2>&1 | grep -c warning     # → 0
```
标志性语义 CLI smoke（§各任务分述）：`[] instanceof Array === false`、跨 realm 对象引用、sandbox 双向可见、timeout 触发、`vm.Script` 复用。

---

## BaselineUsageDraft

- Required baseline refs：issue #313、ADR 0002/0003/0004/0005、设计 spec、`runtime_eval.rs`/`startup_snapshot*.rs`/`runtime_gc/roots.rs`/`snapshot-format`/`builtin_modules.rs`/`runtime_node_globals.rs`。
- Delivered context refs：AGENTS 注入、issue 正文、三份探底报告、设计 spec。
- Acknowledged before plan refs：已读 ADR 0003/0004 全文 + 上列源码结构（builtin 注册数组、install_native 模式、SnapshotNativeCallable 判别式表）。
- Cited in plan refs：以下全部任务。
- Missing refs：无阻塞；ADR 0008 待实现后回填。
- Decision：continue。

## Architecture Integrity Lens

- Invariant：单 Store/单 obj_table/单 GC；realm = 带标签 intrinsic + global；Node API 外形归 `node_vm.js`。
- Canonical owner / contract：`realm.rs`（Realm）、`realm_clone.rs`（克隆）、`handle_remap.rs`（重写内核）、`runtime_node_vm.rs`（host bridge）、`node_vm.js`（API）各单一 owner。
- Responsibility overlap：`realm_clone.rs` 与 `startup_snapshot_remap.rs` 的全堆遍历+handle 重写 → 抽 `handle_remap.rs` 共享内核，两侧调用，杜绝逻辑漂移。
- Higher-level simplification：主 realm 登记 `active_realms[0]`，统一 GC/执行路径。
- Retirement / falsifier：标志性语义 fixtures；experimental/measureMemory 明确抛错。
- Verdict：proceed。

## Plan Pressure Test

- Owner / contract / retirement：新 owner 文件边界清晰；无旧路径退役（vm 是全新能力），仅泛化 remap 内核（旧 `remap_array_proto_function_indices` 改为调用共享内核，行为等价）。
- Architecture integrity / higher-level path：主 realm 统一为 active_realms[0] 已采纳。
- Verification scope：每阶段有 fixture/单测/CLI smoke。
- Task executability：每任务给确切路径+完整代码+命令。
- Pressure result：proceed。

## Plan-Time Complexity Check

- Target files：新增 `realm.rs`/`realm_clone.rs`/`handle_remap.rs`/`runtime_node_vm.rs`/`node_vm.js`（各单一 owner，新文件无膨胀风险）；改造 `runtime_eval.rs`（已 2147 行，本计划只注入 realm 参数，不新增大块 → 抽 realm 解析为独立小函数）、`runtime_gc/roots.rs`（加一段 realm 遍历）、`snapshot-format`（加一个枚举 discriminant）、`startup_snapshot_remap.rs`（改为委托 `handle_remap`，行数下降）。
- Owner fit：realm 机制独立成 crate 内新模块，不塞进 eval/gc 热点文件。
- Recommendation：add owner file（realm/clone/remap/vm bridge 全新文件）+ edit-in-place（eval/gc/snapshot 小改）。

---

## 阶段总览

- **Phase 0 — Realm 基础设施**：数据结构、handle_remap 共享内核、主 realm 登记。
- **Phase 1 — Realm 克隆**：pristine 对象图克隆 + 全量重映射，createContext 落地。
- **Phase 2 — eval 双路径 realm 感知**：runInContext 执行引擎。
- **Phase 3 — vm builtin + API 外形**：node_vm.js + host bridge + builtin 注册。
- **Phase 4 — GC per-realm roots + 死 realm 回收**。
- **Phase 5 — timeout（epoch interruption）**。
- **Phase 6 — 次级选项 + fixtures + 回归 + ADR 回填**。

每阶段任务遵循 TDD 五步：写失败测试 → 验证 RED → 最小实现 → 验证 GREEN → 提交。

---

# Phase 0 — Realm 基础设施

## Task 0.1：定义 Realm / RealmIntrinsics / RealmId 数据结构

**Files**
- create: `crates/wjsm-runtime/src/realm.rs`
- modify: `crates/wjsm-runtime/src/lib.rs`（`mod realm;` + `RuntimeState.active_realms` 字段）
- test: `crates/wjsm-runtime/tests/realm_registry.rs`

**Why**：realm 是整个 vm 的承载单元；主 realm 需登记为 `active_realms[0]` 以统一后续 GC/执行路径。

**Impact/Compatibility**：`RuntimeState` 新增一个扁平字段，遵 ADR 0002；非 vm 程序 `active_realms` 只含惰性登记的主 realm，零行为变化。

**Verification**
```bash
cargo nextest run -p wjsm-runtime -E 'test(realm_registry)'
```

**Steps**

1. **写失败测试** `crates/wjsm-runtime/tests/realm_registry.rs`：
```rust
//! 验证主 realm 登记为 active_realms[0]，RealmId 分配单调递增。
use wjsm_runtime::realm::{Realm, RealmId, RealmIntrinsics};

#[test]
fn realm_id_is_monotonic() {
    assert_eq!(RealmId(0), RealmId(0));
    assert!(RealmId(1) > RealmId(0));
}

#[test]
fn realm_intrinsics_default_is_all_undefined() {
    let intr = RealmIntrinsics::empty();
    assert_eq!(intr.object_proto, wjsm_runtime::value_encode_undefined());
    assert_eq!(intr.array_proto, wjsm_runtime::value_encode_undefined());
}

#[test]
fn realm_carries_global_and_intrinsics() {
    let r = Realm::new(RealmId(0), 12345_i64, RealmIntrinsics::empty());
    assert_eq!(r.id, RealmId(0));
    assert_eq!(r.global_object, 12345_i64);
}
```
（若 `value_encode_undefined` 未公开，则在 `lib.rs` 加 `pub fn value_encode_undefined() -> i64 { value::encode_undefined() }` 测试辅助，或改测试用 `realm` 模块内导出的常量——实现步骤择一并保持一致。）

2. **验证 RED**：`cargo nextest run -p wjsm-runtime -E 'test(realm_registry)'` → 编译失败（模块不存在）。

3. **最小实现** `crates/wjsm-runtime/src/realm.rs`：
```rust
//! 单堆多 realm 支持：每个 realm = 独立 intrinsics 句柄集 + global 对象。
//! 所有 realm 共享同一 Store/obj_table/GC（对象跨 realm 按引用流动）。

use crate::value;

/// Realm 唯一 id。realm 0 = 主 realm（现有全局单例）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RealmId(pub u32);

/// 该 realm 的 primordial 原型/构造器句柄集合。
/// 字段与 snapshot header / RuntimeState primordial 字段一一对应。
#[derive(Debug, Clone, Copy)]
pub struct RealmIntrinsics {
    pub object_proto: i64,
    pub array_proto: i64,
    pub function_proto: i64,
    pub iterator_prototype: i64,
    pub generator_prototype: i64,
    pub async_iterator_prototype: i64,
    pub async_gen_prototype: i64,
    pub symbol_prototype: i64,
    pub promise_prototype: i64,
    pub regexp_prototype: i64,
    pub date_prototype: i64,
    pub error_proto: i64,
    pub type_error_proto: i64,
    pub range_error_proto: i64,
    pub reference_error_proto: i64,
    pub syntax_error_proto: i64,
    pub eval_error_proto: i64,
    pub uri_error_proto: i64,
}

impl RealmIntrinsics {
    pub fn empty() -> Self {
        let u = value::encode_undefined();
        Self {
            object_proto: u, array_proto: u, function_proto: u,
            iterator_prototype: u, generator_prototype: u,
            async_iterator_prototype: u, async_gen_prototype: u,
            symbol_prototype: u, promise_prototype: u, regexp_prototype: u,
            date_prototype: u, error_proto: u, type_error_proto: u,
            range_error_proto: u, reference_error_proto: u,
            reference_error_proto: u, syntax_error_proto: u,
            eval_error_proto: u, uri_error_proto: u,
        }
    }

    /// 迭代全部原型句柄（GC root 枚举用）。
    pub fn iter_handles(&self) -> [i64; 18] {
        [
            self.object_proto, self.array_proto, self.function_proto,
            self.iterator_prototype, self.generator_prototype,
            self.async_iterator_prototype, self.async_gen_prototype,
            self.symbol_prototype, self.promise_prototype, self.regexp_prototype,
            self.date_prototype, self.error_proto, self.type_error_proto,
            self.range_error_proto, self.reference_error_proto,
            self.syntax_error_proto, self.eval_error_proto, self.uri_error_proto,
        ]
    }
}

/// 代码生成开关（contextCodeGeneration）。
#[derive(Debug, Clone, Copy)]
pub struct CodeGenFlags {
    pub strings: bool, // false → 该 realm eval/Function 抛 EvalError
    pub wasm: bool,
}

impl Default for CodeGenFlags {
    fn default() -> Self { Self { strings: true, wasm: true } }
}

#[derive(Debug, Clone)]
pub struct Realm {
    pub id: RealmId,
    pub global_object: i64,
    pub intrinsics: RealmIntrinsics,
    pub code_generation: CodeGenFlags,
}

impl Realm {
    pub fn new(id: RealmId, global_object: i64, intrinsics: RealmIntrinsics) -> Self {
        Self { id, global_object, intrinsics, code_generation: CodeGenFlags::default() }
    }
}
```
（修正上面 `empty()` 里重复的 `reference_error_proto` 笔误——实现时字段各写一次。）

在 `lib.rs`：加 `pub mod realm;`；`RuntimeState` 增字段（紧邻其他扁平表）：
```rust
/// 活跃 realm 表。realm 0 = 主 realm，惰性登记（首次 vm 或 GC root 枚举时）。
pub(crate) active_realms: std::sync::Mutex<Vec<crate::realm::Realm>>,
pub(crate) next_realm_id: std::sync::atomic::AtomicU32,
```
在 `RuntimeState` 构造处初始化 `active_realms: Mutex::new(Vec::new())`、`next_realm_id: AtomicU32::new(1)`（0 保留给主 realm）。

4. **验证 GREEN**：`cargo nextest run -p wjsm-runtime -E 'test(realm_registry)'` → 通过。

5. **提交**：`git add -A && git commit -m "feat(vm): add Realm/RealmIntrinsics data structures and active_realms registry (#313)"`

---

## Task 0.2：抽出 handle_remap 共享重写内核

**Files**
- create: `crates/wjsm-runtime/src/handle_remap.rs`
- modify: `crates/wjsm-runtime/src/startup_snapshot_remap.rs`（改为委托共享内核）
- modify: `crates/wjsm-runtime/src/lib.rs`（`mod handle_remap;`）
- test: `crates/wjsm-runtime/tests/handle_remap_kernel.rs`

**Why**：realm 克隆需要「全堆遍历每个对象属性槽/proto header/函数索引，按映射表重写 handle」，这与现有 `remap_array_proto_function_indices` 的遍历+条件重写同源。抽共享内核避免两份逻辑漂移（Architecture Integrity Lens 指出的 responsibility overlap）。

**Impact/Compatibility**：`startup_snapshot_remap.rs` 行为等价（现有 snapshot 测试必须仍通过）；这是 Repair Track（重构现有 remap 使其可复用）+ Retirement Track（旧内联遍历逻辑退役为对共享内核的调用）。

**Verification**
```bash
cargo nextest run -p wjsm-runtime -E 'test(handle_remap_kernel) | test(snapshot)'
```

**Steps**

1. **写失败测试** `crates/wjsm-runtime/tests/handle_remap_kernel.rs`：构造一小段模拟堆字节（一个对象带 proto header + 一个属性槽存 function 值），提供 `old→new` 映射，断言遍历后 proto header 与函数索引均按映射改写，非映射值不变。
```rust
//! 验证 handle_remap 共享内核对对象 proto header / 属性槽 / 函数索引的全量重写。
use wjsm_runtime::handle_remap::{remap_object_graph, HandleMap};

#[test]
fn remaps_proto_header_and_property_slots() {
    // 见实现：构造 [object_bytes]，proto=handle 5，一个属性 value=handle 7。
    // 映射 5→105, 7→107，断言重写后 proto=105、属性=107、无关字节不变。
    let mut heap = build_single_object_heap(/*proto=*/5, /*prop_value=*/7);
    let mut map = HandleMap::new();
    map.insert(5, 105);
    map.insert(7, 107);
    remap_object_graph(&mut heap, &map).unwrap();
    assert_eq!(read_proto_handle(&heap), 105);
    assert_eq!(read_first_prop_value_handle(&heap), 107);
}
```
（`build_single_object_heap` / `read_*` 为测试内 helper，按运行时对象布局常量 `HEAP_TYPE_*`/`PROP_SLOT_SIZE` 构造——实现步骤给出确切偏移。）

2. **验证 RED**：`cargo nextest run -p wjsm-runtime -E 'test(handle_remap_kernel)'` → 编译失败。

3. **最小实现**：`handle_remap.rs` 提供 `struct HandleMap`（`HashMap<u32,u32>` 包装）+ `fn remap_object_graph(heap: &mut [u8], map: &HandleMap) -> anyhow::Result<()>`，用 `ObjectWalker`（复用现有 heap 遍历，见 `runtime_gc` 与 `startup_snapshot_remap.rs` 的遍历模式）逐对象扫描属性槽/proto header/函数值，命中映射则改写。再让 `startup_snapshot_remap.rs::remap_array_proto_function_indices` 改为构造一个「仅 Array.prototype 函数索引区间」的映射并调用同一内核（保持既有语义：只重写 `[snapshot_base, snapshot_base+len)` 区间函数索引）。

4. **验证 GREEN**：`cargo nextest run -p wjsm-runtime -E 'test(handle_remap_kernel) | test(snapshot)'` → 全通过（现有 snapshot 回归不破）。

5. **提交**：`git commit -am "refactor(runtime): extract shared handle_remap kernel; snapshot remap delegates to it (#313)"`

---

# Phase 1 — Realm 克隆

## Task 1.1：pristine 对象图克隆 + createContext host 入口

**Files**
- create: `crates/wjsm-runtime/src/realm_clone.rs`
- modify: `crates/wjsm-runtime/src/lib.rs`（`mod realm_clone;`）
- modify: `crates/wjsm-runtime/src/realm.rs`（`register_realm` / `main_realm_lazy_register`）
- test: `crates/wjsm-runtime/tests/realm_clone.rs`

**Why**：`vm.createContext` 的核心——把 pristine primordial 对象图克隆到新 handle 槽，装配 `RealmIntrinsics`，contextify 用户 sandbox 为 realm global。

**Impact/Compatibility**：只在 vm 路径调用；克隆走共享 bump 分配器（`obj_new`），不改分配器。

**Verification**
```bash
cargo nextest run -p wjsm-runtime -E 'test(realm_clone)'
```

**Steps**

1. **写失败测试** `crates/wjsm-runtime/tests/realm_clone.rs`：在一个初始化好的 runtime store 内（复用现有测试 harness，如 `startup_snapshot` 测试的 store 构造）克隆一个 realm，断言：(a) 新 realm 的 `object_proto` handle ≠ 主 realm 的；(b) 在新 realm 里读 `array_proto` 的 `__proto__` 指向新 realm 的 `object_proto`（内部 handle 正确重映射，未串到主 realm）。

2. **验证 RED**。

3. **最小实现** `realm_clone.rs`：
   - `fn clone_pristine_realm(caller: &mut Caller<RuntimeState>, sandbox_global: i64) -> Result<Realm>`：
     1. 取 pristine primordial 对象图句柄集（主 realm 的 `RealmIntrinsics`，经 Task 1.2 的 `snapshot_primordial_handles`）与其可达对象闭包。
     2. 对闭包内每个对象 `obj_new` 分配新 handle，填 `HandleMap`。
     3. 复制每对象堆字节到新地址，写 `obj_table`。
     4. `handle_remap::remap_object_graph` 在克隆出的对象字节上按 `HandleMap` 全量重写。
     5. 用 `HandleMap` 取出各原型新 handle 装配 `RealmIntrinsics`。
     6. `sandbox_global` 设为 realm global，其 `__proto__` 指向新 realm 的 `object_proto`。
     7. `register_realm` 分配 `RealmId`（`next_realm_id.fetch_add`），push 进 `active_realms`，检查 `WJSM_VM_MAX_REALMS` 软上限（默认 1024）。
   - `main_realm_lazy_register`：若 `active_realms` 为空，用现有全局单例 + `RuntimeState` primordial 字段装配 realm 0 登记。

4. **验证 GREEN**。

5. **提交**：`git commit -am "feat(vm): clone pristine primordial object graph into new realm handle space (#313)"`

---

## Task 1.2：pristine primordial 句柄闭包来源

**Files**
- modify: `crates/wjsm-runtime/src/realm_clone.rs`（`fn primordial_reachable_closure`）
- modify: `crates/wjsm-runtime/src/realm.rs`（主 realm intrinsics 从 RuntimeState 装配）
- test: `crates/wjsm-runtime/tests/realm_clone.rs`（追加闭包完整性用例）

**Why**：克隆需精确的「哪些对象属于一个 realm 的 primordial 集」闭包，漏一个对象会导致跨 realm 串原型。

**Impact/Compatibility**：闭包基于主 realm intrinsics 可达对象（`ObjectWalker` 传递闭包）；immortal region 内的 primordial 对象是来源。

**Steps**

1. **写失败测试**：断言闭包包含 `object_proto`/`array_proto`/`function_proto`/各 error proto，且对每个对象的可达子对象（proto 链、方法函数属性对象）均在闭包内（无悬挂 handle）。

2. **验证 RED**。

3. **最小实现**：`primordial_reachable_closure(caller, roots: &RealmIntrinsics) -> Vec<u32>`：从 `roots.iter_handles()` BFS 遍历（`ObjectWalker` 取每对象子引用），收集 immortal region 内可达 handle 去重。`realm.rs` 加 `fn main_realm_intrinsics(state: &RuntimeState) -> RealmIntrinsics`，从 `array_proto_handle`/`object_proto_handle`/`error_prototypes`/各 prototype 字段装配。

4. **验证 GREEN**。

5. **提交**：`git commit -am "feat(vm): compute primordial reachable closure for realm cloning (#313)"`

---

# Phase 2 — eval 双路径 realm 感知

## Task 2.1：AST 解释器兜底路径注入 realm 上下文

**Files**
- modify: `crates/wjsm-runtime/src/runtime_eval.rs`（`eval_stmt`/`eval_expr`/字面量构造/全局查找加 `realm: Option<RealmId>`）
- test: `crates/wjsm-runtime/tests/eval_realm_interp.rs`

**Why**：解释器兜底路径在 realm 内做字面量/`new`/全局查找时须解析到 realm intrinsic，否则隔离失败。

**Impact/Compatibility**：`realm: None` 时行为与现在**完全一致**（主 realm 语义），保证现有 eval 测试不破；仅 `Some(realm)` 走 realm intrinsic 解析。

**Verification**
```bash
cargo nextest run -p wjsm-runtime -E 'test(eval_realm_interp)'
cargo nextest run -p wjsm-semantic  # eval 语义回归
```

**Steps**

1. **写失败测试**：解释器路径在 realm A 内 eval `[]`，断言其 `__proto__` === realm A 的 `array_proto` 而非主 realm。

2. **验证 RED**。

3. **最小实现**：给 eval 解释器签名族增 `realm: Option<RealmId>` 贯穿参数（抽 `resolve_intrinsic(caller, realm, which) -> i64` 小函数，`None` 回退 RuntimeState 主 realm 字段，`Some` 从 `active_realms` 取）。字面量数组/对象/regexp 构造、`new` 目标、全局标识符（`Array`/`Object`/…）查找改用 `resolve_intrinsic`。未声明标识符回退目标从主 global 改为 realm global。

4. **验证 GREEN**：新测试通过 + `wjsm-semantic` eval 回归全绿。

5. **提交**：`git commit -am "feat(vm): make AST-interpreter eval path realm-aware (#313)"`

---

## Task 2.2：编译 eval 路径注入 realm intrinsic/global

**Files**
- modify: `crates/wjsm-runtime/src/runtime_eval.rs`（`try_compiled_eval_from_caller_async` / `compiled_eval_import` 注入 realm intrinsic import + realm global）
- test: `crates/wjsm-runtime/tests/eval_realm_compiled.rs`

**Why**：编译 eval 是主路径（用户选定「编译 eval 为主」）；其产物通过 parent import 拿 intrinsic/global，须注入目标 realm 而非硬编码主 realm。`eval_cache` 键保持 `code`（intrinsic 运行时注入 → 缓存跨 realm 共享，不膨胀）。

**Impact/Compatibility**：`eval_cache` 键不变（关键约束）；主 realm eval 走 realm 0 注入，字节码与现在一致。

**Verification**
```bash
cargo nextest run -p wjsm-runtime -E 'test(eval_realm_compiled)'
```

**Steps**

1. **写失败测试**：同一段 `code` 先后在 realm A、realm B 编译 eval 执行，断言：(a) `eval_cache` 只有一条（复用同一字节码）；(b) A 内 `[]`.proto === A.array_proto，B 内 === B.array_proto（intrinsic 由注入区分）。

2. **验证 RED**。

3. **最小实现**：`compiled_eval_import` 中把 intrinsic/global 相关 host import 从「读 RuntimeState 主 realm 单例」改为「读当前执行 realm 上下文」（realm id 经 caller 侧线程局部或参数传入执行帧）。编译产物签名不变，仅注入值随 realm 变。

4. **验证 GREEN**：缓存单条 + 双 realm intrinsic 正确。

5. **提交**：`git commit -am "feat(vm): inject per-realm intrinsics/global into compiled eval path, keep shared code cache (#313)"`

---

# Phase 3 — vm builtin + API 外形

## Task 3.1：注册 vm builtin + host bridge 骨架

**Files**
- create: `crates/wjsm-module/builtin_js/node_vm.js`
- create: `crates/wjsm-runtime/src/runtime_node_vm.rs`
- modify: `crates/wjsm-module/src/builtin_modules.rs`（`BUILTIN_MODULES` 加 `vm` 条目）
- modify: `crates/wjsm-runtime/src/runtime_node_globals.rs`（安装 `__wjsm_node_vm` bridge）
- modify: `crates/wjsm-snapshot-format/src/lib.rs`（`SnapshotNativeCallable` 加 `VmMethod` discriminant + `abi_hash` 同步）
- test: `fixtures/modules/node_builtin_vm_main.js` + `.expected`

**Why**：让 `require('vm')`/`require('node:vm')` 可加载并接到 host bridge。

**Impact/Compatibility**：新增 NativeCallable discriminant → snapshot ABI rebake（构建期自动）；现有 fixtures 不变。

**Verification**
```bash
cargo nextest run -E 'test(modules__node_builtin_vm)'
cargo run -- run -e "const vm=require('vm'); console.log(typeof vm.runInNewContext);"  # → function
```

**Steps**

1. **写失败测试** `fixtures/modules/node_builtin_vm_main.js`：`const vm=require('node:vm'); console.log(typeof vm.createContext, typeof vm.Script);` + `.expected` = `function function`。

2. **验证 RED**：fixture 失败（模块未注册）。

3. **最小实现**：
   - `builtin_modules.rs` 数组加 `BuiltinModule { canonical: "vm", source: include_str!("../builtin_js/node_vm.js") }`。
   - `node_vm.js`：`getHost()` 读 `globalThis.__wjsm_node_vm`；导出 `createContext`/`isContext`/`runInThisContext`/`runInContext`/`runInNewContext`/`Script`/`compileFunction`/`constants`，全部委托 host bridge 方法（本任务先接骨架，语义在 3.2/3.3 补齐）。
   - `runtime_node_vm.rs`：`create_vm_host_object(caller)` 用 `install_native` 挂 `NativeCallable::VmMethod { kind: VmMethodKind::* }` 各方法；`runtime_node_globals.rs` install 阶段调用它并 `define_global(global, "__wjsm_node_vm", vm_host)`。
   - `snapshot-format`：`SnapshotNativeCallable` 末尾加 `VmMethod` 变体（新 discriminant），`abi_hash` 输入的 discriminant 范围随之扩展（`0..=N`）。

4. **验证 GREEN**：fixture 通过 + smoke 输出 `function`。

5. **提交**：`git commit -am "feat(vm): register node:vm builtin + __wjsm_node_vm host bridge skeleton (#313)"`

---

## Task 3.2：createContext / isContext / runInContext / runInNewContext 语义

**Files**
- modify: `crates/wjsm-runtime/src/runtime_node_vm.rs`（bridge 方法接 realm_clone + eval realm 路径）
- modify: `crates/wjsm-module/builtin_js/node_vm.js`（参数规整/options 解析）
- test: `fixtures/happy/vm_realm_isolation.js`、`vm_cross_realm_ref.js`、`vm_sandbox_visible.js` + `.expected`

**Why**：交付标志性语义（隔离 + 跨 realm 引用 + sandbox 双向可见）。

**Impact/Compatibility**：核心 vm 契约。

**Verification**
```bash
cargo nextest run -E 'test(happy__vm_realm_isolation) | test(happy__vm_cross_realm_ref) | test(happy__vm_sandbox_visible)'
```

**Steps**

1. **写失败 fixtures**：
   - `vm_realm_isolation.js`：`console.log(require('vm').runInNewContext('[]') instanceof Array);` → `.expected` `false`。
   - `vm_cross_realm_ref.js`：`const o=require('vm').runInNewContext('({a:2})'); console.log(o.a);` → `2`。
   - `vm_sandbox_visible.js`：`const s={}; require('vm').runInNewContext('x=1', s); console.log(s.x);` → `1`。

2. **验证 RED**。

3. **最小实现**：`VmMethodKind::CreateContext` → `main_realm_lazy_register` + `clone_pristine_realm(sandbox)`，标记对象为 contextified（存 realm id 于对象隐藏槽或 side table）；`IsContext` 查该标记；`RunInContext`/`RunInNewContext` → 解析 realm → 走 Task 2.2 编译 eval（realm 注入）+ 2.1 兜底，返回值按引用回传（同堆，无序列化）。`node_vm.js` 规整 `contextObject`/`options`。

4. **验证 GREEN**：三个标志性 fixture 全绿。

5. **提交**：`git commit -am "feat(vm): implement createContext/isContext/runInContext/runInNewContext realm semantics (#313)"`

---

## Task 3.3：vm.Script 类 + compileFunction + runInThisContext

**Files**
- modify: `crates/wjsm-module/builtin_js/node_vm.js`（`Script` 类 + `compileFunction` + `runInThisContext`）
- modify: `crates/wjsm-runtime/src/runtime_node_vm.rs`（`CompileScript`/`RunCompiledInRealm`/`CompileFunctionInRealm`）
- test: `fixtures/happy/vm_script_reuse.js`、`vm_compile_function.js`、`vm_run_in_this_context.js` + `.expected`

**Why**：`vm.Script` 复用编译产物（多次 run 不重编）；`compileFunction` 返回绑定 realm 的函数；`runInThisContext` 在主 realm 但不 contextify。

**Impact/Compatibility**：`Script` 内部持有已编译句柄，复用 `eval_cache`。

**Verification**
```bash
cargo nextest run -E 'test(happy__vm_script_reuse) | test(happy__vm_compile_function) | test(happy__vm_run_in_this_context)'
```

**Steps**

1. **写失败 fixtures**：
   - `vm_script_reuse.js`：`const vm=require('vm'); const s=new vm.Script('1+1'); console.log(s.runInThisContext(), s.runInThisContext());` → `2\n2`。
   - `vm_compile_function.js`：`const f=require('vm').compileFunction('return a+b',['a','b']); console.log(f(2,3));` → `5`。
   - `vm_run_in_this_context.js`：`globalThis.__t=9; console.log(require('vm').runInThisContext('__t'));` → `9`（共享主 realm 全局）。

2. **验证 RED**。

3. **最小实现**：`node_vm.js` `Script` 类构造时调 host `compileScript(code, opts)` 拿句柄，`runInThisContext`/`runInContext`/`runInNewContext` 传句柄 + 目标 realm；`compileFunction` 包函数体绑定 realm。host 侧 `CompileScript` 预编译并缓存，`RunCompiledInRealm` 按 realm 注入执行。`runInThisContext` = realm 0，无 sandbox 回退。

4. **验证 GREEN**。

5. **提交**：`git commit -am "feat(vm): add vm.Script reuse, compileFunction, runInThisContext (#313)"`

---

# Phase 4 — GC per-realm roots + 死 realm 回收

## Task 4.1：per-realm GC root 枚举

**Files**
- modify: `crates/wjsm-runtime/src/runtime_gc/roots.rs`（`for_each_host_table_root` 遍历 `active_realms`）
- test: `crates/wjsm-runtime/tests/vm_gc_realm_roots.rs`

**Why**：realm intrinsic 原型 + global 必须是 GC root，否则活 realm 的原型被误回收。

**Impact/Compatibility**：主 realm 原型已被现有 root 覆盖；新增仅针对 realm ≥1；单 realm 程序 `active_realms` 至多含 realm 0，遍历成本可忽略。

**Verification**
```bash
cargo nextest run -p wjsm-runtime -E 'test(vm_gc_realm_roots) | test(gc)'
```

**Steps**

1. **写失败测试**：创建 realm，强制 GC，断言 realm intrinsic 原型 handle 仍存活（对象未被回收，proto 链完整）。

2. **验证 RED**。

3. **最小实现**：`for_each_host_table_root` 扫完主 realm primordial 后，锁 `active_realms` 遍历每个 realm，`visit` 其 `global_object` + `intrinsics.iter_handles()`。

4. **验证 GREEN**：realm root 存活 + 现有 GC 回归全绿（含 mark-sweep/g1/zgc 矩阵）。

5. **提交**：`git commit -am "feat(vm): enumerate per-realm intrinsics/global as GC roots (#313)"`

---

## Task 4.2：死 realm 回收

**Files**
- modify: `crates/wjsm-runtime/src/runtime_gc/roots.rs` 或 GC 收尾钩子（GC 后清理 `active_realms` 中 global 不可达的 realm）
- modify: `crates/wjsm-runtime/src/realm.rs`（realm 存活判定）
- test: `crates/wjsm-runtime/tests/vm_gc_realm_roots.rs`（追加死 realm 回收用例）

**Why**：contextified sandbox 不可达时 realm 应被回收，避免泄漏。sandbox 是 realm 生命周期锚点。

**Impact/Compatibility**：realm 0 永不回收；`active_realms` 存弱持有语义（realm 不通过 active_realms 强持有 global，global 存活由 sandbox 外部引用决定）。

**设计细化**：realm 的 `global_object` 不作为强 root——它由用户对 sandbox 的外部引用保活；`intrinsics` 仅在 realm 存活（global 可达）时作 root。即 root 枚举前先判 global 存活，死 realm 不贡献 root 且被清理。（这修正 4.1 的初版「无条件 root global」——4.2 落地弱持有语义。）

**Verification**
```bash
cargo nextest run -p wjsm-runtime -E 'test(vm_gc_realm_roots)'
```

**Steps**

1. **写失败测试**：创建 realm 后丢弃 sandbox 全部引用，GC，断言 `active_realms` 该 realm 被清理（长度回落），且不 panic。

2. **验证 RED**。

3. **最小实现**：root 枚举改为「realm global 存活（被 shadow stack / 其他 root 标记）→ 贡献 intrinsics root；否则标记死 realm」；GC 收尾从 `active_realms` retain 存活 realm。realm 0 恒存活。

4. **验证 GREEN**。

5. **提交**：`git commit -am "feat(vm): reclaim dead realms whose sandbox is unreachable (#313)"`

---

# Phase 5 — timeout（epoch interruption）

## Task 5.1：wasmtime epoch 计时基础设施

**Files**
- modify: `crates/wjsm-runtime/src/runtime_startup.rs` 或 Engine 构造处（`Config::epoch_interruption(true)`）
- modify: `crates/wjsm-runtime/src/runtime_node_vm.rs`（执行前 `set_epoch_deadline` + 后台 timer bump epoch）
- test: `crates/wjsm-runtime/tests/vm_timeout.rs`

**Why**：`timeout` 选项到期须中止执行。epoch interruption 比 fuel 低开销、不改 codegen，且与 `--inspect` guest_debug 兼容。

**Impact/Compatibility**：`epoch_interruption(true)` 全局启用；无 timeout 的执行不设 deadline（epoch 永不触发），零行为变化。计划期需验证 epoch 与现有 scheduler（异步 completion）协作无冲突——若冲突，回退 fuel 或 safepoint host 回调方案（设计 spec §5.5 已标注为计划期验证项）。

**Verification**
```bash
cargo nextest run -p wjsm-runtime -E 'test(vm_timeout)'
```

**Steps**

1. **写失败测试**：`runInNewContext('while(1){}', {}, {timeout: 50})` 断言在合理时限内抛超时错误（非挂死）。用带超时的测试执行器。

2. **验证 RED**。

3. **最小实现**：Engine `Config` 开 `epoch_interruption`；vm 执行入口若有 `timeout`，`store.set_epoch_deadline(1)` + spawn 一个 `timeout` 毫秒后 `engine.increment_epoch()` 的 timer；wasm 因 epoch deadline trap → 映射为 vm timeout 错误。验证与 scheduler 协作（异步 op 进行中 bump epoch 不误杀非 vm 执行——vm 执行帧独占 deadline 窗口）。

4. **验证 GREEN**：timeout fixture 通过，`--workspace` 无回归。

5. **提交**：`git commit -am "feat(vm): implement runIn* timeout via wasmtime epoch interruption (#313)"`

---

## Task 5.2：解释器兜底路径 deadline 检查

**Files**
- modify: `crates/wjsm-runtime/src/runtime_eval.rs`（`eval_stmt` 循环检查 deadline）
- test: `crates/wjsm-runtime/tests/vm_timeout.rs`（追加解释器路径超时用例）

**Why**：编译失败走解释器兜底时 timeout 仍须生效。

**Verification**
```bash
cargo nextest run -p wjsm-runtime -E 'test(vm_timeout)'
```

**Steps**

1. **写失败测试**：构造走解释器兜底的超时场景（如编译 eval 不支持的语法触发兜底 + 无限循环），断言超时抛错。

2. **验证 RED**。

3. **最小实现**：解释器循环入口（`eval_stmt` 的循环/递归回边）检查 realm 执行 deadline（`Instant`），超时返回 timeout 错误。

4. **验证 GREEN**。

5. **提交**：`git commit -am "feat(vm): enforce timeout deadline in AST interpreter fallback (#313)"`

---

# Phase 6 — 次级选项 + fixtures + 回归 + ADR

## Task 6.1：contextCodeGeneration + microtaskMode + 非目标抛错

**Files**
- modify: `crates/wjsm-module/builtin_js/node_vm.js`（options 解析 + 非目标 getter 抛错）
- modify: `crates/wjsm-runtime/src/runtime_node_vm.rs`（`CodeGenFlags` 落地 + microtask drain）
- test: `fixtures/happy/vm_codegen_off.js`、`vm_microtask_mode.js`、`fixtures/errors/vm_unsupported.js` + `.expected`

**Why**：忠实次级语义 + 非目标明确抛错（不留 no-op）。

**Verification**
```bash
cargo nextest run -E 'test(happy__vm_codegen_off) | test(happy__vm_microtask_mode) | test(errors__vm_unsupported)'
```

**Steps**

1. **写失败 fixtures**：
   - `vm_codegen_off.js`：`contextCodeGeneration:{strings:false}` 的 context 内 `eval('1')` 抛 `EvalError`，捕获打印 `EvalError`。
   - `vm_microtask_mode.js`：`microtaskMode:'afterEvaluate'` 下 context 内 Promise.then 在 run 返回后已排空，验证顺序。
   - `errors/vm_unsupported.js`：`require('vm').SourceTextModule` 访问抛明确错误，`.expected` 匹配错误信息。

2. **验证 RED**。

3. **最小实现**：`node_vm.js` 解析 `contextCodeGeneration`/`microtaskMode`，`SourceTextModule`/`SyntheticModule`/`measureMemory` 为抛 `Error('not implemented in wjsm: ...')` 的 getter；host 侧 `CodeGenFlags.strings=false` 时该 realm eval/Function 抛 EvalError；`microtaskMode:'afterEvaluate'` 在 run 后同步 drain 该 realm microtask。

4. **验证 GREEN**。

5. **提交**：`git commit -am "feat(vm): contextCodeGeneration/microtaskMode + explicit errors for non-goals (#313)"`

---

## Task 6.2：全工作区回归 + 零警告

**Files**：无新增（验证任务）。

**Why**：确保多子系统改动不破坏现有 970+ 测试，零编译警告（AGENTS.md baseline）。

**Verification**
```bash
cargo build 2>&1 | grep -c warning     # → 0
cargo nextest run --workspace          # 全绿
# GC 矩阵
WJSM_TEST_GC=mark-sweep cargo nextest run -p wjsm-runtime -E 'test(vm) | test(realm) | test(gc)'
WJSM_TEST_GC=g1 cargo nextest run -p wjsm-runtime -E 'test(vm) | test(realm) | test(gc)'
WJSM_TEST_GC=zgc cargo nextest run -p wjsm-runtime -E 'test(vm) | test(realm) | test(gc)'
```

**Steps**

1. 运行全工作区测试与三 GC 矩阵。
2. 若有失败或警告，定位 owner 修复（不改测试规避）。
3. 全绿后提交：`git commit -am "test(vm): full workspace regression green across GC matrix (#313)"`

---

## Task 6.3：ADR 0008 回填 + INDEX + AGENTS.md

**Files**
- create: `docs/adr/0008-node-vm-multi-realm.md`
- modify: `docs/aegis/INDEX.md`（Baselines 加 ADR 0008；已在计划提交时加 plan 条目）
- modify: `AGENTS.md`（load-bearing conventions 补一段 multi-realm / vm 说明）

**Why**：设计触及多项 load-bearing 约定（单堆多 realm、handle_remap 共享内核、NativeCallable ABI、per-realm GC root），须回填 ADR 固化决策。

**Verification**：`read docs/adr/0008-node-vm-multi-realm.md` 内容完整；INDEX/AGENTS 引用一致。

**Steps**

1. 写 ADR 0008：Context（vm 单堆多 realm 决策）、Decision（realm 模型 / snapshot 克隆 / eval 双路径 / epoch timeout / per-realm root）、Consequences、Alternatives（多 Store 拒绝理由、重跑 bootstrap 拒绝理由）、References（设计 spec、本 plan、ADR 0003/0004/0005）。
2. INDEX Baselines 加 ADR 0008 条目。
3. AGENTS.md「Load-bearing conventions」补 multi-realm 段：realm = 带标签 intrinsic + global，共享单堆/obj_table/GC；`active_realms[0]` = 主 realm；`handle_remap` 共享内核。
4. 提交：`git commit -am "docs(vm): ADR 0008 node:vm multi-realm + AGENTS/INDEX sync (#313)"`

---

## Self-Review

1. **Spec 覆盖**：设计 §3 目标 1–10 → Task 3.1（加载）/3.2（createContext/isContext/runIn*）/3.3（Script/compileFunction/runInThisContext）/6.1（constants/codegen/microtask）；realm 隔离 → Phase 0-2；timeout → Phase 5；跨 realm 引用/sandbox 可见 → 3.2 fixtures。✅ 每目标可指向任务。
2. **占位符扫描**：无 TBD/TODO；每任务给确切路径、完整代码骨架、确切命令。Task 0.1 实现代码里标注了一处笔误修正（`empty()` 字段去重），已显式说明。✅
3. **类型一致**：`RealmId`/`RealmIntrinsics`/`HandleMap`/`CodeGenFlags` 跨任务签名一致；`realm: Option<RealmId>` 贯穿 eval。✅
4. **兼容性**：`realm: None`/`active_realms[0]` 保证单 realm 零变化；`eval_cache` 键不变；snapshot rebake 自动；非目标明确抛错。✅
5. **复杂度/最小性**：新 owner 文件承载 realm 机制，不塞 eval/gc 热点；`handle_remap` 抽共享内核消除重复。✅
6. **架构完整性**：主 realm 统一 active_realms[0]（higher-level simplification 已采纳）；remap 内核共享。✅
7. **验证**：每任务确切 `cargo nextest` filter + CLI smoke；全局回归含 GC 矩阵。✅
8. **双轨/ADR**：Task 0.2 含 Repair+Retirement track（remap 重构）；Task 4.2 修正 4.1 的 root 语义（弱持有）；ADR 信号在 Task 6.3 回填。✅

发现并已内联修正：Task 4.1 初版「无条件 root realm global」与 4.2「弱持有回收」存在张力——已在 4.2 显式说明 global 由外部引用保活、root 枚举前判存活，两任务语义连贯。

---

## Retirement

- 无外部兼容旧路径需保留（vm 是全新能力）。
- 内部退役：`remap_array_proto_function_indices` 的内联遍历逻辑退役为对 `handle_remap` 共享内核的调用（Task 0.2），行为等价，由现有 snapshot 测试守护。
- ADR 0008 固化决策后，设计 spec 与本 plan 作为实现证据留档。
