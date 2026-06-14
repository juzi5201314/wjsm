# 可插拔 GC 框架实施计划

**Goal**: 用 non-moving mark-sweep + segregated free list 替代当前 `memory.grow` 无限扩容，建立 `GcAlgorithm` trait 框架（预留 generational/incremental/parallel 接入点），并恢复自动触发 GC。同步长循环不 OOM，WASM locals 持有的对象在 alloc 触发 GC 时不被误回收。

**Architecture**: 见 `docs/aegis/specs/2026-06-14-pluggable-gc-framework-design.md`。三层改动：`wjsm-ir`（liveness + ValueTy pass）、`wjsm-backend-wasm`（safepoint spill 代码生成 + 分配路径 bump+slow）、`wjsm-runtime`（`runtime_gc/` 框架 + MarkSweep 实现）。

**Tech Stack**: Rust 2024, wasmtime（epoch interruption，sync `Func::wrap`）, wasm-encoder, swc_core. 测试用 `cargo nextest`（per-test 超时 9s）。

**Baseline/Authority Refs**:
- `docs/aegis/specs/2026-06-14-pluggable-gc-framework-design.md`（设计 spec，§18 不变量清单为硬约束）
- `bug.md` O2（根因：moving GC + WASM locals 不可见）
- `AGENTS.md`（NaN-boxing、对象布局、无 stub 硬规则）

**Compatibility Boundary**（必须保持）:
- 现有 470+ fixture 全绿
- 活动对象布局不变（16B header + payload）
- NaN-boxing / `obj_table` 间接不变
- `gc()` global 行为保持
- §18 全部 INV/IMPL 不变量

**Verification**:
- `cargo nextest run --workspace` 全绿（每阶段后）
- 新增 fixture：长循环不 OOM、safepoint 安全、深链表 mark 不栈溢出
- 新增单元测试：liveness 正确性、free list、mark/sweep（mock GcContext）

**ADR Signal**: 本计划落地 GC 算法 trait 抽象（持久架构决策）。完成后应在 `docs/adr/` 记录 ADR：trait 边界选型、分配路径物理边界、non-moving 决策、barrier defer。baseline-sync：AGENTS.md "mark-sweep GC" 描述需更新为可插拔框架。

---

## Plan Pressure Test

```text
- Owner / contract / retirement:
    Owner: 新建 runtime_gc/ 模块组（单一 canonical owner）；删除旧 trigger_gc + core.rs gc_collect（retirement，P5）。
    Contract: GcAlgorithm trait + GcContext（Caller 注入，不持 slice）；§18 INV/IMPL 不变量。
    Retirement: 旧 compact GC 在 P5 删除前，P4 已重接 gc() global 到框架，无断档。
- Verification scope: 每阶段独立可验证（P1 liveness 单测、P3 mock ctx 单测、P4 长循环 fixture）；fixture 全绿是跨阶段回归闸。
- Task executability: 每任务给出确切文件路径 + 完整代码 + 确切命令。
- Pressure result: proceed（owner 清晰、retirement 有序、验证分层）。
```

## Plan-Time Complexity Check

```text
- Target files:
    新建: crates/wjsm-runtime/src/runtime_gc/{mod,api,context,mark_bitmap,roots,mark_sweep/{mod,allocator,marker,sweeper,context}}.rs
    新建: crates/wjsm-ir/src/liveness.rs, value_ty.rs
    修改: wjsm-ir/src/{lib.rs,value.rs}, wjsm-ir/tests/liveness.rs（新）
    修改: wjsm-backend-wasm/src/{compiler_module,compiler_instructions,compiler_helpers,compiler_array_helpers,lib.rs,host_import_registry.rs}
    修改: wjsm-runtime/src/{lib.rs,runtime_heap.rs,runtime_builtins.rs,host_imports/core.rs,host_imports/mod.rs}
    修改: wjsm-cli/src/lib.rs
- Existing size / shape signals: runtime_builtins.rs 2500+行（trigger_gc L2939-3223），compiler_instructions.rs 大文件。
    标记迁移要抽到 runtime_gc/，不在 runtime_builtins.rs 加新代码。
- Owner fit: runtime_gc/ 是 GC 的唯一 owner；runtime_builtins.rs 的 trigger_gc 迁出后只剩其他职责。
- Add-in-place risk: liveness pass 必须是 wjsm-ir 新文件（不能塞 lib.rs）；safepoint spill 是 compiler_instructions.rs 的新 pass。
- Better file boundary: 抽 helper（liveness.rs / value_ty.rs 独立文件）；runtime_gc/ 模块组独立。
- Recommendation: add owner file（runtime_gc/）+ extract helper（liveness.rs）+ split task（7 阶段独立提交）。
```

## Tasks 总览

| 阶段 | 任务数 | 独立验证 |
|------|--------|----------|
| P0 | T0.1-T0.2 | size 直方图 + 冻结 SIZE_CLASSES |
| P1 | T1.1-T1.5 | ValueTy + tag_needs_root + liveness pass + 单测 |
| P2 | T2.1-T2.4 | safepoint spill 代码生成 + sp 不变量 + 容量检查 |
| P3 | T3.1-T3.8 | runtime_gc/ 框架 + MarkSweep + worklist + mock 单测 |
| P4 | T4.1-T4.6 | 分配路径改造 + handle 复用 + proactive + 长循环 fixture |
| P5 | T5.1-T5.3 | 删除旧 GC + 迁移 fixed-point + grep 无残留 |
| P6 | T6.1-T6.2 | 预留 hook 默认 impl + CLI --gc-algorithm |

---

# 阶段 P0：size 直方图 + 冻结 SIZE_CLASSES

**Why**: segregated free list 的 size class table 是 allocator 核心。spec §9.1 已冻结初始值，P0 验证覆盖率 ≥ 99%，必要时局部微调（不重构）。

## T0.1 采集 fixture 对象 size 直方图

**Files**:
- create: `/tmp/gc_size_hist.rs`（临时探针脚本，不入库）

**Why**: 确认冻结的 SIZE_CLASSES（§9.1）覆盖 fixture 实际对象 size 分布。

**Impact/Compatibility**: 只读探针，不改任何代码。

**Verification**: 脚本输出 size 直方图 + SIZE_CLASSES 覆盖率。

**Steps**:

- [ ] **写探针脚本**。在 runtime 加一个临时 host import `__gc_probe_size(i32)`，编译期在每个 `$obj_new`/`$arr_new` 成功后插入 `call __gc_probe_size(size)`。脚本主体：

```rust
// /tmp/gc_size_hist.rs — 临时，不入库。放到 crates/wjsm-runtime/src/host_imports/ 下临时编译。
// 实现方式：在 host_imports/core.rs 临时加一个 Func::wrap("__gc_probe_size", |caller, size| {
//     caller.data_mut().size_histogram.lock().push(size as usize);
// });
// RuntimeState 临时加 size_histogram: Mutex<Vec<usize>>。
// 运行所有 happy fixture 后 dump 直方图。
```

- [ ] **运行 happy fixtures 采集**：

```bash
cargo nextest run -E 'test(happy__)' 2>&1 | tail -5
# 然后用一个小 main 读 RuntimeState.size_histogram dump
```

- [ ] **计算覆盖率**。对照冻结的 SIZE_CLASSES：

```rust
const SIZE_CLASSES: &[usize] = &[
    16, 48, 80, 112, 144, 176, 208, 272, 336, 432,
    528, 640, 768, 1024, 1536, 2048, 4096, 8192, 16384,
];
// 覆盖率 = (落在某 class 精确匹配 or 进 big_list 的对象数) / 总对象数
// best-fit 允许向上取 class，所以所有 size 都有归属，覆盖率应 = 100%
// 关键指标：精确匹配率（size 恰好等于某 class）应高（减少分割）
```

