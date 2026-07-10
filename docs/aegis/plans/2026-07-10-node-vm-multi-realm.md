# node:vm 多 Realm 沙箱实现计划

- 日期：2026-07-10
- 状态：已按用户授权（无需审批 spec）进入实现；**2026-07-10 代码对照审查后修正**
- 关联 issue：#313（issue 正文原将「通用 `vm.runInNewContext`」列为**非目标**；本轮用户明确要求完整落地，作为对该非目标的**范围覆盖**，非 Inspector/net 既有条目的替代）
- 设计 spec：`docs/aegis/specs/2026-07-10-node-vm-multi-realm-design.md`（与本 plan 冲突处**以本 plan 审查修正为准**，实现后回填 design + ADR）

## Goal

在 wjsm 中完整、忠实地实现 `node:vm` 核心稳定全集 + `timeout`，采用「单堆多 realm」模型：所有 realm 共享同一 wasmtime Store / 线性内存 / `obj_table` / GC，每个 realm 拥有独立 intrinsics（`[] instanceof Array === false`），对象跨 realm 按引用自由流动。Realm 诞生走「从主 realm pristine 可达对象图克隆 + 全量 NaN-box handle 重映射」，eval 双路径（编译 eval 为主、AST 解释器兜底）均 realm 感知；**对象分配 / 构造器路径通过 `RuntimeState.execution_realm` 统一解析 intrinsic，而非 TLS**。

## Architecture

```
require('node:vm')
  → builtin_modules.rs 注册 canonical "vm"
  → node_vm.js（API 外形：Script / runIn* / compileFunction / constants）
  → __wjsm_node_vm host bridge（createContext / runInRealm / isContext / compileFunctionInRealm）
      → realm.rs（Realm / RealmIntrinsics / RealmId / active_realms / execution_realm）
      → realm_clone.rs（pristine 可达图克隆 + HandleMap 装配）
          → handle_remap.rs（共享对象图 walker + 可插拔 RemapPolicy）
              ← startup_snapshot_remap.rs（FuncTableIndexRangePolicy）
              ← realm_clone.rs（ObjectHandleMapPolicy）
      → runtime_eval.rs（eval 双路径 + 进入/退出 realm 执行帧）
      → runtime_builtins / support arr_new·obj_new（读 execution_realm 解析 proto）
      → runtime_gc/roots.rs（per-realm 条件 root + 死 realm 回收）
```

主 realm = `active_realms[0]`（现有全局单例惰性登记）。执行帧内 `execution_realm: AtomicU32`（或 `Cell`/`Mutex` 一致风格）指示当前 intrinsic 解析目标；默认 0。

## Tech Stack

- Rust 2024，`swc_core` 解析、`wasm-encoder` codegen、`wasmtime` 执行。
- Realm 数据结构存 `RuntimeState`（扁平字段，遵 ADR 0002）。
- Timeout：**不**把现有 async-yield epoch 路径直接改成 trap 超时（见 Phase 5）；编译路径优先 fuel 或「vm 帧内临时切换 epoch 回调为 trap + 后台 bump」，解释器路径用 `Instant` deadline。
- 测试：`cargo nextest`，fixtures（`fixtures/happy` / `fixtures/errors` / `fixtures/modules` + `.expected`）+ crate 单测。

## Baseline / Authority Refs

- issue #313（vm 原为非目标；本轮覆盖该非目标。Inspector 已由 ADR 0007 落地，与本 plan 正交）。
- ADR 0002（RuntimeState 扁平）、0003（snapshot 边界/重定位规则）、0004（build-time embedded runtime / abi_hash 输入）、0005（pluggable GC root provider）。
- 设计 spec §2.1–§2.3 架构判断；**下列审查结论覆盖 design 中与源码不符的细节**。
- 源码 owner：`runtime_eval.rs`、`startup_snapshot.rs`/`startup_snapshot_remap.rs`、`runtime_gc/roots.rs` + `object_walker.rs`、`host_imports/collections_buffers.rs`、`support_module.rs`（`arr_new`/`obj_new` 读 `__array_proto_handle`/`__object_proto_handle`）、`snapshot-format/src/lib.rs`、`builtin_modules.rs`、`runtime_node_globals.rs`、`types.rs`（`NativeCallable`）、`runtime_startup.rs`（已 `epoch_interruption(true)` + `epoch_deadline_async_yield_and_update`）。

## Compatibility Boundary

- 单 realm 程序行为与开销**完全不变**（主 realm = `active_realms[0]` 惰性登记，非 vm 程序不进克隆路径；`execution_realm` 恒为 0 时分配路径与现网一致）。
- 现有全部 fixture `.expected` 输出不变。
- `RuntimeState` 保持扁平（仅新增字段，不嵌套）。
- snapshot ABI 因新增 `NativeCallable::VmMethod` 而 rebake（构建期自动，`abi_hash` 同步）。
- 非目标（明确抛错，不留 no-op）：`SourceTextModule`/`SyntheticModule`、`measureMemory`、`importModuleDynamically` 完整语义、跨线程 realm、安全沙箱语义。

