//! 帧局部变量的 SSA 视图（不改写 IR，构造在 IR 之侧）。
//!
//! `LoadVar` / `StoreVar` 以变量名寻址，类型分析若按名求解就只能取「该名字全部
//! store 的交」——函数级 `var` 提升产生的 `store x, undefined` 会把整个变量拖成
//! 混合类型，`let x; x = 0;` 与条件初始化同理，热循环因此整体退回 boxed。
//!
//! 本模块对帧局部变量做标准的支配边界 φ 插入 + 支配树重命名，为每个 `LoadVar`
//! 解出其到达定义。`varLoop` 里循环头的 φ 只汇合 `0` 与 `i + 1`，提升写入的
//! `undefined` 被同块内的支配性赋值杀死，于是整条链可证为 f64。
//!
//! 选择「在 IR 之侧构造」而非「重写 IR」：后端已用 Cranelift `Variable` 做过一次
//! SSA 构造，materialize 到 IR 不会改善代码生成，却会让全部 lowering 快照失效。

use std::collections::{BTreeSet, HashMap};

use crate::cfg::ControlFlowGraph;
use crate::{BasicBlockId, Function, Instruction, ValueId};

/// 一个变量在某处的到达定义。
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum VarDef {
    /// 由某条 `StoreVar` 写入的 SSA 值。
    Value(ValueId),
    /// 合流点的 φ 节点下标，见 [`VariableSsa::phi_sources`]。
    Phi(u32),
    /// 该路径上变量尚未被写入。对非参数局部即 `undefined`；对参数即入参初值。
    Entry,
}

/// 一个 φ 节点：归属变量与各前驱边上的到达定义。
///
/// 归属变量必须记录：[`VarDef::Entry`] 对「已证明为 f64 的入参」和「尚未初始化
/// 的局部」含义不同，求解方要按变量名区分。
struct PhiNode<'a> {
    variable: &'a str,
    sources: Vec<VarDef>,
}

/// 帧局部变量的 SSA 解析结果。
pub struct VariableSsa<'a> {
    /// `LoadVar` 的 dest → 到达定义。未参与分析的变量不在表内。
    loads: HashMap<ValueId, VarDef>,
    /// φ 节点；下标即 [`VarDef::Phi`] 的载荷。
    phis: Vec<PhiNode<'a>>,
}

impl<'a> VariableSsa<'a> {
    /// 对 `names` 中的变量构造 SSA 视图。调用方负责只传入帧局部候选名
    /// （见 `Function::frame_local_candidate_names`），本函数不再过滤。
    pub fn build(function: &'a Function, names: &BTreeSet<&'a str>) -> Self {
        let mut ssa = Self {
            loads: HashMap::new(),
            phis: Vec::new(),
        };
        if names.is_empty() || function.blocks().is_empty() {
            return ssa;
        }
        let cfg = ControlFlowGraph::build(function);
        let idom = cfg.immediate_dominators();
        let frontiers = cfg.dominance_frontiers(&idom);
        let children = cfg.dominator_tree_children(&idom);
        for name in names {
            ssa.build_variable(function, &cfg, &frontiers, &children, name);
        }
        ssa
    }

    pub fn load_definition(&self, dest: ValueId) -> Option<VarDef> {
        self.loads.get(&dest).copied()
    }

    pub fn phi_sources(&self, phi: u32) -> &[VarDef] {
        self.phis
            .get(phi as usize)
            .map(|node| node.sources.as_slice())
            .unwrap_or_default()
    }

