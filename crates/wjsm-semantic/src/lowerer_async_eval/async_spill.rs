use super::*;
use crate::passes::direct_call::{instr_uses, instruction_dest, terminator_uses};

/// SSA 值的块级 use/def 集合。
///
/// `edge_uses[pred]`：后继块 phi 以 `pred` 为来源使用的值集合——phi 的语义
/// use 发生在前驱边末尾，必须直接并入 `pred` 的 live_out。
struct ValueUseDef {
    use_sets: Vec<HashSet<ValueId>>,
    def_sets: Vec<HashSet<ValueId>>,
    edge_uses: Vec<HashSet<ValueId>>,
}

/// 收集每个 SSA 值的定义所在块。
fn collect_value_def_blocks(blocks: &[BasicBlock]) -> HashMap<ValueId, BasicBlockId> {
    let mut defs = HashMap::new();
    for block in blocks {
        for instruction in block.instructions() {
            if let Some(dest) = instruction_dest(instruction) {
                defs.insert(dest, block.id());
            }
        }
    }
    defs
}

/// 计算每个块对 SSA 值的 use/def 集合（use 只计入本块 def 之前的读取）。
fn compute_value_use_def(blocks: &[BasicBlock]) -> ValueUseDef {
    let block_count = blocks.len();
    let mut use_sets: Vec<HashSet<ValueId>> = vec![HashSet::new(); block_count];
    let mut def_sets: Vec<HashSet<ValueId>> = vec![HashSet::new(); block_count];
    let mut edge_uses: Vec<HashSet<ValueId>> = vec![HashSet::new(); block_count];

    for block in blocks {
        let bid = block.id().0 as usize;
        let mut local_def: HashSet<ValueId> = HashSet::new();
        for instruction in block.instructions() {
            if let Instruction::Phi { sources, .. } = instruction {
                for source in sources {
                    edge_uses[source.predecessor.0 as usize].insert(source.value);
                }
            } else {
                for used in instr_uses(instruction) {
                    if !local_def.contains(&used) {
                        use_sets[bid].insert(used);
                    }
                }
            }
            if let Some(dest) = instruction_dest(instruction) {
                local_def.insert(dest);
                def_sets[bid].insert(dest);
            }
        }
        for used in terminator_uses(block.terminator()) {
            if !local_def.contains(&used) {
                use_sets[bid].insert(used);
            }
        }
    }

    ValueUseDef {
        use_sets,
        def_sets,
        edge_uses,
    }
}

