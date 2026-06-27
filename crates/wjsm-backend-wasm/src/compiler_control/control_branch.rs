use super::*;

impl Compiler {
    /// 返回 `Ok(true)` 表示该分支以终止指令（Return/Unreachable/br）结束，
    /// 调用者可据此判断是否需要发射 `Unreachable` 以避免 WASM 验证错误；
    /// 返回 `Ok(false)` 表示分支正常落入（fall through）后续代码。
    pub(crate) fn compile_branch_body(
        &mut self,
        module: &IrModule,
        blocks: &[BasicBlock],
        idx: usize,
    ) -> Result<bool> {
        self.compile_branch_body_with_context(module, blocks, idx, 0, 0)
    }

    pub(super) fn compile_branch_body_in_case(
        &mut self,
        module: &IrModule,
        blocks: &[BasicBlock],
        idx: usize,
        case_start: usize,
        extra_depth: u32,
    ) -> Result<bool> {
        self.compile_branch_body_with_context(module, blocks, idx, case_start, extra_depth)
    }

    pub(super) fn compile_branch_body_with_context(
        &mut self,
        module: &IrModule,
        blocks: &[BasicBlock],
        idx: usize,
        case_start: usize,
        extra_depth: u32,
    ) -> Result<bool> {
        if idx >= blocks.len() {
            return Ok(false);
        }
        let block = &blocks[idx];

        let mut suspended = false;
        for (instr_idx, instruction) in block.instructions().iter().enumerate() {
            self.set_emit_cursor(idx, instr_idx);
            if self.compile_instruction(module, instruction)? {
                suspended = true;
                break;
            }
        }

        if suspended {
            // Suspend 终止当前执行路径（函数返回给运行时），
            // 视为已终止，避免调用者错误计算 merge block。
            return Ok(true);
        }

        match block.terminator() {
            Terminator::Return { value } => {
                self.emit_return(value);
                Ok(true)
            }
            Terminator::Jump { target } => {
                let target_idx = target.0 as usize;
                if let Some(depth) = self.loop_continue_depth(target_idx) {
                    // back-edge：continue 循环
                    self.emit_phi_moves(blocks, idx, target_idx);
                    let adj = if extra_depth > 0 && target_idx < case_start {
                        depth + extra_depth
                    } else {
                        depth
                    };
                    self.emit(WasmInstruction::Br(adj));
                    Ok(true)
                } else if let Some(depth) = self.loop_break_depth(target_idx) {
                    // 跳到循环出口：break
                    self.emit_phi_moves(blocks, idx, target_idx);
                    let adj = if extra_depth > 0 && target_idx < case_start {
                        depth + extra_depth
                    } else {
                        depth
                    };
                    self.emit(WasmInstruction::Br(adj));
                    Ok(true)
                } else if target_idx < idx && block_has_suspend(&blocks[target_idx]) {
                    // async 状态机的循环头可能位于另一个 switch case 中，不能用当前 case 的 label 回跳；
                    // 这里内联到下一个 suspend，让循环体能够调度下一轮 resume。
                    self.emit_phi_moves(blocks, idx, target_idx);
                    self.compile_branch_body_with_context(
                        module,
                        blocks,
                        target_idx,
                        case_start,
                        extra_depth,
                    )
                } else if target_idx < idx
                    && matches!(blocks[target_idx].terminator(), Terminator::Jump { .. })
                    && self.loop_stack.iter().rev().any(|loop_info| {
                        target_idx > loop_info.header_idx
                            && self.can_reach_loop_header(blocks, target_idx, loop_info.header_idx)
                    })
                {
                    // 分支内跳回 loop update 块：内联发射目标块的指令，然后 br 到循环头。
                    // 不能递归调用 compile_branch_body_with_context，因为当前 if_depth
                    // 会导致 update 块的 continue br 深度偏移错误。
                    self.emit_phi_moves(blocks, idx, target_idx);
                    let target_block = &blocks[target_idx];
                    for (instr_idx, instruction) in target_block.instructions().iter().enumerate() {
                        self.set_emit_cursor(target_idx, instr_idx);
                        self.compile_instruction(module, instruction)?;
                    }
                    if let Some(depth) = self.loop_continue_depth(match target_block.terminator() {
                        Terminator::Jump { target } => target.0 as usize,
                        _ => target_idx,
                    }) {
                        let adj = if extra_depth > 0 && target_idx < case_start {
                            depth + extra_depth
                        } else {
                            depth
                        };
                        self.emit(WasmInstruction::Br(adj));
                        Ok(true)
                    } else {
                        Ok(true)
                    }
                } else if self.if_depth > 0
                    && target_idx < idx
                    && matches!(
                        blocks.get(target_idx).map(|b| b.terminator()),
                        Some(
                            Terminator::Return { .. }
                                | Terminator::Throw { .. }
                                | Terminator::Unreachable
                        )
                    )
                {
                    self.emit_phi_moves(blocks, idx, target_idx);
                    self.compile_branch_body_with_context(
                        module,
                        blocks,
                        target_idx,
                        case_start,
                        extra_depth,
                    )
                } else if self.if_depth > 0
                    && target_idx < idx
                    && !self.compiled_blocks.contains(&target_idx)
                    && count_predecessors(blocks, target_idx) <= 1
                {
                    self.emit_phi_moves(blocks, idx, target_idx);
                    self.compiled_blocks.insert(target_idx);
                    self.compile_branch_body_with_context(
                        module,
                        blocks,
                        target_idx,
                        case_start,
                        extra_depth,
                    )
                } else if self.if_depth > 0 && target_idx > idx {
                    self.emit_phi_moves(blocks, idx, target_idx);
                    let should_follow = !self.compiled_blocks.contains(&target_idx)
                        && count_predecessors(blocks, target_idx) <= 1;
                    if should_follow {
                        self.compiled_blocks.insert(target_idx);
                        return self.compile_branch_body_with_context(
                            module,
                            blocks,
                            target_idx,
                            case_start,
                            extra_depth,
                        );
                    }
                    Ok(false)
                } else {
                    self.emit_phi_moves(blocks, idx, target_idx);
                    Ok(false)
                }
            }
            Terminator::Throw { value } => {
                self.emit(WasmInstruction::LocalGet(self.local_idx(value.0)));
                let func_idx = self
                    .builtin_func_indices
                    .get(&Builtin::CreateException)
                    .copied()
                    .expect("CreateException import must be registered");
                self.emit(WasmInstruction::Call(func_idx));
                self.emit(WasmInstruction::Return);
                Ok(true)
            }
            Terminator::Unreachable => Ok(true),
            Terminator::Switch {
                value,
                cases,
                default_block,
                exit_block,
            } => {
                self.compiled_blocks.insert(idx);
                let exit_idx = exit_block.0 as usize;
                let default_target_idx = default_block.0 as usize;

                struct SwitchEntry {
                    is_default: bool,
                    constant_idx: Option<u32>,
                    target_idx: usize,
                }

                let mut entries: Vec<SwitchEntry> = Vec::new();
                for case in cases.iter() {
                    entries.push(SwitchEntry {
                        is_default: false,
                        constant_idx: Some(case.constant.0),
                        target_idx: case.target.0 as usize,
                    });
                }
                entries.push(SwitchEntry {
                    is_default: true,
                    constant_idx: None,
                    target_idx: default_target_idx,
                });

                entries.sort_by_key(|entry| entry.target_idx);

                let num_entries = entries.len();
                let default_pos = entries.iter().position(|entry| entry.is_default).unwrap();
                let loops = detect_loops(blocks);

                self.compiled_blocks.insert(default_target_idx);

                self.emit(WasmInstruction::Block(BlockType::Empty));
                for _ in 0..num_entries {
                    self.emit(WasmInstruction::Block(BlockType::Empty));
                }

                for (i, entry) in entries.iter().enumerate() {
                    if entry.is_default {
                        continue;
                    }
                    self.emit(WasmInstruction::LocalGet(self.local_idx(value.0)));
                    let const_val = self.encode_constant(
                        &module.constants()[entry.constant_idx.unwrap() as usize],
                        module,
                    )?;
                    self.emit(WasmInstruction::I64Const(const_val));
                    self.emit(WasmInstruction::I64Eq);
                    self.emit(WasmInstruction::BrIf(i as u32));
                }
                self.emit(WasmInstruction::Br(default_pos as u32));

                for (i, entry) in entries.iter().enumerate() {
                    if i == default_pos {
                        self.compiled_blocks.remove(&default_target_idx);
                    }
                    self.emit(WasmInstruction::End);
                    let switch_break_depth = (num_entries - i - 1) as u32;
                    let switch_extra_depth = extra_depth + (num_entries - i) as u32;
                    self.compile_switch_case(
                        module,
                        blocks,
                        entry.target_idx,
                        exit_idx,
                        switch_break_depth,
                        switch_extra_depth,
                        &loops,
                    )?;
                }

                self.emit(WasmInstruction::End);
                if self.compiled_blocks.contains(&exit_idx) {
                    Ok(true)
                } else {
                    self.compile_branch_body_with_context(
                        module,
                        blocks,
                        exit_idx,
                        case_start,
                        extra_depth,
                    )
                }
            }
            Terminator::Branch {
                condition,
                true_block,
                false_block,
            } => {
                // 分支体内嵌套的 if/else
                let true_idx = true_block.0 as usize;
                let false_idx = false_block.0 as usize;

                let common_direct_jump = match (blocks.get(true_idx), blocks.get(false_idx)) {
                    (Some(true_block), Some(false_block)) => {
                        match (true_block.terminator(), false_block.terminator()) {
                            (
                                Terminator::Jump {
                                    target: true_target,
                                },
                                Terminator::Jump {
                                    target: false_target,
                                },
                            ) if true_target == false_target => Some(true_target.0 as usize),
                            _ => None,
                        }
                    }
                    _ => None,
                };
                if let Some(common_idx) = common_direct_jump
                    && self.loop_continue_depth(common_idx).is_none()
                    && self.loop_break_depth(common_idx).is_none()
                    && !block_has_suspend(&blocks[true_idx])
                    && !block_has_suspend(&blocks[false_idx])
                {
                    self.emit_to_bool_i32(condition.0);
                    self.if_depth += 1;
                    self.emit(WasmInstruction::If(BlockType::Empty));

                    self.compiled_blocks.insert(true_idx);
                    for (instr_idx, instruction) in
                        blocks[true_idx].instructions().iter().enumerate()
                    {
                        self.set_emit_cursor(true_idx, instr_idx);
                        self.compile_instruction(module, instruction)?;
                    }
                    self.emit_phi_moves(blocks, true_idx, common_idx);

                    self.emit(WasmInstruction::Else);
                    self.compiled_blocks.insert(false_idx);
                    for (instr_idx, instruction) in
                        blocks[false_idx].instructions().iter().enumerate()
                    {
                        self.set_emit_cursor(false_idx, instr_idx);
                        self.compile_instruction(module, instruction)?;
                    }
                    self.emit_phi_moves(blocks, false_idx, common_idx);

                    self.emit(WasmInstruction::End);
                    self.if_depth -= 1;
                    // 若 common 目标已被之前的 common_direct_jump 编译，不再重编译
                    if self.branch_inline_compiled.contains(&common_idx) {
                        return Ok(false);
                    }
                    self.branch_inline_compiled.insert(common_idx);
                    return self.compile_branch_body_with_context(
                        module,
                        blocks,
                        common_idx,
                        case_start,
                        extra_depth,
                    );
                }

                self.emit_to_bool_i32(condition.0);
                self.if_depth += 1;
                self.emit(WasmInstruction::If(BlockType::Empty));

                let true_is_merge = self.is_merge_block(blocks, false_idx, true_idx);
                let false_is_merge = self.is_merge_block(blocks, true_idx, false_idx);

                let true_terminates = if true_is_merge {
                    self.emit_phi_moves(blocks, idx, true_idx);
                    self.emit(WasmInstruction::Nop);
                    false
                } else {
                    self.compiled_blocks.insert(true_idx);
                    self.compile_branch_body_with_context(
                        module,
                        blocks,
                        true_idx,
                        case_start,
                        extra_depth,
                    )?
                };

                self.emit(WasmInstruction::Else);
                let false_terminates = if false_is_merge {
                    self.emit_phi_moves(blocks, idx, false_idx);
                    self.emit(WasmInstruction::Nop);
                    false
                } else {
                    self.compiled_blocks.insert(false_idx);
                    self.compile_branch_body_with_context(
                        module,
                        blocks,
                        false_idx,
                        case_start,
                        extra_depth,
                    )?
                };

                self.emit(WasmInstruction::End);
                self.if_depth -= 1;

                if true_terminates && false_terminates {
                    Ok(true)
                } else {
                    // 处理内层 merge block（如嵌套三元的中间 Phi）
                    // compile_structured 的 Branch 处理器有此逻辑，
                    // 但 compile_branch_body 缺少，导致内层 Phi 的 phi_local 从未被赋值。
                    let merge = if false_is_merge {
                        false_idx
                    } else if true_is_merge {
                        true_idx
                    } else if true_terminates && !false_terminates {
                        self.branch_continuation_target(blocks, false_idx)
                            .unwrap_or_else(|| self.find_merge(blocks, true_idx, false_idx))
                    } else if false_terminates && !true_terminates {
                        self.branch_continuation_target(blocks, true_idx)
                            .unwrap_or_else(|| self.find_merge(blocks, true_idx, false_idx))
                    } else {
                        self.find_merge(blocks, true_idx, false_idx)
                    };

                    if self.compiled_blocks.contains(&merge) {
                        // merge 已被某分支体编译，为 fall-through 路径重发射其 Phi
                        if let Some(merge_block) = blocks.get(merge) {
                            for instruction in merge_block.instructions() {
                                if let Instruction::Phi { dest, .. } = instruction
                                    && let Some(&phi_local) = self.phi_locals.get(&dest.0)
                                {
                                    self.emit(WasmInstruction::LocalGet(phi_local));
                                    self.emit(WasmInstruction::LocalSet(self.local_idx(dest.0)));
                                }
                            }
                        }
                        Ok(false)
                    } else {
                        // 仅对非 loop 的简单 Jump 做内联 merge 编译
                        // 条件：merge 必须为 Jump 终止，且目标非 loop 头/exit
                        let is_simple_merge = blocks.get(merge).is_some_and(|b| {
                            if !b.instructions().is_empty() {
                                return false;
                            }
                            if count_predecessors(blocks, merge) > 1 {
                                return false;
                            }
                            if let Terminator::Jump { target } = b.terminator() {
                                let t = target.0 as usize;
                                !self.loop_continue_depth(t).is_some()
                                    && !self.loop_break_depth(t).is_some()
                            } else {
                                false
                            }
                        });
                        if is_simple_merge {
                            self.compiled_blocks.insert(merge);
                            return self.compile_branch_body_with_context(
                                module,
                                blocks,
                                merge,
                                case_start,
                                extra_depth,
                            );
                        }
                        Ok(false)
                    } // closes inner else
                } // closes outer else
            } // closes Branch arm
        }
    }