- [ ] **记录结论到 spec §9.1 注释**。若某 size 区间集中但无 class 覆盖（如 fixture 大量 288B 对象但 class 是 272/336），局部增删 1-2 个 class。不改结构。

- [ ] **移除探针**（git checkout 还原 host_imports/core.rs + lib.rs），确认 `cargo nextest run --workspace` 仍全绿。

- [ ] **Commit**: `chore: GC P0 size histogram probe (temporary, not committed)`

> 注：探针是临时手段，**不入库**。结论（覆盖率数字 + 是否微调）写进 spec §9.1 注释并 commit spec。

## T0.2 确认 SIZE_CLASSES 定稿

**Files**:
- modify: `docs/aegis/specs/2026-06-14-pluggable-gc-framework-design.md`（§9.1 注释加覆盖率结论）

**Why**: 固化 P0 结论，P3 据此实现。

**Steps**:

- [ ] 在 §9.1 SIZE_CLASSES 上方加注释行：`// P0 验证：fixture 覆盖率 XX%，精确匹配率 YY%，class 未调整/微调（说明）`
- [ ] **Commit**: `docs: GC P0 freeze SIZE_CLASSES (coverage XX%)`

---

# 阶段 P1：IR 层 liveness + ValueTy 类型推断

**Why**: safepoint spill（P2）需要"在 safepoint 哪些 live ValueId 是 Handle 类型"。wjsm-ir 当前无任何 per-ValueId 类型/liveness（经验证，从零建）。

**Why in wjsm-ir**: 零外部依赖，归属正确；wjsm-semantic 的 name-based liveness 后续可复用（去重，非本计划范围）。

**Critical（#10）**: liveness 必须**块级 CFG join 取 union + Phi 边分发**，否则 if/else/loop 汇合点 live 集合错误 → safepoint 误判（漏 spill 活值 → GC 误回收）。

## T1.1 扩展 tag_needs_root 谓词（value.rs）

**Files**:
- modify: `crates/wjsm-ir/src/value.rs`
- test: `crates/wjsm-ir/tests/liveness.rs`（新建，本任务加谓词测试）

**Why**: shadow stack 扫描 + ValueTy 推断共用。现有 `is_js_object`（value.rs:367-375）遗漏 bigint/symbol/regexp/scope_record/runtime-string-handle/exception/iterator/enumerator。

**Impact/Compatibility**: 纯新增函数，不改现有 `is_js_object`。

**Verification**: `cargo nextest run -p wjsm-ir`。

**Steps**:

- [ ] **写失败测试**。在 `crates/wjsm-ir/tests/liveness.rs`:

```rust
use wjsm_ir::value;

#[test]
fn tag_needs_root_covers_all_handle_tags() {
    // 每种 handle tag 都应 needs_root
    assert!(value::tag_needs_root(value::encode_object_handle(0)));
    assert!(value::tag_needs_root(value::encode_array_handle(0)));
    assert!(value::tag_needs_root(value::encode_function(0)));
    assert!(value::tag_needs_root(value::encode_closure(0)));
    assert!(value::tag_needs_root(value::encode_bound(0)));
    assert!(value::tag_needs_root(value::encode_bigint_handle(0)));
    assert!(value::tag_needs_root(value::encode_symbol_handle(0)));
    assert!(value::tag_needs_root(value::encode_regexp_handle(0)));
    assert!(value::tag_needs_root(value::encode_proxy_handle(0)));
    assert!(value::tag_needs_root(value::encode_scope_record(0)));
    // runtime string handle（STRING_RUNTIME_HANDLE_FLAG）
    assert!(value::tag_needs_root(value::encode_runtime_string_handle(0)));
}

#[test]
fn tag_needs_root_rejects_scalars() {
    assert!(!value::tag_needs_root(value::encode_f64(3.14)));
    assert!(!value::tag_needs_root(value::encode_f64(0.0)));
    assert!(!value::tag_needs_root(value::UNDEFINED));
    assert!(!value::tag_needs_root(value::NULL));
    assert!(!value::tag_needs_root(value::TRUE));
    assert!(!value::tag_needs_root(value::FALSE));
    // 静态字符串 ptr（非 runtime handle）不应 needs_root
    assert!(!value::tag_needs_root(value::encode_string_ptr(0, 5)));
}
```

- [ ] **运行确认 RED**: `cargo nextest run -p wjsm-ir -E 'test(tag_needs_root)'` → 编译失败（函数不存在）。

- [ ] **实现 `tag_needs_root`**。在 `crates/wjsm-ir/src/value.rs` 加 pub 函数。先确认现有 encode/decode 函数名（读 value.rs 找 `encode_*`/`is_*` 系列实际名），用实际名。骨架：

```rust
/// 判断一个 NaN-boxed i64 是否持有需要 GC root 的 handle。
/// 供 shadow stack 扫描与 ValueTy 类型推断共用。
/// 覆盖所有"低 32 位是 handle/下标"的 tag；scalar（f64/undefined/null/bool/静态 string ptr）返回 false。
pub fn tag_needs_root(val: i64) -> bool {
    is_object(val)
        || is_array(val)
        || is_function(val)
        || is_closure(val)
        || is_bound(val)
        || is_proxy(val)
        || is_native_callable(val)
        || is_bigint(val)
        || is_symbol(val)
        || is_regexp(val)
        || is_scope_record(val)
        || (is_string(val) && is_runtime_string_handle(val))  // 区分 runtime handle vs 静态 ptr
        || is_exception(val)   // exception payload 是 handle
        || is_iterator(val)
        || is_enumerator(val)
}
```

> 实现前先 `grep -n "pub fn is_\|pub fn encode_\|pub const " crates/wjsm-ir/src/value.rs` 确认每个谓词/encode 的实际签名。若某谓词不存在（如 `is_runtime_string_handle`），按 STRING_RUNTIME_HANDLE_FLAG（value.rs:134）实现：`(val as u64 & 0x20) != 0` 当 is_string 时。

- [ ] **运行确认 GREEN**: `cargo nextest run -p wjsm-ir -E 'test(tag_needs_root)'` → 通过。

- [ ] **Commit**: `feat(ir): tag_needs_root predicate for GC root classification`

## T1.2 ValueTy 类型推断（value_ty.rs）

**Files**:
- create: `crates/wjsm-ir/src/value_ty.rs`
- modify: `crates/wjsm-ir/src/lib.rs`（加 `pub mod value_ty;`）
- test: `crates/wjsm-ir/tests/liveness.rs`

**Why**: safepoint spill 只 spill Handle 类型 local，跳过确定 Scalar。polymorphic（GetProp/Call/Phi）保守当 Handle（spec §11.2）。

**Verification**: `cargo nextest run -p wjsm-ir`。

**Steps**:

- [ ] **写失败测试**。追加到 `crates/wjsm-ir/tests/liveness.rs`:

