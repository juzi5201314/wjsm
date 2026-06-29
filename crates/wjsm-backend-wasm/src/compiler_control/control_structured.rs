use super::*;

impl Compiler {
    pub(crate) fn compile_structured(
        &mut self,
        module: &IrModule,
        function: &IrFunction,
        start_idx: usize,
    ) -> Result<()> {
        let blocks = function.blocks();
        let loops = detect_loops(blocks);
        self.loop_stack.clear();
        self.if_depth = 0;
        let mut idx = start_idx;

        while idx < blocks.len() {
            while let Some(top) = self.loop_stack.last() {
                // 循环出口检测：idx 必须在循环外（无法通过 CFG 到达循环头）
                let is_outside_loop = if idx >= top.exit_idx {
                    // idx >= exit_idx，但需确认是否真的在循环外
                    // 如果 idx 可达 loop header，则仍在循环体内
                    !self.can_reach_loop_header(blocks, idx, top.header_idx)
                } else {
                    false
                };
                if is_outside_loop {
                    self.emit(WasmInstruction::End);
                    self.emit(WasmInstruction::End);
                    self.loop_stack.pop();
                } else {
                    break;
                }
            }

            if self.compiled_blocks.contains(&idx) {
                break;
            }

            if let Some(loop_info) = loops.iter().find(|l| l.header_idx == idx) {
                self.emit(WasmInstruction::Block(BlockType::Empty));
                self.emit(WasmInstruction::Loop(BlockType::Empty));
                self.loop_stack.push(loop_info.clone());
            }

            self.compiled_blocks.insert(idx);

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
                // Async 状态机：suspend 后的 resume 块只应由 Switch case 编译，禁止线性续编。
                idx = blocks.len();
                continue;
            }

            match block.terminator() {
                Terminator::Return { value } => {
                    self.emit_return(value);
                    idx += 1;
                }
                Terminator::Unreachable => {
                    // 死代码块 — 跳过
                    idx += 1;
                }
                Terminator::Jump { target } => {
                    let target_idx = target.0 as usize;
                    if let Some(depth) = self.loop_continue_depth(target_idx) {
                        // back-edge：continue 循环
                        self.emit_phi_moves(blocks, idx, target_idx);
                        self.emit(WasmInstruction::Br(depth));
                        idx += 1;
                    } else if let Some(depth) = self.loop_break_depth(target_idx) {
                        // 跳到循环出口：break
                        self.emit_phi_moves(blocks, idx, target_idx);
                        self.emit(WasmInstruction::Br(depth));
                        idx += 1;
                    } else if target_idx == idx + 1 {
                        // 自然 fall-through
                        idx = target_idx;
                    } else if target_idx > idx {
                        // 前向跳转到非相邻 block（如 try/catch 跳过 finally_entry）
                        // 中间的 block 是不可达的，直接跳到目标
                        self.emit_phi_moves(blocks, idx, target_idx);
                        idx = target_idx;
                    } else {
                        // 后向跳转但不是循环 — 不应发生
                        self.emit_phi_moves(blocks, idx, target_idx);
                        idx = target_idx;
                    }
                }
                Terminator::Branch {
                    condition,
                    true_block,
                    false_block,
                } => {
                    let true_idx = true_block.0 as usize;
                    let false_idx = false_block.0 as usize;

                    // 循环头条件（while/for 模式）：
                    // true → body, false → exit
                    // 发射：condition → i32.eqz → br_if (break if falsy)
                    if self.loop_stack.last().is_some_and(|l| {
                        l.header_idx == idx
                            && matches!(
                                block.terminator(),
                                Terminator::Branch { false_block, .. }
                                    if false_block.0 as usize == l.exit_idx
                            )
                    }) {
                        self.emit_to_bool_i32(condition.0);
                        self.emit(WasmInstruction::I32Eqz);
                        let break_depth = self.loop_break_depth(false_idx).unwrap_or(1);
                        self.emit(WasmInstruction::BrIf(break_depth));
                        idx = true_idx;
                        continue;
                    }

                    // do-while 条件（true 目标是循环头）：
                    // true → header (continue), false → exit
                    // 发射：condition → br_if (continue if truthy)
                    if let Some(depth) = self.loop_continue_depth(true_idx) {
                        self.emit_to_bool_i32(condition.0);
                        self.emit(WasmInstruction::BrIf(depth));
                        idx = false_idx;
                        continue;
                    }
                    // 自循环检测：某个分支通过 Jump 链最终跳回当前块
                    // （如 catch 块内 IsException → throw 跳回 catch 入口）
                    let true_self_loop = chain_jumps_to(blocks, true_idx, idx);
                    let false_self_loop = chain_jumps_to(blocks, false_idx, idx);

                    if true_self_loop || false_self_loop {
                        // 生成: block { loop { if (cond) { true_body; br 1 } else { false_body; br 2 } } }
                        self.emit(WasmInstruction::Block(BlockType::Empty));
                        self.emit(WasmInstruction::Loop(BlockType::Empty));

                        self.emit_to_bool_i32(condition.0);
                        self.emit(WasmInstruction::If(BlockType::Empty));

                        // true 分支
                        self.compiled_blocks.insert(true_idx);
                        self.compile_branch_body(module, blocks, true_idx)?;
                        self.emit(WasmInstruction::Br(1)); // continue loop

                        self.emit(WasmInstruction::Else);
                        // false 分支
                        self.compiled_blocks.insert(false_idx);
                        self.compile_branch_body(module, blocks, false_idx)?;
                        self.emit(WasmInstruction::Br(2)); // break block

                        self.emit(WasmInstruction::End); // end if
                        self.emit(WasmInstruction::End); // end loop
                        self.emit(WasmInstruction::End); // end block

                        // 找到出口 merge（不跳回自身的分支沿 Jump 链的最终目标）
                        let exit_target = if true_self_loop {
                            resolve_jump_chain(blocks, false_idx)
                        } else {
                            resolve_jump_chain(blocks, true_idx)
                        };
                        idx = exit_target;
                        continue;
                    }

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

                        let true_jumps_direct = blocks.get(true_idx).is_some_and(|b| {
                            matches!(
                                b.terminator(),
                                Terminator::Jump { target } if target.0 as usize == common_idx
                            )
                        });
                        let false_jumps_direct = blocks.get(false_idx).is_some_and(|b| {
                            matches!(
                                b.terminator(),
                                Terminator::Jump { target } if target.0 as usize == common_idx
                            )
                        });

                        self.compiled_blocks.insert(true_idx);
                        if true_jumps_direct {
                            for (instr_idx, instruction) in
                                blocks[true_idx].instructions().iter().enumerate()
                            {
                                self.set_emit_cursor(true_idx, instr_idx);
                                self.compile_instruction(module, instruction)?;
                            }
                            self.emit_phi_moves(blocks, true_idx, common_idx);
                        } else {
                            self.compile_branch_body(module, blocks, true_idx)?;
                        }

                        self.emit(WasmInstruction::Else);
                        self.compiled_blocks.insert(false_idx);
                        if false_jumps_direct {
                            for (instr_idx, instruction) in
                                blocks[false_idx].instructions().iter().enumerate()
                            {
                                self.set_emit_cursor(false_idx, instr_idx);
                                self.compile_instruction(module, instruction)?;
                            }
                            self.emit_phi_moves(blocks, false_idx, common_idx);
                        } else {
                            self.compile_branch_body(module, blocks, false_idx)?;
                        }

                        self.emit(WasmInstruction::End);
                        self.if_depth -= 1;
                        // common_idx 可能已被 compile_branch_body 递归编译。
                        // 若已编译，跳到下一个未编译块，避免重复编译。
                        if self.compiled_blocks.contains(&common_idx) {
                            let mut next = common_idx + 1;
                            while next < blocks.len() && self.compiled_blocks.contains(&next) {
                                next += 1;
                            }
                            idx = next;
                        } else {
                            idx = common_idx;
                        }
                        continue;
                    }

                    // 普通 if/else
                    self.emit_to_bool_i32(condition.0);
                    self.if_depth += 1;
                    self.emit(WasmInstruction::If(BlockType::Empty));

                    let true_is_merge = self.is_merge_block(blocks, false_idx, true_idx);
                    let false_is_merge = self.is_merge_block(blocks, true_idx, false_idx);

                    let true_terminates = if true_is_merge {
                        self.emit_phi_moves(blocks, idx, true_idx);
                        false
                    } else {
                        self.compiled_blocks.insert(true_idx);
                        self.compile_branch_body(module, blocks, true_idx)?
                    };
                    // true 分支未终结时，需要 br 跳出 if 到 merge 块
                    if !true_terminates {
                        self.emit(WasmInstruction::Br(0));
                    }
                    self.emit(WasmInstruction::Else);
                    // true 分支未终结时，检查其续编链是否到达 false 分支。
                    // 若是，则 false 分支不在 Else 中编译——两分支在 if/else 之后汇合。
                    let true_reaches_false = !true_terminates && {
                        let mut cont = self.branch_continuation_target(blocks, true_idx);
                        let mut visited = std::collections::HashSet::new();
                        let mut found = false;
                        while let Some(c) = cont {
                            if c == false_idx {
                                found = true;
                                break;
                            }
                            if !visited.insert(c) {
                                break;
                            }
                            cont = self.branch_continuation_target(blocks, c);
                        }
                        found
                    };
                    let false_terminates = if false_is_merge || true_reaches_false {
                        if !false_is_merge {
                            self.emit_phi_moves(blocks, idx, false_idx);
                        }
                        false
                    } else {
                        self.compiled_blocks.insert(false_idx);
                        self.compile_branch_body(module, blocks, false_idx)?
                    };

                    self.emit(WasmInstruction::End);
                    self.if_depth -= 1;

                    // 检测两分支都Jump到同一外部块（try-catch的catch入口）
                    let both_jump_same = {
                        let tb = blocks.get(true_idx);
                        let fb = blocks.get(false_idx);
                        if let (Some(t), Some(f)) = (tb, fb) {
                            if let (
                                Terminator::Jump { target: tt },
                                Terminator::Jump { target: ft },
                            ) = (t.terminator(), f.terminator())
                            {
                                tt.0 == ft.0
                            } else {
                                false
                            }
                        } else {
                            false
                        }
                    };

                    if both_jump_same && !true_terminates && !false_terminates {
                        // 两分支都以Jump到common target结束，但compile_branch_body返回false
                        // 说明Jump被当作fall-through，实际应该br到合并点
                        // 这里不插入unreachable
                    }

                    // 继续到 merge block
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

                    // 如果 merge 是循环头，当前分支体已经以 continue 回到循环，
                    // 后续应编译循环出口块，而不是跳过出口直接落到函数尾部。
                    if self
                        .loop_stack
                        .last()
                        .is_some_and(|loop_info| merge < loop_info.header_idx)
                    {
                        while self
                            .loop_stack
                            .last()
                            .is_some_and(|loop_info| merge < loop_info.header_idx)
                        {
                            self.emit(WasmInstruction::End);
                            self.emit(WasmInstruction::End);
                            self.loop_stack.pop();
                        }
                        idx = merge;
                    } else if let Some(exit_idx) = self.loop_exit_for_header(merge) {
                        idx = exit_idx;
                    } else if self.compiled_blocks.contains(&merge) {
                        idx = self.next_after_compiled_merge(blocks, merge, true_idx, false_idx);
                    } else {
                        idx = merge;
                    }

                    // 当 merge 已被编译（作为某分支主体的一部分），而另一个分支未终结时，
                    // 需要为 fall-through 路径重新发射 merge 块的终止器。
                    if self.compiled_blocks.contains(&merge)
                        && !(true_terminates && false_terminates)
                        && let Some(merge_block) = blocks.get(merge)
                    {
                        // 重新发射 phi 指令：将 phi_local 复制到 SSA local
                        for instruction in merge_block.instructions() {
                            if let Instruction::Phi { dest, .. } = instruction
                                && let Some(&phi_local) = self.phi_locals.get(&dest.0)
                            {
                                self.emit(WasmInstruction::LocalGet(phi_local));
                                self.emit(WasmInstruction::LocalSet(self.local_idx(dest.0)));
                            }
                        }
                        // 重新发射 merge 块的终止器
                        match merge_block.terminator() {
                            Terminator::Return { value } => {
                                self.emit_return(value);
                            }
                            Terminator::Jump { target } => {
                                // merge 块跳转到后续块：跟踪跳转链找到未编译的续块
                                let mut jump_target = target.0 as usize;
                                while self.compiled_blocks.contains(&jump_target)
                                    && jump_target < blocks.len()
                                {
                                    match blocks[jump_target].terminator() {
                                        Terminator::Jump { target: t } => {
                                            jump_target = t.0 as usize;
                                        }
                                        Terminator::Return { .. }
                                        | Terminator::Throw { .. }
                                        | Terminator::Unreachable => break,
                                        _ => break,
                                    }
                                }
                                if jump_target < blocks.len()
                                    && !self.compiled_blocks.contains(&jump_target)
                                {
                                    idx = jump_target;
                                    continue;
                                }
                            }
                            _ => {}
                        }
                    }
                }
                Terminator::Switch {
                    value,
                    cases,
                    default_block,
                    exit_block,
                } => {
                    let exit_idx = exit_block.0 as usize;
                    self.compiled_blocks.insert(idx);
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

                    entries.sort_by_key(|e| e.target_idx);

                    let num_entries = entries.len();
                    let default_pos = entries.iter().position(|e| e.is_default).unwrap();

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
                        let entry_target = entry.target_idx;
                        let switch_break_depth = (num_entries - i - 1) as u32;
                        let extra_depth = (num_entries - i) as u32;
                        self.compile_switch_case(
                            module,
                            blocks,
                            entry_target,
                            exit_idx,
                            switch_break_depth,
                            extra_depth,
                            &loops,
                        )?;
                    }

                    self.emit(WasmInstruction::End);

                    // 当 exit == default 时，exit block 的内容已在 default case 位置编译过
                    if exit_idx == default_target_idx {
                        // 检查exit是否在循环内（会回到loop header）
                        let exit_in_loop = self.loop_stack.last().is_some()
                            && self.can_reach_loop_header(
                                blocks,
                                exit_idx,
                                self.loop_stack.last().unwrap().header_idx,
                            );

                        if exit_in_loop {
                            // exit在循环内，已完整编译，跳过重新发射
                            // 继续到循环出口或结束
                            if let Some(loop_info) = self.loop_stack.last()
                                && !self.compiled_blocks.contains(&loop_info.exit_idx)
                            {
                                idx = loop_info.exit_idx;
                                continue;
                            }
                            idx = blocks.len();
                        } else {
                            // exit不在循环内，跳过重新发射（exit已在default case完整编译）
                            // switch break会跳到switch End，续编exit之后的block
                            idx = exit_idx + 1;
                        }
                    } else if self.compiled_blocks.contains(&exit_idx) {
                        // exit != default 但已被编译（所有case都continue/break跳过了exit）
                        // 检查是否有循环出口需要续编
                        if let Some(loop_info) = self.loop_stack.last()
                            && loop_info.exit_idx != exit_idx
                            && !self.compiled_blocks.contains(&loop_info.exit_idx)
                        {
                            // 循环出口未编译，续编它
                            idx = loop_info.exit_idx;
                            continue;
                        }
                        // exit已编译且无其他出口，结束
                        idx = blocks.len();
                    } else {
                        // exit未编译，正常续编
                        idx = exit_idx;
                    }
                }
                Terminator::Throw { value } => {
                    // 将异常值编码为 TAG_EXCEPTION 返回给调用方
                    self.emit_eval_var_frame_exit();
                    self.emit(WasmInstruction::LocalGet(self.local_idx(value.0)));
                    let func_idx = self
                        .builtin_func_indices
                        .get(&Builtin::CreateException)
                        .copied()
                        .expect("CreateException import must be registered");
                    self.emit(WasmInstruction::Call(func_idx));
                    self.emit(WasmInstruction::Return);
                    idx += 1;
                }
            }
        }

        // 关闭所有剩余的循环
        while self.loop_stack.pop().is_some() {
            self.emit(WasmInstruction::End); // loop end
            self.emit(WasmInstruction::End); // block end
        }

        // 函数返回 i64 时，确保所有控制流路径末尾都有值在栈上。
        // 到达此处意味着没有任何块以 Return 结束——应被视为 unreachable。
        if self.current_func_returns_value {
            self.emit(WasmInstruction::Unreachable);
        }

        Ok(())
    }
}
