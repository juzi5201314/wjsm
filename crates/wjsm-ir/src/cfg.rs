//! 基本块控制流图分析：后继、前驱、可达性、支配树与支配边界。
//!
//! 全仓唯一的 CFG 工具来源。此前 `verify`、语义 pass 与后端各持一份
//! `terminator_successors`，重复后继是否去重的语义互不一致；支配集也有两份
//! 独立实现。本模块统一后继为「按出现顺序去重」，并以 Cooper–Harvey–Kennedy
//! 迭代法一次算出立即支配者，支配集与支配边界都从支配树导出。

use std::collections::{HashMap, HashSet};

use crate::{BasicBlockId, Function, Terminator};

/// 终止器的后继块，按出现顺序去重。
///
/// 去重是必要的：`Branch` 两侧可能指向同一块，`Switch` 的 case / default /
/// exit 也可能重合；前驱表与支配计算都以「边的目标集合」为单位，重复项只会
/// 让前驱表出现同一块多次并干扰 phi 校验。
pub fn terminator_successors(terminator: &Terminator) -> Vec<BasicBlockId> {
    let mut successors = Vec::new();
    let push = |target: BasicBlockId, successors: &mut Vec<BasicBlockId>| {
        if !successors.contains(&target) {
            successors.push(target);
        }
    };
    match terminator {
        Terminator::Return { .. }
        | Terminator::Throw { .. }
        | Terminator::Unreachable
        | Terminator::Deopt { .. } => {}
        Terminator::Jump { target } => push(*target, &mut successors),
        Terminator::Branch {
            true_block,
            false_block,
            ..
        } => {
            push(*true_block, &mut successors);
            push(*false_block, &mut successors);
        }
        Terminator::Switch {
            cases,
            default_block,
            exit_block,
            ..
        } => {
            for case in cases {
                push(case.target, &mut successors);
            }
            push(*default_block, &mut successors);
            push(*exit_block, &mut successors);
        }
    }
    successors
}

/// 一个函数的控制流图快照。块下标即 [`BasicBlockId`] 的数值。
pub struct ControlFlowGraph {
    entry: BasicBlockId,
    successors: Vec<Vec<BasicBlockId>>,
    predecessors: Vec<Vec<BasicBlockId>>,
    reachable: Vec<bool>,
    /// 逆后序：支配迭代与 SSA 重命名都按此序推进以最快收敛。
    reverse_post_order: Vec<BasicBlockId>,
    /// 逆后序中的位置，`usize::MAX` 表示不可达。
    order_index: Vec<usize>,
}

impl ControlFlowGraph {
    pub fn build(function: &Function) -> Self {
        let count = function.blocks().len();
        let successors: Vec<Vec<BasicBlockId>> = function
            .blocks()
            .iter()
            .map(|block| {
                terminator_successors(block.terminator())
                    .into_iter()
                    .filter(|target| (target.0 as usize) < count)
                    .collect()
            })
            .collect();
        let mut predecessors = vec![Vec::new(); count];
        for (index, targets) in successors.iter().enumerate() {
            let block = BasicBlockId(u32::try_from(index).expect("block index fits u32"));
            for target in targets {
                predecessors[target.0 as usize].push(block);
            }
        }

        let mut reachable = vec![false; count];
        let mut post_order = Vec::with_capacity(count);
        if count > 0 {
            depth_first_post_order(
                function.entry(),
                &successors,
                &mut reachable,
                &mut post_order,
            );
        }
        let reverse_post_order: Vec<BasicBlockId> = post_order.into_iter().rev().collect();
        let mut order_index = vec![usize::MAX; count];
        for (position, block) in reverse_post_order.iter().enumerate() {
            order_index[block.0 as usize] = position;
        }

        Self {
            entry: function.entry(),
            successors,
            predecessors,
            reachable,
            reverse_post_order,
            order_index,
        }
    }

    pub fn entry(&self) -> BasicBlockId {
        self.entry
    }

    pub fn block_count(&self) -> usize {
        self.successors.len()
    }

