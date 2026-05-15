use wjsm_ir::{BasicBlock, Instruction, Terminator};

use super::state::LoopInfo;

pub(crate) fn block_has_suspend(block: &BasicBlock) -> bool {
    block
        .instructions()
        .iter()
        .any(|instruction| matches!(instruction, Instruction::Suspend { .. }))
}

/// 检测 CFG 中的循环（通过 back-edge 识别）。
/// 返回按 header_idx 排序的 LoopInfo 列表。

pub(crate) fn detect_loops(blocks: &[BasicBlock]) -> Vec<LoopInfo> {
    use std::collections::{HashMap, HashSet};
    let mut back_edges: HashMap<usize, Vec<usize>> = HashMap::new();

    for (i, block) in blocks.iter().enumerate() {
        match block.terminator() {
            Terminator::Jump { target } => {
                let t = target.0 as usize;
                if t <= i {
                    back_edges.entry(t).or_default().push(i);
                }
            }
            Terminator::Branch { true_block, .. } => {
                // do-while 模式：true → header（通过 Branch 实现的 back-edge）
                let t = true_block.0 as usize;
                if t <= i {
                    back_edges.entry(t).or_default().push(i);
                }
            }
            _ => {}
        }
    }

    let mut loops: Vec<LoopInfo> = Vec::new();
    // NOTE: 此处对每个 back-edge 做前向可达性分析以过滤无效循环。
    // 在大型 CFG 上可能有性能影响，未来可考虑使用 dominator tree 分析替代。
    'next_edge: for (header_idx, latches) in &back_edges {
        let mut reachable: HashSet<usize> = HashSet::new();
        let mut stack = vec![*header_idx];
        while let Some(idx) = stack.pop() {
            if reachable.contains(&idx) {
                continue;
            }
            reachable.insert(idx);
            if idx >= blocks.len() {
                continue;
            }
            match blocks[idx].terminator() {
                Terminator::Jump { target } => {
                    stack.push(target.0 as usize);
                }
                Terminator::Branch {
                    true_block,
                    false_block,
                    ..
                } => {
                    stack.push(true_block.0 as usize);
                    stack.push(false_block.0 as usize);
                }
                Terminator::Switch {
                    cases,
                    default_block,
                    exit_block,
                    ..
                } => {
                    for case in cases {
                        stack.push(case.target.0 as usize);
                    }
                    stack.push(default_block.0 as usize);
                    stack.push(exit_block.0 as usize);
                }
                _ => {}
            }
        }
        let mut any_latch_reachable = false;
        for latch in latches {
            if reachable.contains(latch) {
                any_latch_reachable = true;
                break;
            }
        }
        if !any_latch_reachable {
            continue 'next_edge;
        }

        let exit_idx = match blocks[*header_idx].terminator() {
            // while/for 模式：header 有 Branch，false 分支是出口
            Terminator::Branch { false_block, .. } => false_block.0 as usize,
            _ => {
                // do-while 模式：header 没有 Branch，找到指向 header 的 Branch
                let mut exit = *header_idx + 1;
                for block in blocks.iter() {
                    if let Terminator::Branch {
                        true_block,
                        false_block,
                        ..
                    } = block.terminator()
                    {
                        if true_block.0 as usize == *header_idx {
                            exit = false_block.0 as usize;
                            break;
                        }
                    }
                }
                exit
            }
        };
        loops.push(LoopInfo {
            header_idx: *header_idx,
            exit_idx,
        });
    }

    loops.sort_by_key(|l| l.header_idx);
    loops
}


pub(crate) fn is_eval_memory_var_name(name: &str) -> bool {
    !matches!(name, "$env" | "$this" | "$eval_env")
        && !name.ends_with(".$env")
        && !name.ends_with(".$this")
}


pub(crate) fn max_instruction_value_id(instruction: &Instruction) -> u32 {
    match instruction {
        Instruction::Const { dest, .. } => dest.0,
        Instruction::Binary { dest, lhs, rhs, .. } => dest.0.max(lhs.0).max(rhs.0),
        Instruction::Unary { dest, value, .. } => dest.0.max(value.0),
        Instruction::Compare { dest, lhs, rhs, .. } => dest.0.max(lhs.0).max(rhs.0),
        Instruction::Phi { dest, sources } => sources
            .iter()
            .map(|s| s.value.0)
            .max()
            .unwrap_or(0)
            .max(dest.0),
        Instruction::CallBuiltin { dest, args, .. } => {
            let args_max = args.iter().map(|v| v.0).max().unwrap_or(0);
            dest.map_or(args_max, |d| d.0.max(args_max))
        }
        Instruction::LoadVar { dest, .. } => dest.0,
        Instruction::StoreVar { value, .. } => value.0,
        Instruction::Call {
            dest,
            callee,
            this_val,
            args,
        } => {
            let args_max = args.iter().map(|v| v.0).max().unwrap_or(0);
            let max_val = callee.0.max(this_val.0).max(args_max);
            dest.map_or(max_val, |d| d.0.max(max_val))
        }
        Instruction::NewObject { dest, capacity: _ } => dest.0,
        Instruction::GetProp { dest, object, key } => dest.0.max(object.0).max(key.0),
        Instruction::SetProp { object, key, value } => object.0.max(key.0).max(value.0),
        Instruction::DeleteProp { dest, object, key } => dest.0.max(object.0).max(key.0),
        Instruction::SetProto { object, value } => object.0.max(value.0),
        Instruction::NewArray { dest, capacity: _ } => dest.0,
        Instruction::GetElem {
            dest,
            object,
            index,
        } => dest.0.max(object.0).max(index.0),
        Instruction::SetElem {
            object,
            index,
            value,
        } => object.0.max(index.0).max(value.0),
        Instruction::StringConcatVa { dest, parts } => {
            parts.iter().map(|v| v.0).max().unwrap_or(0).max(dest.0)
        }
        Instruction::OptionalGetProp { dest, object, key } => dest.0.max(object.0).max(key.0),
        Instruction::OptionalGetElem { dest, object, key } => dest.0.max(object.0).max(key.0),
        Instruction::OptionalCall {
            dest,
            callee,
            this_val,
            args,
        } => {
            let args_max = args.iter().map(|v| v.0).max().unwrap_or(0);
            let max_val = callee.0.max(this_val.0).max(args_max);
            dest.0.max(max_val)
        }
        Instruction::ObjectSpread { dest, source } => dest.0.max(source.0),
        Instruction::GetSuperBase { dest } => dest.0,
        Instruction::NewPromise { dest } => dest.0,
        Instruction::PromiseResolve { promise, value } => promise.0.max(value.0),
        Instruction::PromiseReject { promise, reason } => promise.0.max(reason.0),
        Instruction::Suspend { promise, .. } => promise.0,
        Instruction::CollectRestArgs { dest, .. } => dest.0,
    }
}