    /// φ 节点归属的变量名。
    pub fn phi_variable(&self, phi: u32) -> Option<&'a str> {
        self.phis.get(phi as usize).map(|node| node.variable)
    }

    pub fn phi_count(&self) -> usize {
        self.phis.len()
    }

    fn build_variable(
        &mut self,
        function: &'a Function,
        cfg: &ControlFlowGraph,
        frontiers: &[Vec<BasicBlockId>],
        children: &[Vec<BasicBlockId>],
        name: &'a str,
    ) {
        let block_count = cfg.block_count();
        let mut defining_blocks = Vec::new();
        for block in function.blocks() {
            if !cfg.is_reachable(block.id()) {
                continue;
            }
            if block
                .instructions()
                .iter()
                .any(|instruction| matches!(instruction, Instruction::StoreVar { name: n, .. } if n == name))
            {
                defining_blocks.push(block.id());
            }
        }
        if defining_blocks.is_empty() {
            // 只读变量（含未被本函数写入的参数）：所有 load 都取入口定义。
            self.record_entry_loads(function, cfg, name);
            return;
        }

        // 迭代支配边界上插 φ。
        let mut phi_at: Vec<Option<u32>> = vec![None; block_count];
        let mut worklist = defining_blocks.clone();
        while let Some(block) = worklist.pop() {
            for frontier in &frontiers[block.0 as usize] {
                if phi_at[frontier.0 as usize].is_some() {
                    continue;
                }
                let phi = u32::try_from(self.phis.len()).expect("phi 数在 u32 内");
                self.phis.push(PhiNode {
                    variable: name,
                    sources: Vec::new(),
                });
                phi_at[frontier.0 as usize] = Some(phi);
                if !defining_blocks.contains(frontier) {
                    defining_blocks.push(*frontier);
                    worklist.push(*frontier);
                }
            }
        }

        // 支配树前序重命名：进入块时压入本块产生的定义，离开时弹出。
        let mut current: Vec<Option<VarDef>> = vec![None; block_count];
        let entry = cfg.entry();
        let mut stack = vec![(entry, VarDef::Entry, false)];
        while let Some((block, incoming, processed)) = stack.pop() {
            if processed {
                continue;
            }
            let mut reaching = incoming;
            if let Some(phi) = phi_at[block.0 as usize] {
                reaching = VarDef::Phi(phi);
            }
            for instruction in function.blocks()[block.0 as usize].instructions() {
                match instruction {
                    Instruction::LoadVar { dest, name: n } if n == name => {
                        self.loads.insert(*dest, reaching);
                    }
                    Instruction::StoreVar { name: n, value } if n == name => {
                        reaching = VarDef::Value(*value);
                    }
                    _ => {}
                }
            }
            current[block.0 as usize] = Some(reaching);
            for child in &children[block.0 as usize] {
                stack.push((*child, reaching, false));
            }
        }

        // φ 输入：取每个前驱块出口处的到达定义。
        for (index, phi) in phi_at.iter().enumerate() {
            let Some(phi) = phi else { continue };
            let block = BasicBlockId(u32::try_from(index).expect("block index fits u32"));
            let sources: Vec<VarDef> = cfg
                .predecessors(block)
                .iter()
                .filter(|predecessor| cfg.is_reachable(**predecessor))
                .map(|predecessor| current[predecessor.0 as usize].unwrap_or(VarDef::Entry))
                .collect();
            self.phis[*phi as usize].sources = sources;
        }
    }

    fn record_entry_loads(&mut self, function: &Function, cfg: &ControlFlowGraph, name: &str) {
        for block in function.blocks() {
            if !cfg.is_reachable(block.id()) {
                continue;
            }
            for instruction in block.instructions() {
                if let Instruction::LoadVar { dest, name: n } = instruction
                    && n == name
                {
                    self.loads.insert(*dest, VarDef::Entry);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BasicBlock, ConstantId, Terminator};

    /// 复刻 `function f() { var t = 0; for (var i = 0; i < N; i++) { t += i; } return t; }`
    /// 经 lowering 后的形状：bb0 先写入提升产生的 undefined，再写入数字初值。
    fn hoisted_var_loop() -> Function {
        let mut function = Function::new("varLoop", BasicBlockId(0));

        let mut bb0 = BasicBlock::new(BasicBlockId(0));
        bb0.push_instruction(Instruction::Const {
            dest: ValueId(0),
            constant: ConstantId(0), // undefined
        });
        bb0.push_instruction(Instruction::StoreVar {
            name: "$1.i".into(),
            value: ValueId(0),
        });
        bb0.push_instruction(Instruction::Const {
            dest: ValueId(1),
            constant: ConstantId(1), // 0
        });
        bb0.push_instruction(Instruction::StoreVar {
            name: "$1.i".into(),
            value: ValueId(1),
        });
        bb0.set_terminator(Terminator::Jump {
            target: BasicBlockId(1),
        });

        let mut bb1 = BasicBlock::new(BasicBlockId(1));
        bb1.push_instruction(Instruction::LoadVar {
            dest: ValueId(2),
            name: "$1.i".into(),
        });
        bb1.set_terminator(Terminator::Branch {
            condition: ValueId(2),
            true_block: BasicBlockId(2),
            false_block: BasicBlockId(3),
        });

        let mut bb2 = BasicBlock::new(BasicBlockId(2));
        bb2.push_instruction(Instruction::LoadVar {
            dest: ValueId(3),
            name: "$1.i".into(),
        });
        bb2.push_instruction(Instruction::StoreVar {
            name: "$1.i".into(),
            value: ValueId(4),
        });
        bb2.set_terminator(Terminator::Jump {
            target: BasicBlockId(1),
        });

        let mut bb3 = BasicBlock::new(BasicBlockId(3));
        bb3.set_terminator(Terminator::Return { value: None });

        for block in [bb0, bb1, bb2, bb3] {
            function.push_block(block);
        }
        function
    }

    #[test]
    fn hoisted_undefined_store_is_killed_by_dominating_assignment() {
        let function = hoisted_var_loop();
        let names = BTreeSet::from(["$1.i"]);
        let ssa = VariableSsa::build(&function, &names);

        // 循环头的 load 读 φ；φ 只汇合初值 0 与回边的 i+1，提升的 undefined 不参与。
        let Some(VarDef::Phi(phi)) = ssa.load_definition(ValueId(2)) else {
            panic!("循环头 load 应解析为 φ");
        };
        let sources = ssa.phi_sources(phi);
        assert_eq!(sources.len(), 2);
        assert!(sources.contains(&VarDef::Value(ValueId(1))));
        assert!(sources.contains(&VarDef::Value(ValueId(4))));
        assert!(!sources.contains(&VarDef::Value(ValueId(0))));
        // 循环体内的 load 同样读 φ。
        assert_eq!(ssa.load_definition(ValueId(3)), Some(VarDef::Phi(phi)));
    }

    #[test]
    fn straight_line_load_reads_the_latest_store() {
        let mut function = Function::new("linear", BasicBlockId(0));
        let mut block = BasicBlock::new(BasicBlockId(0));
        block.push_instruction(Instruction::StoreVar {
            name: "$1.x".into(),
            value: ValueId(0),
        });
        block.push_instruction(Instruction::LoadVar {
            dest: ValueId(1),
            name: "$1.x".into(),
        });
        block.push_instruction(Instruction::StoreVar {
            name: "$1.x".into(),
            value: ValueId(2),
        });
        block.push_instruction(Instruction::LoadVar {
            dest: ValueId(3),
            name: "$1.x".into(),
        });
        block.set_terminator(Terminator::Return { value: None });
        function.push_block(block);

        let ssa = VariableSsa::build(&function, &BTreeSet::from(["$1.x"]));
        assert_eq!(
            ssa.load_definition(ValueId(1)),
            Some(VarDef::Value(ValueId(0)))
        );
        assert_eq!(
            ssa.load_definition(ValueId(3)),
            Some(VarDef::Value(ValueId(2)))
        );
    }

    #[test]
    fn conditional_initialization_merges_both_arms() {
        // bb0 → {bb1: x = a, bb2: x = b} → bb3: load x
        let mut function = Function::new("conditional", BasicBlockId(0));
        let mut bb0 = BasicBlock::new(BasicBlockId(0));
        bb0.set_terminator(Terminator::Branch {
            condition: ValueId(0),
            true_block: BasicBlockId(1),
            false_block: BasicBlockId(2),
        });
        let mut bb1 = BasicBlock::new(BasicBlockId(1));
        bb1.push_instruction(Instruction::StoreVar {
            name: "$1.x".into(),
            value: ValueId(1),
        });
        bb1.set_terminator(Terminator::Jump {
            target: BasicBlockId(3),
        });
        let mut bb2 = BasicBlock::new(BasicBlockId(2));
        bb2.push_instruction(Instruction::StoreVar {
            name: "$1.x".into(),
            value: ValueId(2),
        });
        bb2.set_terminator(Terminator::Jump {
            target: BasicBlockId(3),
        });
        let mut bb3 = BasicBlock::new(BasicBlockId(3));
        bb3.push_instruction(Instruction::LoadVar {
            dest: ValueId(3),
            name: "$1.x".into(),
        });
        bb3.set_terminator(Terminator::Return { value: None });
        for block in [bb0, bb1, bb2, bb3] {
            function.push_block(block);
        }

        let ssa = VariableSsa::build(&function, &BTreeSet::from(["$1.x"]));
        let Some(VarDef::Phi(phi)) = ssa.load_definition(ValueId(3)) else {
            panic!("合流处 load 应解析为 φ");
        };
        let sources = ssa.phi_sources(phi);
        assert_eq!(sources.len(), 2);
        assert!(sources.contains(&VarDef::Value(ValueId(1))));
        assert!(sources.contains(&VarDef::Value(ValueId(2))));
    }

    #[test]
    fn partially_initialized_variable_keeps_entry_definition() {
        // bb0 → {bb1: x = a, bb2: 不写} → bb3: load x
        let mut function = Function::new("partial", BasicBlockId(0));
        let mut bb0 = BasicBlock::new(BasicBlockId(0));
        bb0.set_terminator(Terminator::Branch {
            condition: ValueId(0),
            true_block: BasicBlockId(1),
            false_block: BasicBlockId(2),
        });
        let mut bb1 = BasicBlock::new(BasicBlockId(1));
        bb1.push_instruction(Instruction::StoreVar {
            name: "$1.x".into(),
            value: ValueId(1),
        });
        bb1.set_terminator(Terminator::Jump {
            target: BasicBlockId(3),
        });
        let mut bb2 = BasicBlock::new(BasicBlockId(2));
        bb2.set_terminator(Terminator::Jump {
            target: BasicBlockId(3),
        });
        let mut bb3 = BasicBlock::new(BasicBlockId(3));
        bb3.push_instruction(Instruction::LoadVar {
            dest: ValueId(2),
            name: "$1.x".into(),
        });
        bb3.set_terminator(Terminator::Return { value: None });
        for block in [bb0, bb1, bb2, bb3] {
            function.push_block(block);
        }

        let ssa = VariableSsa::build(&function, &BTreeSet::from(["$1.x"]));
        let Some(VarDef::Phi(phi)) = ssa.load_definition(ValueId(2)) else {
            panic!("合流处 load 应解析为 φ");
        };
        let sources = ssa.phi_sources(phi);
        assert!(sources.contains(&VarDef::Value(ValueId(1))));
        assert!(sources.contains(&VarDef::Entry));
    }
}
