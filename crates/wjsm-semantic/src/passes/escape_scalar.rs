//! 逃逸分析 + 标量替换（Escape Analysis + Scalar Replacement）pass。
//!
//! 识别函数内局部 `NewObject` 对象，如果对象不逃逸（所有使用都是 SetProp/GetProp
//! 常量字符串键、SetProto、CallBuiltin(IsJsObject)），则：
//!
//! 1. 把 GetProp 读取结果的使用处替换为 SetProp 写入的值；
//! 2. 删除 NewObject、SetProp、SetProto、CallBuiltin(IsJsObject) 指令。
//!
//! 保守策略：任何非上述模式的使用（Call、Return、StoreVar、Phi、LoadVar 等）→ 逃逸。

use super::cfg_fold::terminator_successors;
use super::direct_call::{collect_uses, instr_uses, instruction_dest, terminator_uses};
use std::collections::{BTreeSet, HashMap, HashSet};
use wjsm_ir::{
    BasicBlockId, Builtin, Constant, ConstantId, FunctionId, Instruction, Module, Terminator,
    ValueId, is_host_shared_variable,
};

/// 支配关系查询：CHK（Cooper–Harvey–Kennedy）idom 不动点 + 支配树 DFS 区间，
/// 构建 O(边数 × 少量轮次)，`dominates` 查询 O(1)。旧的显式支配集算法在每轮
/// 内重扫全部终止器求前驱，块数上千时（intrinsic 守卫分叉展开后常见）呈立方
/// 级爆炸。不可达块沿用旧约定：被任何块支配（其支配集从未收缩），自身不支配
/// 任何可达块。
struct Dominators {
    /// 支配树 DFS 进入序（按块 id 索引；不可达块无意义）。
    tin: Vec<u32>,
    /// 支配树 DFS 离开序。
    tout: Vec<u32>,
    reachable: Vec<bool>,
}

impl Dominators {
    fn compute(function: &wjsm_ir::Function) -> Self {
        let n = function.blocks().len();
        let entry = function.entry().0 as usize;
        // 迭代 DFS 求后序；逆序即 RPO。
        let mut postorder = Vec::with_capacity(n);
        let mut reachable = vec![false; n];
        let mut stack = vec![(entry, false)];
        while let Some((block, expanded)) = stack.pop() {
            if expanded {
                postorder.push(block);
                continue;
            }
            if reachable[block] {
                continue;
            }
            reachable[block] = true;
            stack.push((block, true));
            for succ in terminator_successors(function.blocks()[block].terminator()) {
                let succ = succ.0 as usize;
                if succ < n && !reachable[succ] {
                    stack.push((succ, false));
                }
            }
        }
        let mut rpo_index = vec![u32::MAX; n];
        for (index, &block) in postorder.iter().rev().enumerate() {
            rpo_index[block] = index as u32;
        }
        // 前驱表只建一次（仅可达块出边）。
        let mut preds: Vec<Vec<usize>> = vec![Vec::new(); n];
        for (block, block_reachable) in reachable.iter().enumerate() {
            if !*block_reachable {
                continue;
            }
            for succ in terminator_successors(function.blocks()[block].terminator()) {
                let succ = succ.0 as usize;
                if succ < n {
                    preds[succ].push(block);
                }
            }
        }
        // CHK idom 不动点（RPO 序处理，可归约 CFG 常数轮收敛）。
        let mut idom: Vec<Option<usize>> = vec![None; n];
        idom[entry] = Some(entry);
        let intersect = |idom: &[Option<usize>], rpo_index: &[u32], a: usize, b: usize| {
            let (mut a, mut b) = (a, b);
            while a != b {
                while rpo_index[a] > rpo_index[b] {
                    a = idom[a].expect("已处理块必有 idom");
                }
                while rpo_index[b] > rpo_index[a] {
                    b = idom[b].expect("已处理块必有 idom");
                }
            }
            a
        };
        let mut changed = true;
        while changed {
            changed = false;
            for &block in postorder.iter().rev() {
                if block == entry {
                    continue;
                }
                let mut new_idom = None;
                for &pred in &preds[block] {
                    if idom[pred].is_none() {
                        continue;
                    }
                    new_idom = Some(match new_idom {
                        None => pred,
                        Some(current) => intersect(&idom, &rpo_index, pred, current),
                    });
                }
                if new_idom.is_some() && idom[block] != new_idom {
                    idom[block] = new_idom;
                    changed = true;
                }
            }
        }
        // 支配树 DFS 区间（a 支配 b ⇔ tin[a] ≤ tin[b] ∧ tout[b] ≤ tout[a]）。
        let mut children: Vec<Vec<usize>> = vec![Vec::new(); n];
        for (block, parent) in idom.iter().enumerate() {
            if let Some(parent) = *parent
                && parent != block
            {
                children[parent].push(block);
            }
        }
        let mut tin = vec![0u32; n];
        let mut tout = vec![0u32; n];
        let mut clock = 0u32;
        let mut stack = vec![(entry, false)];
        while let Some((block, expanded)) = stack.pop() {
            if expanded {
                tout[block] = clock;
                clock += 1;
                continue;
            }
            tin[block] = clock;
            clock += 1;
            stack.push((block, true));
            for &child in &children[block] {
                stack.push((child, false));
            }
        }
        Self {
            tin,
            tout,
            reachable,
        }
    }

    /// a 是否支配 b（含 a == b）。
    fn dominates(&self, a: BasicBlockId, b: BasicBlockId) -> bool {
        let (a, b) = (a.0 as usize, b.0 as usize);
        if !self.reachable[b] {
            return true;
        }
        if !self.reachable[a] {
            return false;
        }
        self.tin[a] <= self.tin[b] && self.tout[b] <= self.tout[a]
    }
}

#[derive(Clone, Debug)]
pub(super) struct PropertyWrite {
    pub(super) key: String,
    pub(super) block: BasicBlockId,
    pub(super) index: usize,
    pub(super) value: ValueId,
}

#[derive(Clone, Debug)]
pub(super) struct PropertyRead {
    pub(super) key: String,
    pub(super) block: BasicBlockId,
    pub(super) index: usize,
    pub(super) dest: ValueId,
}

#[derive(Clone, Debug)]
pub(super) struct CandidateAnalysis {
    pub(super) writes: Vec<PropertyWrite>,
    pub(super) reads: Vec<PropertyRead>,
    pub(super) delete_targets: Vec<(BasicBlockId, usize)>,
    pub(super) result_replacements: Vec<(ValueId, ValueId)>,
    pub(super) escapes: bool,
}

fn is_new_object_value(function: &wjsm_ir::Function, value: ValueId) -> bool {
    function.blocks().iter().any(|block| {
        block.instructions().iter().any(|instruction| {
            matches!(instruction, Instruction::NewObject { dest, .. } if *dest == value)
        })
    })
}

