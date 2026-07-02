//! Per-ValueId liveness 分析（spec §11.1）。
//!
//! 供 GC safepoint spill 用：safepoint 处需知道哪些 ValueId 活跃且为 Handle 类型。
//!
//! 算法：标准 backward dataflow，块级 live_in/live_out 迭代到不动点，
//! 再块内 backward 细化到 per-instruction。
//!
//! **Phi 边分发（关键，#10）**：`Phi { dest, sources }` 的每个入参
//! `PhiSource { predecessor, value }` 不计入所在块的 use 集，而是对
//! `predecessor` 块的 live_out 贡献 `value`。这样 if/else/loop 汇合点的
//! Phi 源只在对应前驱边活跃，不会污染其他分支。
//!
//! **契约**：`compute_liveness(f)[(block_id, i)]` = 紧邻指令 `i` 执行*前*
//! 活跃的 ValueId 集合；`(block_id, len)` = 块出口（live_out）。
use std::collections::{HashMap, HashSet};
use wjsm_ir::{BasicBlockId, Function, Instruction, PhiSource, Terminator, ValueId};

/// 计算每个 block 的后继列表。
fn successors(f: &Function) -> HashMap<BasicBlockId, Vec<BasicBlockId>> {
    let mut succ: HashMap<BasicBlockId, Vec<BasicBlockId>> = HashMap::new();
    for bb in f.blocks() {
        let s: Vec<BasicBlockId> = match bb.terminator() {
            Terminator::Return { .. } | Terminator::Throw { .. } | Terminator::Unreachable => {
                vec![]
            }
            Terminator::Jump { target } => vec![*target],
            Terminator::Branch {
                true_block,
                false_block,
                ..
            } => vec![*true_block, *false_block],
            Terminator::Switch {
                cases,
                default_block,
                exit_block,
                ..
            } => {
                // 去重（BasicBlockId 无 Ord，用 HashSet）
                let mut seen: HashSet<BasicBlockId> = HashSet::new();
                let mut v: Vec<BasicBlockId> = Vec::new();
                for t in cases
                    .iter()
                    .map(|c| c.target)
                    .chain([*default_block, *exit_block])
                {
                    if seen.insert(t) {
                        v.push(t);
                    }
                }
                v
            }
        };
        succ.insert(bb.id(), s);
    }
    succ
}

/// 取 producing instruction 的 dest（def）。非 producing 返回 None。
fn instr_dest(ins: &Instruction) -> Option<ValueId> {
    use Instruction::*;
    Some(match ins {
        Const { dest, .. }
        | Binary { dest, .. }
        | Unary { dest, .. }
        | Compare { dest, .. }
        | Phi { dest, .. }
        | StringConcatVa { dest, .. }
        | LoadVar { dest, .. }
        | NewObject { dest, .. }
        | GetProp { dest, .. }
        | DeleteProp { dest, .. }
        | NewArray { dest, .. }
        | GetElem { dest, .. }
        | OptionalGetProp { dest, .. }
        | OptionalGetElem { dest, .. }
        | OptionalCall { dest, .. }
        | ObjectSpread { dest, .. }
        | GetSuperBase { dest }
        | GetSuperConstructor { dest }
        | NewPromise { dest }
        | CollectRestArgs { dest, .. }
        | IsException { dest, .. }
        | EncodeException { dest, .. }
        | ExceptionToObject { dest, .. } => *dest,
        Call { dest, .. }
        | CallBuiltin { dest, .. }
        | SuperCall { dest, .. }
        | ConstructCall { dest, .. } => (*dest)?,
        // 非 producing
        StoreVar { .. }
        | SetProp { .. }
        | SetProto { .. }
        | SetElem { .. }
        | PromiseResolve { .. }
        | PromiseReject { .. }
        | Suspend { .. }
        | GeneratorSuspend { .. } => return None,
    })
}