## Verification（全局验收）

```bash
cargo nextest run --workspace          # 全绿，零编译警告
cargo build 2>&1 | grep -c warning     # → 0
```
标志性语义 CLI smoke：`[] instanceof Array === false`、跨 realm 对象引用、sandbox 双向可见、timeout 触发、`vm.Script` 复用。

---

## BaselineUsageDraft

- Required baseline refs：issue #313、ADR 0002/0003/0004/0005、设计 spec、`runtime_eval.rs`/`startup_snapshot*.rs`/`runtime_gc/roots.rs`/`object_walker.rs`/`support_module.rs`/`snapshot-format`/`builtin_modules.rs`/`runtime_node_globals.rs`/`runtime_startup.rs`。
- Delivered context refs：AGENTS 注入、issue 正文、设计 spec、2026-07-10 源码对照审查。
- Acknowledged before plan refs：已读 ADR 0003/0004/0007；确认 `remap_array_proto_function_indices` 只改 **WASM 函数表索引**（非 object handle）；确认 compiled eval 从父模块 **import 同一批 mutable globals**（含 `__array_proto_handle`/`__object_proto_handle`）；确认 `epoch_deadline_async_yield_and_update(1)` 已占用 epoch 策略；确认 `eval_cache` 键含 `code|has_scope_bridge|var_writes_to_scope|data_base|version`（**不是**纯 `code`）。
- Cited in plan refs：以下全部任务。
- Missing refs：无阻塞；ADR 0008 待实现后回填。
- Decision：continue（审查修正后）。

## Architecture Integrity Lens

- Invariant：单 Store/单 obj_table/单 GC；realm = 带标签 intrinsic + global；Node API 外形归 `node_vm.js`；**分配/构造 intrinsic 解析唯一源 = `execution_realm` → `active_realms[id].intrinsics`（0 时读现有 WASM global / RuntimeState 字段）**。
- Canonical owner：`realm.rs`、`realm_clone.rs`、`handle_remap.rs`、`runtime_node_vm.rs`、`node_vm.js`。
- Responsibility overlap：对象图遍历共享 walker；**RemapPolicy 分叉**（函数表索引区间 vs handle 映射），禁止把两种语义揉成一个 `HandleMap`。
- Higher-level simplification：主 realm 登记 `active_realms[0]`；执行帧 `with_execution_realm` 统一 eval/host/support 语义。
- Retirement / falsifier：标志性 fixtures；experimental/measureMemory 明确抛错。
- Verdict：proceed（按修正后的注入/timeout/remap 契约）。

## Plan Pressure Test

- Owner / contract / retirement：新 owner 文件边界清晰；`remap_array_proto_function_indices` 改为 walker + `FuncTableIndexRangePolicy`，行为等价。
- Architecture integrity：主 realm 统一 + `execution_realm` 消除「只改 eval、分配仍读主 proto」的半成品路径。
- Verification scope：每阶段有 fixture/单测/CLI smoke；含「构造器/`new Array`/字面量」跨路径隔离。
- Task executability：每任务给确切路径、关键契约、命令；大段示意代码仅作形状参考，以实现时对照源码布局常量为准。
- Pressure result：proceed。

## Plan-Time Complexity Check

- Target files：新增 `realm.rs`/`realm_clone.rs`/`handle_remap.rs`/`runtime_node_vm.rs`/`node_vm.js`；改造 `runtime_eval.rs`（realm 执行帧 + 签名贯通，禁止再把 2k 行文件无序膨胀——realm 解析抽独立小函数/子模块）、`runtime_gc/roots.rs`、`snapshot-format`、`startup_snapshot_remap.rs`（变薄）、`runtime_builtins.rs` / 必要时 `support` 路径旁路、`types.rs` + native bridge。
- Owner fit：realm 机制独立模块；eval 只注入执行帧。
- Recommendation：add owner file + edit-in-place。

---

## 审查修正摘要（相对初版 plan / design）