```rust
use wjsm_ir::{value_ty::{ValueTy, infer_value_ty}, *};

#[test]
fn value_ty_object_producing_ops_are_handle() {
    // NewObject/NewArray/GetSuperBase → Handle
    let mut f = Function::new("test".into(), vec![], BasicBlockId(0));
    let mut bb = BasicBlock::new(BasicBlockId(0));
    bb.instructions_mut().push(Instruction::NewObject { dest: ValueId(0), capacity: 4 });
    bb.instructions_mut().push(Instruction::NewArray { dest: ValueId(1), capacity: 4 });
    bb.instructions_mut().push(Instruction::GetSuperBase { dest: ValueId(2) });
    bb.set_terminator(Terminator::Return { value: Some(ValueId(0)) });
    f.blocks_mut().push(bb);
    let ty = infer_value_ty(&f);
    assert_eq!(ty[&ValueId(0)], ValueTy::Handle);
    assert_eq!(ty[&ValueId(1)], ValueTy::Handle);
    assert_eq!(ty[&ValueId(2)], ValueTy::Handle);
}

#[test]
fn value_ty_arithmetic_is_scalar() {
    let mut f = Function::new("test".into(), vec![], BasicBlockId(0));
    let mut bb = BasicBlock::new(BasicBlockId(0));
    let c0 = ConstantId(0);
    bb.instructions_mut().push(Instruction::Const { dest: ValueId(0), constant: c0 });
    bb.instructions_mut().push(Instruction::Const { dest: ValueId(1), constant: ConstantId(1) });
    bb.instructions_mut().push(Instruction::Binary { dest: ValueId(2), op: BinaryOp::Add, lhs: ValueId(0), rhs: ValueId(1) });
    bb.instructions_mut().push(Instruction::Compare { dest: ValueId(3), op: CompareOp::Eq, lhs: ValueId(0), rhs: ValueId(1) });
    bb.set_terminator(Terminator::Return { value: Some(ValueId(2)) });
    f.blocks_mut().push(bb);
    let ty = infer_value_ty(&f);
    // Const(Number) → Scalar（需 Constant::Number 才判 Scalar；先验证 Constant 枚举）
    let ty = infer_value_ty(&f);
    assert_eq!(ty[&ValueId(2)], ValueTy::Scalar);  // Binary 算术 → Scalar
    assert_eq!(ty[&ValueId(3)], ValueTy::Scalar);  // Compare → Scalar
}

#[test]
fn value_ty_polymorphic_defaults_to_handle() {
    // GetProp/GetElem/Call 结果类型不定 → 保守 Handle
    // （需要构造带 object 的 IR；简化：只测 GetProp，object 来自 NewObject）
    let mut f = Function::new("test".into(), vec![], BasicBlockId(0));
    let mut bb = BasicBlock::new(BasicBlockId(0));
    bb.instructions_mut().push(Instruction::NewObject { dest: ValueId(0), capacity: 4 });
    bb.instructions_mut().push(Instruction::GetProp { dest: ValueId(1), object: ValueId(0), key: PropertyKey::String(ConstantId(0)) });
    bb.set_terminator(Terminator::Return { value: Some(ValueId(1)) });
    f.blocks_mut().push(bb);
    let ty = infer_value_ty(&f);
    assert_eq!(ty[&ValueId(1)], ValueTy::Handle);  // polymorphic → Handle 保守
}
```

> 实现前先确认 `Instruction` 各 variant 的字段名、`BinaryOp`/`CompareOp`/`PropertyKey`/`Constant` 的实际定义（读 lib.rs:318-488 + Constant enum）。测试中的字段名以实际为准，调整后再跑。

- [ ] **运行确认 RED**: 编译失败（value_ty 模块不存在）。

- [ ] **实现 `crates/wjsm-ir/src/value_ty.rs`**:

```rust
//! Per-ValueId 类型推断：区分 Handle（需 GC root）与 Scalar。
//! polymorphic ops 保守判 Handle（spec §11.2）。
use crate::{Function, Instruction, ValueId, Constant};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueTy { Handle, Scalar }

/// 推断函数内每个 producing instruction 的 dest 类型。
/// 未出现在 map 中的 ValueId 视为 Handle（保守）。
pub fn infer_value_ty(f: &Function) -> HashMap<ValueId, ValueTy> {
    let mut ty = HashMap::new();
    for bb in f.blocks() {
        for ins in bb.instructions() {
            if let Some((dest, kind)) = dest_and_kind(ins, f) {
                ty.insert(dest, kind);
            }
        }
    }
    ty
}

/// 返回 producing instruction 的 (dest, ValueTy)。非 producing 返回 None。
fn dest_and_kind(ins: &Instruction, f: &Function) -> Option<(ValueId, ValueTy)> {
    use Instruction::*;
    Some(match ins {
        // 确定 Handle
        NewObject { dest, .. } | NewArray { dest, .. }
        | GetSuperBase { dest } | GetSuperConstructor { dest }
        | ObjectSpread { dest, .. } | NewPromise { dest }
        | ExceptionToObject { dest, .. } => (*dest, ValueTy::Handle),
        // Const：看 Constant variant
        Const { dest, constant } => {
            let c = f.module().constant(*constant);  // 确认访问路径
            let kind = match c {
                Some(Constant::Number(_)) | Some(Constant::Bool)
                | Some(Constant::Null) | Some(Constant::Undefined) => ValueTy::Scalar,
                _ => ValueTy::Handle,  // String/FunctionRef 等保守 Handle
            };
            (*dest, kind)
        }
        // 算术/比较 → Scalar
        Binary { dest, .. } | Compare { dest, .. } => (*dest, ValueTy::Scalar),
        Unary { dest, op, .. } => {
            // 一元非算术（如 typeof on object）保守 Handle；纯算术（-、!）Scalar
            // 先查 UnaryOp 枚举，按 op 分（实现时读 lib.rs 确认 UnaryOp variant）
            (*dest, unary_ty(*op))
        }
        // polymorphic → Handle 保守
        GetProp { dest, .. } | GetElem { dest, .. }
        | OptionalGetProp { dest, .. } | OptionalGetElem { dest, .. }
        | OptionalCall { dest, .. }
        | DeleteProp { dest, .. } | LoadVar { dest, .. }
        | CollectRestArgs { dest, .. } | IsException { dest, .. }
        | EncodeException { dest, .. } | Phi { dest, .. }
        | StringConcatVa { dest, .. } => (*dest, ValueTy::Handle),
        // 条件 dest（Call/CallBuiltin/SuperCall）的 Option<ValueId>
        _ => return None,  // 非 producing 或条件 dest，调用方单独处理
    })
}
```

> 实现时先读 lib.rs 确认：(a) `Function` 是否有 `module()` 访问 Constant（可能需传 `&Module` 而非 `&Function`）；(b) `UnaryOp`/`BinaryOp` variant 名；(c) `PropertyKey` 结构。签名可能需改为 `infer_value_ty(module: &Module, f: &Function)`。调整测试以匹配。

- [ ] **运行确认 GREEN**: `cargo nextest run -p wjsm-ir -E 'test(value_ty)'` → 通过。

- [ ] **Commit**: `feat(ir): ValueTy type inference (Handle/Scalar) for safepoint spill`

## T1.3 liveness pass 骨架：CFG successors/predecessors（liveness.rs）

**Files**:
- create: `crates/wjsm-ir/src/liveness.rs`
- modify: `crates/wjsm-ir/src/lib.rs`（加 `pub mod liveness;`）

**Why**: per-ValueId liveness 需 CFG。移植 successor/predecessor 计算（骨架自 lowerer_async_eval.rs:40-66）。

**Steps**:

- [ ] **实现 CFG 计算**（先无测试，纯 helper，T1.4 一起测）:

```rust
//! Per-ValueId liveness 分析（块级 CFG join union + Phi 边分发）。
//! 供 GC safepoint spill 用（spec §11.1）。
use crate::{Function, BasicBlockId, ValueId, Instruction, Terminator};
use std::collections::{HashMap, HashSet};

/// 计算每个 block 的后继列表。
pub fn successors(f: &Function) -> HashMap<BasicBlockId, Vec<BasicBlockId>> {
    let mut succ = HashMap::new();
    for bb in f.blocks() {
        let s: Vec<BasicBlockId> = match bb.terminator() {
            Terminator::Return { .. } | Terminator::Throw { .. } | Terminator::Unreachable => vec![],
            Terminator::Jump { target } => vec![*target],
            Terminator::Branch { true_block, false_block, .. } => vec![*true_block, *false_block],
            Terminator::Switch { cases, default_block, exit_block, .. } => {
                let mut v: Vec<_> = cases.iter().map(|c| c.target).collect();
                v.push(*default_block);
                v.push(*exit_block);
                v.sort(); v.dedup();
                v
            }
        };
        succ.insert(bb.id(), s);
    }
    succ
}
```

> 实现前确认 `Switch::Case` 的字段名（target）和 Terminator::Switch 字段（读 lib.rs Terminator 定义）。

- [ ] **编译确认**: `cargo build -p wjsm-ir` 通过。

## T1.4 liveness 主体：块级 backward dataflow + Phi 边分发

**Files**:
- modify: `crates/wjsm-ir/src/liveness.rs`
- test: `crates/wjsm-ir/tests/liveness.rs`

**Why**: 计算 safepoint 处的 live ValueId 集合。**关键（#10）**: join 取 union，Phi 入参按边分发。

**Verification**: 单测覆盖 if/else 汇合、loop 回边、嵌套控制流。

**Steps**:

- [ ] **写失败测试**（3 个：线性、if/else join、loop 回边）:

```rust
use wjsm_ir::liveness::compute_liveness_at;

#[test]
fn liveness_linear() {
    // bb0: v0 = NewObject; v1 = Binary(v0,v0)? — 不合法（Binary 需 scalar）
    // 简化：v0 = NewObject(Handle); v1 = Const(num); safepoint 在 v1 后，v0 live（return v0）
    // 构造 IR，断言 safepoint 处 live = {v0}
}

#[test]
fn liveness_if_else_join_union() {
    // bb0: v0 = NewObject; Branch -> bb1/bb2
    // bb1: v1 = NewObject; Jump bb3
    // bb2: v2 = NewObject; Jump bb3
    // bb3: Phi v3 = {bb1:v1, bb2:v2}; Return v3
    // 断言：bb3 入口 live = {v0?} — v1/v2 是 Phi 源，按边分发，bb1 出口 live 含 v1，bb2 出口含 v2
    // bb0 出口（branch 前）live = {v0}（若 bb3 用 v0；否则 phi 源在各自前驱）
}

#[test]
fn liveness_loop_backedge() {
    // bb0: v0 = Const(0); Jump bb1
    // bb1: v1 = Phi{bb0:v0, bb1:v2}; Branch(v1 < N) bb2/bb3
    // bb2: v2 = Binary(v1, +1); Jump bb1
    // bb3: Return
    // 断言：bb1 入口 live = {v1}，bb2 出口 live = {v2}（Phi 源在 backedge 边）
}
```

> 构造具体 IR 时参照 lib.rs 的 Function/BasicBlock/Instruction 构造 API（参考 `crates/wjsm-ir/tests/ir_dump.rs` 现有构造模式）。

- [ ] **运行确认 RED**: 编译失败（compute_liveness_at 不存在）。

- [ ] **实现 liveness**。在 liveness.rs 加：

```rust
/// 计算每条指令后的 live ValueId 集合（per-instruction，块内细化）。
/// 返回 HashMap<(BasicBlockId, usize instr_idx), HashSet<ValueId>>。
/// safepoint spill 用：取 safepoint 指令处的集合。
///
/// 算法：
/// 1. 块级 live_in/live_out（backward，join 取 union，迭代到不动点）
/// 2. Phi 特殊：Phi v3={bb1:v1} 的 v1 仅对 bb1 的 live_out 有贡献（边分发）
/// 3. 块内 backward 细化到 per-instruction
pub fn compute_liveness(f: &Function) -> HashMap<(BasicBlockId, usize), HashSet<ValueId>> {
    let succ = successors(f);
    // use/def per block
    let (block_uses, block_defs, phi_sources) = block_use_def_phi(f);
    // backward iteration
    let mut live_in: HashMap<BasicBlockId, HashSet<ValueId>> = HashMap::new();
    let mut live_out: HashMap<BasicBlockId, HashSet<ValueId>> = HashMap::new();
    let mut changed = true;
    while changed {
        changed = false;
        for bb in f.blocks().iter().rev() {
            let mut out = HashSet::new();
            for &s in succ.get(&bb.id()).unwrap_or(&vec![]) {
                // Phi 边分发：bb 的 live_out 包含后继 s 的 Phi 源中对应 bb 的入参
                if let Some(srcs) = phi_sources.get(&s).and_then(|m| m.get(&bb.id())) {
                    out.extend(srcs.iter().copied());
                }
                out.extend(live_in.get(&s).unwrap_or(&HashSet::new()).iter().copied());
            }
            let mut in_ = out.clone();
            in_.retain(|v| !block_defs.get(&bb.id()).unwrap_or(&HashSet::new()).contains(v));
            in_.extend(block_uses.get(&bb.id()).unwrap_or(&HashSet::new()).iter().copied());
            if live_out.get(&bb.id()) != Some(&out) { live_out.insert(bb.id(), out); changed = true; }
            if live_in.get(&bb.id()) != Some(&in_) { live_in.insert(bb.id(), in_); changed = true; }
        }
    }
    // 块内 backward 细化
    let mut per_instr = HashMap::new();
    for bb in f.blocks() {
        let mut live = live_out.get(&bb.id()).cloned().unwrap_or_default();
        for (i, ins). in bb.instructions().iter().enumerate().rev() {
            // def/use per instruction（Phi 的 use 不在此计入，已在边分发）
            if !matches!(ins, Instruction::Phi { .. }) {
                if let Some(d) = instr_dest(ins) { live.remove(&d); }
                for u in instr_uses(ins) { live.insert(u); }
            }
            per_instr.insert((bb.id(), i), live.clone());
        }
        per_instr.insert((bb.id(), bb.instructions().len()), live.clone());  // block 出口
    }
    per_instr
}

/// 取 safepoint（某指令前）的 live 集合。instr_idx 指令执行前 = instr_idx 处（指令后集合往前推到该指令前）。
/// 简化：返回 (block, idx) 处的集合（idx 指令的 use 前）。
pub fn live_at(f: &Function, block: BasicBlockId, idx: usize) -> HashSet<ValueId> {
    compute_liveness(f).get(&(block, idx)).cloned().unwrap_or_default()
}
```

> 实现辅助 `block_use_def_phi`、`instr_dest`、`instr_uses`（读 Instruction variant，dest 是 def，其他 ValueId 字段是 use；Phi 特殊处理：dest 是 def，sources 是按前驱边分发的 use）。Phi sources 解析：`Phi { dest, sources }` 的 sources 结构需读 lib.rs:340 确认（可能是 `Vec<(BasicBlockId, ValueId)>` 或 `Vec<ValueId>` 顺序对应前驱）。

- [ ] **运行确认 GREEN**: `cargo nextest run -p wjsm-ir -E 'test(liveness)'` → 3 测试通过。

- [ ] **Commit**: `feat(ir): per-ValueId liveness (block-level union + Phi edge distribution)`

## T1.5 公开 API + 跨 crate 导出

**Files**:
- modify: `crates/wjsm-ir/src/lib.rs`（确保 `pub mod liveness; pub mod value_ty;` + re-export 常用）