type ObjectFamily = (
    HashSet<ValueId>,
    HashSet<String>,
    Vec<(BasicBlockId, usize)>,
);

fn try_forward_store(
    function: &wjsm_ir::Function,
    name: &str,
    store_block: BasicBlockId,
    store_idx: usize,
    dom: &Dominators,
    family: &mut HashSet<ValueId>,
    delete_targets: &mut Vec<(BasicBlockId, usize)>,
) -> bool {
    let mut matching_loads = Vec::new();

    // 1. 同块内：在 store_idx 之后、下一个 StoreVar 之前的 LoadVar
    if let Some(block) = function.block_by_id(store_block) {
        for (idx, ins) in block.instructions().iter().enumerate().skip(store_idx + 1) {
            match ins {
                Instruction::StoreVar { name: n, .. } if n == name => break,
                Instruction::LoadVar { name: n, dest } if n == name => {
                    matching_loads.push((store_block, idx, *dest));
                }
                _ => {}
            }
        }
    }

    // 2. 跨块：被 store_block 支配的后继块中的 LoadVar（前提是该块无更近的前置 StoreVar）
    for block in function.blocks() {
        if block.id() == store_block {
            continue;
        }
        if dom.dominates(store_block, block.id()) {
            for (idx, ins) in block.instructions().iter().enumerate() {
                match ins {
                    Instruction::StoreVar { name: n, .. } if n == name => {
                        break;
                    }
                    Instruction::LoadVar { name: n, dest } if n == name => {
                        matching_loads.push((block.id(), idx, *dest));
                    }
                    _ => {}
                }
            }
        }
    }

    for (b_id, idx, dest) in matching_loads {
        family.insert(dest);
        delete_targets.push((b_id, idx));
    }
    delete_targets.push((store_block, store_idx));
    true
}

fn collect_object_family(
    function: &wjsm_ir::Function,
    candidate_dest: ValueId,
    dom: &Dominators,
) -> Option<ObjectFamily> {
    let mut family = HashSet::from([candidate_dest]);
    let forwarded_vars = HashSet::new();
    let mut delete_targets = Vec::new();

    loop {
        let family_len = family.len();
        for block in function.blocks() {
            for (index, instruction) in block.instructions().iter().enumerate() {
                match instruction {
                    Instruction::StoreVar { name, value } if family.contains(value) => {
                        if is_host_shared_variable(name) {
                            return None;
                        }
                        try_forward_store(
                            function,
                            name,
                            block.id(),
                            index,
                            dom,
                            &mut family,
                            &mut delete_targets,
                        );
                    }
                    Instruction::Phi { dest, sources }
                        if sources.iter().any(|source| family.contains(&source.value)) =>
                    {
                        if sources.is_empty()
                            || sources.iter().any(|source| {
                                !family.contains(&source.value)
                                    && !is_new_object_value(function, source.value)
                            })
                        {
                            return None;
                        }
                        for source in sources {
                            family.insert(source.value);
                        }
                        family.insert(*dest);
                        delete_targets.push((block.id(), index));
                    }
                    Instruction::CreateDataProperty { dest, object, .. }
                        if family.contains(object) =>
                    {
                        family.insert(*dest);
                    }
                    _ => {}
                }
            }
        }
        if family.len() == family_len {
            break;
        }
    }

    Some((family, forwarded_vars, delete_targets))
}

fn analyze_candidate(
    function: &wjsm_ir::Function,
    candidate_dest: ValueId,
    const_strings: &HashMap<ValueId, String>,
    dom: &Dominators,
) -> CandidateAnalysis {
    let Some((family, _, mut delete_targets)) =
        collect_object_family(function, candidate_dest, dom)
    else {
        return CandidateAnalysis {
            writes: Vec::new(),
            reads: Vec::new(),
            delete_targets: Vec::new(),
            result_replacements: Vec::new(),
            escapes: true,
        };
    };
    let mut writes = Vec::new();
    let mut reads = Vec::new();
    let mut result_replacements = Vec::new();
    let initial_keys: HashSet<String> = function
        .blocks()
        .iter()
        .flat_map(|block| block.instructions())
        .filter_map(|instruction| match instruction {
            Instruction::CreateDataProperty { object, key, .. } if family.contains(object) => {
                const_strings.get(key).cloned()
            }
            _ => None,
        })
        .collect();
    let mut escapes = function.blocks().iter().any(|block| {
        terminator_uses(block.terminator())
            .into_iter()
            .any(|value| family.contains(&value))
    });

    for block in function.blocks() {
        for (index, instruction) in block.instructions().iter().enumerate() {
            match instruction {
                Instruction::NewObject { dest, .. } if family.contains(dest) => {
                    delete_targets.push((block.id(), index));
                }
                Instruction::StoreVar { value, .. } if family.contains(value) => {
                    delete_targets.push((block.id(), index));
                }
                Instruction::LoadVar { dest, .. } if family.contains(dest) => {
                    delete_targets.push((block.id(), index));
                }
                Instruction::Phi { dest, sources } if family.contains(dest) => {
                    if sources.iter().any(|source| !family.contains(&source.value)) {
                        escapes = true;
                    }
                    delete_targets.push((block.id(), index));
                }
                Instruction::CreateDataProperty {
                    object, key, value, ..
                } if family.contains(object) => {
                    if family.contains(value) {
                        escapes = true;
                    }
                    if let Some(key) = const_strings.get(key) {
                        writes.push(PropertyWrite {
                            key: key.clone(),
                            block: block.id(),
                            index,
                            value: *value,
                        });
                        delete_targets.push((block.id(), index));
                    } else {
                        escapes = true;
                    }
                }
                Instruction::SetProp {
                    dest,
                    object,
                    key,
                    value,
                    ..
                } if family.contains(object) => {
                    let Some(key_name) = const_strings.get(key) else {
                        escapes = true;
                        continue;
                    };
                    // 仅处理已由 CreateDataProperty 建立的自有数据属性；
                    // 对从原型继承的属性，[[Set]] 可能调用用户可变 accessor。
                    if !initial_keys.contains(key_name) || family.contains(value) {
                        escapes = true;
                        continue;
                    }
                    writes.push(PropertyWrite {
                        key: key_name.clone(),
                        block: block.id(),
                        index,
                        value: *value,
                    });
                    // 普通对象 [[Set]] 成功时返回 stored；将结果替换为
                    // value 后，原有 IsException 检查仍保留 Binary 等异常语义。
                    result_replacements.push((*dest, *value));
                    delete_targets.push((block.id(), index));
                }
                Instruction::GetProp { object, key, dest } if family.contains(object) => {
                    if let Some(key) = const_strings.get(key) {
                        reads.push(PropertyRead {
                            key: key.clone(),
                            block: block.id(),
                            index,
                            dest: *dest,
                        });
                        delete_targets.push((block.id(), index));
                    } else {
                        escapes = true;
                    }
                }
                Instruction::SetProto { object, value } if family.contains(object) => {
                    if family.contains(value) {
                        escapes = true;
                    }
                    delete_targets.push((block.id(), index));
                }
                Instruction::CallBuiltin {
                    dest,
                    builtin,
                    args,
                } if *builtin == wjsm_ir::Builtin::IsJsObject
                    && args.iter().any(|value| family.contains(value)) =>
                {
                    let result_has_use = dest.is_some_and(|value| {
                        used_in_terminator(function, value)
                            || !collect_uses(function, value).is_empty()
                    });
                    if args.len() != 1 || result_has_use {
                        escapes = true;
                    } else {
                        delete_targets.push((block.id(), index));
                    }
                }
                _ if instr_uses(instruction)
                    .into_iter()
                    .any(|value| family.contains(&value)) =>
                {
                    escapes = true;
                }
                _ => {}
            }
        }
    }

    delete_targets.sort_unstable_by_key(|(block, index)| (block.0, *index));
    delete_targets.dedup();
    CandidateAnalysis {
        writes,
        reads,
        delete_targets,
        result_replacements,
        escapes,
    }
}
#[derive(Clone, Debug)]
pub(super) struct PropertyPhi {
    pub(super) block: BasicBlockId,
    pub(super) dest: ValueId,
    pub(super) sources: Vec<wjsm_ir::PhiSource>,
    pub(super) key: String,
}

