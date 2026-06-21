use super::*;

impl Compiler {
    pub(crate) fn compile_switch_case(
        &mut self,
        module: &IrModule,
        blocks: &[BasicBlock],
        case_start: usize,
        exit_idx: usize,
        switch_break_depth: u32,
        extra_depth: u32,
        loops: &[LoopInfo],
    ) -> Result<()> {
        let initial_loop_depth = self.loop_stack.len();
        let mut idx = case_start;

        loop {
            if idx >= blocks.len() {
                break;
            }

            // 关闭已到达出口的循环
            while let Some(top) = self.loop_stack.last() {
                if idx >= top.exit_idx && self.loop_stack.len() > initial_loop_depth {
                    self.emit(WasmInstruction::End);
                    self.emit(WasmInstruction::End);
                    self.loop_stack.pop();
                } else {
                    break;
                }
            }

            // 在循环头打开 block/loop
            if let Some(loop_info) = loops.iter().find(|l| l.header_idx == idx) {
                self.emit(WasmInstruction::Block(BlockType::Empty));
                self.emit(WasmInstruction::Loop(BlockType::Empty));
                self.loop_stack.push(loop_info.clone());
            }

            // Switch case 入口必须编译，即使 compiled_blocks 已标记（避免 async resume 块被跳过）。
            if self.compiled_blocks.contains(&idx) && idx != case_start {
                break;
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
                break;
            }

            match block.terminator() {
                Terminator::Return { value } => {
                    self.emit_return(value);
                    break;
                }
                Terminator::Unreachable => {
                    break;
                }
                Terminator::Jump { target } => {
                    let target_idx = target.0 as usize;
                    if target_idx == exit_idx {
                        // switch break
                        self.emit_phi_moves(blocks, idx, target_idx);
                        self.emit(WasmInstruction::Br(switch_break_depth + self.if_depth));
                        break;
                    } else if let Some(depth) = self.loop_continue_depth(target_idx) {
                        // loop continue（仅当循环在 case body 外部时加 extra_depth）
                        self.emit_phi_moves(blocks, idx, target_idx);
                        let adj = if target_idx >= case_start {
                            depth
                        } else {
                            depth + extra_depth
                        };
                        self.emit(WasmInstruction::Br(adj));
                        if target_idx >= case_start {
                            idx += 1; // 循环在 case body 内部，继续编译下一个 block（循环出口）
                        } else {
                            break; // 循环在 case body 外部（switch 在循环内），退出 case body 编译
                        }
                    } else if let Some(depth) = self.loop_break_depth(target_idx) {
                        // loop break（仅当循环在 case body 外部时加 extra_depth）
                        self.emit_phi_moves(blocks, idx, target_idx);
                        let adj = if target_idx >= case_start {
                            depth
                        } else {
                            depth + extra_depth
                        };
                        self.emit(WasmInstruction::Br(adj));
                        if target_idx >= case_start {
                            idx = target_idx; // 循环在 case body 内部，继续到循环出口
                        } else {
                            break; // 循环在 case body 外部，退出 case body 编译
                        }
                    } else if target_idx == idx + 1 {
                        // 自然 fall-through
                        idx = target_idx;
                    } else if target_idx > idx {
                        // 前向跳转
                        self.emit_phi_moves(blocks, idx, target_idx);
                        idx = target_idx;
                    } else {
                        // 后向跳转
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
                        let adj = if false_idx >= case_start {
                            break_depth
                        } else {
                            break_depth + extra_depth
                        };
                        self.emit(WasmInstruction::BrIf(adj));
                        idx = true_idx;
                        continue;
                    }

                    // do-while 条件（true 目标是循环头）
                    if let Some(depth) = self.loop_continue_depth(true_idx) {
                        self.emit_to_bool_i32(condition.0);
                        let adj = if true_idx >= case_start {
                            depth
                        } else {
                            depth + extra_depth
                        };
                        self.emit(WasmInstruction::BrIf(adj));
                        idx = false_idx;
                        continue;
                    }
                    // 自循环检测：某个分支通过 Jump 链最终跳回当前块
                    let true_self_loop = chain_jumps_to(blocks, true_idx, idx);
                    let false_self_loop = chain_jumps_to(blocks, false_idx, idx);

                    if true_self_loop || false_self_loop {
                        self.emit(WasmInstruction::Block(BlockType::Empty));
                        self.emit(WasmInstruction::Loop(BlockType::Empty));

                        self.emit_to_bool_i32(condition.0);
                        self.emit(WasmInstruction::If(BlockType::Empty));

                        // true 分支
                        self.compiled_blocks.insert(true_idx);
                        self.compile_branch_body_in_case(
                            module,
                            blocks,
                            true_idx,
                            case_start,
                            extra_depth,
                        )?;
                        self.emit(WasmInstruction::Br(1));

                        self.emit(WasmInstruction::Else);
                        // false 分支
                        self.compiled_blocks.insert(false_idx);
                        self.compile_branch_body_in_case(
                            module,
                            blocks,
                            false_idx,
                            case_start,
                            extra_depth,
                        )?;
                        self.emit(WasmInstruction::Br(2));

                        self.emit(WasmInstruction::End);
                        self.emit(WasmInstruction::End);
                        self.emit(WasmInstruction::End);

                        let exit_target = if true_self_loop {
                            resolve_jump_chain(blocks, false_idx)
                        } else {
                            resolve_jump_chain(blocks, true_idx)
                        };
                        idx = exit_target;
                        continue;
                    }

                    // 普通 if/else
                    self.emit_to_bool_i32(condition.0);
                    self.if_depth += 1;
                    self.emit(WasmInstruction::If(BlockType::Empty));

                    let true_is_merge = self.is_merge_block(blocks, false_idx, true_idx);
                    let false_is_merge = self.is_merge_block(blocks, true_idx, false_idx);
                    let true_exits_switch =
                        self.branch_continuation_target(blocks, true_idx) == Some(exit_idx);
                    let false_exits_switch =
                        self.branch_continuation_target(blocks, false_idx) == Some(exit_idx);

                    let true_terminates = if true_is_merge {
                        self.emit_phi_moves(blocks, idx, true_idx);
                        self.emit(WasmInstruction::Nop);
                        false
                    } else {
                        self.compiled_blocks.insert(true_idx);
                        let terminates = self.compile_branch_body_in_case(
                            module,
                            blocks,
                            true_idx,
                            case_start,
                            extra_depth,
                        )?;
                        if !terminates && true_exits_switch {
                            self.emit(WasmInstruction::Br(switch_break_depth + self.if_depth));
                            true
                        } else {
                            terminates
                        }
                    };

                    self.emit(WasmInstruction::Else);
                    let false_terminates = if false_is_merge {
                        self.emit_phi_moves(blocks, idx, false_idx);
                        self.emit(WasmInstruction::Nop);
                        false
                    } else {
                        self.compiled_blocks.insert(false_idx);
                        let terminates = self.compile_branch_body_in_case(
                            module,
                            blocks,
                            false_idx,
                            case_start,
                            extra_depth,
                        )?;
                        if !terminates && false_exits_switch {
                            self.emit(WasmInstruction::Br(switch_break_depth + self.if_depth));
                            true
                        } else {
                            terminates
                        }
                    };

                    self.emit(WasmInstruction::End);
                    self.if_depth -= 1;

                    if true_terminates && false_terminates {
                        self.emit(WasmInstruction::Unreachable);
                        break;
                    }

                    // 继续到 merge block
                    let merge = if false_is_merge {
                        false_idx
                    } else if true_is_merge {
                        true_idx
                    } else {
                        self.find_merge(blocks, true_idx, false_idx)
                    };

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
                }
                Terminator::Switch {
                    value,
                    cases,
                    default_block,
                    exit_block: nested_exit,
                } => {
                    // case body 内嵌套的 switch
                    let num_cases = cases.len();
                    let nested_default_idx = default_block.0 as usize;
                    let nested_exit_idx = nested_exit.0 as usize;
                    // 发射嵌套 switch 的 WASM blocks
                    self.emit(WasmInstruction::Block(BlockType::Empty));
                    self.emit(WasmInstruction::Block(BlockType::Empty));
                    for _ in 0..num_cases {
                        self.emit(WasmInstruction::Block(BlockType::Empty));
                    }

                    // 发射比较链
                    for (i, case) in cases.iter().enumerate() {
                        self.emit(WasmInstruction::LocalGet(self.local_idx(value.0)));
                        let const_val = self.encode_constant(
                            &module.constants()[case.constant.0 as usize],
                            module,
                        )?;
                        self.emit(WasmInstruction::I64Const(const_val));
                        self.emit(WasmInstruction::I64Eq);
                        self.emit(WasmInstruction::BrIf(i as u32));
                    }
                    self.emit(WasmInstruction::Br(num_cases as u32));

                    // 编译嵌套 case bodies
                    // 性能优化：直接迭代 cases，避免创建中间向量。
                    for (i, case) in cases.iter().enumerate() {
                        self.emit(WasmInstruction::End);
                        let cidx = case.target.0 as usize;
                        self.compiled_blocks.insert(cidx);
                        let nested_break = (num_cases - i) as u32;
                        let nested_extra = extra_depth + (num_cases - i) as u32 + 1;
                        self.compile_switch_case(
                            module,
                            blocks,
                            cidx,
                            nested_exit_idx,
                            nested_break,
                            nested_extra,
                            loops,
                        )?;
                    }

                    // 编译嵌套 default body
                    self.emit(WasmInstruction::End);
                    self.compiled_blocks.insert(nested_default_idx);
                    self.compile_switch_case(
                        module,
                        blocks,
                        nested_default_idx,
                        nested_exit_idx,
                        0,
                        extra_depth + 1,
                        loops,
                    )?;

                    // 关闭 nested exit block
                    self.emit(WasmInstruction::End);

                    idx = nested_exit_idx;
                }
                Terminator::Throw { value } => {
                    self.emit_eval_var_frame_exit();
                    self.emit(WasmInstruction::LocalGet(self.local_idx(value.0)));
                    let func_idx = self
                        .builtin_func_indices
                        .get(&Builtin::CreateException)
                        .copied()
                        .expect("CreateException import must be registered");
                    self.emit(WasmInstruction::Call(func_idx));
                    self.emit(WasmInstruction::Return);
                    break;
                }
            }
        }

        // 关闭在 case body 内打开的循环
        while self.loop_stack.len() > initial_loop_depth {
            self.loop_stack.pop();
            self.emit(WasmInstruction::End);
            self.emit(WasmInstruction::End);
        }

        Ok(())
    }
}