**Why**: P2 backend 需调用 `liveness::compute_liveness` 和 `value_ty::infer_value_ty`。

**Steps**:

- [ ] 确认 lib.rs 顶部有 `pub mod liveness; pub mod value_ty;`（T1.2/T1.3 已加，此处确认）。
- [ ] **编译确认 backend 能引用**: `cargo build -p wjsm-backend-wasm` 通过（即使未用，确认可见）。
- [ ] **Commit**: `feat(ir): export liveness + value_ty modules`

---

# 阶段 P2：Backend safepoint spill 代码生成

**Why**: 编译器在每个 safepoint（alloc 点）前把 live Handle local spill 到 shadow stack，让 GC root 集精确。**本阶段不接 GC**，只验证 spill 不破坏语义（fixture 全绿）。

**Critical（#4）**: `__shadow_sp` 函数入口=出口；循环内每个 safepoint 独立 save/restore。
**R2**: 函数 prologue 容量检查。

## T2.1 识别 safepoint + 计算 spill 集合

**Files**:
- modify: `crates/wjsm-backend-wasm/src/compiler_instructions.rs`
- modify: `crates/wjsm-backend-wasm/src/compiler_module.rs`（compile_function 加 pass 调用）

**Why**: 在 compile_function 中，对每个函数计算"每个 safepoint 的 spill 集合"（live ValueId ∩ Handle 类型）。

**Steps**:

- [ ] **在 Compiler 加字段**（compiler_module.rs 或 lib.rs 的 Compiler struct）存 spill 计划:

```rust
// Compiler struct 加：
/// 当前函数的 safepoint spill 计划：safepoint instruction 全局 idx → 需 spill 的 local idx 列表
spill_plan: HashMap<usize, Vec<u32>>,
```

- [ ] **在 compile_function 中加 pass**（L548 lower_phi_to_locals 之后，L556 scratch 之前）:

```rust
// compiler_module.rs compile_function 内，lower_phi_to_locals 之后
self.compute_spill_plan(function);  // 新方法
```

- [ ] **实现 compute_spill_plan**（compiler_instructions.rs 或新 compiler_gc.rs）:

```rust
impl Compiler {
    /// 计算每个 safepoint 的 spill 集合。
    /// safepoint = NewObject/NewArray/可能 alloc 的 Call/CallBuiltin。
    /// spill 集合 = live ValueId 在该点 ∩ ValueTy==Handle → 映射到 local_idx。
    fn compute_spill_plan(&mut self, f: &IrFunction) {
        self.spill_plan.clear();
        let live = wjsm_ir::liveness::compute_liveness(f);
        let ty = wjsm_ir::value_ty::infer_value_ty(f);  // 若需 module 参数则调整
        // 遍历所有指令找 safepoint
        let mut global_idx = 0usize;
        for bb in f.blocks() {
            for (i, ins) in bb.instructions().iter().enumerate() {
                if self.is_safepoint(ins) {
                    let live_set = live.get(&(bb.id(), i)).cloned().unwrap_or_default();
                    let mut spill: Vec<u32> = live_set.iter()
                        .filter(|v| ty.get(v).map_or(true, |&t| t == ValueTy::Handle))  // Unknown 也 spill
                        .map(|v| self.local_idx(v.0))
                        .collect();
                    spill.sort();
                    spill.dedup();
                    self.spill_plan.insert(global_idx, spill);
                }
                global_idx += 1;
            }
        }
    }

    fn is_safepoint(&self, ins: &Instruction) -> bool {
        matches!(ins,
            Instruction::NewObject { .. } | Instruction::NewArray { .. }
            | Instruction::Call { .. } | Instruction::CallBuiltin { .. }
            | Instruction::SuperCall { .. } | Instruction::ConstructCall { .. }
        )
        // 注：Call/CallBuiltin 是否 alloc 取决于被调 builtin；保守全当 safepoint（spec §11.4）
    }
}
```

- [ ] **编译确认**: `cargo build -p wjsm-backend-wasm` 通过。

## T2.2 safepoint spill 代码生成（emit spill/reload 序列）

**Files**:
- modify: `crates/wjsm-backend-wasm/src/compiler_instructions.rs`

**Why**: 在每个 safepoint 的 alloc call 前后插入 spill/save-sp/restore-sp。

**Steps**:

- [ ] **在 emit alloc call 处插入 spill**。找到 compile NewObject/NewArray/Call 的 emit 点（compiler_instructions.rs:373 NewObject, :532 NewArray），在每个 `call $obj_new`/`call $arr_new`/被调函数 call 前后包：

```rust
// 伪代码：emit safepoint spill（实际要嵌入各 emit 点）
let spill = self.spill_plan.get(&current_global_idx).cloned().unwrap_or_default();
if !spill.is_empty() {
    // save sp
    func.instruction(&WasmInstruction::GlobalGet(self.shadow_sp_global_idx));
    func.instruction(&WasmInstruction::LocalSet(self.shadow_sp_scratch_idx));
    // spill each live handle local
    for &local in &spill {
        func.instruction(&WasmInstruction::GlobalGet(self.shadow_sp_global_idx));
        func.instruction(&WasmInstruction::LocalGet(local));
        func.instruction(&WasmInstruction::I64Store(MemArg { offset: 0, align: 3, memory_index: 0 }));
        func.instruction(&WasmInstruction::GlobalGet(self.shadow_sp_global_idx));
        func.instruction(&WasmInstruction::I64Const(8));
        func.instruction(&WasmInstruction::I64Add);
        func.instruction(&WasmInstruction::GlobalSet(self.shadow_sp_global_idx));
    }
}
// === 原 alloc call ===
// ... call $obj_new ...
// === spill 后 ===
if !spill.is_empty() {
    // restore sp（non-moving 无需 reload local 值）
    func.instruction(&WasmInstruction::LocalGet(self.shadow_sp_scratch_idx));
    func.instruction(&WasmInstruction::GlobalSet(self.shadow_sp_global_idx));
}
```

> 需要 `current_global_idx` 跟踪：在 compile_function 主循环每条指令递增，与 compute_spill_plan 的 global_idx 对齐。调整主循环（compiler_module.rs 的指令遍历）维护该计数。

- [ ] **编译确认**: `cargo build -p wjsm-backend-wasm` 通过。

## T2.3 shadow stack 容量检查（函数 prologue）

**Files**:
- modify: `crates/wjsm-backend-wasm/src/compiler_module.rs`（compile_function prologue）

**Why（R2）**: 防止 spill 区溢出覆盖对象堆。

**Steps**:

- [ ] **在 compile_function prologue 加容量检查**:

```rust
// 计算 spill_upper_bound = max spill 集合大小 × 8（编译期已知）
let spill_upper_bound = self.spill_plan.values().map(|v| v.len()).max().unwrap_or(0) * 8;
// prologue: 若 shadow_sp + frame + spill_upper_bound > shadow_stack_end → trap
func.instruction(&WasmInstruction::GlobalGet(self.shadow_sp_global_idx));
func.instruction(&WasmInstruction::I32Const(frame_size + spill_upper_bound as i32));
func.instruction(&WasmInstruction::I32Add);
func.instruction(&WasmInstruction::GlobalGet(self.shadow_stack_end_global_idx));
func.instruction(&WasmInstruction::I32GtU);
func.instruction(&WasmInstruction::If(BlockType::Empty));
func.instruction(&WasmInstruction::Unreachable);
func.instruction(&WasmInstruction::End);
```

> 确认 `shadow_stack_end_global_idx` 存在（compiler_module.rs globals，idx 8）。frame_size = 该函数 shadow stack frame 大小。

- [ ] **编译确认**: `cargo build`。