| # | 问题 | 修正 |
|---|---|---|
| 1 | 把 `remap_array_proto_function_indices` 当成「object handle 重映射」泛化源 | 共享 **walker**；策略分 `FuncTableIndexRangePolicy`（改 `TAG_FUNCTION` 的 table idx）与 `ObjectHandleMapPolicy`（改 object/array/… NaN-box handle / proto header / accessor getter·setter） |
| 2 | compiled eval「TLS 注入 realm」 | 禁用 TLS。采用 `RuntimeState.execution_realm` + **进入 realm 时 swap/restore** 父模块 `__array_proto_handle`/`__object_proto_handle`（及解释器/`alloc_*` 读同一源）；可重入栈保存旧值 |
| 3 | `arr_new`/`ArrayConstructor`/`alloc_array` 仍读主 realm WASM global | Phase 2 强制：字面量 **与** `new Array`/`Array()`/`Object()` 在 `execution_realm≠0` 时使用该 realm intrinsics（测例覆盖） |
| 4 | timeout 直接 `epoch_interruption` + bump | epoch **已被** `epoch_deadline_async_yield_and_update` 占用；vm timeout 用 fuel 或「vm 帧内临时改 trap 回调 + 结束必恢复 async_yield」；解释器 `Instant` |
| 5 | `eval_cache`「键=code、双 realm 仅一条」 | 现网键含 `data_base` 等；断言改为「同 code+同 cache 维度命中；**字节码不编码 realm id**」；禁止错误的「全局只有一条」断言 |
| 6 | `RealmIntrinsics` 缺 `aggregate_error` 等，且 `empty()` 有重复字段笔误 | 与 `RuntimeState`/`roots.rs` 已 root 的 primordial 对齐（含 AggregateError、TypedArray 族等，见 Task 0.1） |
| 7 | Task 4.1 无条件 root global 与 4.2 弱持有矛盾 | 自 4.1 起采用「仅当 global 已被其它 root 标记才贡献 intrinsics；realm 0 除外」 |
| 8 | createContext 未写清 contextify | `runInContext` 以 sandbox 为 eval `scope_env`（非 scope_record 时走对象属性读写）；并安装 per-realm 内建全局 |
| 9 | 克隆「snapshot 字节」措辞 | 克隆 **主 realm 当前 pristine 可达图**（immortal 内 primordial + 其属性闭包）；`WJSM_STARTUP_SNAPSHOT=0` 同样从引导后主 realm 取图 |
| 10 | microtaskMode 未说明实现 | `afterEvaluate`：run 前记录 queue 长度/世代，run 后 drain **本轮入队** 的 microtask（单队列，不 per-realm 复制 scheduler） |
| 11 | NativeCallable 只加 `VmMethod` | 正确；但 **realm 内 `Array`/`Object` 等构造器**若仍是无 realm 标签的 `ArrayConstructor`，构造会绑主 proto——克隆后须安装 **关闭于 RealmId 的构造器**（`NativeCallable` 增 `RealmArrayConstructor{realm}` 等，或统一 `RealmCtor{kind, realm}`），并进入 `SnapshotNativeCallable` **仅当可无状态序列化**；带 `RealmId` 的变体 **禁止**进 snapshot 子集，只存在于动态表 |
| 12 | issue #313 表述 | 标明「覆盖原非目标」，避免与已完成的 Inspector/net 条目混淆 |

---

## 阶段总览

- **Phase 0 — Realm 基础设施**：数据结构、`execution_realm`、handle_remap walker+policies、主 realm 登记。
- **Phase 1 — Realm 克隆**：pristine 可达图克隆 + createContext 内核。
- **Phase 2 — 执行帧 + eval 双路径 realm 感知**：swap globals、解释器/编译 eval、构造器路径。
- **Phase 3 — vm builtin + API 外形**：node_vm.js + host bridge + 标志性 fixtures。
- **Phase 4 — GC per-realm 条件 roots + 死 realm 回收**。
- **Phase 5 — timeout**（与 async-yield epoch 协作）。
- **Phase 6 — 次级选项 + 回归 + ADR**。

每阶段任务遵循 TDD：失败测试 → RED → 实现 → GREEN → 提交。

---

# Phase 0 — Realm 基础设施

## Task 0.1：定义 Realm / RealmIntrinsics / RealmId / execution_realm

**Files**
- create: `crates/wjsm-runtime/src/realm.rs`
- modify: `crates/wjsm-runtime/src/lib.rs`（`mod realm;` + `RuntimeState` 字段）
- test: `crates/wjsm-runtime/tests/realm_registry.rs`

**Why**：realm 是 vm 承载单元；`execution_realm` 是后续分配/eval 的唯一「当前 realm」指示。

**Impact/Compatibility**：扁平新字段；默认 `execution_realm=0`、空 `active_realms` 直至惰性登记。

**Verification**
```bash
cargo nextest run -p wjsm-runtime -E 'test(realm_registry)'
```

**Steps**

1. **写失败测试** `crates/wjsm-runtime/tests/realm_registry.rs`：
```rust
//! 主 realm 登记为 active_realms[0]；RealmId 可序；intrinsics empty 全为 undefined。
use wjsm_runtime::realm::{Realm, RealmId, RealmIntrinsics};

#[test]
fn realm_id_is_monotonic() {
    assert!(RealmId(1) > RealmId(0));
}

#[test]
fn realm_intrinsics_empty_is_all_undefined() {
    let intr = RealmIntrinsics::empty();
    // 用 realm 模块 re-export 的 undefined 常量，避免强迫 pub value API
    assert!(intr.iter_roots().iter().all(|&h| h == RealmIntrinsics::UNDEFINED));
}

#[test]
fn realm_carries_global_and_intrinsics() {
    let r = Realm::new(RealmId(0), 12345_i64, RealmIntrinsics::empty());
    assert_eq!(r.id, RealmId(0));
    assert_eq!(r.global_object, 12345_i64);
}
```

2. **验证 RED**：模块不存在 → 编译失败。