fn function_predecessors(function: &wjsm_ir::Function) -> Vec<Vec<BasicBlockId>> {
    let mut predecessors = vec![Vec::new(); function.blocks().len()];
    for block in function.blocks() {
        for successor in terminator_successors(block.terminator()) {
            predecessors[successor.0 as usize].push(block.id());
        }
    }
    predecessors
}

fn reaching_value(state: &HashSet<usize>, writes: &[&PropertyWrite]) -> Option<ValueId> {
    let mut values = state.iter().map(|index| writes[*index].value);
    let first = values.next()?;
    values.all(|value| value == first).then_some(first)
}

fn state_before_read(
    block: BasicBlockId,
    index: usize,
    entry: &[HashSet<usize>],
    write_at: &HashMap<(BasicBlockId, usize), usize>,
) -> HashSet<usize> {
    let mut state = entry[block.0 as usize].clone();
    for instruction_index in 0..index {
        if let Some(write) = write_at.get(&(block, instruction_index)) {
            state.clear();
            state.insert(*write);
        }
    }
    state
}

pub(super) fn next_value_id(function: &wjsm_ir::Function) -> u32 {
    let mut next = 0_u32;
    for block in function.blocks() {
        for instruction in block.instructions() {
            if let Some(dest) = instruction_dest(instruction) {
                next = next.max(dest.0.saturating_add(1));
            }
            for value in instr_uses(instruction) {
                next = next.max(value.0.saturating_add(1));
            }
            if let Instruction::Phi { sources, .. } = instruction {
                for source in sources {
                    next = next.max(source.value.0.saturating_add(1));
                }
            }
        }
        for value in terminator_uses(block.terminator()) {
            next = next.max(value.0.saturating_add(1));
        }
    }
    next
}

pub(super) fn resolve_property_replacements(
    function: &wjsm_ir::Function,
    analysis: &CandidateAnalysis,
    next_value: &mut u32,
) -> Option<(HashMap<ValueId, ValueId>, Vec<PropertyPhi>)> {
    let predecessors = function_predecessors(function);
    let mut replacements = HashMap::new();
    let mut phis = Vec::new();
    let keys: BTreeSet<String> = analysis
        .writes
        .iter()
        .map(|write| write.key.clone())
        .collect();

    for key in keys {
        let writes: Vec<&PropertyWrite> = analysis
            .writes
            .iter()
            .filter(|write| write.key == key)
            .collect();
        let mut write_at = HashMap::new();
        for (index, write) in writes.iter().enumerate() {
            write_at.insert((write.block, write.index), index);
        }

        let mut entry = vec![HashSet::new(); function.blocks().len()];
        let mut exit = vec![HashSet::new(); function.blocks().len()];
        loop {
            let mut next_entry = vec![HashSet::new(); function.blocks().len()];
            let mut next_exit = vec![HashSet::new(); function.blocks().len()];
            for block in function.blocks() {
                let block_index = block.id().0 as usize;
                for predecessor in &predecessors[block_index] {
                    next_entry[block_index].extend(exit[predecessor.0 as usize].iter().copied());
                }
                let mut state = next_entry[block_index].clone();
                for index in 0..block.instructions().len() {
                    if let Some(write) = write_at.get(&(block.id(), index)) {
                        state.clear();
                        state.insert(*write);
                    }
                }
                next_exit[block_index] = state;
            }
            if next_entry == entry && next_exit == exit {
                break;
            }
            entry = next_entry;
            exit = next_exit;
        }

        // 多个到达写入且值不同的块需要一个 SSA 表示；先分配所有 dest，
        // 这样循环头之间的回边可以互相引用已经确定的 ValueId。
        let mut entry_phis = HashMap::<BasicBlockId, ValueId>::new();
        for block in function.blocks() {
            let block_index = block.id().0 as usize;
            if !entry[block_index].is_empty()
                && reaching_value(&entry[block_index], &writes).is_none()
            {
                let dest = ValueId(*next_value);
                *next_value = next_value.saturating_add(1);
                entry_phis.insert(block.id(), dest);
            }
        }

        for (&block, &dest) in &entry_phis {
            let incoming = &predecessors[block.0 as usize];
            if incoming.is_empty() {
                return None;
            }
            let mut sources = Vec::with_capacity(incoming.len());
            for predecessor in incoming {
                let predecessor_index = predecessor.0 as usize;
                let value = reaching_value(&exit[predecessor_index], &writes).or_else(|| {
                    let has_write = function.blocks()[predecessor_index]
                        .instructions()
                        .iter()
                        .enumerate()
                        .any(|(index, _)| write_at.contains_key(&(*predecessor, index)));
                    (!has_write).then(|| entry_phis.get(predecessor).copied())?
                });
                let value = value?;
                sources.push(wjsm_ir::PhiSource {
                    predecessor: *predecessor,
                    value,
                });
            }
            phis.push(PropertyPhi {
                block,
                dest,
                sources,
                key: key.clone(),
            });
        }

        for read in analysis.reads.iter().filter(|read| read.key == key) {
            let state = state_before_read(read.block, read.index, &entry, &write_at);
            let value =
                reaching_value(&state, &writes).or_else(|| entry_phis.get(&read.block).copied())?;
            replacements.insert(read.dest, value);
        }
    }
    Some((replacements, phis))
}