/// 标准后向迭代 liveness（值域为 SSA ValueId），返回每块入口的 live_in。
/// CFG 与变量 liveness 一致：suspend 块的逻辑后继是 resume 块。
fn compute_value_liveness(
    blocks: &[BasicBlock],
    successors: &[Vec<BasicBlockId>],
    sets: &ValueUseDef,
) -> Vec<HashSet<ValueId>> {
    let block_count = blocks.len();
    let mut live_in: Vec<HashSet<ValueId>> = vec![HashSet::new(); block_count];
    let mut live_out: Vec<HashSet<ValueId>> = vec![HashSet::new(); block_count];

    loop {
        let mut changed = false;
        for block in blocks.iter().rev() {
            let bid = block.id().0 as usize;

            let mut new_live_out = sets.edge_uses[bid].clone();
            for successor in &successors[bid] {
                new_live_out.extend(live_in[successor.0 as usize].iter().copied());
            }
            if new_live_out != live_out[bid] {
                live_out[bid] = new_live_out;
                changed = true;
            }

            let mut new_live_in = sets.use_sets[bid].clone();
            for value in &live_out[bid] {
                if !sets.def_sets[bid].contains(value) {
                    new_live_in.insert(*value);
                }
            }
            if new_live_in != live_in[bid] {
                live_in[bid] = new_live_in;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    live_in
}

/// 从块级分配器取新 ValueId（与 `Lowerer::alloc_value` 同规则）。
fn alloc(next_value: &mut u32) -> ValueId {
    let id = ValueId(*next_value);
    *next_value += 1;
    id
}

/// 把 `load` 插到块末尾；若最后一条指令是 Suspend/GeneratorSuspend，则插到
/// 它之前（suspend 块的不变量：Suspend 之后不允许再有指令）。
fn insert_load_at_block_end(block: &mut BasicBlock, load: Instruction) {
    let position = match block.instructions().last() {
        Some(Instruction::Suspend { .. } | Instruction::GeneratorSuspend { .. }) => {
            block.instructions().len() - 1
        }
        _ => block.instructions().len(),
    };
    block.instructions_mut().insert(position, load);
}

/// 把指令中对 `from` 的全部使用替换为 `to`（phi source 由调用方单独处理）。
fn replace_value_in_instruction(instruction: &mut Instruction, from: ValueId, to: ValueId) {
    let replace = |value: &mut ValueId| {
        if *value == from {
            *value = to;
        }
    };
    let replace_all = |values: &mut Vec<ValueId>| {
        for value in values {
            if *value == from {
                *value = to;
            }
        }
    };
    use Instruction::*;
    match instruction {
        Binary { lhs, rhs, .. } | Compare { lhs, rhs, .. } => {
            replace(lhs);
            replace(rhs);
        }
        Unary { value, .. }
        | StoreVar { value, .. }
        | Suspend { promise: value, .. }
        | GeneratorSuspend { result: value, .. }
        | IsException { value, .. }
        | EncodeException { value, .. }
        | ExceptionToObject { value, .. }
        | GuardSameFunction { callee: value, .. }
        | GuardElementsKind { array: value, .. }
        | GuardShape { object: value, .. }
        | LoadSlot { object: value, .. }
        | GuardTag { value, .. }
        | GuardCallTarget { callee: value, .. } => replace(value),
        StringConcatVa { parts, .. } => replace_all(parts),
        InitObjectLiteral { values, .. } => replace_all(values),
        CallBuiltin { args, .. } => replace_all(args),
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
        }
        | ConstructCall {
            callee,
            this_val,
            args,
            ..
        } => {
            replace(callee);
            replace(this_val);
            replace_all(args);
        }
        GetProp {
            object, key, latch, ..
        } => {
            replace(object);
            replace(key);
            if let Some(latch) = latch {
                replace(latch);
            }
        }
        DeleteProp { object, key, .. } => {
            replace(object);
            replace(key);
        }
        GetElem {
            object,
            index,
            latch,
            ..
        } => {
            replace(object);
            replace(index);
            if let Some(latch) = latch {
                replace(latch);
            }
        }
        SetProp {
            object, key, value, ..
        }
        | CreateDataProperty {
            object, key, value, ..
        }
        | SetElem {
            object,
            index: key,
            value,
            ..
        } => {
            replace(object);
            replace(key);
            replace(value);
        }
        SetProto { object, value } => {
            replace(object);
            replace(value);
        }
        // ObjectSpread 的 use 是被写入对象与 spread 源；dest 是结果定义。
        ObjectSpread { object, source, .. } => {
            replace(object);
            replace(source);
        }
        PromiseResolve { promise, value }
        | PromiseReject {
            promise,
            reason: value,
        } => {
            replace(promise);
            replace(value);
        }
        Const { .. }
        | LoadVar { .. }
        | Phi { .. }
        | NewObject { .. }
        | NewArray { .. }
        | CloneArrayTemplate { .. }
        | GetSuperBase { .. }
        | GetSuperConstructor { .. }
        | NewPromise { .. }
        | CollectRestArgs { .. }
        | DebugCheck { .. } => {}
        LoadEnvSlot { dest, env, key, .. } => {
            replace(dest);
            replace(env);
            replace(key);
        }
        StoreEnvSlot {
            dest,
            env,
            value,
            key,
            ..
        } => {
            if let Some(dest) = dest {
                replace(dest);
            }
            replace(env);
            replace(value);
            replace(key);
        }
        StoreSlot { object, value, .. } => {
            replace(object);
            replace(value);
        }
    }
}

/// 把 terminator 中对 `from` 的使用替换为 `to`。
fn replace_value_in_terminator(terminator: &mut Terminator, from: ValueId, to: ValueId) {
    match terminator {
        Terminator::Return { value: Some(value) }
        | Terminator::Throw { value }
        | Terminator::Branch {
            condition: value, ..
        }
        | Terminator::Switch { value, .. } => {
            if *value == from {
                *value = to;
            }
        }
        Terminator::Deopt { frames } => {
            for frame in frames {
                for live in &mut frame.lives {
                    if *live == from {
                        *live = to;
                    }
                }
            }
        }
        Terminator::Return { value: None } | Terminator::Jump { .. } | Terminator::Unreachable => {}
    }
}

/// 把 SSA 值 `value` 降级为具名变量 `name`：定义处紧随 StoreVar，
/// 每个使用点改为紧邻的 LoadVar 新值。降级后该值仅剩 def 与 spill store，
/// 支配性自然成立；跨 suspend 的取值改经变量 save/restore 机制传递。
fn demote_value_to_var(
    blocks: &mut [BasicBlock],
    next_value: &mut u32,
    value: ValueId,
    name: &str,
) {
    insert_spill_store_after_def(blocks, value, name);
    for block in blocks.iter_mut() {
        rewrite_block_uses(block, next_value, value, name);
    }
    patch_phi_sources(blocks, next_value, value, name);
}

/// 在 `value` 的定义指令之后紧随插入 spill StoreVar。
fn insert_spill_store_after_def(blocks: &mut [BasicBlock], value: ValueId, name: &str) {
    for block in blocks.iter_mut() {
        if let Some(position) = block
            .instructions()
            .iter()
            .position(|instruction| instruction_dest(instruction) == Some(value))
        {
            block.instructions_mut().insert(
                position + 1,
                Instruction::StoreVar {
                    name: name.to_string(),
                    value,
                },
            );
            return;
        }
    }
}

/// 把单个块内（含 terminator）对 `value` 的使用改写为紧邻的 LoadVar 新值。
/// 跳过 spill StoreVar 本身（它必须保持对原值的引用）；phi 由调用方单独处理。
fn rewrite_block_uses(block: &mut BasicBlock, next_value: &mut u32, value: ValueId, name: &str) {
    let mut index = 0;
    while index < block.instructions().len() {
        let instruction = &block.instructions()[index];
        let is_spill_store = matches!(
            instruction,
            Instruction::StoreVar { name: store_name, value: stored }
                if store_name == name && *stored == value
        );
        let uses_value = !is_spill_store
            && !matches!(instruction, Instruction::Phi { .. })
            && instr_uses(instruction).contains(&value);
        if uses_value {
            let fresh = alloc(next_value);
            block.instructions_mut().insert(
                index,
                Instruction::LoadVar {
                    dest: fresh,
                    name: name.to_string(),
                },
            );
            replace_value_in_instruction(&mut block.instructions_mut()[index + 1], value, fresh);
            index += 2;
        } else {
            index += 1;
        }
    }
    if terminator_uses(block.terminator()).contains(&value) {
        let fresh = alloc(next_value);
        insert_load_at_block_end(
            block,
            Instruction::LoadVar {
                dest: fresh,
                name: name.to_string(),
            },
        );
        replace_value_in_terminator(block.terminator_mut(), value, fresh);
    }
}

/// 把 phi source 对 `value` 的使用改写为前驱块末尾的 LoadVar 新值。
/// phi 的语义 use 发生在前驱边；phi 一定位于块首，向前驱末尾插入
/// 不影响已记录的 phi 指令下标。
fn patch_phi_sources(blocks: &mut [BasicBlock], next_value: &mut u32, value: ValueId, name: &str) {
    let mut patches: Vec<(usize, usize, usize, BasicBlockId)> = Vec::new();
    for (block_index, block) in blocks.iter().enumerate() {
        for (instruction_index, instruction) in block.instructions().iter().enumerate() {
            if let Instruction::Phi { sources, .. } = instruction {
                for (source_index, source) in sources.iter().enumerate() {
                    if source.value == value {
                        patches.push((
                            block_index,
                            instruction_index,
                            source_index,
                            source.predecessor,
                        ));
                    }
                }
            }
        }
    }
    for (block_index, instruction_index, source_index, predecessor) in patches {
        let fresh = alloc(next_value);
        insert_load_at_block_end(
            &mut blocks[predecessor.0 as usize],
            Instruction::LoadVar {
                dest: fresh,
                name: name.to_string(),
            },
        );
        if let Instruction::Phi { sources, .. } =
            &mut blocks[block_index].instructions_mut()[instruction_index]
        {
            sources[source_index].value = fresh;
        }
    }
}

impl Lowerer {
    /// 把跨 suspend 存活的 SSA 临时值溢出为具名帧局部变量。
    ///
    /// resume 后函数经 dispatch switch 重新进入，suspend 之前定义的 SSA 临时值
    /// 在恢复路径上没有定义（支配性破坏；运行时寄存器/帧也早已随 return 失效）。
    /// 典型触发：`s += await p`、`f(x, await p)` 等在 await 前先求值的中间结果。
    /// 这里把此类值降级为「定义处 StoreVar + 使用点 LoadVar」的具名变量，并把
    /// 变量名注册进各 suspend 的可见绑定，交由 resolve_pending_suspends 的变量
    /// liveness 机制照常生成 continuation save/restore。
    ///
    /// 入口块定义的值除外：dispatch 路径每次恢复都会重新执行入口序言，其值
    /// 天然重建（与既有行为一致），无需溢出。
    pub(super) fn spill_cross_suspend_temps(&mut self, pending: &mut [PendingSuspend]) {
        let spill_values: Vec<ValueId> = {
            let blocks = self.current_function.blocks();
            if blocks.is_empty() {
                return;
            }
            let entry = blocks[0].id();
            let (successors, _predecessors) = build_cfg(blocks, pending);
            let def_blocks = collect_value_def_blocks(blocks);
            let sets = compute_value_use_def(blocks);
            let live_in = compute_value_liveness(blocks, &successors, &sets);

            let mut ordered: Vec<u32> = pending
                .iter()
                .flat_map(|suspend| live_in[suspend.resume_block.0 as usize].iter())
                .filter(|value| def_blocks.get(value).is_some_and(|block| *block != entry))
                .map(|value| value.0)
                .collect();
            ordered.sort_unstable();
            ordered.dedup();
            ordered.into_iter().map(ValueId).collect()
        };
        if spill_values.is_empty() {
            return;
        }

        let function_scope = self.function_scope_id_stack.last().copied().unwrap_or(0);
        let mut spill_names = Vec::with_capacity(spill_values.len());
        for (index, value) in spill_values.into_iter().enumerate() {
            let name = format!("$suspend_spill_{function_scope}_{index}");
            demote_value_to_var(
                &mut self.current_function.blocks,
                &mut self.next_value,
                value,
                &name,
            );
            spill_names.push(name);
        }
        for suspend in pending.iter_mut() {
            suspend.visible_bindings.extend(spill_names.iter().cloned());
        }
    }
}
