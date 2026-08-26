use std::collections::{HashMap, HashSet};

use wjsm_ir::{BasicBlockId, Function, Instruction, Terminator, ValueId};

/// 每个可收集边之前必须发布的 boxed SSA roots。
pub(crate) struct RootPlan {
    instruction_roots: HashMap<(BasicBlockId, usize), Vec<ValueId>>,
    terminator_roots: HashMap<BasicBlockId, Vec<ValueId>>,
}

impl RootPlan {
    pub(crate) fn build(function: &Function, f64_values: &HashSet<ValueId>) -> Self {
        let mut block_uses = HashMap::with_capacity(function.blocks().len());
        let mut block_defs = HashMap::with_capacity(function.blocks().len());
        let mut phi_defs = HashMap::with_capacity(function.blocks().len());
        let mut phi_edge_uses: HashMap<(BasicBlockId, BasicBlockId), HashSet<ValueId>> =
            HashMap::new();

        for block in function.blocks() {
            let mut uses = HashSet::new();
            let mut defs = HashSet::new();
            let mut block_phi_defs = HashSet::new();
            for instruction in block.instructions() {
                if let Instruction::Phi { dest, sources } = instruction {
                    defs.insert(*dest);
                    block_phi_defs.insert(*dest);
                    for source in sources {
                        phi_edge_uses
                            .entry((source.predecessor, block.id()))
                            .or_default()
                            .insert(source.value);
                    }
                    continue;
                }
                for value in instruction_uses(instruction) {
                    if !defs.contains(&value) {
                        uses.insert(value);
                    }
                }
                if let Some(destination) = instruction_destination(instruction) {
                    defs.insert(destination);
                }
            }
            for value in terminator_uses(block.terminator()) {
                if !defs.contains(&value) {
                    uses.insert(value);
                }
            }
            block_uses.insert(block.id(), uses);
            block_defs.insert(block.id(), defs);
            phi_defs.insert(block.id(), block_phi_defs);
        }

        let mut live_in: HashMap<BasicBlockId, HashSet<ValueId>> = function
            .blocks()
            .iter()
            .map(|block| (block.id(), HashSet::new()))
            .collect();
        let mut live_out = live_in.clone();
        loop {
            let mut changed = false;
            for block in function.blocks().iter().rev() {
                let mut outgoing = HashSet::new();
                for successor in terminator_successors(block.terminator()) {
                    if let Some(successor_live) = live_in.get(&successor) {
                        outgoing.extend(
                            successor_live
                                .iter()
                                .filter(|value| !phi_defs[&successor].contains(value))
                                .copied(),
                        );
                    }
                    if let Some(edge_uses) = phi_edge_uses.get(&(block.id(), successor)) {
                        outgoing.extend(edge_uses.iter().copied());
                    }
                }

                let mut incoming = block_uses[&block.id()].clone();
                incoming.extend(
                    outgoing
                        .iter()
                        .filter(|value| !block_defs[&block.id()].contains(value))
                        .copied(),
                );
                if live_out[&block.id()] != outgoing {
                    live_out.insert(block.id(), outgoing);
                    changed = true;
                }
                if live_in[&block.id()] != incoming {
                    live_in.insert(block.id(), incoming);
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }

        let mut instruction_roots = HashMap::new();
        let mut terminator_roots = HashMap::new();
        for block in function.blocks() {
            let mut live = live_out[&block.id()].clone();
            live.extend(terminator_uses(block.terminator()));
            terminator_roots.insert(block.id(), boxed_roots(&live, f64_values));

            for (index, instruction) in block.instructions().iter().enumerate().rev() {
                if let Some(destination) = instruction_destination(instruction) {
                    live.remove(&destination);
                }
                if !matches!(instruction, Instruction::Phi { .. }) {
                    live.extend(instruction_uses(instruction));
                }
                instruction_roots.insert((block.id(), index), boxed_roots(&live, f64_values));
            }
        }

        Self {
            instruction_roots,
            terminator_roots,
        }
    }

    pub(crate) fn before_instruction(&self, block: BasicBlockId, instruction: usize) -> &[ValueId] {
        self.instruction_roots
            .get(&(block, instruction))
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    pub(crate) fn before_terminator(&self, block: BasicBlockId) -> &[ValueId] {
        self.terminator_roots
            .get(&block)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    pub(crate) fn max_roots(&self) -> usize {
        self.instruction_roots
            .values()
            .chain(self.terminator_roots.values())
            .map(Vec::len)
            .max()
            .unwrap_or(0)
    }
}

fn boxed_roots(live: &HashSet<ValueId>, f64_values: &HashSet<ValueId>) -> Vec<ValueId> {
    let mut roots: Vec<_> = live.difference(f64_values).copied().collect();
    roots.sort_unstable_by_key(|value| value.0);
    roots
}

fn instruction_uses(instruction: &Instruction) -> HashSet<ValueId> {
    if matches!(instruction, Instruction::Phi { .. }) {
        return HashSet::new();
    }
    let destination = instruction_destination(instruction);
    let mut values = HashSet::new();
    let mut remapped = instruction.clone();
    remapped.remap_values(&mut |value| {
        values.insert(value);
        value
    });
    if let Some(destination) = destination {
        values.remove(&destination);
    }
    values
}

fn instruction_destination(instruction: &Instruction) -> Option<ValueId> {
    match instruction {
        Instruction::Const { dest, .. }
        | Instruction::Binary { dest, .. }
        | Instruction::Unary { dest, .. }
        | Instruction::Compare { dest, .. }
        | Instruction::Phi { dest, .. }
        | Instruction::StringConcatVa { dest, .. }
        | Instruction::LoadVar { dest, .. }
        | Instruction::NewObject { dest, .. }
        | Instruction::GetProp { dest, .. }
        | Instruction::SetProp { dest, .. }
        | Instruction::CreateDataProperty { dest, .. }
        | Instruction::DeleteProp { dest, .. }
        | Instruction::NewArray { dest, .. }
        | Instruction::CloneArrayTemplate { dest, .. }
        | Instruction::InitObjectLiteral { dest, .. }
        | Instruction::GetElem { dest, .. }
        | Instruction::SetElem { dest, .. }
        | Instruction::OptionalGetProp { dest, .. }
        | Instruction::OptionalGetElem { dest, .. }
        | Instruction::OptionalCall { dest, .. }
        | Instruction::GetSuperBase { dest }
        | Instruction::GetSuperConstructor { dest }
        | Instruction::NewPromise { dest }
        | Instruction::CollectRestArgs { dest, .. }
        | Instruction::IsException { dest, .. }
        | Instruction::GuardSameFunction { dest, .. }
        | Instruction::EncodeException { dest, .. }
        | Instruction::ExceptionToObject { dest, .. } => Some(*dest),
        Instruction::CallBuiltin { dest, .. }
        | Instruction::Call { dest, .. }
        | Instruction::SuperCall { dest, .. }
        | Instruction::ConstructCall { dest, .. } => *dest,
        Instruction::StoreVar { .. }
        | Instruction::SetProto { .. }
        | Instruction::ObjectSpread { .. }
        | Instruction::PromiseResolve { .. }
        | Instruction::PromiseReject { .. }
        | Instruction::Suspend { .. }
        | Instruction::GeneratorSuspend { .. }
        | Instruction::DebugCheck { .. } => None,
    }
}

fn terminator_uses(terminator: &Terminator) -> HashSet<ValueId> {
    let mut values = HashSet::new();
    let mut remapped = terminator.clone();
    remapped.remap_values(&mut |value| {
        values.insert(value);
        value
    });
    values
}

pub(crate) fn terminator_successors(terminator: &Terminator) -> Vec<BasicBlockId> {
    let mut successors = Vec::new();
    match terminator {
        Terminator::Return { .. } | Terminator::Throw { .. } | Terminator::Unreachable => {}
        Terminator::Jump { target } => successors.push(*target),
        Terminator::Branch {
            true_block,
            false_block,
            ..
        } => {
            successors.push(*true_block);
            if true_block != false_block {
                successors.push(*false_block);
            }
        }
        Terminator::Switch {
            cases,
            default_block,
            exit_block,
            ..
        } => {
            for case in cases {
                if !successors.contains(&case.target) {
                    successors.push(case.target);
                }
            }
            for target in [*default_block, *exit_block] {
                if !successors.contains(&target) {
                    successors.push(target);
                }
            }
        }
    }
    successors
}

#[cfg(test)]
mod tests {
    use super::*;
    use wjsm_ir::{BasicBlock, Builtin, Constant, Program};

    #[test]
    fn dead_values_are_absent_from_later_safepoints() {
        let mut program = Program::new();
        let first = program.add_constant(Constant::String("dead".into()));
        let second = program.add_constant(Constant::String("live".into()));
        let mut function = Function::new("main", BasicBlockId(0));
        let mut block = BasicBlock::new(BasicBlockId(0));
        block.push_instruction(Instruction::Const {
            dest: ValueId(0),
            constant: first,
        });
        block.push_instruction(Instruction::Const {
            dest: ValueId(1),
            constant: second,
        });
        block.push_instruction(Instruction::CallBuiltin {
            dest: None,
            builtin: Builtin::ConsoleLog,
            args: vec![ValueId(1)],
        });
        block.set_terminator(Terminator::Return {
            value: Some(ValueId(1)),
        });
        function.push_block(block);

        let plan = RootPlan::build(&function, &HashSet::new());
        assert_eq!(plan.before_instruction(BasicBlockId(0), 2), &[ValueId(1)]);
    }

    #[test]
    fn phi_sources_are_live_on_predecessor_edges() {
        let mut function = Function::new("main", BasicBlockId(0));
        let mut predecessor = BasicBlock::new(BasicBlockId(0));
        predecessor.set_terminator(Terminator::Jump {
            target: BasicBlockId(1),
        });
        let mut join = BasicBlock::new(BasicBlockId(1));
        join.push_instruction(Instruction::Phi {
            dest: ValueId(1),
            sources: vec![wjsm_ir::PhiSource {
                predecessor: BasicBlockId(0),
                value: ValueId(0),
            }],
        });
        join.set_terminator(Terminator::Return {
            value: Some(ValueId(1)),
        });
        function.push_block(predecessor);
        function.push_block(join);

        let plan = RootPlan::build(&function, &HashSet::new());
        assert_eq!(plan.before_terminator(BasicBlockId(0)), &[ValueId(0)]);
        assert_eq!(plan.before_terminator(BasicBlockId(1)), &[ValueId(1)]);
    }

    #[test]
    fn proven_f64_values_are_excluded() {
        let mut function = Function::new("main", BasicBlockId(0));
        let mut block = BasicBlock::new(BasicBlockId(0));
        block.push_instruction(Instruction::CallBuiltin {
            dest: None,
            builtin: Builtin::ConsoleLog,
            args: vec![ValueId(0), ValueId(1)],
        });
        block.set_terminator(Terminator::Return { value: None });
        function.push_block(block);
        let f64_values = HashSet::from([ValueId(0)]);

        let plan = RootPlan::build(&function, &f64_values);
        assert_eq!(plan.before_instruction(BasicBlockId(0), 0), &[ValueId(1)]);
    }
}