pub(super) fn close_replacements(replacements: &mut HashMap<ValueId, ValueId>) {
    let keys: Vec<_> = replacements.keys().copied().collect();
    for key in keys {
        let mut value = replacements[&key];
        let mut seen = HashSet::new();
        while let Some(next) = replacements.get(&value).copied() {
            if next == value || !seen.insert(value) {
                break;
            }
            value = next;
        }
        replacements.insert(key, value);
    }
}

fn deleted_defs_are_unused(
    original: &wjsm_ir::Function,
    function: &wjsm_ir::Function,
    delete_targets: &HashSet<(BasicBlockId, usize)>,
) -> bool {
    let deleted_defs: HashSet<_> = delete_targets
        .iter()
        .filter_map(|(block_id, index)| {
            original
                .block_by_id(*block_id)
                .and_then(|block| block.instructions().get(*index))
                .and_then(instruction_dest)
        })
        .collect();
    for block in function.blocks() {
        for (index, instruction) in block.instructions().iter().enumerate() {
            if delete_targets.contains(&(block.id(), index)) {
                continue;
            }
            if instr_uses(instruction)
                .into_iter()
                .any(|value| deleted_defs.contains(&value))
            {
                return false;
            }
            if let Instruction::Phi { sources, .. } = instruction
                && sources
                    .iter()
                    .any(|source| deleted_defs.contains(&source.value))
            {
                return false;
            }
        }
        if terminator_uses(block.terminator())
            .into_iter()
            .any(|value| deleted_defs.contains(&value))
        {
            return false;
        }
    }
    true
}

pub(crate) fn run(module: &mut Module) {
    if module
        .functions()
        .iter()
        .any(|function| function.has_eval())
    {
        return;
    }

    eliminate_array_templates(module);
    eliminate_dead_string_computations(module);
    crate::passes::escape_scalar_record::run(module);

    let mut any_change = true;
    while any_change {
        any_change = false;
        let constants_base = module.constants().to_vec();
        for function_index in 0..module.functions().len() {
            let mut candidates = Vec::new();
            let mut replacements = HashMap::new();
            let mut delete_targets = HashSet::new();
            let mut property_phis = Vec::new();
            {
                let function = &module.functions()[function_index];
                let mut const_strings = HashMap::new();
                for block in function.blocks() {
                    for instruction in block.instructions() {
                        if let Instruction::Const { dest, constant } = instruction
                            && let Some(Constant::String(value)) =
                                constants_base.get(constant.0 as usize)
                        {
                            const_strings.insert(*dest, value.clone());
                        }
                    }
                }
                let dom = Dominators::compute(function);
                for block in function.blocks() {
                    for instruction in block.instructions() {
                        if let Instruction::NewObject { dest, .. } = instruction
                            && !used_in_terminator(function, *dest)
                        {
                            candidates.push((*dest, block.id()));
                        }
                    }
                }
                let mut next_value = next_value_id(function);
                for (candidate_dest, _definition_block) in candidates {
                    let analysis =
                        analyze_candidate(function, candidate_dest, &const_strings, &dom);
                    if analysis.escapes || analysis.writes.is_empty() {
                        continue;
                    }
                    let Some((candidate_replacements, candidate_phis)) =
                        resolve_property_replacements(function, &analysis, &mut next_value)
                    else {
                        continue;
                    };
                    replacements.extend(candidate_replacements);
                    replacements.extend(analysis.result_replacements);
                    delete_targets.extend(analysis.delete_targets);
                    property_phis.extend(candidate_phis);
                }
            }

            if replacements.is_empty() && delete_targets.is_empty() {
                continue;
            }
            close_replacements(&mut replacements);
            for phi in &mut property_phis {
                for source in &mut phi.sources {
                    if let Some(value) = replacements.get(&source.value) {
                        source.value = *value;
                    }
                }
            }

            let function_id = FunctionId(function_index as u32);
            let function = module
                .function_mut(function_id)
                .expect("function id must be valid");
            let mut preview = function.clone();
            apply_value_replacements(&mut preview, &replacements);
            if !deleted_defs_are_unused(function, &preview, &delete_targets) {
                continue;
            }
            let replaced = apply_value_replacements(function, &replacements);
            let mut by_block: HashMap<BasicBlockId, Vec<usize>> = HashMap::new();
            for (block_id, index) in &delete_targets {
                by_block.entry(*block_id).or_default().push(*index);
            }
            for (block_id, mut indices) in by_block {
                indices.sort_unstable_by(|left, right| right.cmp(left));
                indices.dedup();
                if let Some(block) = function.block_by_id_mut(block_id) {
                    let instructions = block.instructions_mut();
                    for index in indices {
                        if index < instructions.len() {
                            instructions.remove(index);
                        }
                    }
                }
            }
            let had_property_phis = !property_phis.is_empty();

            for phi in property_phis {
                if let Some(block) = function.block_by_id_mut(phi.block) {
                    block.instructions_mut().insert(
                        0,
                        Instruction::Phi {
                            dest: phi.dest,
                            sources: phi.sources,
                        },
                    );
                }
            }
            any_change = replaced || !delete_targets.is_empty() || had_property_phis;
        }
    }
}

/// 检查 ValueId 是否被任何 block 的终止器使用（逃逸）。
fn used_in_terminator(function: &wjsm_ir::Function, target: ValueId) -> bool {
    for block in function.blocks() {
        if terminator_uses(block.terminator()).contains(&target) {
            return true;
        }
    }
    false
}

/// 在函数的所有指令和终止器中替换 ValueId。
pub(super) fn apply_value_replacements(
    function: &mut wjsm_ir::Function,
    replacements: &HashMap<ValueId, ValueId>,
) -> bool {
    if replacements.is_empty() {
        return false;
    }

    let mut changed = false;

    for block in function.blocks_mut() {
        for instruction in block.instructions_mut() {
            if replace_in_instruction(instruction, replacements) {
                changed = true;
            }
        }

        replace_in_terminator(block.terminator_mut(), replacements);
    }

    changed
}