    /// Emit Phi moves: for each Phi instruction in the target block that references
    /// the current predecessor block, emit a move from the source value to the Phi local.
    pub(crate) fn emit_phi_moves(
        &mut self,
        blocks: &[BasicBlock],
        pred_idx: usize,
        target_idx: usize,
    ) {
        if target_idx >= blocks.len() {
            return;
        }
        let target_block = &blocks[target_idx];
        for instruction in target_block.instructions() {
            if let Instruction::Phi { dest, sources } = instruction {
                for source in sources {
                    if source.predecessor.0 as usize == pred_idx
                        && let Some(&phi_local) = self.phi_locals.get(&dest.0)
                    {
                        self.emit(WasmInstruction::LocalGet(self.local_idx(source.value.0)));
                        self.emit(WasmInstruction::LocalSet(phi_local));
                    }
                }
            }
        }
    }

    /// Check if `false_idx` is the natural merge block for a branch where
    /// true path is at `true_idx` and false path is at `false_idx`.
    pub(crate) fn is_merge_block(
        &self,
        blocks: &[BasicBlock],
        true_idx: usize,
        false_idx: usize,
    ) -> bool {
        if let Some(true_block) = blocks.get(true_idx) {
            match true_block.terminator() {
                Terminator::Jump { target } if target.0 as usize == false_idx => return true,
                _ => {}
            }
        }
        if let Some(false_block) = blocks.get(false_idx) {
            for instruction in false_block.instructions() {
                if let Instruction::Phi { sources, .. } = instruction
                    && sources.len() > 1
                    && sources.iter().any(|s| s.predecessor.0 as usize == true_idx)
                {
                    return true;
                }
            }
        }
        false
    }