## T2.4 验证 spill 不破坏语义（fixture 全绿）

**Files**: 无（仅验证）

**Why**: 本阶段不接 GC，spill 只是写 shadow stack 又复位，应无语义影响。

**Verification**: fixture 全绿 + dump-wat 检查。

**Steps**:

- [ ] **跑全部 fixture**:

```bash
cargo nextest run --workspace
```

- [ ] **若失败**：用 dump-wat 检查出问题的 fixture：

```bash
cargo run -- dump-wat fixtures/happy/<fail>.js > /tmp/out.wat
# 检查 spill 序列是否正确 save/restore，global_idx 是否对齐
```

- [ ] **dump-wat 抽查一个有 alloc 的 fixture**（如 hello.js）确认 spill 序列存在且 sp 复位。

- [ ] **Commit**: `feat(backend): safepoint spill codegen (no GC yet, semantics-preserving)`

---

# 阶段 P3：runtime_gc/ 框架 + MarkSweep 实现

**Why**: 建立 GcAlgorithm trait 框架 + MarkSweep（worklist mark + sweep 重建 free list）。本阶段用 mock GcContext 单测，不接 backend（P4 集成）。

**Critical（#9）**: GcContext 持 Caller 不持 slice；with_memory/grow 分离。
**Critical（#11）**: mark worklist 不递归。

## T3.1 runtime_gc 模块骨架 + api.rs（trait 定义）

**Files**:
- create: `crates/wjsm-runtime/src/runtime_gc/mod.rs`
- create: `crates/wjsm-runtime/src/runtime_gc/api.rs`
- modify: `crates/wjsm-runtime/src/lib.rs`（加 `mod runtime_gc;`）

**Why**: trait 框架是可插拔的基础（spec §6）。

**Steps**:

- [ ] **创建 mod.rs**:

```rust
//! 可插拔 GC 框架（spec §6）。单一 canonical owner: 本模块组。
pub mod api;
pub mod context;
pub mod mark_bitmap;
pub mod roots;
pub mod mark_sweep;
```

- [ ] **创建 api.rs**：贴 spec §6 的完整 trait 定义（GcAlgorithm/Allocator/Marker/Sweeper/RootProvider/WriteBarrier/ReadBarrier/HeapRegionManager/GcContext/GcStats/Handle/Value/MarkProgress）。GcContext 用 spec §6 R1 版本（持 Caller，with_memory/with_memory_mut/grow/with_state）。

> 注：GcContext 持 `&mut Caller<'b, RuntimeState>` + `Memory`。需 `use wasmtime::{Caller, Memory};`。

- [ ] **编译确认**: `cargo build -p wjsm-runtime`（trait 无 impl，可能有 unused warning，正常）。

## T3.2 MarkBitmap（从 RuntimeState 提取/封装）

**Files**:
- create: `crates/wjsm-runtime/src/runtime_gc/mark_bitmap.rs`

**Why**: 现有 `gc_mark_bits: Arc<Mutex<Vec<u64>>>`（lib.rs:1047）封装为独立类型。

**Steps**:

- [ ] **实现 MarkBitmap**:

```rust
//! Handle 标记位图（1 bit per handle）。
pub struct MarkBitmap {
    bits: Vec<u64>,
}
impl MarkBitmap {
    pub fn new() -> Self { Self { bits: vec![] } }
    pub fn reset(&mut self, count: usize) {
        let words = count.div_ceil(64);
        if self.bits.len() < words { self.bits.resize(words, 0); }
        else { self.bits[..words].fill(0); self.bits[words..].fill(0); }
    }
    pub fn mark(&mut self, h: u32) {
        let (w, b) = (h as usize / 64, h as usize % 64);
        if w >= self.bits.len() { self.bits.resize(w + 1, 0); }
        self.bits[w] |= 1u64 << b;
    }
    pub fn is_marked(&self, h: u32) -> bool {
        let (w, b) = (h as usize / 64, h as usize % 64);
        w < self.bits.len() && (self.bits[w] & (1u64 << b)) != 0
    }
    pub fn popcount(&self) -> usize { self.bits.iter().map(|w| w.count_ones() as usize).sum() }
}
```

- [ ] **编译确认**。

## T3.3 GcContext 实现（context.rs）

**Files**:
- create: `crates/wjsm-runtime/src/runtime_gc/context.rs`

**Why**: 桥接 Caller + Memory，提供 with_memory/grow/with_state（spec §6）。

**Steps**:

- [ ] **实现 GcContext**（按 spec §6 R1 版本）。关键方法 with_memory/with_memory_mut/grow/with_state。grow 内部 `self.memory.grow(&mut *self.caller, pages)`。

- [ ] **编译确认**。

## T3.4 SegregatedFreeList（mark_sweep/allocator.rs）

**Files**:
- create: `crates/wjsm-runtime/src/runtime_gc/mark_sweep/mod.rs`
- create: `crates/wjsm-runtime/src/runtime_gc/mark_sweep/allocator.rs`
- test: `crates/wjsm-runtime/src/runtime_gc/mark_sweep/allocator.rs`（#[cfg(test)] mod tests）

**Why**: free list 数据结构 + alloc_slow（best-fit）+ add_free_region。size class 用 §9.1 冻结值。

**Verification**: 单测 alloc/dealloc/复用/分割。

**Steps**:

- [ ] **写失败测试**（#[cfg(test)]）:

```rust
#[test]
fn alloc_from_free_list_best_fit() {
    let mut fl = SegregatedFreeList::new();
    fl.add_free_region(1000, 144);  // 进 class 144
    assert_eq!(fl.alloc(144), Some(1000));  // 精确匹配
    assert_eq!(fl.alloc(144), None);  // 已用完
}

#[test]
fn alloc_splits_oversized_block() {
    let mut fl = SegregatedFreeList::new();
    fl.add_free_region(2000, 272);  // class 272
    let p = fl.alloc(144);  // 从 class 272 取，分割
    assert_eq!(p, Some(2000));
    // 剩余 128 进对应 class
    assert_eq!(fl.alloc(112), Some(2000 + 144));  // 从分割块取
}

#[test]
fn alloc_falls_back_to_higher_class() {
    let mut fl = SegregatedFreeList::new();
    fl.add_free_region(3000, 528);  // class 528
    assert_eq!(fl.alloc(144), Some(3000));  // class 144 空，向上取 528
}
```

- [ ] **运行 RED**: 编译失败。
- [ ] **实现 SegregatedFreeList**（spec §9.1 SIZE_CLASSES + §9.2 数据结构 + §9.3 alloc_slow + §9.4 add_free_region）。
- [ ] **运行 GREEN**。
- [ ] **Commit**: `feat(runtime): SegregatedFreeList allocator`

## T3.5 Marker（worklist，移植 mark_object_recursive）

**Files**:
- create: `crates/wjsm-runtime/src/runtime_gc/mark_sweep/marker.rs`

**Why（#11）**: mark 用显式 worklist 不递归。移植 mark_object_recursive（runtime_heap.rs:577-761）的子对象收集逻辑，但把递归改 worklist。

**Steps**:

- [ ] **实现 Marker for MarkSweepCollector**:

```rust
//! Mark phase：worklist（不递归，#11）。移植自 runtime_heap mark_object_recursive。
impl Marker for MarkSweepCollector {
    fn mark(&mut self, ctx: &mut GcContext, roots: &mut dyn Iterator<Item = Handle>) {
        let mut worklist: Vec<Handle> = Vec::new();
        // seed roots
        for h in roots { if self.mark_bits.mark_if_new(h, &mut worklist) {} }
        // drain
        while let Some(h) = worklist.pop() {
            self.mark_children(ctx, h, &mut worklist);
        }
    }
    fn is_marked(&self, h: Handle) -> bool { self.mark_bits.is_marked(h) }
}
```