fn replace_in_instruction(ins: &mut Instruction, replacements: &HashMap<ValueId, ValueId>) -> bool {
    let mut changed = false;

    match ins {
        Instruction::Const { .. }
        | Instruction::LoadVar { .. }
        | Instruction::NewObject { .. }
        | Instruction::NewArray { .. }
        | Instruction::CloneArrayTemplate { .. }
        | Instruction::InitObjectLiteral { .. }
        | Instruction::GetSuperBase { .. }
        | Instruction::GetSuperConstructor { .. }
        | Instruction::NewPromise { .. }
        | Instruction::CollectRestArgs { .. }
        | Instruction::DebugCheck { .. } => {}
        Instruction::Binary { lhs, rhs, .. } | Instruction::Compare { lhs, rhs, .. } => {
            if let Some(new) = replacements.get(lhs) {
                *lhs = *new;
                changed = true;
            }
            if let Some(new) = replacements.get(rhs) {
                *rhs = *new;
                changed = true;
            }
        }
        Instruction::Unary { value, .. } => {
            if let Some(new) = replacements.get(value) {
                *value = *new;
                changed = true;
            }
        }
        Instruction::StringConcatVa { parts, .. } => {
            for part in parts.iter_mut() {
                if let Some(new) = replacements.get(part) {
                    *part = *new;
                    changed = true;
                }
            }
        }
        Instruction::GetProp { object, key, .. } => {
            if let Some(new) = replacements.get(object) {
                *object = *new;
                changed = true;
            }
            if let Some(new) = replacements.get(key) {
                *key = *new;
                changed = true;
            }
        }
        Instruction::SetProp {
            dest,
            object,
            key,
            value,
            ..
        }
        | Instruction::CreateDataProperty {
            dest,
            object,
            key,
            value,
        } => {
            if let Some(new) = replacements.get(dest) {
                *dest = *new;
                changed = true;
            }
            if let Some(new) = replacements.get(object) {
                *object = *new;
                changed = true;
            }
            if let Some(new) = replacements.get(key) {
                *key = *new;
                changed = true;
            }
            if let Some(new) = replacements.get(value) {
                *value = *new;
                changed = true;
            }
        }
        Instruction::SetProto { object, value } => {
            if let Some(new) = replacements.get(object) {
                *object = *new;
                changed = true;
            }
            if let Some(new) = replacements.get(value) {
                *value = *new;
                changed = true;
            }
        }
        Instruction::GetElem { object, index, .. } => {
            if let Some(new) = replacements.get(object) {
                *object = *new;
                changed = true;
            }
            if let Some(new) = replacements.get(index) {
                *index = *new;
                changed = true;
            }
        }
        Instruction::ElemShapeGuard { array, .. } => {
            if let Some(new) = replacements.get(array) {
                *array = *new;
                changed = true;
            }
        }
        Instruction::GetElemGuarded {
            object,
            index,
            guard,
            ..
        } => {
            for operand in [object, index, guard] {
                if let Some(new) = replacements.get(operand) {
                    *operand = *new;
                    changed = true;
                }
            }
        }
        Instruction::GetPropGuarded {
            object, key, guard, ..
        } => {
            for operand in [object, key, guard] {
                if let Some(new) = replacements.get(operand) {
                    *operand = *new;
                    changed = true;
                }
            }
        }
        Instruction::SetElem {
            dest,
            object,
            index,
            value,
            ..
        } => {
            if let Some(new) = replacements.get(dest) {
                *dest = *new;
                changed = true;
            }
            if let Some(new) = replacements.get(object) {
                *object = *new;
                changed = true;
            }
            if let Some(new) = replacements.get(index) {
                *index = *new;
                changed = true;
            }
            if let Some(new) = replacements.get(value) {
                *value = *new;
                changed = true;
            }
        }
        Instruction::OptionalGetProp { object, key, .. }
        | Instruction::OptionalGetElem { object, key, .. } => {
            if let Some(new) = replacements.get(object) {
                *object = *new;
                changed = true;
            }
            if let Some(new) = replacements.get(key) {
                *key = *new;
                changed = true;
            }
        }
        Instruction::OptionalCall {
            callee,
            this_val,
            args,
            ..
        }
        | Instruction::Call {
            callee,
            this_val,
            args,
            ..
        }
        | Instruction::SuperCall {
            callee,
            this_val,
            args,
            ..
        } => {
            if let Some(new) = replacements.get(callee) {
                *callee = *new;
                changed = true;
            }
            if let Some(new) = replacements.get(this_val) {
                *this_val = *new;
                changed = true;
            }
            for arg in args.iter_mut() {
                if let Some(new) = replacements.get(arg) {
                    *arg = *new;
                    changed = true;
                }
            }
        }
        Instruction::ConstructCall {
            callee,
            this_val,
            args,
            ..
        } => {
            if let Some(new) = replacements.get(callee) {
                *callee = *new;
                changed = true;
            }
            if let Some(new) = replacements.get(this_val) {
                *this_val = *new;
                changed = true;
            }
            for arg in args.iter_mut() {
                if let Some(new) = replacements.get(arg) {
                    *arg = *new;
                    changed = true;
                }
            }
        }
        Instruction::CallBuiltin { args, .. } => {
            for arg in args.iter_mut() {
                if let Some(new) = replacements.get(arg) {
                    *arg = *new;
                    changed = true;
                }
            }
        }
        Instruction::DeleteProp { object, key, .. } => {
            if let Some(new) = replacements.get(object) {
                *object = *new;
                changed = true;
            }
            if let Some(new) = replacements.get(key) {
                *key = *new;
                changed = true;
            }
        }
        Instruction::PromiseResolve { promise, value }
        | Instruction::PromiseReject {
            promise,
            reason: value,
        } => {
            if let Some(new) = replacements.get(promise) {
                *promise = *new;
                changed = true;
            }
            if let Some(new) = replacements.get(value) {
                *value = *new;
                changed = true;
            }
        }
        Instruction::Suspend { promise, .. } => {
            if let Some(new) = replacements.get(promise) {
                *promise = *new;
                changed = true;
            }
        }
        Instruction::GeneratorSuspend { result, .. } => {
            if let Some(new) = replacements.get(result) {
                *result = *new;
                changed = true;
            }
        }
        Instruction::IsException { value, .. }
        | Instruction::EncodeException { value, .. }
        | Instruction::ExceptionToObject { value, .. } => {
            if let Some(new) = replacements.get(value) {
                *value = *new;
                changed = true;
            }
        }
        Instruction::GuardSameFunction { callee, .. } => {
            if let Some(new) = replacements.get(callee) {
                *callee = *new;
                changed = true;
            }
        }
        Instruction::ObjectSpread { object, source, .. } => {
            if let Some(new) = replacements.get(object) {
                *object = *new;
                changed = true;
            }
            if let Some(new) = replacements.get(source) {
                *source = *new;
                changed = true;
            }
        }
        Instruction::StoreVar { value, .. } => {
            if let Some(new) = replacements.get(value) {
                *value = *new;
                changed = true;
            }
        }
        Instruction::Phi { sources, .. } => {
            for source in sources.iter_mut() {
                if let Some(new) = replacements.get(&source.value) {
                    source.value = *new;
                    changed = true;
                }
            }
        }
    }

    changed
}