    /// Find the merge block where true and false paths converge.
    pub(crate) fn find_merge(
        &self,
        blocks: &[BasicBlock],
        true_idx: usize,
        false_idx: usize,
    ) -> usize {
        let true_continuation = self.branch_continuation_target(blocks, true_idx);
        let false_continuation = self.branch_continuation_target(blocks, false_idx);

        match (true_continuation, false_continuation) {
            (Some(left), Some(right)) if left == right => return left,
            (Some(target), None) | (None, Some(target)) => return target,
            _ => {}
        }

        let direct_uncompiled_backward_jump = |branch_idx: usize| -> Option<usize> {
            let block = blocks.get(branch_idx)?;
            let Terminator::Jump { target } = block.terminator() else {
                return None;
            };
            let target_idx = target.0 as usize;
            (target_idx < branch_idx
                && !self.compiled_blocks.contains(&target_idx)
                && self.loop_continue_depth(target_idx).is_none()
                && self.loop_break_depth(target_idx).is_none())
            .then_some(target_idx)
        };

        if let Some(target) = direct_uncompiled_backward_jump(true_idx) {
            return target;
        }
        if let Some(target) = direct_uncompiled_backward_jump(false_idx) {
            return target;
        }

        // 单分支 Jump 到 merge 时，优先用另一分支的 continuation（try/catch 汇合）
        if let Some(true_block) = blocks.get(true_idx)
            && let Terminator::Jump { target } = true_block.terminator()
        {
            if let Some(cont) = false_continuation {
                return cont;
            }
            return target.0 as usize;
        }
        if let Some(false_block) = blocks.get(false_idx)
            && let Terminator::Jump { target } = false_block.terminator()
        {
            if let Some(cont) = true_continuation {
                return cont;
            }
            return target.0 as usize;
        }
        // Default: the block after the false block
        false_idx + 1
    }