/// 取非 Phi 指令的 ValueId 操作数（uses）。Phi 的 use 经边分发处理，不在此返回。
fn instr_uses(ins: &Instruction) -> Vec<ValueId> {
    use Instruction::*;
    match ins {
        Binary { lhs, rhs, .. } | Compare { lhs, rhs, .. } => vec![*lhs, *rhs],
        Unary { value, .. } => vec![*value],
        StringConcatVa { parts, .. } => parts.clone(),
        GetProp { object, key, .. } => vec![*object, *key],
        SetProp { object, key, value } => vec![*object, *key, *value],
        SetProto { object, value } => vec![*object, *value],
        LoadVar { .. } => vec![],
        NewObject { .. }
        | NewArray { .. }
        | GetSuperBase { .. }
        | GetSuperConstructor { .. }
        | NewPromise { .. }
        | CollectRestArgs { .. } => vec![],
        GetElem { object, index, .. } => vec![*object, *index],
        SetElem {
            object: _,
            index,
            value,
        } => vec![*index, *value],
        OptionalGetProp { object, key, .. } | OptionalGetElem { object, key, .. } => {
            vec![*object, *key]
        }
        OptionalCall {
            callee,
            this_val,
            args,
            ..
        } => {
            let mut v = vec![*callee, *this_val];
            v.extend(args.iter().copied());
            v
        }
        ObjectSpread { source, .. } => vec![*source],
        Call {
            callee,
            this_val,
            args,
            ..
        }
        | SuperCall {
            callee,
            this_val,
            args,
            ..
        } => {
            let mut v = vec![*callee, *this_val];
            v.extend(args.iter().copied());
            v
        }
        ConstructCall {
            callee,
            this_val,
            args,
            ..
        } => {
            // ConstructCall 无 dest，但消费 callee/this/args
            let mut v = vec![*callee, *this_val];
            v.extend(args.iter().copied());
            v
        }
        CallBuiltin { args, .. } => args.clone(),
        DeleteProp { object, key, .. } => vec![*object, *key],
        PromiseResolve { promise, value }
        | PromiseReject {
            promise,
            reason: value,
        } => {
            vec![*promise, *value]
        }
        Suspend { promise, .. } => vec![*promise],
        GeneratorSuspend { result, .. } => vec![*result],
        IsException { value, .. }
        | EncodeException { value, .. }
        | ExceptionToObject { value, .. } => {
            vec![*value]
        }
        Phi { .. } => vec![], // Phi use 经边分发，不计入块 use 集
        Const { .. } => vec![],
        StoreVar { value, .. } => vec![*value],
    }
}

/// 块级 use/def + Phi 源（按后继块索引）。
/// 返回 (block_use, block_def, phi_sources)。
/// phi_sources[succ_block][pred_block] = 该 succ 的 Phi 中来自 pred 的入参 ValueId 列表。
/// 类型别名：减少嵌套 HashMap 的复杂度
type BlockUse = HashMap<BasicBlockId, HashSet<ValueId>>;
type BlockDef = HashMap<BasicBlockId, HashSet<ValueId>>;
type PhiSources = HashMap<BasicBlockId, HashMap<BasicBlockId, Vec<ValueId>>>;

fn block_use_def_phi(f: &Function) -> (BlockUse, BlockDef, PhiSources) {
    let mut block_use: HashMap<BasicBlockId, HashSet<ValueId>> = HashMap::new();
    let mut block_def: HashMap<BasicBlockId, HashSet<ValueId>> = HashMap::new();
    // phi_sources[succ][pred] = Vec<ValueId>
    let mut phi_sources: HashMap<BasicBlockId, HashMap<BasicBlockId, Vec<ValueId>>> =
        HashMap::new();

    for bb in f.blocks() {
        let mut uses = HashSet::new();
        let mut defs = HashSet::new();
        for ins in bb.instructions() {
            if let Some(d) = instr_dest(ins) {
                defs.insert(d);
            }
            // Phi 的 use 不计入；其他指令的 use 计入（先 use 后 def 语义：若同一变量
            // 在块内先 use 后 def，仍算 live-through，但 wjsm IR 是 SSA-like，dest
            // 唯一，故 use 与 def 不重叠，顺序不影响 use 集）
            for u in instr_uses(ins) {
                uses.insert(u);
            }
            // Phi：dest 是 def；sources 经边分发
            if let Instruction::Phi { dest, sources } = ins {
                defs.insert(*dest);
                for PhiSource { predecessor, value } in sources {
                    phi_sources
                        .entry(bb.id())
                        .or_default()
                        .entry(*predecessor)
                        .or_default()
                        .push(*value);
                }
            }
        }
        // terminator 的 use（return 值 / branch 条件 / throw 值 / switch 值）
        // terminator 无 def，其 use 计入块 use 集。
        for u in terminator_uses(bb.terminator()) {
            uses.insert(u);
        }
        block_use.insert(bb.id(), uses);
        block_def.insert(bb.id(), defs);
    }
    (block_use, block_def, phi_sources)
}

