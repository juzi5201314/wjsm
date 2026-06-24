use super::*;
use std::collections::{HashMap, HashSet};

/// 推迟发射 save/restore 的 suspend 记录
#[derive(Debug, Clone)]
pub(super) struct PendingSuspend {
    /// Suspend 指令所在的 block
    pub(super) suspend_block: BasicBlockId,
    /// resume 后执行起始 block
    pub(super) resume_block: BasicBlockId,
    /// 该 suspend 点可见的所有绑定（async_visible_binding_names 结果）
    pub(super) visible_bindings: Vec<String>,
}

/// 构建 CFG：返回 successors、predecessors 映射。
/// Suspend block 的逻辑 successor 是 resume_block，而不是 terminator 的 Jump 目标。
fn build_cfg(
    blocks: &[BasicBlock],
    pending_suspends: &[PendingSuspend],
) -> (Vec<Vec<BasicBlockId>>, Vec<Vec<BasicBlockId>>) {
    let block_count = blocks.len();
    let suspend_to_resume: HashMap<BasicBlockId, BasicBlockId> = pending_suspends
        .iter()
        .map(|pending| (pending.suspend_block, pending.resume_block))
        .collect();

    let mut successors: Vec<Vec<BasicBlockId>> = vec![Vec::new(); block_count];
    let mut predecessors: Vec<Vec<BasicBlockId>> = vec![Vec::new(); block_count];

    for block in blocks {
        let bid = block.id();
        let targets: Vec<BasicBlockId> = if let Some(&resume) = suspend_to_resume.get(&bid) {
            vec![resume]
        } else {
            match block.terminator() {
                Terminator::Jump { target } => vec![*target],
                Terminator::Branch {
                    true_block,
                    false_block,
                    ..
                } => vec![*true_block, *false_block],
                Terminator::Switch {
                    cases,
                    default_block,
                    ..
                } => {
                    let mut targets = Vec::with_capacity(cases.len() + 1);
                    targets.extend(cases.iter().map(|case| case.target));
                    targets.push(*default_block);
                    targets
                }
                Terminator::Return { .. } | Terminator::Throw { .. } | Terminator::Unreachable => {
                    Vec::new()
                }
            }
        };

        let bid_index = bid.0 as usize;
        for target in targets {
            successors[bid_index].push(target);
            predecessors[target.0 as usize].push(bid);
        }
    }

    (successors, predecessors)
}

/// 计算每个 block 的 use 和 def 集合，只考虑用户变量，排除 async 内部绑定。
/// 当前函数局部用 LoadVar/StoreVar；经 env 的 GetProp(键为 `$scope.name` 字符串) 须计入捕获 use。
fn compute_use_def(
    blocks: &[BasicBlock],
    constants: &[Constant],
) -> (Vec<HashSet<String>>, Vec<HashSet<String>>) {
    let mut use_sets: Vec<HashSet<String>> = vec![HashSet::new(); blocks.len()];
    let mut def_sets: Vec<HashSet<String>> = vec![HashSet::new(); blocks.len()];

    for block in blocks {
        let bid = block.id().0 as usize;
        let mut local_def: HashSet<String> = HashSet::new();
        let instrs: Vec<_> = block.instructions().to_vec();
        let mut const_strings: HashMap<ValueId, String> = HashMap::new();
        let mut load_var_dests: HashMap<ValueId, String> = HashMap::new();

        for instr in &instrs {
            match instr {
                Instruction::Const { dest, constant } => {
                    let idx = constant.0 as usize;
                    if let Some(Constant::String(s)) = constants.get(idx) {
                        const_strings.insert(*dest, s.clone());
                    }
                }
                Instruction::LoadVar { dest, name } => {
                    load_var_dests.insert(*dest, name.clone());
                }
                _ => {}
            }
        }

        for instr in &instrs {
            match instr {
                Instruction::LoadVar { name, .. } => {
                    if !Lowerer::is_async_internal_binding(name) && !local_def.contains(name) {
                        use_sets[bid].insert(name.clone());
                    }
                }
                Instruction::StoreVar { name, .. } if !Lowerer::is_async_internal_binding(name) => {
                    local_def.insert(name.clone());
                    def_sets[bid].insert(name.clone());
                }
                Instruction::GetProp { object, key, .. } => {
                    let env_load = load_var_dests.get(object).is_some_and(|n| {
                        n.contains(".$shared_env") || n.ends_with(".$env") || n == "$eval_env"
                    });
                    if env_load
                        && let Some(binding_ir) = const_strings.get(key)
                        && binding_ir.starts_with('$')
                        && binding_ir.contains('.')
                        && !Lowerer::is_async_internal_binding(binding_ir)
                        && !local_def.contains(binding_ir)
                    {
                        use_sets[bid].insert(binding_ir.clone());
                    }
                }
                _ => {}
            }
        }
    }

    (use_sets, def_sets)
}

/// 标准后向迭代 liveness 分析，返回每个 block 入口处的 live_in 集合。
fn compute_liveness(
    blocks: &[BasicBlock],
    successors: &[Vec<BasicBlockId>],
    use_sets: &[HashSet<String>],
    def_sets: &[HashSet<String>],
) -> Vec<HashSet<String>> {
    let block_count = blocks.len();
    let mut live_in: Vec<HashSet<String>> = vec![HashSet::new(); block_count];
    let mut live_out: Vec<HashSet<String>> = vec![HashSet::new(); block_count];

    loop {
        let mut changed = false;

        for block in blocks.iter().rev() {
            let bid = block.id().0 as usize;

            let mut new_live_out: HashSet<String> = HashSet::new();
            for &successor in &successors[bid] {
                new_live_out.extend(live_in[successor.0 as usize].iter().cloned());
            }

            if new_live_out != live_out[bid] {
                live_out[bid] = new_live_out;
                changed = true;
            }

            let mut new_live_in = use_sets[bid].clone();
            for var in &live_out[bid] {
                if !def_sets[bid].contains(var) {
                    new_live_in.insert(var.clone());
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


mod async_env;
mod async_main;
mod async_bindings;
mod async_await_yield;
mod async_import_promise;