    pub(super) fn branch_continuation_target(&self, blocks: &[BasicBlock], start_idx: usize) -> Option<usize> {
        fn walk(
            blocks: &[BasicBlock],
            idx: usize,
            visited: &mut std::collections::HashSet<usize>,
        ) -> Option<usize> {
            if !visited.insert(idx) {
                return Some(idx);
            }

            let block = blocks.get(idx)?;
            match block.terminator() {
                Terminator::Jump { target } => Some(target.0 as usize),
                Terminator::Branch {
                    true_block,
                    false_block,
                    ..
                } => {
                    let true_target = walk(blocks, true_block.0 as usize, visited);
                    let false_target = walk(blocks, false_block.0 as usize, visited);
                    match (true_target, false_target) {
                        (Some(left), Some(right)) if left == right => Some(left),
                        (Some(target), None) | (None, Some(target)) => Some(target),
                        _ => None,
                    }
                }
                Terminator::Switch { exit_block, .. } => Some(exit_block.0 as usize),
                Terminator::Return { .. } | Terminator::Throw { .. } | Terminator::Unreachable => {
                    None
                }
            }
        }

        walk(blocks, start_idx, &mut std::collections::HashSet::new())
    }

    pub(super) fn next_after_compiled_merge(
        &self,
        blocks: &[BasicBlock],
        merge: usize,
        true_idx: usize,
        false_idx: usize,
    ) -> usize {
        if let Some(continuation) = self.branch_continuation_target(blocks, merge) {
            let target = self
                .loop_exit_for_header(continuation)
                .unwrap_or(continuation);
            if target < blocks.len() && !self.compiled_blocks.contains(&target) {
                return target;
            }
        }

        let mut next = true_idx.max(false_idx) + 1;
        while next < blocks.len() && self.compiled_blocks.contains(&next) {
            next += 1;
        }
        next
    }