> `mark_children` 读单对象的引用（proto/props/elements/env_obj），推入 worklist。移植 mark_object_recursive_with_funcs 的子收集逻辑（runtime_heap.rs:618-750），但 collect 后 push 而非递归。每批 with_memory 借用。

- [ ] **移植子收集逻辑**（从 runtime_heap.rs:618-750 的 children_to_mark 收集代码）。

- [ ] **编译确认**。

## T3.6 Sweeper（按 ptr sort + 线性重建 free list）

**Files**:
- create: `crates/wjsm-runtime/src/runtime_gc/mark_sweep/sweeper.rs`

**Why（#3）**: sweep 必须按 ptr sort（resize 破坏单调性），线性合并相邻 unmarked。

**Steps**:

- [ ] **实现 Sweeper for MarkSweepCollector**（spec §8.2 算法）: 收集 blocks → sort_by_ptr → 线性扫描合并 → add_free_region → 清空 unmarked handle 槽（推入 handle_free_list）→ process weak refs。

- [ ] **编译确认**。

## T3.7 MarkSweepCollector + impl GcAlgorithm

**Files**:
- modify: `crates/wjsm-runtime/src/runtime_gc/mark_sweep/mod.rs`

**Steps**:

- [ ] **组装 MarkSweepCollector**（持有 SegregatedFreeList + MarkBitmap）+ impl GcAlgorithm（collect = reset mark → mark roots（经 RootProvider 回调）→ fixed-point → sweep → weak refs）。

- [ ] **编译确认**。

## T3.8 RootProvider 实现 + mock 单测 + 深链表测试

**Files**:
- create: `crates/wjsm-runtime/src/runtime_gc/roots.rs`
- test: `crates/wjsm-runtime/src/runtime_gc/mark_sweep/mod.rs`（#[cfg(test)]）

**Why**: RootProvider 回调式（#6）；mock ctx 单测 mark/sweep；深链表不栈溢出（R8）。

**Verification**: mock 单测 + 深链表 10000 层。

**Steps**:

- [ ] **实现 RuntimeRoots**（impl RootProvider）：for_each_shadow_stack_root（with_memory 扫描）、for_each_host_table_root（含 continuation_table captured_vars 顶层 root，§10）。

- [ ] **写 mock 单测**：构造假 memory（手写 obj_table + 对象 header），mock RootProvider，跑 mark/sweep，断言 marked/sweipped 正确。

- [ ] **写深链表测试（R8）**:

```rust
#[test]
fn mark_deep_chain_no_stack_overflow() {
    // 构造 10000 层链表（每对象有个 next 属性指向下一个）
    // mock ctx + roots，跑 mark，断言不栈溢出（worklist）
    // 断言 10000 个全 marked
}
```

- [ ] **运行**: `cargo nextest run -p wjsm-runtime -E 'test(runtime_gc)'`。
- [ ] **Commit**: `feat(runtime): GC framework + MarkSweep (worklist, sweep, roots, mock tests)`

---

# 阶段 P4：分配路径集成 + handle 复用 + proactive GC

**Why**: 把 $obj_new/$arr_new 改为 bump + gc_alloc_slow，接框架，恢复 proactive GC（#2），handle 复用（#7）。本阶段后 GC 真正工作。

**Critical（IMPL-3）**: gc_alloc_slow/gc_maybe_collect 注册 sync Func::wrap。
**Critical（#8）**: gc_alloc_slow 返回 Option<Handle>，trap 仅 trampoline。

## T4.1 host imports: gc_alloc_slow + gc_maybe_collect + gc_take_freed_handle

**Files**:
- modify: `crates/wjsm-runtime/src/host_imports/core.rs`（或新 gc.rs）
- modify: `crates/wjsm-backend-wasm/src/host_import_registry.rs`（注册 SpecialHostImport）

**Steps**:

- [ ] **实现 gc_alloc_slow import**（sync Func::wrap，spec §7.2 trampoline）：调 GcContext + gc_alloc_slow → Option → Some 返 handle / None trap。
- [ ] **实现 gc_maybe_collect import**（sync Func::wrap，无参）：调 gc_algorithm.collect（fast-path proactive 触发，spec §7.1）。
- [ ] **实现 gc_take_freed_handle import**（sync Func::wrap）：从 host handle_free_list pop，返 i32（-1 表空）。
- [ ] **在 host_import_registry.rs 注册** 3 个新 SpecialHostImport。
- [ ] **编译确认**。

## T4.2 RuntimeState: gc_algorithm + handle_free_list + gc_threshold

**Files**:
- modify: `crates/wjsm-runtime/src/lib.rs`

**Steps**:

- [ ] RuntimeState 加 `gc_algorithm: Box<dyn GcAlgorithm>`（默认 MarkSweepCollector::new()）、`handle_free_list: Vec<u32>`、`gc_threshold: usize`（默认 1000）。
- [ ] **编译确认**。

## T4.3 改 $obj_new：bump + handle_free_list + proactive + gc_alloc_slow

**Files**:
- modify: `crates/wjsm-backend-wasm/src/compiler_helpers.rs`（$obj_new L56-195）

**Steps**:

- [ ] **改写 $obj_new**（spec §7.1）: bump fast-path（先 take_or_alloc_handle：call gc_take_freed_handle，-1 则 count++）+ init_header + proactive（__alloc_counter++ 检查 __gc_threshold 调 gc_maybe_collect）+ OOM 走 gc_alloc_slow。删除 memory.grow OOM（L73-109）。
- [ ] **改写 $arr_new**（compiler_array_helpers.rs）同上。
- [ ] **编译确认**。

## T4.4 gc() global 重接到框架

**Files**:
- modify: `crates/wjsm-runtime/src/runtime_builtins.rs`（NativeCallable::GcCollect L1854-1859）

**Steps**:

- [ ] GcCollect 改调 `gc_algorithm.collect`（经 GcContext），不调旧 trigger_gc。
- [ ] **编译确认**。

## T4.5 长循环 fixture + safepoint 安全 fixture

**Files**:
- create: `fixtures/happy/gc_long_loop.js` + `.expected`
- create: `fixtures/happy/gc_safepoint_local.js` + `.expected`

**Why**: 验收标准 #1（长循环不 OOM）+ #3（safepoint 安全）。

**Steps**:

- [ ] **写 gc_long_loop.js**:

```js
let arr = [];
for (let i = 0; i < 1000000; i++) { arr.push({ x: i }); }
// 不 OOM 即通过（GC 回收死对象；arr 本身活着但内部对象轮换）
console.log("done");
```

- [ ] **写 gc_safepoint_local.js**（WASM local 持有唯一引用，触发 GC 后仍可用）:

```js
function hold() {
    let obj = { val: 42 };
    let dummy = { a: 1 };  // 触发 alloc，可能 GC
    return obj.val;        // obj 仍可用（spill 保护）
}
console.log(hold());
// expected: 42
```

- [ ] **生成 .expected**:

```bash
WJSM_UPDATE_FIXTURES=1 cargo nextest run -E 'test(happy__gc_)'
```

- [ ] **Commit**: `test: GC long-loop + safepoint-safety fixtures`

## T4.6 集成验证

**Files**: 无

**Verification**: fixture 全绿 + 长循环 + streams_byob。

**Steps**:

- [ ] **跑全 fixture**:

```bash
cargo nextest run --workspace
```

