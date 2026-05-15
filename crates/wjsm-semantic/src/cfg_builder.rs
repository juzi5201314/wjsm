use wjsm_ir::{BasicBlock, BasicBlockId, Function, Instruction, Terminator, ValueId};

#[derive(Debug, Clone)]
pub(crate) struct LabelContext {
    pub(crate) label: Option<String>,
    pub(crate) kind: LabelKind,
    pub(crate) break_target: BasicBlockId,
    pub(crate) continue_target: Option<BasicBlockId>,
    pub(crate) iterator_to_close: Option<ValueId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LabelKind {
    Loop,
    Switch,
    Block,
}

#[derive(Debug, Clone)]
pub(crate) struct FinallyContext {
    pub(crate) _finally_block: BasicBlockId,
    pub(crate) _after_finally_block: BasicBlockId,
}

#[derive(Debug, Clone)]
pub(crate) struct TryContext {
    pub(crate) catch_entry: Option<BasicBlockId>,
    pub(crate) exception_var: String,
    pub(crate) label_depth: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StmtFlow {
    Open(BasicBlockId),
    Terminated,
}

pub(crate) struct FunctionBuilder {
    pub(crate) _name: String,
    pub(crate) _entry: BasicBlockId,
    pub(crate) blocks: Vec<BasicBlock>,
    pub(crate) has_eval: bool,
}

impl FunctionBuilder {
    pub(crate) fn new(name: impl Into<String>, entry: BasicBlockId) -> Self {
        Self {
            _name: name.into(),
            _entry: entry,
            blocks: vec![BasicBlock::new(entry)],
            has_eval: false,
        }
    }

    pub(crate) fn mark_has_eval(&mut self) {
        self.has_eval = true;
    }

    pub(crate) fn has_eval(&self) -> bool {
        self.has_eval
    }

    pub(crate) fn new_block(&mut self) -> BasicBlockId {
        let id = BasicBlockId(self.blocks.len() as u32);
        self.blocks.push(BasicBlock::new(id));
        id
    }

    pub(crate) fn append_instruction(&mut self, block: BasicBlockId, instruction: Instruction) {
        if let Some(b) = self.block_mut(block) {
            b.push_instruction(instruction);
        }
    }

    pub(crate) fn set_terminator(&mut self, block: BasicBlockId, terminator: Terminator) {
        if let Some(b) = self.block_mut(block) {
            b.set_terminator(terminator);
        }
    }

    pub(crate) fn block_mut(&mut self, id: BasicBlockId) -> Option<&mut BasicBlock> {
        self.blocks.get_mut(id.0 as usize)
    }

    pub(crate) fn block(&self, id: BasicBlockId) -> Option<&BasicBlock> {
        self.blocks.get(id.0 as usize)
    }

    pub(crate) fn ensure_jump_or_terminated(&mut self, from: StmtFlow, target: BasicBlockId) -> StmtFlow {
        match from {
            StmtFlow::Terminated => StmtFlow::Terminated,
            StmtFlow::Open(block) => {
                let is_unreachable = self
                    .block(block)
                    .map_or(false, |b| matches!(b.terminator(), Terminator::Unreachable));
                if is_unreachable {
                    self.set_terminator(block, Terminator::Jump { target });
                }
                StmtFlow::Open(target)
            }
        }
    }

    #[allow(dead_code)]
    pub(crate) fn finish(self) -> Function {
        let entry = self._entry;
        let mut function = Function::new(self._name, entry);
        function.set_has_eval(self.has_eval);
        function
    }

    pub(crate) fn into_blocks(mut self) -> Vec<BasicBlock> {
        std::mem::take(&mut self.blocks)
    }
}