/// 取 terminator 的 ValueId 操作数（uses）。terminator 无 def。
fn terminator_uses(t: &Terminator) -> Vec<ValueId> {
    match t {
        Terminator::Return { value: Some(v) } => vec![*v],
        Terminator::Return { value: None } => vec![],
        Terminator::Jump { .. } | Terminator::Unreachable => vec![],
        Terminator::Branch { condition, .. } => vec![*condition],
        Terminator::Switch { value, .. } => vec![*value],
        Terminator::Throw { value } => vec![*value],
    }
}

/// 计算 per-instruction 活跃集。
///
/// `result[(block_id, i)]` = 紧邻指令 `i` 执行*前*活跃的 ValueId 集合。
/// `(block_id, len)` = 块出口（= live_out）。
pub fn compute_liveness(f: &Function) -> HashMap<(BasicBlockId, usize), HashSet<ValueId>> {
    let succ = successors(f);
    let (block_use, block_def, phi_sources) = block_use_def_phi(f);

    // ── 块级 backward dataflow 迭代到不动点 ──
    let mut live_in: HashMap<BasicBlockId, HashSet<ValueId>> = HashMap::new();
    let mut live_out: HashMap<BasicBlockId, HashSet<ValueId>> = HashMap::new();
    let mut changed = true;
    while changed {
        changed = false;
        for bb in f.blocks().iter().rev() {
            let id = bb.id();
            // live_out = ∪ successors: (后继 live_in) ∪ (该后继 Phi 中来自本块的入参)
            let mut out = HashSet::new();
            for &s in succ.get(&id).unwrap_or(&vec![]) {
                if let Some(pred_map) = phi_sources.get(&s)
                    && let Some(srcs) = pred_map.get(&id)
                {
                    out.extend(srcs.iter().copied());
                }
                out.extend(live_in.get(&s).unwrap_or(&HashSet::new()).iter().copied());
            }
            // live_in = use ∪ (live_out \ def)
            let defs = block_def.get(&id).unwrap();
            let mut in_ = out.clone();
            in_.retain(|v| !defs.contains(v));
            in_.extend(block_use.get(&id).unwrap().iter().copied());

            if live_out.get(&id) != Some(&out) {
                live_out.insert(id, out);
                changed = true;
            }
            if live_in.get(&id) != Some(&in_) {
                live_in.insert(id, in_);
                changed = true;
            }
        }
    }

    // ── 块内 backward 细化到 per-instruction ──
    // 细化起点 = live_out ∪ terminator_uses（terminator 在最后一条指令之后、块出口
    // 之前执行，其 use 在“最后一条指令后”这一刻活跃）。从该起点 backward：
    // 每条指令 i：live = (live ∪ uses(i)) \ defs(i)，写入 per_instr[(id, i)]。
    //
    // 契约：per_instr[(id, len)] = 最后一条指令执行*后*（terminator 执行前）活跃集
    //       = live_out ∪ terminator_uses。
    //       per_instr[(id, i)] = 紧邻指令 i 执行*前*活跃集。
    let mut per_instr: HashMap<(BasicBlockId, usize), HashSet<ValueId>> = HashMap::new();
    for bb in f.blocks() {
        let id = bb.id();
        let mut live = live_out.get(&id).cloned().unwrap_or_default();
        // terminator 的 use 在块出口（最后一条指令后）活跃
        for u in terminator_uses(bb.terminator()) {
            live.insert(u);
        }
        let instrs = bb.instructions();
        per_instr.insert((id, instrs.len()), live.clone());
        for (i, ins) in instrs.iter().enumerate().rev() {
            // Phi 的 def/use 不在此细化（已由块级 + 边分发处理）：
            //   - Phi dest 在块级 def 集中（live 已在进入块时移除）
            //   - Phi sources 经边分发到前驱 live_out
            // 故 per-instruction 细化时，Phi 指令不改变 live（其 def 已反映在块入口，
            //   sources 不在本块消费）。跳过 Phi 的 def/use 调整。
            if !matches!(ins, Instruction::Phi { .. }) {
                if let Some(d) = instr_dest(ins) {
                    live.remove(&d);
                }
                for u in instr_uses(ins) {
                    live.insert(u);
                }
            }
            per_instr.insert((id, i), live.clone());
        }
    }
    per_instr
}