    pub fn successors(&self, block: BasicBlockId) -> &[BasicBlockId] {
        self.successors
            .get(block.0 as usize)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    pub fn predecessors(&self, block: BasicBlockId) -> &[BasicBlockId] {
        self.predecessors
            .get(block.0 as usize)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    pub fn is_reachable(&self, block: BasicBlockId) -> bool {
        self.reachable
            .get(block.0 as usize)
            .copied()
            .unwrap_or(false)
    }

    pub fn reverse_post_order(&self) -> &[BasicBlockId] {
        &self.reverse_post_order
    }

    /// 立即支配者。入口块与不可达块为 `None`。
    pub fn immediate_dominators(&self) -> Vec<Option<BasicBlockId>> {
        let count = self.block_count();
        let mut idom: Vec<Option<BasicBlockId>> = vec![None; count];
        if count == 0 || !self.is_reachable(self.entry) {
            return idom;
        }
        idom[self.entry.0 as usize] = Some(self.entry);

        let mut changed = true;
        while changed {
            changed = false;
            for block in &self.reverse_post_order {
                if *block == self.entry {
                    continue;
                }
                let mut new_idom: Option<BasicBlockId> = None;
                for predecessor in self.predecessors(*block) {
                    if idom[predecessor.0 as usize].is_none() {
                        continue;
                    }
                    new_idom = Some(match new_idom {
                        None => *predecessor,
                        Some(current) => self.intersect(*predecessor, current, &idom),
                    });
                }
                if new_idom.is_some() && idom[block.0 as usize] != new_idom {
                    idom[block.0 as usize] = new_idom;
                    changed = true;
                }
            }
        }
        // 入口自支配只是迭代的初值约定，对外统一表示为「无立即支配者」。
        idom[self.entry.0 as usize] = None;
        idom
    }

    /// 沿逆后序位置上行求两个块在支配树上的最近公共祖先。
    fn intersect(
        &self,
        mut left: BasicBlockId,
        mut right: BasicBlockId,
        idom: &[Option<BasicBlockId>],
    ) -> BasicBlockId {
        while left != right {
            while self.order_index[left.0 as usize] > self.order_index[right.0 as usize] {
                match idom[left.0 as usize] {
                    Some(next) if next != left => left = next,
                    _ => return right,
                }
            }
            while self.order_index[right.0 as usize] > self.order_index[left.0 as usize] {
                match idom[right.0 as usize] {
                    Some(next) if next != right => right = next,
                    _ => return left,
                }
            }
        }
        left
    }

    /// 支配边界：`frontiers[b]` 是所有「b 支配其某个前驱但不严格支配自身」的块。
    pub fn dominance_frontiers(&self, idom: &[Option<BasicBlockId>]) -> Vec<Vec<BasicBlockId>> {
        let mut frontiers = vec![Vec::new(); self.block_count()];
        for block in &self.reverse_post_order {
            let predecessors = self.predecessors(*block);
            if predecessors.len() < 2 {
                continue;
            }
            for predecessor in predecessors {
                let mut runner = *predecessor;
                while Some(runner) != idom[block.0 as usize] && self.is_reachable(runner) {
                    let frontier = &mut frontiers[runner.0 as usize];
                    if !frontier.contains(block) {
                        frontier.push(*block);
                    }
                    match idom[runner.0 as usize] {
                        Some(next) => runner = next,
                        None => break,
                    }
                }
            }
        }
        frontiers
    }

    /// 支配树的子块表，按块下标索引。
    pub fn dominator_tree_children(&self, idom: &[Option<BasicBlockId>]) -> Vec<Vec<BasicBlockId>> {
        let mut children = vec![Vec::new(); self.block_count()];
        for (index, parent) in idom.iter().enumerate() {
            let Some(parent) = parent else { continue };
            children[parent.0 as usize].push(BasicBlockId(
                u32::try_from(index).expect("block index fits u32"),
            ));
        }
        for list in &mut children {
            list.sort_unstable_by_key(|block| block.0);
        }
        children
    }

    /// DFS 回边集合：边 `(u, v)` 中 v 是 u 在 DFS 树上的（在栈）祖先。
    ///
    /// 任意有向图的每个环都至少含一条 DFS 回边（首个被发现的环上节点经白路径
    /// 成为其余环上节点的祖先），因此以此集合作为协作式轮询点可保证每个运行时
    /// 循环每圈至少轮询一次；与「块 id 递减即回边」不同，表达式级分叉产生的
    /// 乱序前向边不会被误判。
    pub fn dfs_back_edges(&self) -> HashSet<(BasicBlockId, BasicBlockId)> {
        let count = self.block_count();
        let mut back_edges = HashSet::new();
        if count == 0 || (self.entry.0 as usize) >= count {
            return back_edges;
        }
        let mut visited = vec![false; count];
        let mut on_stack = vec![false; count];
        let mut stack = vec![(self.entry, 0usize)];
        visited[self.entry.0 as usize] = true;
        on_stack[self.entry.0 as usize] = true;
        while let Some((block, cursor)) = stack.pop() {
            let targets = self.successors(block);
            if cursor < targets.len() {
                stack.push((block, cursor + 1));
                let next = targets[cursor];
                if on_stack[next.0 as usize] {
                    back_edges.insert((block, next));
                } else if !visited[next.0 as usize] {
                    visited[next.0 as usize] = true;
                    on_stack[next.0 as usize] = true;
                    stack.push((next, 0));
                }
            } else {
                on_stack[block.0 as usize] = false;
            }
        }
        back_edges
    }

    /// 完整支配集：`dom[b]` 含 b 自身。不可达块只被自身支配。
    pub fn dominator_sets(&self) -> HashMap<BasicBlockId, HashSet<BasicBlockId>> {
        let idom = self.immediate_dominators();
        let mut sets = HashMap::with_capacity(self.block_count());
        for index in 0..self.block_count() {
            let block = BasicBlockId(u32::try_from(index).expect("block index fits u32"));
            let mut set = HashSet::from([block]);
            if self.is_reachable(block) {
                let mut cursor = idom[index];
                while let Some(parent) = cursor {
                    if !set.insert(parent) {
                        break;
                    }
                    cursor = idom[parent.0 as usize];
                }
            }
            sets.insert(block, set);
        }
        sets
    }
}

fn depth_first_post_order(
    entry: BasicBlockId,
    successors: &[Vec<BasicBlockId>],
    reachable: &mut [bool],
    post_order: &mut Vec<BasicBlockId>,
) {
    // 显式栈：函数块数可达数百，递归会在深链 CFG 上有栈溢出风险。
    let mut stack = vec![(entry, 0usize)];
    if (entry.0 as usize) >= reachable.len() {
        return;
    }
    reachable[entry.0 as usize] = true;
    while let Some((block, cursor)) = stack.pop() {
        let targets = &successors[block.0 as usize];
        if cursor < targets.len() {
            stack.push((block, cursor + 1));
            let next = targets[cursor];
            if !reachable[next.0 as usize] {
                reachable[next.0 as usize] = true;
                stack.push((next, 0));
            }
        } else {
            post_order.push(block);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BasicBlock, ValueId};

    fn diamond() -> Function {
        // bb0 → {bb1, bb2} → bb3
        let mut function = Function::new("diamond", BasicBlockId(0));
        let mut entry = BasicBlock::new(BasicBlockId(0));
        entry.set_terminator(Terminator::Branch {
            condition: ValueId(0),
            true_block: BasicBlockId(1),
            false_block: BasicBlockId(2),
        });
        let mut left = BasicBlock::new(BasicBlockId(1));
        left.set_terminator(Terminator::Jump {
            target: BasicBlockId(3),
        });
        let mut right = BasicBlock::new(BasicBlockId(2));
        right.set_terminator(Terminator::Jump {
            target: BasicBlockId(3),
        });
        let mut join = BasicBlock::new(BasicBlockId(3));
        join.set_terminator(Terminator::Return { value: None });
        for block in [entry, left, right, join] {
            function.push_block(block);
        }
        function
    }

    #[test]
    fn branch_to_same_target_yields_one_successor() {
        let successors = terminator_successors(&Terminator::Branch {
            condition: ValueId(0),
            true_block: BasicBlockId(1),
            false_block: BasicBlockId(1),
        });
        assert_eq!(successors, vec![BasicBlockId(1)]);
    }

    #[test]
    fn diamond_dominators_and_frontier() {
        let function = diamond();
        let cfg = ControlFlowGraph::build(&function);
        let idom = cfg.immediate_dominators();
        assert_eq!(idom[0], None);
        assert_eq!(idom[1], Some(BasicBlockId(0)));
        assert_eq!(idom[2], Some(BasicBlockId(0)));
        assert_eq!(idom[3], Some(BasicBlockId(0)));

        let frontiers = cfg.dominance_frontiers(&idom);
        assert_eq!(frontiers[1], vec![BasicBlockId(3)]);
        assert_eq!(frontiers[2], vec![BasicBlockId(3)]);
        assert!(frontiers[0].is_empty());
        assert!(frontiers[3].is_empty());
    }

    #[test]
    fn loop_header_is_its_own_frontier() {
        // bb0 → bb1 ⇄ bb2, bb1 → bb3
        let mut function = Function::new("loop", BasicBlockId(0));
        let mut entry = BasicBlock::new(BasicBlockId(0));
        entry.set_terminator(Terminator::Jump {
            target: BasicBlockId(1),
        });
        let mut header = BasicBlock::new(BasicBlockId(1));
        header.set_terminator(Terminator::Branch {
            condition: ValueId(0),
            true_block: BasicBlockId(2),
            false_block: BasicBlockId(3),
        });
        let mut body = BasicBlock::new(BasicBlockId(2));
        body.set_terminator(Terminator::Jump {
            target: BasicBlockId(1),
        });
        let mut exit = BasicBlock::new(BasicBlockId(3));
        exit.set_terminator(Terminator::Return { value: None });
        for block in [entry, header, body, exit] {
            function.push_block(block);
        }

        let cfg = ControlFlowGraph::build(&function);
        let idom = cfg.immediate_dominators();
        assert_eq!(idom[1], Some(BasicBlockId(0)));
        assert_eq!(idom[2], Some(BasicBlockId(1)));
        assert_eq!(idom[3], Some(BasicBlockId(1)));
        let frontiers = cfg.dominance_frontiers(&idom);
        assert_eq!(frontiers[2], vec![BasicBlockId(1)]);
    }

    #[test]
    fn dfs_back_edges_ignore_id_descending_forward_edges() {
        // bb0 → bb1(头) → bb4 → {bb2(体), bb3(出口)}；bb2 → bb5 → bb1。
        // bb4→bb2 与 bb5→bb1 都是块 id 递减的边，但只有 bb5→bb1 闭合环。
        let mut function = Function::new("loop", BasicBlockId(0));
        let mut entry = BasicBlock::new(BasicBlockId(0));
        entry.set_terminator(Terminator::Jump {
            target: BasicBlockId(1),
        });
        let mut header = BasicBlock::new(BasicBlockId(1));
        header.set_terminator(Terminator::Jump {
            target: BasicBlockId(4),
        });
        let mut body = BasicBlock::new(BasicBlockId(2));
        body.set_terminator(Terminator::Jump {
            target: BasicBlockId(5),
        });
        let mut exit = BasicBlock::new(BasicBlockId(3));
        exit.set_terminator(Terminator::Return { value: None });
        let mut dispatch = BasicBlock::new(BasicBlockId(4));
        dispatch.set_terminator(Terminator::Branch {
            condition: ValueId(0),
            true_block: BasicBlockId(2),
            false_block: BasicBlockId(3),
        });
        let mut latch = BasicBlock::new(BasicBlockId(5));
        latch.set_terminator(Terminator::Jump {
            target: BasicBlockId(1),
        });
        for block in [entry, header, body, exit, dispatch, latch] {
            function.push_block(block);
        }

        let cfg = ControlFlowGraph::build(&function);
        assert_eq!(
            cfg.dfs_back_edges(),
            HashSet::from([(BasicBlockId(5), BasicBlockId(1))])
        );
    }

    #[test]
    fn dfs_back_edges_include_self_loop() {
        let mut function = Function::new("spin", BasicBlockId(0));
        let mut entry = BasicBlock::new(BasicBlockId(0));
        entry.set_terminator(Terminator::Jump {
            target: BasicBlockId(1),
        });
        let mut spin = BasicBlock::new(BasicBlockId(1));
        spin.set_terminator(Terminator::Jump {
            target: BasicBlockId(1),
        });
        for block in [entry, spin] {
            function.push_block(block);
        }
        let cfg = ControlFlowGraph::build(&function);
        assert_eq!(
            cfg.dfs_back_edges(),
            HashSet::from([(BasicBlockId(1), BasicBlockId(1))])
        );
    }

    #[test]
    fn unreachable_block_is_dominated_only_by_itself() {
        let mut function = diamond();
        let mut orphan = BasicBlock::new(BasicBlockId(4));
        orphan.set_terminator(Terminator::Return { value: None });
        function.push_block(orphan);

        let cfg = ControlFlowGraph::build(&function);
        assert!(!cfg.is_reachable(BasicBlockId(4)));
        let sets = cfg.dominator_sets();
        assert_eq!(sets[&BasicBlockId(4)], HashSet::from([BasicBlockId(4)]));
        assert_eq!(
            sets[&BasicBlockId(3)],
            HashSet::from([BasicBlockId(0), BasicBlockId(3)])
        );
    }
}