fn replace_in_terminator(terminator: &mut Terminator, replacements: &HashMap<ValueId, ValueId>) {
    match terminator {
        Terminator::Return { value: Some(v) } => {
            if let Some(new) = replacements.get(v) {
                *v = *new;
            }
        }
        Terminator::Branch { condition, .. } => {
            if let Some(new) = replacements.get(condition) {
                *condition = *new;
            }
        }
        Terminator::Switch { value, .. } => {
            if let Some(new) = replacements.get(value) {
                *value = *new;
            }
        }
        Terminator::Throw { value } => {
            if let Some(new) = replacements.get(value) {
                *value = *new;
            }
        }
        Terminator::Return { value: None } | Terminator::Jump { .. } | Terminator::Unreachable => {}
    }
}

fn const_id_or_add_in_module(module: &mut Module, constant: Constant) -> ConstantId {
    for (i, c) in module.constants().iter().enumerate() {
        if *c == constant {
            return ConstantId(i as u32);
        }
    }
    module.add_constant(constant)
}

fn eliminate_array_templates(module: &mut Module) -> bool {
    let mut any_change = false;
    let constants_base = module.constants().to_vec();

    for function_index in 0..module.functions().len() {
        let function_id = FunctionId(function_index as u32);
        let mut raw_reads_to_replace = Vec::new();
        let mut delete_targets = HashSet::new();
        let mut next_val;

        {
            let function = &module.functions()[function_index];
            next_val = next_value_id(function);
            let dom = Dominators::compute(function);

            let mut candidates = Vec::new();
            for block in function.blocks() {
                for instruction in block.instructions() {
                    if let Instruction::CloneArrayTemplate { dest, template } = instruction
                        && !used_in_terminator(function, *dest)
                    {
                        candidates.push((*dest, *template));
                    }
                }
            }

            for (candidate_dest, template_id) in candidates {
                let Some((family, _, fam_deletes)) =
                    collect_object_family(function, candidate_dest, &dom)
                else {
                    continue;
                };

                let Some(Constant::ArrayTemplate(elem_const_ids)) =
                    constants_base.get(template_id.0 as usize)
                else {
                    continue;
                };

                let mut escapes = false;
                let mut reads_to_replace = Vec::new();

                for block in function.blocks() {
                    for (index, instruction) in block.instructions().iter().enumerate() {
                        match instruction {
                            Instruction::CloneArrayTemplate { dest, .. }
                                if family.contains(dest) => {}
                            Instruction::StoreVar { value, .. } if family.contains(value) => {}
                            Instruction::LoadVar { dest, .. } if family.contains(dest) => {}
                            Instruction::GetProp { object, key, dest }
                                if family.contains(object) =>
                            {
                                let is_length = function.blocks().iter().any(|b| {
                                    b.instructions().iter().any(|ins| {
                                        matches!(
                                            ins,
                                            Instruction::Const { dest: d, constant }
                                                if *d == *key
                                                    && matches!(
                                                        constants_base.get(constant.0 as usize),
                                                        Some(Constant::String(s)) if s == "length"
                                                    )
                                        )
                                    })
                                });
                                if is_length {
                                    let len_f64 = elem_const_ids.len() as f64;
                                    reads_to_replace.push((
                                        block.id(),
                                        index,
                                        *dest,
                                        Constant::Number(len_f64),
                                    ));
                                } else {
                                    escapes = true;
                                    break;
                                }
                            }
                            Instruction::GetElem {
                                object,
                                index: idx_val,
                                dest,
                            } if family.contains(object) => {
                                if let Some(idx) = resolve_array_index(
                                    function,
                                    *idx_val,
                                    &family,
                                    elem_const_ids.len(),
                                    &constants_base,
                                ) {
                                    let elem_id = elem_const_ids[idx];
                                    let elem_const = constants_base[elem_id.0 as usize].clone();
                                    reads_to_replace.push((block.id(), index, *dest, elem_const));
                                } else {
                                    escapes = true;
                                    break;
                                }
                            }
                            Instruction::IsException { dest, value } if family.contains(value) => {
                                reads_to_replace.push((
                                    block.id(),
                                    index,
                                    *dest,
                                    Constant::Bool(false),
                                ));
                            }
                            Instruction::CallBuiltin {
                                builtin: Builtin::ExceptionValue,
                                args,
                                ..
                            } if args.iter().any(|v| family.contains(v)) => {
                                // 异常载荷提取在非异常模板上不可达，不视为逃逸
                            }
                            _ if instr_uses(instruction)
                                .into_iter()
                                .any(|v| family.contains(&v)) =>
                            {
                                escapes = true;
                                break;
                            }
                            _ => {}
                        }
                    }
                    if escapes {
                        break;
                    }
                }

                if escapes {
                    continue;
                }

                delete_targets.extend(fam_deletes);
                for block in function.blocks() {
                    for (index, instruction) in block.instructions().iter().enumerate() {
                        if let Instruction::CloneArrayTemplate { dest, .. } = instruction
                            && family.contains(dest)
                        {
                            delete_targets.insert((block.id(), index));
                        }
                    }
                }

                raw_reads_to_replace.extend(reads_to_replace);
            }
        }

        if raw_reads_to_replace.is_empty() && delete_targets.is_empty() {
            continue;
        }

        let mut replacements = HashMap::new();
        let mut consts_to_insert = Vec::new();
        for (block_id, index, read_dest, const_val) in raw_reads_to_replace {
            let const_id = const_id_or_add_in_module(module, const_val);
            let new_dest = ValueId(next_val);
            next_val += 1;
            consts_to_insert.push((block_id, index, new_dest, const_id));
            replacements.insert(read_dest, new_dest);
            delete_targets.insert((block_id, index));
        }

        let function = module.function_mut(function_id).expect("valid fid");
        apply_value_replacements(function, &replacements);

        let mut by_block: HashMap<BasicBlockId, Vec<usize>> = HashMap::new();
        for (block_id, index) in &delete_targets {
            by_block.entry(*block_id).or_default().push(*index);
        }
        for (block_id, mut indices) in by_block {
            indices.sort_unstable_by(|left, right| right.cmp(left));
            indices.dedup();
            if let Some(block) = function.block_by_id_mut(block_id) {
                let instructions = block.instructions_mut();
                for index in indices {
                    if index < instructions.len() {
                        instructions.remove(index);
                    }
                }
            }
        }

        for (block_id, _index, dest, constant) in consts_to_insert {
            if let Some(block) = function.block_by_id_mut(block_id) {
                block
                    .instructions_mut()
                    .insert(0, Instruction::Const { dest, constant });
            }
        }

        any_change = true;
    }

    any_change
}

