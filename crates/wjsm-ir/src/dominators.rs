//! 支配关系查询：CHK（Cooper–Harvey–Kennedy）idom 不动点 + 支配树 DFS 区间，
//! 构建 O(边数 × 少量轮次)，`dominates` 查询 O(1)。
//!
//! IR 验证器与语义层优化 pass 共用本实现：旧的显式支配集不动点算法
//! （每块维护 `HashSet<BasicBlockId>`、逐轮做集合交）在块数上千时
//! （intrinsic 守卫分叉展开后常见）呈立方级爆炸。
//!
//! 不可达块约定与旧算法一致：不可达块被任何块支配（其支配集从未收缩），
//! 自身不支配任何可达块。

use crate::{BasicBlockId, Function, Terminator};

/// 支配树查询结构，见模块级注释。
pub struct Dominators {
    /// 支配树 DFS 进入序（按块 id 索引；不可达块无意义）。
    tin: Vec<u32>,
    /// 支配树 DFS 离开序。
    tout: Vec<u32>,
    reachable: Vec<bool>,
}

/// 遍历终止器的后继块（可能重复访问同一目标，调用方需自行幂等）。
fn for_each_successor(terminator: &Terminator, mut visit: impl FnMut(BasicBlockId)) {
    match terminator {
        Terminator::Return { .. }
        | Terminator::Throw { .. }
        | Terminator::Unreachable
        | Terminator::Deopt { .. } => {}
        Terminator::Jump { target } => visit(*target),
        Terminator::Branch {
            true_block,
            false_block,
            ..
        } => {
            visit(*true_block);
            visit(*false_block);
        }
        Terminator::Switch {
            cases,
            default_block,
            exit_block,
            ..
        } => {
            for case in cases {
                visit(case.target);
            }
            visit(*default_block);
            visit(*exit_block);
        }
    }
}

impl Dominators {
    /// 依赖块 id 等于其在 `function.blocks()` 中索引的不变量
    /// （由 FunctionBuilder::new_block 保证，验证器亦显式检查）。
    pub fn compute(function: &Function) -> Self {
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
            for_each_successor(function.blocks()[block].terminator(), |succ| {
                let succ = succ.0 as usize;
                if succ < n && !reachable[succ] {
                    stack.push((succ, false));
                }
            });
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
            for_each_successor(function.blocks()[block].terminator(), |succ| {
                let succ = succ.0 as usize;
                if succ < n {
                    preds[succ].push(block);
                }
            });
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

    /// a 是否支配 b（含 a == b）。不可达 b 恒被支配；不可达 a 不支配任何可达块。
    pub fn dominates(&self, a: BasicBlockId, b: BasicBlockId) -> bool {
        let (a, b) = (a.0 as usize, b.0 as usize);
        if !self.reachable[b] {
            return true;
        }
        if !self.reachable[a] {
            return false;
        }
        self.tin[a] <= self.tin[b] && self.tout[b] <= self.tout[a]
    }

    /// 块是否从入口可达。
    pub fn is_reachable(&self, block: BasicBlockId) -> bool {
        self.reachable[block.0 as usize]
    }
}