    /// 检查 block_idx 是否能通过 CFG 到达 header_idx（判断是否在循环体内）
    pub(super) fn can_reach_loop_header(
        &self,
        blocks: &[BasicBlock],
        block_idx: usize,
        header_idx: usize,
    ) -> bool {
        let mut visited = std::collections::HashSet::new();
        let mut stack = vec![block_idx];
        while let Some(current) = stack.pop() {
            if current == header_idx {
                return true;
            }
            if !visited.insert(current) {
                continue;
            }
            if let Some(block) = blocks.get(current) {
                match block.terminator() {
                    Terminator::Jump { target } => stack.push(target.0 as usize),
                    Terminator::Branch {
                        true_block,
                        false_block,
                        ..
                    } => {
                        stack.push(true_block.0 as usize);
                        stack.push(false_block.0 as usize);
                    }
                    _ => {}
                }
            }
        }
        false
    }

    pub(crate) fn emit_return(&mut self, value: &Option<ValueId>) {
        if let Some(v) = value {
            self.emit(WasmInstruction::LocalGet(self.local_idx(v.0)));
        } else if self.current_func_returns_value {
            self.emit(WasmInstruction::I64Const(value::encode_undefined()));
        }
        self.emit_eval_var_frame_exit();
        self.emit(WasmInstruction::Return);
    }

    pub(crate) fn loop_continue_depth(&self, target_idx: usize) -> Option<u32> {
        let len = self.loop_stack.len();
        for (i, l) in self.loop_stack.iter().enumerate().rev() {
            if l.header_idx == target_idx {
                return Some(2 * (len - 1 - i) as u32 + self.if_depth);
            }
        }
        None
    }

    pub(crate) fn loop_break_depth(&self, target_idx: usize) -> Option<u32> {
        let len = self.loop_stack.len();
        for (i, l) in self.loop_stack.iter().enumerate().rev() {
            if l.exit_idx == target_idx {
                return Some(2 * (len - 1 - i) as u32 + 1 + self.if_depth);
            }
        }
        None
    }

    pub(super) fn loop_exit_for_header(&self, header_idx: usize) -> Option<usize> {
        self.loop_stack
            .iter()
            .rev()
            .find(|loop_info| loop_info.header_idx == header_idx)
            .map(|loop_info| loop_info.exit_idx)
    }

    // ── Instruction compilation ─────────────────────────────────────────────
}