fn resolve_array_index(
    function: &wjsm_ir::Function,
    idx_val: ValueId,
    family: &HashSet<ValueId>,
    elem_len: usize,
    constants: &[Constant],
) -> Option<usize> {
    for b in function.blocks() {
        for ins in b.instructions() {
            match ins {
                Instruction::Const { dest: d, constant } if *d == idx_val => {
                    if let Some(Constant::Number(n)) = constants.get(constant.0 as usize)
                        && *n >= 0.0
                        && (*n as usize) < elem_len
                        && n.fract() == 0.0
                    {
                        return Some(*n as usize);
                    }
                }
                Instruction::Binary {
                    dest: d,
                    op: wjsm_ir::BinaryOp::Sub,
                    lhs,
                    rhs,
                } if *d == idx_val => {
                    let lhs_is_len = for_instruction(function, *lhs, |lhs_ins| {
                        matches!(lhs_ins, Instruction::GetProp { object, key, .. }
                        if family.contains(object) && for_instruction(function, *key, |k_ins| {
                            matches!(k_ins, Instruction::Const { constant, .. }
                                if matches!(constants.get(constant.0 as usize), Some(Constant::String(s)) if s == "length"))
                        }))
                    });
                    if lhs_is_len {
                        let rhs_val = for_instruction(function, *rhs, |rhs_ins| {
                            if let Instruction::Const { constant, .. } = rhs_ins
                                && let Some(Constant::Number(k)) =
                                    constants.get(constant.0 as usize)
                            {
                                Some(*k)
                            } else {
                                None
                            }
                        });
                        if let Some(k) = rhs_val {
                            let n = elem_len as f64 - k;
                            if n >= 0.0 && (n as usize) < elem_len && n.fract() == 0.0 {
                                return Some(n as usize);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
    None
}

fn for_instruction<T>(
    function: &wjsm_ir::Function,
    target: ValueId,
    f: impl FnOnce(&Instruction) -> T,
) -> T
where
    T: Default,
{
    for b in function.blocks() {
        for ins in b.instructions() {
            if let Some(d) = instruction_dest(ins)
                && d == target
            {
                return f(ins);
            }
        }
    }
    T::default()
}

fn eliminate_dead_string_computations(module: &mut Module) -> bool {
    let mut changed = false;
    let mut any_change = true;
    let mut round = 0;
    while any_change && round < 4 {
        any_change = false;
        round += 1;
        let constants_base = module.constants().to_vec();

        for function_index in 0..module.functions().len() {
            let function_id = FunctionId(function_index as u32);
            let mut raw_reads_to_replace = Vec::new();
            let mut delete_targets = HashSet::new();
            let mut next_val;

            {
                let function = &module.functions()[function_index];
                next_val = next_value_id(function);
                let dom = Dominators::compute(function);

                let mut candidates: Vec<(ValueId, Option<f64>)> = Vec::new();

                for block in function.blocks() {
                    for instruction in block.instructions() {
                        match instruction {
                            Instruction::CallBuiltin {
                                builtin: Builtin::StringSlice,
                                dest: Some(dest),
                                args,
                            } if args.len() >= 3 && !used_in_terminator(function, *dest) => {
                                let start_val =
                                    find_const_number(function, args[1], &constants_base);
                                let end_val = find_const_number(function, args[2], &constants_base);
                                if let (Some(s), Some(e)) = (start_val, end_val)
                                    && s >= 0.0
                                    && e >= s
                                {
                                    candidates.push((*dest, Some(e - s)));
                                }
                            }
                            Instruction::StringConcatVa { dest, parts }
                                if !used_in_terminator(function, *dest) =>
                            {
                                let mut total_len = 0.0;
                                let mut all_known = true;
                                for part in parts {
                                    if let Some(len) =
                                        find_part_length(function, *part, &constants_base)
                                    {
                                        total_len += len;
                                    } else {
                                        all_known = false;
                                        break;
                                    }
                                }
                                if all_known {
                                    candidates.push((*dest, Some(total_len)));
                                }
                            }
                            _ => {}
                        }
                    }
                }

                for (candidate_dest, known_length) in candidates {
                    let Some(len_f64) = known_length else {
                        continue;
                    };
                    let Some((family, _, fam_deletes)) =
                        collect_object_family(function, candidate_dest, &dom)
                    else {
                        continue;
                    };

                    let mut escapes = false;
                    let mut reads_to_replace = Vec::new();

                    for block in function.blocks() {
                        for (index, instruction) in block.instructions().iter().enumerate() {
                            match instruction {
                                Instruction::CallBuiltin {
                                    builtin: Builtin::StringSlice,
                                    dest: Some(dest),
                                    ..
                                } if family.contains(dest) => {}
                                Instruction::StringConcatVa { dest, .. }
                                    if family.contains(dest) => {}
                                Instruction::StoreVar { value, .. } if family.contains(value) => {}
                                Instruction::LoadVar { dest, .. } if family.contains(dest) => {}
                                Instruction::GetProp { object, key, dest }
                                    if family.contains(object) =>
                                {
                                    let is_length = function.blocks().iter().any(|b| {
                                        b.instructions().iter().any(|ins| {
                                        matches!(
                                            ins,
                                            Instruction::Const { dest: d, constant }
                                                if *d == *key
                                                    && matches!(
                                                        constants_base.get(constant.0 as usize),
                                                        Some(Constant::String(s)) if s == "length"
                                                    )
                                        )
                                    })
                                    });
                                    if is_length {
                                        reads_to_replace.push((
                                            block.id(),
                                            index,
                                            *dest,
                                            Constant::Number(len_f64),
                                        ));
                                    } else {
                                        escapes = true;
                                        break;
                                    }
                                }
                                Instruction::IsException { dest, value }
                                    if family.contains(value) =>
                                {
                                    reads_to_replace.push((
                                        block.id(),
                                        index,
                                        *dest,
                                        Constant::Bool(false),
                                    ));
                                }
                                Instruction::CallBuiltin {
                                    builtin: Builtin::ExceptionValue,
                                    args,
                                    ..
                                } if args.iter().any(|v| family.contains(v)) => {}
                                _ if instr_uses(instruction)
                                    .into_iter()
                                    .any(|v| family.contains(&v)) =>
                                {
                                    escapes = true;
                                    break;
                                }
                                _ => {}
                            }
                        }
                        if escapes {
                            break;
                        }
                    }

                    if escapes {
                        continue;
                    }

                    delete_targets.extend(fam_deletes);
                    for block in function.blocks() {
                        for (index, instruction) in block.instructions().iter().enumerate() {
                            match instruction {
                                Instruction::CallBuiltin {
                                    builtin: Builtin::StringSlice,
                                    dest: Some(dest),
                                    ..
                                }
                                | Instruction::StringConcatVa { dest, .. }
                                    if family.contains(dest) =>
                                {
                                    delete_targets.insert((block.id(), index));
                                }
                                _ => {}
                            }
                        }
                    }

                    raw_reads_to_replace.extend(reads_to_replace);
                }

                // 清理无后续使用的死字符串构建循环（如 s += BASE + i）
                let mut dead_var_names: HashSet<String> = HashSet::new();
                for block in function.blocks() {
                    for instruction in block.instructions() {
                        if let Instruction::CallBuiltin {
                            builtin: Builtin::StringBuilderFinish,
                            args,
                            dest: None,
                        } = instruction
                            && let Some(arg) = args.first()
                        {
                            for ins in block.instructions() {
                                if let Instruction::LoadVar { name, dest: d } = ins
                                    && d == arg
                                {
                                    dead_var_names.insert(name.clone());
                                }
                            }
                        }
                    }
                }

                for var_name in dead_var_names {
                    let mut safe_to_eliminate = true;
                    let mut var_delete_sites = Vec::new();

                    for block in function.blocks() {
                        for (index, instruction) in block.instructions().iter().enumerate() {
                            match instruction {
                            Instruction::StoreVar { name, .. } if name == &var_name => {
                                var_delete_sites.push((block.id(), index));
                            }
                            Instruction::LoadVar { name, dest } if name == &var_name => {
                                let uses = collect_uses(function, *dest);
                                for use_instr in uses {
                                    let is_builder_use = match use_instr {
                                        Instruction::CallBuiltin {
                                            builtin: Builtin::StringBuilderAppend,
                                            args,
                                            ..
                                        } if args.first() == Some(dest) => true,
                                        Instruction::CallBuiltin {
                                            builtin: Builtin::StringBuilderFinish,
                                            args,
                                            ..
                                        } if args.first() == Some(dest) => true,
                                        _ => false,
                                    };
                                    if !is_builder_use {
                                        safe_to_eliminate = false;
                                        break;
                                    }
                                }
                                if safe_to_eliminate {
                                    var_delete_sites.push((block.id(), index));
                                }
                            }
                            Instruction::CallBuiltin {
                                builtin: Builtin::StringBuilderAppend,
                                args,
                                ..
                            } if args.first().is_some_and(|a| {
                                for_instruction(function, *a, |ins| {
                                    matches!(ins, Instruction::LoadVar { name, .. } if name == &var_name)
                                })
                            }) =>
                            {
                                var_delete_sites.push((block.id(), index));
                            }
                            Instruction::CallBuiltin {
                                builtin: Builtin::StringBuilderFinish,
                                args,
                                ..
                            } if args.first().is_some_and(|a| {
                                for_instruction(function, *a, |ins| {
                                    matches!(ins, Instruction::LoadVar { name, .. } if name == &var_name)
                                })
                            }) =>
                            {
                                var_delete_sites.push((block.id(), index));
                            }
                            _ => {}
                        }
                        }
                    }

                    if safe_to_eliminate {
                        delete_targets.extend(var_delete_sites);
                    }
                }
            }

            if raw_reads_to_replace.is_empty() && delete_targets.is_empty() {
                continue;
            }

            let mut replacements = HashMap::new();
            let mut consts_to_insert = Vec::new();
            for (block_id, index, read_dest, const_val) in raw_reads_to_replace {
                let const_id = const_id_or_add_in_module(module, const_val);
                let new_dest = ValueId(next_val);
                next_val += 1;
                consts_to_insert.push((block_id, index, new_dest, const_id));
                replacements.insert(read_dest, new_dest);
                delete_targets.insert((block_id, index));
            }

            // 确保被删除指令的 is_exception 也被加入 delete_targets，并将其 dest 替换为 false 常量
            let ex_sites: Vec<(BasicBlockId, usize, ValueId)> = {
                let function = &module.functions()[function_index];
                let mut deleted_values: HashSet<ValueId> = HashSet::new();
                for block in function.blocks() {
                    for (index, instruction) in block.instructions().iter().enumerate() {
                        if delete_targets.contains(&(block.id(), index))
                            && let Some(dest) = instruction_dest(instruction)
                        {
                            deleted_values.insert(dest);
                        }
                    }
                }

                let mut sites = Vec::new();
                for block in function.blocks() {
                    for (index, instruction) in block.instructions().iter().enumerate() {
                        if let Instruction::IsException { dest, value } = instruction
                            && deleted_values.contains(value)
                            && !delete_targets.contains(&(block.id(), index))
                        {
                            sites.push((block.id(), index, *dest));
                        }
                    }
                }
                sites
            };

            for (block_id, index, ex_dest) in ex_sites {
                let false_id = const_id_or_add_in_module(module, Constant::Bool(false));
                let new_dest = ValueId(next_val);
                next_val += 1;
                consts_to_insert.push((block_id, index, new_dest, false_id));
                replacements.insert(ex_dest, new_dest);
                delete_targets.insert((block_id, index));
            }

            let function = module.function_mut(function_id).expect("valid fid");
            apply_value_replacements(function, &replacements);

            let mut by_block: HashMap<BasicBlockId, Vec<usize>> = HashMap::new();
            for (block_id, index) in &delete_targets {
                by_block.entry(*block_id).or_default().push(*index);
            }
            for (block_id, mut indices) in by_block {
                indices.sort_unstable_by(|left, right| right.cmp(left));
                indices.dedup();
                if let Some(block) = function.block_by_id_mut(block_id) {
                    let instructions = block.instructions_mut();
                    for index in indices {
                        if index < instructions.len() {
                            instructions.remove(index);
                        }
                    }
                }
            }

            for (block_id, _index, dest, constant) in consts_to_insert {
                if let Some(block) = function.block_by_id_mut(block_id) {
                    block
                        .instructions_mut()
                        .insert(0, Instruction::Const { dest, constant });
                }
            }

            any_change = true;
        }
        changed |= any_change;
    }

    changed
}

fn find_const_number(
    function: &wjsm_ir::Function,
    val: ValueId,
    constants: &[Constant],
) -> Option<f64> {
    for b in function.blocks() {
        for ins in b.instructions() {
            if let Instruction::Const { dest, constant } = ins
                && *dest == val
                && let Some(Constant::Number(n)) = constants.get(constant.0 as usize)
            {
                return Some(*n);
            }
        }
    }
    None
}

fn find_part_length(
    function: &wjsm_ir::Function,
    val: ValueId,
    constants: &[Constant],
) -> Option<f64> {
    for b in function.blocks() {
        for ins in b.instructions() {
            if let Instruction::Const { dest, constant } = ins
                && *dest == val
            {
                match constants.get(constant.0 as usize) {
                    Some(Constant::String(s)) => return Some(s.encode_utf16().count() as f64),
                    Some(Constant::Number(n))
                        if (0.0..=1_000_000_000.0).contains(n) && n.fract() == 0.0 =>
                    {
                        let val = *n as u64;
                        if val == 0 {
                            return Some(1.0);
                        }
                        let mut digits = 0.0;
                        let mut temp = val;
                        while temp > 0 {
                            digits += 1.0;
                            temp /= 10;
                        }
                        return Some(digits);
                    }
                    _ => {}
                }
            }
        }
    }
    None
}