/// 计算 per-instruction **变量槽占用**（供 GC safepoint spill）。
///
/// 与经典 live-variable（load-use）不同：JS 变量在 `StoreVar` 后一直占着 wasm local，
/// 直到下一次对同名 `StoreVar` 覆盖。期间 handle 仅活在变量 local 里，per-ValueId
/// liveness 不可见；若 safepoint 不 spill 该 local，GC 会误回收。
///
/// 块入口合并前驱 `occupied_out`（循环头取并集）；块内按指令正向维护占用集。
/// 契约键与 `compute_var_liveness` 相同，供 `current_spill_locals` 查询。
pub fn compute_var_liveness(f: &Function) -> HashMap<(BasicBlockId, usize), HashSet<String>> {
    let succ = successors(f);

    // 块级 occupied_out：块出口时哪些变量槽仍持有值（正向数据流不动点）。
    let mut occupied_in: HashMap<BasicBlockId, HashSet<String>> = HashMap::new();
    let mut occupied_out: HashMap<BasicBlockId, HashSet<String>> = HashMap::new();
    let mut changed = true;
    while changed {
        changed = false;
        for bb in f.blocks() {
            let id = bb.id();
            let mut in_set: HashSet<String> = HashSet::new();
            let preds: Vec<BasicBlockId> = f
                .blocks()
                .iter()
                .filter_map(|b| {
                    let bid = b.id();
                    succ.get(&bid)
                        .map(|ss| ss.as_slice())
                        .and_then(|ss| ss.contains(&id).then_some(bid))
                })
                .collect();
            for p in &preds {
                in_set.extend(occupied_out.get(p).cloned().unwrap_or_default());
            }

            let mut out_set = in_set.clone();
            for ins in bb.instructions() {
                if let Instruction::StoreVar { name, .. } = ins {
                    out_set.insert(name.clone());
                }
            }

            if occupied_in.get(&id) != Some(&in_set) {
                occupied_in.insert(id, in_set);
                changed = true;
            }
            if occupied_out.get(&id) != Some(&out_set) {
                occupied_out.insert(id, out_set);
                changed = true;
            }
        }
    }

    // 块内正向：指令 i 之前槽占用 = 入口 occupied_in + 此前 StoreVar。
    let mut per_instr: HashMap<(BasicBlockId, usize), HashSet<String>> = HashMap::new();
    for bb in f.blocks() {
        let id = bb.id();
        let mut slots = occupied_in.get(&id).cloned().unwrap_or_default();
        let instrs = bb.instructions();
        for (i, ins) in instrs.iter().enumerate() {
            per_instr.insert((id, i), slots.clone());
            if let Instruction::StoreVar { name, .. } = ins {
                slots.insert(name.clone());
            }
        }
        per_instr.insert((id, instrs.len()), slots);
    }
    per_instr
}
