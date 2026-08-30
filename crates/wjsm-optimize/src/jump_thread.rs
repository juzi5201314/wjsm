//! 空跳转块穿线：无指令的 `jump T` 把入边改到 T。
//!
//! for 循环 lowering 会在条件、体、自增之间留下纯跳板块。后端按 IR 块
//! 发射 Cranelift 块，跳板会拆散 typed f64 热循环、迫使归纳变量往栈上
//! 溢出。本阶段只穿**目标不含 Phi** 的跳板——穿进循环头会打乱归纳 Phi
//! 的前驱集合。含指令的块（哪怕只有 Phi）一律不动。

use wjsm_ir::{BasicBlockId, Function, Instruction, Terminator};

/// 对单个函数穿线。返回是否改写了任何终止器。
pub(crate) fn run_function(function: &mut Function) -> bool {
    let n = function.blocks().len();
    if n == 0 {
        return false;
    }
    let trampoline = trampoline_targets(function);
    if trampoline.iter().all(Option::is_none) {
        return false;
    }
    let mut changed = false;
    for block in function.blocks_mut() {
        let old = block.terminator().clone();
        let mut term = old.clone();
        term.remap_blocks(&mut |id| {
            trampoline
                .get(id.0 as usize)
                .copied()
                .flatten()
                .unwrap_or(id)
        });
        if term != old {
            block.set_terminator(term);
            changed = true;
        }
    }
    changed
}

/// 块 id → 压缩后的最终非跳板目标。入口块与含 Phi 的目标不参与。
fn trampoline_targets(function: &Function) -> Vec<Option<BasicBlockId>> {
    let n = function.blocks().len();
    let entry = function.entry();
    let mut trampoline = vec![None; n];
    for block in function.blocks() {
        if block.id() == entry || !block.instructions().is_empty() {
            continue;
        }
        let Terminator::Jump { target } = block.terminator() else {
            continue;
        };
        if *target == block.id() || (target.0 as usize) >= n {
            continue;
        }
        if function.blocks()[target.0 as usize]
            .instructions()
            .iter()
            .any(|instruction| matches!(instruction, Instruction::Phi { .. }))
        {
            continue;
        }
        trampoline[block.id().0 as usize] = Some(*target);
    }
    for i in 0..n {
        let Some(start) = trampoline[i] else {
            continue;
        };
        let mut current = start;
        for _ in 0..n {
            let Some(next) = trampoline[current.0 as usize] else {
                break;
            };
            if next.0 as usize == i {
                break;
            }
            current = next;
        }
        trampoline[i] = Some(current);
    }
    trampoline
}

#[cfg(test)]
mod tests {
    use super::*;
    use wjsm_ir::{BasicBlock, Terminator as Term};
    use wjsm_parser::parse_module;
    use wjsm_semantic::lower_module;

    fn reachable_empty_jumps(function: &Function) -> usize {
        let mut seen = vec![false; function.blocks().len()];
        let mut stack = vec![function.entry()];
        while let Some(id) = stack.pop() {
            let idx = id.0 as usize;
            if idx >= seen.len() || seen[idx] {
                continue;
            }
            seen[idx] = true;
            for succ in wjsm_ir::cfg::terminator_successors(function.blocks()[idx].terminator()) {
                stack.push(succ);
            }
        }
        function
            .blocks()
            .iter()
            .filter(|block| {
                seen[block.id().0 as usize]
                    && block.instructions().is_empty()
                    && matches!(block.terminator(), Term::Jump { .. })
            })
            .count()
    }

    #[test]
    fn threads_empty_jump_into_phi_free_target() {
        let mut function = Function::new("t", BasicBlockId(0));
        let mut entry = BasicBlock::new(BasicBlockId(0));
        entry.set_terminator(Term::Jump {
            target: BasicBlockId(1),
        });
        let mut mid = BasicBlock::new(BasicBlockId(1));
        mid.set_terminator(Term::Jump {
            target: BasicBlockId(2),
        });
        let mut exit = BasicBlock::new(BasicBlockId(2));
        exit.set_terminator(Term::Return { value: None });
        function.push_block(entry);
        function.push_block(mid);
        function.push_block(exit);

        assert!(run_function(&mut function));
        assert_eq!(
            function.blocks()[0].terminator(),
            &Term::Jump {
                target: BasicBlockId(2)
            }
        );
    }

    #[test]
    fn for_loop_work_has_no_reachable_empty_jumps_after_optimize() {
        let source = r#"
function work() {
  let s = 0.0;
  for (let i = 0; i < 4; i++) s += i * 1.0001;
  return s;
}
"#;
        let module = parse_module(source).expect("解析");
        let mut program = lower_module(module, false).expect("lowering");
        crate::optimize_sound(&mut program);
        let work = program
            .functions()
            .iter()
            .find(|function| function.name() == "work")
            .expect("work");
        assert_eq!(
            reachable_empty_jumps(work),
            0,
            "for 循环跳板应被穿掉：{}",
            work.blocks()
                .iter()
                .map(|block| format!("{}: {}", block.id(), block.terminator()))
                .collect::<Vec<_>>()
                .join("; ")
        );
    }
}