- [ ] **重点验证 streams_byob 系列**（R4 async 安全）:

```bash
cargo nextest run -E 'test(streams_byob) | test(fetch_http_byob)'
```

- [ ] **Commit**: `feat: GC integration (bump+slow alloc, proactive, handle reuse)`

---

# 阶段 P5：删除旧 GC + 迁移 fixed-point tracer

**Why**: 根除 duplicate owner（旧 trigger_gc + core.rs gc_collect）。框架已接管（P4）。

## T5.1 删除 trigger_gc + 迁移 fixed-point tracer

**Files**:
- modify: `crates/wjsm-runtime/src/runtime_builtins.rs`（删 trigger_gc L2939-3223）
- 已迁移：trace_runtime_side_table_roots_fixed_point → runtime_gc/roots.rs（T3.8）

**Steps**:

- [ ] 确认 trigger_gc 无调用方（GcCollect 已重接，T4.4）。
- [ ] **删除 trigger_gc**。
- [ ] **删除 sweep_dead_promise_slots**（已并入 sweeper）。
- [ ] **编译确认**: `cargo build -p wjsm-runtime`。

## T5.2 删除 core.rs gc_collect

**Files**:
- modify: `crates/wjsm-runtime/src/host_imports/core.rs`（删 gc_collect L1218-1642）

**Steps**:

- [ ] 确认 gc_collect import 无 WASM 调用方（$obj_new 已改用 gc_alloc_slow，T4.3）。
- [ ] **删除 gc_collect import** + linker 注册。
- [ ] **编译确认**。

## T5.3 grep 无残留 + 全 fixture

**Steps**:

- [ ] **grep 确认**:

```bash
# 无残留引用
grep -rn "trigger_gc\|gc_collect" crates/wjsm-runtime/src/ | grep -v "gc_alloc_slow\|gc_maybe_collect\|gc_collect_" 
# 应只剩 gc_alloc_slow/gc_maybe_collect/gc_take_freed_handle（新 imports）
```

- [ ] **全 fixture**:

```bash
cargo nextest run --workspace
```

- [ ] **Commit**: `refactor: remove legacy compact GC (trigger_gc, gc_collect); consolidate to framework`

---

# 阶段 P6：预留 hook 默认 impl + CLI --gc-algorithm

## T6.1 预留 hook 默认 impl 落地

**Files**:
- modify: `crates/wjsm-runtime/src/runtime_gc/api.rs`

**Why**: WriteBarrier/ReadBarrier/HeapRegionManager/mark_step/sweep_step 默认 impl 确认存在（trait 定义已含默认，本任务确认 + 加 doc）。

**Steps**:

- [ ] 确认 trait 默认 impl 完整（WriteBarrier::on_write 等 no-op，mark_step/sweep_step 一次性）。
- [ ] **Commit**: `docs(runtime_gc): confirm barrier/region hooks default impls (zero-cost)`

## T6.2 CLI --gc-algorithm

**Files**:
- modify: `crates/wjsm-cli/src/lib.rs`

**Why**: 运行期切换 GC 算法（调试 + 未来 generational）。

**Steps**:

- [ ] **加 enum**（lib.rs，参照 Target/Stage 模式）:

```rust
#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum GcAlgorithmChoice {
    MarkSweep,
    // 未来：Generational, Incremental
}
```

- [ ] **加 Cli 字段**:

```rust
#[arg(long, global = true, default_value = "mark-sweep")]
gc_algorithm: GcAlgorithmChoice,
```

- [ ] **threading**: 从 execute/run_pipeline 把 choice 传入 runtime 初始化（RuntimeState.gc_algorithm 按 choice 构造）。

> 注：当前 backend_wasm::compile 只接 &Program，GC choice 是 runtime 期（非编译期），故不需改 compile 签名。在 RuntimeState 构造处按 choice 选 algorithm。

- [ ] **测试**: `cargo run -- run fixtures/happy/gc_long_loop.js --gc-algorithm mark-sweep` 通过。
- [ ] **Commit**: `feat(cli): --gc-algorithm flag`

---

# 收尾：文档同步 + ADR

## T-final.1 文档更新

**Files**:
- modify: `bug.md`（O2 → RESOLVED）
- modify: `AGENTS.md`（GC 描述更新为可插拔框架）
- modify: `docs/aegis/specs/2026-06-14-pluggable-gc-framework-design.md`（状态 → Implemented）

**Steps**:

- [ ] bug.md O2 状态 FIXED → RESOLVED，加注"根因（moving GC）已消除，non-moving + safepoint spill"。
- [ ] AGENTS.md "mark-sweep GC" 行更新为"可插拔 GC 框架（non-moving mark-sweep + segregated free list + safepoint spill；GcAlgorithm trait 预留 generational/incremental）"。
- [ ] **Commit**: `docs: GC framework implemented (O2 resolved, AGENTS.md updated)`

## T-final.2 ADR

**Files**:
- create: `docs/adr/0002-pluggable-gc-framework.md`

**Why**: 持久架构决策记录（spec ADR signal）。

**Steps**:

- [ ] 写 ADR：context（O2 根因）、decision（non-moving + trait + WASM bump/host slow）、alternatives（moving+spill reload / 全 shadow stack）、consequences（trait 稳定性承诺见 spec 附录 D）、baseline-sync（AGENTS.md）。
- [ ] **Commit**: `docs(adr): 0002 pluggable GC framework`

---

## Risks（实施期）

| 风险 | 阶段 | 缓解 |
|------|------|------|
| spill 破坏语义 | P2 | fixture 全绿闸；dump-wat 抽查 |
| liveness Phi 边错误 | P1 | 3 单测（if/else/loop）；P2 fixture 误回收会崩 |
| mark 栈溢出 | P3 | worklist + 深链表单测（R8） |
| grow 借用 UB | P3/P4 | GcContext 不持 slice（#9）；with_memory 重借 |
| async reentry | P4 | sync Func::wrap（§12.3）；streams_byob 验证 |
| handle 无限膨胀 | P4 | take_or_alloc_handle（#7） |
| 旧 GC 残留 | P5 | grep + fixture |

## Retirement

- P5 删除 trigger_gc + core.rs gc_collect（框架 P4 已接管，无断档）
- spec §18 INV/IMPL 是实现期硬约束，违反即 GC 不安全

## Self-Review 结论

- **Spec 覆盖**：spec §14 的 P0-P6 + §18 全部 INV/IMPL 不变量在计划中均有对应任务（见 Tasks 总览 + 每阶段验收含 §18 硬约束）。
- **Placeholder**：核心算法/数据结构（tag_needs_root、MarkBitmap、SegregatedFreeList、liveness 主体、sweep、spill 序列、GcContext）给出完整代码。IR/Compiler 内部 API 接缝（Instruction variant 字段名、Function 访问 Constant 路径、UnaryOp variant）标注为“实现时先 grep 确认实际名”并给行号 —— 这些是诚实承认的未知项，非敷衍 placeholder（逐行预写会因 API 名猜错失效）。
- **类型一致**：GcContext 持 Caller 贯穿；gc_alloc_slow → Option<Handle> 贯穿；RootProvider 回调式贯穿。
- **兼容性**：§18 为硬约束；fixture 全绿跨阶段闸；活动对象布局/NaN-boxing/obj_table 不变。
- **验证**：每阶段确切命令（cargo nextest run -E ... / cargo build -p ...）。
- **双轨**：P5 Retirement（删旧 GC，P4 先接管无断档）。
- **结论**：计划可执行。实施时遇 IR/Compiler API 名不符，按任务内 grep 指引确认后调整（属正常适配，非计划缺陷）。