3. **实现** `realm.rs`（形状如下；字段与 `RuntimeState` / `roots.rs` 已 root 集合对齐，实现时以源码为准扩全）：
```rust
//! 单堆多 realm：每 realm = intrinsics 句柄集 + global；共享 Store/obj_table/GC。

use crate::value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RealmId(pub u32);

/// 与 RuntimeState primordial / roots.rs 显式 root 对齐的 per-realm 句柄集。
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
    pub aggregate_error_proto: i64,
    // 实现时按 roots.rs 补齐：buffer / text_encoder / text_decoder / typedarray_protos…
}

impl RealmIntrinsics {
    pub const UNDEFINED: i64 = /* value::encode_undefined() 的常量折叠或 fn */;

    pub fn empty() -> Self { /* 每字段各写一次 UNDEFINED，禁止重复字段 */ }

    /// GC / 克隆 BFS 用的全部根句柄（i64 NaN-box）。
    pub fn iter_roots(&self) -> impl Iterator<Item = i64> + '_ { /* … */ }
}

#[derive(Debug, Clone, Copy)]
pub struct CodeGenFlags {
    pub strings: bool, // false → 该 realm 内 eval/Function 抛 EvalError
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

`RuntimeState` 新增（扁平，初始化）：
```rust
/// 活跃 realm。0 = 主 realm（惰性登记）。不因登记而强持有 global（见 Phase 4）。
pub(crate) active_realms: std::sync::Mutex<Vec<crate::realm::Realm>>,
pub(crate) next_realm_id: std::sync::atomic::AtomicU32, // 从 1 起；0 留给主 realm
/// 当前执行帧目标 realm；分配/构造/字面量 intrinsic 解析读此字段。
pub(crate) execution_realm: std::sync::atomic::AtomicU32,
```

`realm.rs` 另提供：
- `main_realm_intrinsics(state, env globals) -> RealmIntrinsics`（从现有字段装配）
- `main_realm_lazy_register(...)`
- `with_execution_realm(state, id, f)`（保存/恢复 `execution_realm`，支持嵌套）

4. **验证 GREEN**。

5. **提交**：`feat(vm): add Realm/RealmIntrinsics/execution_realm registry (#313)`

---

## Task 0.2：抽出 handle_remap 共享 walker + 可插拔 policy

**Files**
- create: `crates/wjsm-runtime/src/handle_remap.rs`
- modify: `crates/wjsm-runtime/src/startup_snapshot_remap.rs`（委托 walker + `FuncTableIndexRangePolicy`）
- modify: `crates/wjsm-runtime/src/lib.rs`
- test: `crates/wjsm-runtime/tests/handle_remap_kernel.rs` + 现有 `startup_snapshot_gc_fixes`

**Why**：snapshot 恢复与 realm 克隆都要扫对象图，但**改写语义不同**。

**现网事实**（`startup_snapshot_remap.rs`）：只处理 `HEAP_TYPE_OBJECT` 属性槽里 **`value::is_function` 的 WASM table index** 落在 `[snapshot_base, snapshot_base+len)` 的值；**不**改 proto header、**不**改 object handle。

**Impact/Compatibility**：`remap_array_proto_function_indices` 对外签名与行为保持；内部改 walker 调用。

**Verification**
```bash
cargo nextest run -p wjsm-runtime -E 'test(handle_remap_kernel) | test(snapshot)'
```

**Steps**

1. **写失败测试**：
   - **Policy A（函数表）**：与现网等价——属性槽 function idx 在区间内被平移，object handle 不变。
   - **Policy B（handle map）**：构造单对象堆片段：`proto` header = handle 5；数据属性 value = `encode_object_handle(7)`；accessor 的 getter/setter 各含 handle；映射 `5→105, 7→107,…`；断言 proto/value/getter/setter 均改写，非映射值与非 handle 标签（number/undefined）不变。
   - helper 按 `HEAP_OBJECT_*` / `PROP_SLOT_*` 常量手写字节（对照 `object_walker` / 现 remap 测试）。

2. **验证 RED**。

3. **实现契约**：
```rust
/// old_handle_index → new_handle_index（裸 u32 handle，不是完整 i64）
pub struct HandleMap { /* HashMap<u32,u32> */ }

pub trait RemapPolicy {
    /// 对扫描到的 i64 槽位就地改写；proto header 的 u32 另有钩子。
    fn remap_value(&self, raw: i64) -> i64;
    fn remap_proto_handle(&self, h: u32) -> u32;
}

pub struct FuncTableIndexRangePolicy { pub snapshot_base: u32, pub table_len: u32, pub current_base: u32 }
// remap_value: 仅 is_function 且 idx∈range → encode_function_idx(current_base + off)；proto 不变

pub struct ObjectHandleMapPolicy<'a> { pub map: &'a HandleMap }
// remap_value: 对 object/array/closure/bound/… 等带 handle 的 tag decode→map→encode；
//              function **table idx** 默认不改（克隆后方法仍指向同一 WASM 表项，与 Node 共享内建实现一致）
// remap_proto_handle: map 命中则替换

pub fn walk_and_remap_heap(heap: &mut [u8], policy: &dyn RemapPolicy) -> anyhow::Result<()>;
// 遍历 OBJECT/ARRAY（及实现时确认的其它 heap type）：
// - proto header（OBJECT）
// - 数据属性 value；ACCESSOR 的 getter/setter（现网 FuncTable policy 跳过 accessor——保持；
//   ObjectHandleMapPolicy **必须**处理 getter/setter）
// - ARRAY 元素槽若存 handle 亦处理（ObjectHandleMapPolicy）
```

`remap_array_proto_function_indices` = `walk_and_remap_heap(data, &FuncTableIndexRangePolicy{…})`。

4. **验证 GREEN**（含现有 snapshot 回归）。

5. **提交**：`refactor(runtime): shared object-graph walker with pluggable remap policies (#313)`

---

# Phase 1 — Realm 克隆

## Task 1.1：pristine 可达图克隆

**Files**
- create: `crates/wjsm-runtime/src/realm_clone.rs`
- modify: `lib.rs`、`realm.rs`
- test: `crates/wjsm-runtime/tests/realm_clone.rs`

**Why**：`createContext` 内核——新 handle 槽上的独立 intrinsic 图。

**Impact/Compatibility**：仅 vm 路径；分配走现有 bump/`obj_new`；克隆结果落在 **dynamic heap**（可被 GC），来源可读 immortal。

**Verification**
```bash
cargo nextest run -p wjsm-runtime -E 'test(realm_clone)'
```

**Steps**

1. **写失败测试**（复用 startup/runtime store harness）：
   - 新 realm `array_proto` handle ≠ 主 realm；
   - 新 `array_proto` 的 proto 链指向**新** `object_proto`（非主 realm handle）；
   - 新图内无悬挂：闭包内每个对象的子引用要么在 HandleMap 像内，要么是故意共享的非对象（number/string primitive/native 无堆对象）。

2. **验证 RED**。

3. **实现** `clone_pristine_realm(caller, sandbox_global) -> Result<Realm>`：
   1. `main_realm_lazy_register`；
   2. `roots = main_realm_intrinsics(...)`；
   3. `closure = primordial_reachable_closure(caller, &roots)`（Task 1.2，可本任务内联第一版）；
   4. 对闭包每个 handle：`obj_new`/按 heap type 分配同等 capacity 的新对象，复制槽位字节，填 `HandleMap`；
   5. 对每个**新**对象字节调用 `walk_and_remap`（或按对象 `remap_object_at`）+ `ObjectHandleMapPolicy`；
   6. 用 map 装配 `RealmIntrinsics`；
   7. `sandbox_global` 的 `[[Prototype]]` → 新 `object_proto`（Node contextify 语义）；
   8. **安装 per-realm 内建全局**到 sandbox：`Object`/`Array`/`Function`/`Error`/… 为**带 `RealmId` 的构造器 NativeCallable**（见修正 #11），其 `.prototype` 指向本 realm intrinsics；`globalThis`/`this` 绑定按 Node；
   9. `register_realm`：`next_realm_id.fetch_add`，`active_realms.push`，检查 `WJSM_VM_MAX_REALMS`（默认 1024）。

**禁止**：整段 immortal 字节 memcpy 当克隆；禁止假设 embedded snapshot blob 可直接第二次 restore 到新 handle 区（restore 路径是 1:1 原位）。

4. **验证 GREEN**。

5. **提交**：`feat(vm): clone pristine primordial graph into new realm handles (#313)`

---

## Task 1.2：primordial 可达闭包完整性

**Files**
- modify: `realm_clone.rs`、`realm.rs`
- test: 追加闭包用例

**Why**：漏对象 → 跨 realm 串原型。

**Steps**

1. **测试**：闭包含全部 `RealmIntrinsics` 根；对每个对象，`ObjectWalker::visit_object_children` 给出的子 handle 均在闭包内（或显式白名单共享——默认**不**共享堆对象）。
2. **实现**：`primordial_reachable_closure`：从 `iter_roots()` BFS；过滤无效 handle；实现期记录是否限制在 immortal 区间——若方法属性指向 dynamic 区对象，**仍必须纳入闭包**（以 walker 为准，不以 immortal 边界偷懒）。
3. **GREEN + 提交**：`feat(vm): primordial reachable closure for realm clone (#313)`

---

# Phase 2 — 执行帧 + eval 双路径 realm 感知

## Task 2.0：`with_execution_realm` + WASM global swap/restore

**Files**
- modify: `realm.rs`、`runtime_eval.rs`（或 `runtime_node_vm.rs` 执行入口）、必要时 `runtime_heap`/`alloc_*`
- test: `crates/wjsm-runtime/tests/execution_realm_frame.rs`

**Why**：compiled eval **import 父模块同一 mutable `__array_proto_handle`/`__object_proto_handle`**（见 `compiler_core.rs` eval imports + `try_compiled_eval_from_caller_async`）；`support` 的 `arr_new` 从该 global 写数组 proto。只改解释器不够。

**契约**：
```text
enter(realm_id):
  push old_execution_realm, old_array_proto_global, old_object_proto_global
  execution_realm = realm_id
  if realm_id != 0:
    set __array_proto_handle = decode(intrinsics.array_proto)
    set __object_proto_handle = decode(intrinsics.object_proto)
  // 主 realm：globals 已是权威源，可 no-op 或重写为自身
exit:
  restore globals + execution_realm（嵌套安全）
```

**可重入**：realm A 内 host 回调再进 realm B 必须栈式保存。

**测试**：enter(B) 后读 WASM global == B.array_proto；exit 后恢复主 realm。

**提交**：`feat(vm): execution_realm frame with array/object proto global swap (#313)`

---

## Task 2.1：AST 解释器路径 realm 感知

**Files**
- modify: `runtime_eval.rs`（签名可贯通 `RealmId` **或** 只读 `execution_realm`——推荐**只读 execution_realm**，避免 2k 行函数签名大爆炸）
- test: `crates/wjsm-runtime/tests/eval_realm_interp.rs`

**Why**：解释器字面量/`new`/全局查找须用当前 realm。

**Steps**

1. **测试**：在 realm A 的执行帧内解释器 eval `[]`，其 `[[Prototype]]` === A.array_proto。
2. **实现**：`resolve_intrinsic(caller, which)`：`execution_realm==0` → 现有 RuntimeState/WASM global；否则 → `active_realms[id].intrinsics`。数组/对象/regexp 字面量、未声明标识符回退到 **当前 realm.global_object**（由调用方把 sandbox 设为 scope_env，见 3.2）。
3. **GREEN + 提交**：`feat(vm): AST interpreter respects execution_realm intrinsics (#313)`

---

## Task 2.2：编译 eval 路径 + 构造器路径

**Files**
- modify: `runtime_eval.rs`（`try_compiled_eval_*` 在调用前后包 `with_execution_realm`）；`runtime_builtins.rs`（`ArrayConstructor`/`ObjectConstructor`/…）；带 `RealmId` 的 ctor 变体分派
- test: `crates/wjsm-runtime/tests/eval_realm_compiled.rs`

**Why**：编译路径靠 global swap（2.0）即可让 `arr_new` 写对 proto；**但** `new Array` 走 `NativeCallable::ArrayConstructor` → `alloc_array`，必须同样尊重 `execution_realm` 或使用 realm 关闭构造器。

**eval_cache 正确断言**（对照 `cached_eval_wasm`）：
- 键 = hash(`code`, `has_scope_bridge`, `var_writes_to_scope`, `data_base`, `SCOPE_RECORD_CACHE_VERSION`)；
- **不得**把 `realm_id` 编进键（字节码 realm 无关）；
- 测试：同一 `code` 在 A/B 执行，若 `data_base` 等维度相同则命中同一缓存项；**不要**断言「整表 size==1」（`data_base` 随堆前进常变）。

**Steps**

1. **测试**：
   - 编译路径：A/B 各 `[]`，proto 分属 A/B；
   - `new Array` / `Array()` / `Object()` 同隔离；
   - 缓存键不含 realm。
2. **实现**：执行入口统一 `with_execution_realm`；ctor 分派读 `execution_realm`；realm 安装的构造器可为 `RealmCtor { kind, realm: RealmId }`（**不**进入 `SnapshotNativeCallable`）。
3. **GREEN + 提交**：`feat(vm): compiled eval + constructors honor execution_realm (#313)`

---

# Phase 3 — vm builtin + API 外形

## Task 3.1：注册 vm builtin + host bridge 骨架

**Files**
- create: `crates/wjsm-module/builtin_js/node_vm.js`
- create: `crates/wjsm-runtime/src/runtime_node_vm.rs`
- modify: `builtin_modules.rs`、`runtime_node_globals.rs`、`types.rs`（`NativeCallable::VmMethod { kind }`）、`startup_snapshot_native_bridge.rs`、`wjsm-snapshot-format`（**仅**无状态可快照子集；`VmMethod` 若无状态可进 snapshot，与 `NetMethod` 等模式一致）
- test: `fixtures/modules/node_builtin_vm_main.js` + `.expected`

**Verification**
```bash
cargo nextest run -E 'test(modules__node_builtin_vm)'
cargo run -- run -e "const vm=require('vm'); console.log(typeof vm.runInNewContext);"
```

**Steps**

1. fixture：`typeof createContext` / `typeof Script` → `function function`。
2. `BUILTIN_MODULES` 增 `canonical: "vm"`；`node_vm.js` 经 `globalThis.__wjsm_node_vm` 调 host；骨架方法可先抛「未实现」**仅限**尚未到 3.2 的分支——但 3.1 验收的 typeof 必须为 function。
3. `install_native` 模式对齐 `runtime_node_globals.rs`。
4. **提交**：`feat(vm): register node:vm builtin + host bridge skeleton (#313)`

---

## Task 3.2：createContext / isContext / runInContext / runInNewContext

**Files**
- modify: `runtime_node_vm.rs`、`node_vm.js`
- test: `fixtures/happy/vm_realm_isolation.js`、`vm_cross_realm_ref.js`、`vm_sandbox_visible.js` + `.expected`

**Contextify 契约（必须写清）**：
1. `createContext(sandbox)` → `clone_pristine_realm` + 标记 sandbox 为 contextified（**side table**：`contextified: Mutex<HashMap<u32 /*handle*/, RealmId>>`，优于隐藏槽，免改对象布局）；
2. `isContext` 查 side table；
3. `runInContext(code, sandbox)`：
   - 查 RealmId；
   - `with_execution_realm(id, || eval(code, scope_env = sandbox))`；
   - 未声明赋值写到 sandbox 属性（现有 `eval_write_binding` 对非 scope_record 对象已 `set_host_data_property`）；
4. 返回值 **同堆引用** 回传，无 structuredClone；
5. `runInNewContext` = create + run。

**Fixtures**
- `vm_realm_isolation.js`：`runInNewContext('[]') instanceof Array` → `false`
- `vm_cross_realm_ref.js`：`runInNewContext('({a:2})').a` → `2`
- `vm_sandbox_visible.js`：`s={}; runInNewContext('x=1',s); s.x` → `1`

**提交**：`feat(vm): createContext/isContext/runIn* realm semantics (#313)`

---

## Task 3.3：vm.Script + compileFunction + runInThisContext

**Files**
- modify: `node_vm.js`、`runtime_node_vm.rs`
- test: `vm_script_reuse.js`、`vm_compile_function.js`、`vm_run_in_this_context.js`

**契约**：
- `Script` 持有预编译产物句柄/缓存键维度；多次 `runIn*` 不重编（在 `data_base` 策略允许时——若 `data_base` 必须每次 reserve，则 Script 固定编译时 data_base 或改为不 bake 绝对地址；**实现时优先让 Script 路径稳定可复用**，可单独 reserve 或复用 compile_eval 的稳定布局）；
- `runInThisContext`：`execution_realm=0`，`scope_env` 不绑 sandbox（读主 global）；
- `compileFunction`：函数对象捕获定义时 realm（`execution_realm` 或显式 context 选项）。

**提交**：`feat(vm): Script reuse, compileFunction, runInThisContext (#313)`

---

# Phase 4 — GC per-realm roots + 死 realm 回收

## Task 4.1：条件 root 枚举（弱 global）

**Files**
- modify: `runtime_gc/roots.rs`
- test: `crates/wjsm-runtime/tests/vm_gc_realm_roots.rs`

**契约（自本任务起一次定稿，避免 4.1/4.2 互相打架）**：
- realm 0：现有 primordial root 路径保持；`active_realms[0]` 不额外强持有 global（主 `js_global_object` 已有 root）。
- realm ≥1：
  - **不要**把 `global_object` 无条件 `visit` 当强 root；
  - 若 `global` 的 handle 在本轮已由 shadow stack / host table / 其它 root 标记为 live → 再 `visit` 其 `intrinsics.iter_roots()`；
  - 否则该 realm 本轮不贡献 intrinsic root（允许原型随 sandbox 一起死）。

**测试**：sandbox 仍被局部变量持有时 GC → 原型仍在；现有 `test(gc)` 不回归。

**提交**：`feat(vm): conditional per-realm GC roots for live sandboxes (#313)`

---

## Task 4.2：死 realm 从 active_realms 清理

**Files**
- modify: `roots.rs` 或 GC 收尾钩子、`realm.rs`
- test: 丢弃 sandbox 全部引用 → GC → `active_realms` 去掉该 id，无 panic

**实现**：GC 后 `retain`：realm0 恒留；其它仅当 global handle 仍 live。清理 side table `contextified` 条目。

**提交**：`feat(vm): reclaim dead realms when sandbox unreachable (#313)`

---

# Phase 5 — timeout

## Task 5.1：编译路径 timeout（与 async-yield epoch 协作）

**Files**
- modify: `runtime_node_vm.rs`（vm 执行入口）、可能 `runtime_startup.rs` / store 执行包装
- test: `crates/wjsm-runtime/tests/vm_timeout.rs` + `fixtures/happy/vm_timeout.js`

**现网事实**：
- `Config::epoch_interruption(true)` **已启用**；
- Store 使用 `set_epoch_deadline(1)` + **`epoch_deadline_async_yield_and_update(1)`**（协作式异步让出，**不是**超时 trap）；
- 仓库内**尚无**通用 `engine.increment_epoch()` 超时器。

**禁止**：假定「无 timeout 则 epoch 永不触发」；禁止在不恢复回调的情况下全局改成 trap。

**选定实现（按序尝试，计划期内用测例钉死一种）**：

**方案 T-fuel（推荐默认）**：
1. vm `timeout` 存在时，对 Store 启用 fuel（或为该次调用 `set_fuel`）；
2. 后台/截止时间到 → fuel 耗尽 → trap 映射为 vm timeout 错误；
3. 不影响 epoch async yield。

**方案 T-epoch-scoped（仅当 fuel 与 Winch/guest_debug 冲突时）**：
1. 进入 timed vm 帧：保存当前 epoch 回调；改为 `epoch_deadline_trap`；`set_epoch_deadline(1)`；
2. `std::thread`/`tokio` 定时 `engine.increment_epoch()`；
3. 退出帧（含 panic 路径）：**必定**恢复 `epoch_deadline_async_yield_and_update(1)` 与 deadline；
4. 证明：超时 trap 不破坏后续普通 async 执行；并发 host completion 不误杀（vm 帧独占）。

**测试**：`runInNewContext('while(1){}', {}, {timeout: 50})` 有限时间内抛错；随后主 realm 普通 async/定时器仍可用。

**提交**：`feat(vm): runIn* timeout without breaking async-yield epoch (#313)`

---

## Task 5.2：解释器路径 deadline

**Files**
- modify: `runtime_eval.rs`（循环/回边检查 `Instant`）
- test: 强制走解释器兜底的死循环 + timeout

**提交**：`feat(vm): interpreter deadline for vm timeout fallback (#313)`

---

# Phase 6 — 次级选项 + fixtures + 回归 + ADR

## Task 6.1：contextCodeGeneration + microtaskMode + 非目标抛错

**Files**
- modify: `node_vm.js`、`runtime_node_vm.rs`、`realm` flags
- test: `fixtures/happy/vm_codegen_off.js`、`vm_microtask_mode.js`、`fixtures/errors/vm_unsupported.js`

**microtaskMode: 'afterEvaluate'**：
- run 前：`let gen = microtask_queue.len()` 或单调 `microtask_epoch`；
- run 后：drain 直至队列回到世代基线 / 排空本轮新增（注意 drain 中又 enqueue 的 Promise 链应继续排空到稳态，对齐 Node afterEvaluate）；
- **不**复制整套 scheduler。

**非目标**：`SourceTextModule`/`SyntheticModule`/`measureMemory` getter 抛 `Error('not implemented in wjsm: …')`。

**提交**：`feat(vm): codegen flags, microtaskMode, explicit non-goal errors (#313)`

---

## Task 6.2：全工作区回归 + GC 矩阵

```bash
cargo build 2>&1 | grep -c warning   # → 0
cargo nextest run --workspace
WJSM_TEST_GC=mark-sweep cargo nextest run -p wjsm-runtime -E 'test(vm) | test(realm) | test(gc)'
WJSM_TEST_GC=g1 cargo nextest run -p wjsm-runtime -E 'test(vm) | test(realm) | test(gc)'
WJSM_TEST_GC=zgc cargo nextest run -p wjsm-runtime -E 'test(vm) | test(realm) | test(gc)'
```

失败则修 owner，禁止改 fixture 规避。

---

## Task 6.3：ADR 0008 + INDEX + AGENTS.md

**Files**
- create: `docs/adr/0008-node-vm-multi-realm.md`
- modify: `docs/aegis/INDEX.md`、`AGENTS.md`（及中文 CLAUDE 若有 load-bearing 段）

**ADR 必写决策**：
1. 单堆多 realm；
2. pristine **可达图克隆**（非二次 snapshot restore）；
3. walker + 双 RemapPolicy；
4. `execution_realm` + proto global swap（非 TLS）；
5. timeout 与 async-yield epoch 隔离策略；
6. 条件 GC root / sandbox 生命周期；
7. `VmMethod` / `RealmCtor` 与 snapshot ABI 边界。

**提交**：`docs(vm): ADR 0008 node:vm multi-realm + AGENTS/INDEX sync (#313)`

---

## Self-Review

1. **Spec 覆盖**：目标 1–10 → 3.1–3.3 / 6.1 / Phase 0–2 / 5；标志性语义 fixtures 齐全。
2. **占位符**：无 TBD；实现细节以源码常量为准，示意代码标为形状。
3. **类型一致**：`RealmId` / `RealmIntrinsics` / `HandleMap` / `RemapPolicy` / `execution_realm` 贯通。
4. **兼容性**：`execution_realm=0` 零行为变化；snapshot 仅扩展无状态 discriminant；带 `RealmId` 的 ctor **不**进 snapshot。
5. **与源码一致性**（本轮审查）：remap 语义、epoch 占用、eval import globals、`eval_cache` 键、roots 集合、`arr_new` proto 来源均已对齐。
6. **架构完整性**：分配/字面量/构造器/eval 四条路径统一执行帧，避免「假隔离」。
7. **验证**：每任务 nextest filter；全局 GC 矩阵。
8. **双轨**：Task 0.2 Repair+Retirement；Phase 4 弱持有一次定稿。

---

## Retirement

- 外部：无旧 vm API。
- 内部：`remap_array_proto_function_indices` 内联遍历退役为 walker + `FuncTableIndexRangePolicy`；行为由 snapshot 测试守护。
- design spec 中与本 plan 冲突的「TLS 注入 / epoch 简单超时 / 纯 code 缓存键 / handle 与函数表混同」以本 plan 为准，实现后回写 design + ADR 0008。
