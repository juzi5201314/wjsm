use wjsm_ir::{BasicBlock, BasicBlockId, Function, Instruction, Terminator, ValueId};
use swc_core::ecma::ast as swc_ast;

// ── CFG Builder ─────────────────────────────────────────────────────────

/// Internal helper that encapsulates CFG construction for one function.
pub(crate) struct FunctionBuilder {
    _name: String,
    _entry: BasicBlockId,
    pub(crate) blocks: Vec<BasicBlock>,
    has_eval: bool,
    /// 该函数调用的"已知函数声明"变量名→FunctionId（Layer 3 callee 分析）。
    /// store_function_decl_callee 填充，finalize 时转移到 IR Function。
    known_callee_vars: std::collections::HashMap<String, wjsm_ir::FunctionId>,
}

impl FunctionBuilder {
    pub(crate) fn new(name: impl Into<String>, entry: BasicBlockId) -> Self {
        Self {
            _name: name.into(),
            _entry: entry,
            blocks: vec![BasicBlock::new(entry)],
            has_eval: false,
            known_callee_vars: std::collections::HashMap::new(),
        }
    }

    pub(crate) fn mark_has_eval(&mut self) {
        self.has_eval = true;
    }

    pub(crate) fn has_eval(&self) -> bool {
        self.has_eval
    }

    /// 记录 callee 变量（scope-qualified IR name）→ FunctionId（Layer 3）。
    pub(crate) fn record_known_callee(
        &mut self,
        ir_name: String,
        function_id: wjsm_ir::FunctionId,
    ) {
        self.known_callee_vars.insert(ir_name, function_id);
    }

    pub(crate) fn take_known_callee_vars(
        &mut self,
    ) -> std::collections::HashMap<String, wjsm_ir::FunctionId> {
        std::mem::take(&mut self.known_callee_vars)
    }

    pub(crate) fn name(&self) -> &str {
        &self._name
    }

    pub(crate) fn new_block(&mut self) -> BasicBlockId {
        let id = BasicBlockId(self.blocks.len() as u32);
        self.blocks.push(BasicBlock::new(id));
        id
    }
    pub(crate) fn last_block_id(&self) -> BasicBlockId {
        BasicBlockId(self.blocks.len().saturating_sub(1) as u32)
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

    /// O(1) 通过 id 获取 block 可变引用。
    ///
    /// # 性能优化
    /// 由于 block id 等于其在 blocks 向量中的索引（由 new_block 保证），
    /// 使用直接索引访问而非 iter_mut().find()，将 O(n) 降为 O(1)。
    pub(crate) fn block_mut(&mut self, id: BasicBlockId) -> Option<&mut BasicBlock> {
        self.blocks.get_mut(id.0 as usize)
    }

    /// O(1) 通过 id 获取 block 引用。
    ///
    /// # 性能优化
    /// 由于 block id 等于其在 blocks 向量中的索引（由 new_block 保证），
    /// 使用直接索引访问而非 iter().find()，将 O(n) 降为 O(1)。
    pub(crate) fn block(&self, id: BasicBlockId) -> Option<&BasicBlock> {
        self.blocks.get(id.0 as usize)
    }

    /// 以只读切片暴露当前函数的 blocks，用于函数级分析阶段。
    pub(crate) fn blocks(&self) -> &[BasicBlock] {
        &self.blocks
    }

    /// Ensure control flow from `from` reaches `target`.
    ///
    /// - If `from` is `Terminated`: no-op, returns `Terminated`.
    /// - If `from` is `Open(block)` and block has Unreachable terminator: set Jump { target }.
    /// - Returns `StmtFlow::Open(target)` so caller can continue writing to target.
    pub(crate) fn ensure_jump_or_terminated(
        &mut self,
        from: StmtFlow,
        target: BasicBlockId,
    ) -> StmtFlow {
        match from {
            StmtFlow::Terminated => StmtFlow::Terminated,
            StmtFlow::Open(block) => {
                let is_unreachable = self
                    .block(block)
                    .is_some_and(|b| matches!(b.terminator(), Terminator::Unreachable));
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

    /// 与 into_blocks 相同但接受 &mut self，用于不能消费 Lowerer 的场景。
    pub(crate) fn take_blocks(&mut self) -> Vec<BasicBlock> {
        std::mem::take(&mut self.blocks)
    }
}

// ── Label & Finally tracking ────────────────────────────────────────────

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
    pub(crate) finalizer_index: Option<usize>,
}

/// 当前在作用域内、尚未运行的 try-finally 的 finally 块。
/// `label_depth` 为进入该 try 时的 label_stack 长度，用于 abrupt completion
/// 展开时按嵌套深度与 for-of 迭代器关闭交错排序。
#[derive(Debug, Clone)]
pub(crate) struct PendingFinalizer {
    pub(crate) block: swc_ast::BlockStmt,
    pub(crate) label_depth: usize,
}

/// The flow state after lowering a statement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StmtFlow {
    /// Control flow continues in the given basic block.
    Open(BasicBlockId),
    /// The statement terminated control flow (return, throw, break, continue, unreachable).
    Terminated,
}
