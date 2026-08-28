//! array_inline pass：把「回调可静态解析的数组高阶函数调用」展开为显式循环 IR。
//!
//! 背景：`arr.map(cb)` 在 lowering 时降为单条 `CallBuiltin(ArrayMap, arr, cb, thisArg)`，
//! 运行时 `dispatch_array_callback` 逐元素经 `invoke_callable → prepare_call →
//! call_indirect → finish_call` 全套动态调用。本 pass 在 `direct_call` 之后、
//! `inline_for_ea` 之前运行：把这类调用展开为带 `ArrayIsArray` 守卫的显式循环 +
//! 普通 `Call` 指令；展开后回调是 `Const(FunctionRef)` 时被 `inline_for_ea` 阶段 A
//! 真正内联进循环（无捕获回调 → 每元素零调用成本），有捕获回调则省掉 host builtin
//! dispatch 层。
//!
//! 展开范围：ForEach / Map / Filter / Find / FindIndex / FindLast / FindLastIndex /
//! Some / Every，以及**有 initialValue** 的 Reduce / ReduceRight。FlatMap / sort /
//! toSorted / TypedArray 版方法保持原 builtin。回调 def 必须是
//! `Const(FunctionRef)` 或 `CallBuiltin(CreateClosure)`，否则保持原 builtin。

use std::collections::HashMap;

use wjsm_ir::{
    BasicBlock, BasicBlockId, BinaryOp, Builtin, Constant, ConstantId, FunctionId, Instruction,
    Module, PhiSource, Terminator, ValueId,
};

use super::direct_call::instruction_dest;
use super::inline_for_ea::{find_exception_path, max_value_id_in_function, undefined_const_id};

/// 可展开的数组回调方法类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    ForEach,
    Map,
    Filter,
    Find,
    FindIndex,
    FindLast,
    FindLastIndex,
    Some,
    Every,
    Reduce,
    ReduceRight,
}

impl Kind {
    fn from_builtin(builtin: Builtin) -> Option<Self> {
        Some(match builtin {
            Builtin::ArrayForEach => Kind::ForEach,
            Builtin::ArrayMap => Kind::Map,
            Builtin::ArrayFilter => Kind::Filter,
            Builtin::ArrayFind => Kind::Find,
            Builtin::ArrayFindIndex => Kind::FindIndex,
            Builtin::ArrayFindLast => Kind::FindLast,
            Builtin::ArrayFindLastIndex => Kind::FindLastIndex,
            Builtin::ArraySome => Kind::Some,
            Builtin::ArrayEvery => Kind::Every,
            Builtin::ArrayReduce => Kind::Reduce,
            Builtin::ArrayReduceRight => Kind::ReduceRight,
            _ => return None,
        })
    }

    /// 是否为 reduce 类（回调四参数、带 accumulator）。
    fn is_reduce(self) -> bool {
        matches!(self, Kind::Reduce | Kind::ReduceRight)
    }

    /// 是否反向迭代（索引 len-1 → 0）。
    fn reverse(self) -> bool {
        matches!(
            self,
            Kind::ReduceRight | Kind::FindLast | Kind::FindLastIndex
        )
    }

    /// 是否在回调 truthy 时提前退出（find/findIndex/some）。
    fn exits_on_truthy(self) -> bool {
        matches!(
            self,
            Kind::Find | Kind::FindIndex | Kind::FindLast | Kind::FindLastIndex | Kind::Some
        )
    }
}

/// 一个可展开站点。
struct Candidate {
    func_idx: u32,
    block_idx: u32,
    instr_idx: usize,
    kind: Kind,
    builtin: Builtin,
    /// 原 CallBuiltin 的 dest（展开后由 merge phi 重新定义）。
    dest: ValueId,
    /// 数组实参（args[0]）。
    arr: ValueId,
    /// 回调实参（args[1]）。
    cb: ValueId,
    /// 原参数（慢路径原样重放）。
    args: Vec<ValueId>,
    /// iterate 类 this 实参（args[2]，缺省 None → undefined）。
    this_val: Option<ValueId>,
    /// reduce 类 initialValue（args[2]）。
    initial: Option<ValueId>,
    /// 语句级异常路径（tmp 变量名, catch 块）；None → 函数级 Throw。
    exc: Option<(String, BasicBlockId)>,
}

/// 查找或追加指定常量（返回已有下标，避免重复）。
fn find_or_add_constant(module: &mut Module, constant: Constant) -> ConstantId {
    if let Some(index) = module.constants().iter().position(|c| *c == constant) {
        ConstantId(index as u32)
    } else {
        module.add_constant(constant)
    }
}

/// 递增分配的 fresh ValueId。
struct ValueGen {
    next: u32,
}

impl ValueGen {
    fn new(start: u32) -> Self {
        Self { next: start }
    }

    fn fresh(&mut self) -> ValueId {
        let id = ValueId(self.next);
        self.next += 1;
        id
    }
}

/// 运行 array_inline pass：单轮展开（每个 CallBuiltin 站点展开一次，不做不动点迭代）。
pub(crate) fn run(module: &mut Module) {
    // 全局守卫：eval 可动态改写绑定，保守禁用（与 direct_call 一致）。
    if module.functions().iter().any(|f| f.has_eval()) {
        return;
    }

    // 预收集（不可变借用阶段）。
    let constants_snapshot: Vec<Constant> = module.constants().to_vec();
    let per_func_defs: Vec<HashMap<ValueId, Instruction>> = module
        .functions()
        .iter()
        .map(|f| {
            let mut defs = HashMap::new();
            for block in f.blocks() {
                for instr in block.instructions() {
                    if let Some(dest) = instruction_dest(instr) {
                        defs.insert(dest, instr.clone());
                    }
                }
            }
            defs
        })
        .collect();
    let per_func_max_value: Vec<u32> = module
        .functions()
        .iter()
        .map(max_value_id_in_function)
        .collect();

    // 候选收集。
    let mut candidates: Vec<Candidate> = Vec::new();
    for (func_idx, function) in module.functions().iter().enumerate() {
        for (block_idx, block) in function.blocks().iter().enumerate() {
            for (instr_idx, instr) in block.instructions().iter().enumerate() {
                let Instruction::CallBuiltin {
                    dest: Some(dest),
                    builtin,
                    args,
                } = instr
                else {
                    continue;
                };
                let Some(kind) = Kind::from_builtin(*builtin) else {
                    continue;
                };
                // reduce 仅展开有 initialValue（args.len() >= 3）的调用。
                if kind.is_reduce() && args.len() < 3 {
                    continue;
                }
                if args.len() < 2 {
                    continue;
                }
                // 回调 def 必须是 Const(FunctionRef) 或 CallBuiltin(CreateClosure)。
                let cb = args[1];
                match per_func_defs[func_idx].get(&cb) {
                    Some(Instruction::Const { constant, .. }) => {
                        match constants_snapshot.get(constant.0 as usize) {
                            Some(Constant::FunctionRef(_)) => {}
                            _ => continue,
                        }
                    }
                    Some(Instruction::CallBuiltin {
                        builtin: Builtin::CreateClosure,
                        dest: Some(_),
                        ..
                    }) => {}
                    _ => continue,
                }
                let this_val = if kind.is_reduce() {
                    None
                } else {
                    args.get(2).copied()
                };
                let initial = if kind.is_reduce() {
                    args.get(2).copied()
                } else {
                    None
                };
                let exc = find_exception_path(function, block_idx, instr_idx, *dest);
                candidates.push(Candidate {
                    func_idx: func_idx as u32,
                    block_idx: block_idx as u32,
                    instr_idx,
                    kind,
                    builtin: *builtin,
                    dest: *dest,
                    arr: args[0],
                    cb,
                    args: args.clone(),
                    this_val,
                    initial,
                    exc,
                });
            }
        }
    }

    if candidates.is_empty() {
        return;
    }

    // 逆序执行：追加块不影响既有块 id 与调用前指令索引。
    candidates.sort_by_key(|c| (c.func_idx, c.block_idx, c.instr_idx as u32));
    candidates.reverse();
    let mut current_max_value = per_func_max_value;
    for candidate in candidates {
        expand_site(module, &candidate, &mut current_max_value);
    }
}

/// 展开单个站点（块分裂 + 守卫 + 慢路径 + 显式循环 + merge）。
fn expand_site(module: &mut Module, cand: &Candidate, current_max_value: &mut [u32]) {
    let func_idx = cand.func_idx as usize;
    let block_idx = cand.block_idx as usize;
    let instr_idx = cand.instr_idx;
    let caller_id = FunctionId(cand.func_idx);

    // ── 常量（先加，避免与函数可变借用冲突）──
    let str_length = find_or_add_constant(module, Constant::String("length".to_string()));
    let zero_const = find_or_add_constant(module, Constant::Number(0.0));
    let one_const = find_or_add_constant(module, Constant::Number(1.0));
    let neg_one_const = find_or_add_constant(module, Constant::Number(-1.0));
    let undef_const = undefined_const_id(module);
    let true_const = find_or_add_constant(module, Constant::Bool(true));
    let false_const = find_or_add_constant(module, Constant::Bool(false));

    // ── 分裂调用块：pre 留在原块，post + 原终止器进 B_post ──
    let (pre_instructions, post_instructions, orig_terminator) = {
        let block = &module.functions()[func_idx].blocks()[block_idx];
        (
            block.instructions()[..instr_idx].to_vec(),
            block.instructions()[instr_idx + 1..].to_vec(),
            block.terminator().clone(),
        )
    };

    // ── 块 id 分配（顺序即追加顺序，最小化后向边）──
    let mut next_id = module.functions()[func_idx].blocks().len() as u32;
    let mut alloc_block = || {
        let id = BasicBlockId(next_id);
        next_id += 1;
        id
    };
    let guard = alloc_block();
    let slow = alloc_block();
    let fast = alloc_block();
    let header = alloc_block();
    let body = alloc_block();
    let call_blk = alloc_block();
    let ok_blk = alloc_block();
    let push_blk = (cand.kind == Kind::Filter).then(&mut alloc_block);
    let found_blk = cand.kind.exits_on_truthy().then(&mut alloc_block);
    let fail_blk = (cand.kind == Kind::Every).then(&mut alloc_block);
    let next_blk = matches!(
        cand.kind,
        Kind::Filter
            | Kind::Find
            | Kind::FindIndex
            | Kind::FindLast
            | Kind::FindLastIndex
            | Kind::Some
            | Kind::Every
    )
    .then(&mut alloc_block);
    let skip_blk = alloc_block();
    let exc_blk = alloc_block();
    let next = alloc_block();
    let exit = alloc_block();
    let merge = alloc_block();
    let b_post = alloc_block();

    // ── ValueId 分配：先分配被「后建块」前向引用的值 ──
    let mut vg = ValueGen::new(current_max_value[func_idx] + 1);
    let slow_result = vg.fresh();
    let i2 = vg.fresh();
    let acc2 = cand.kind.is_reduce().then(|| vg.fresh());

    let mut blocks: Vec<BasicBlock> = Vec::new();

    // ── guard：is_array 运行时守卫 ──
    {
        let mut b = BasicBlock::new(guard);
        let is_arr = vg.fresh();
        b.push_instruction(Instruction::CallBuiltin {
            dest: Some(is_arr),
            builtin: Builtin::ArrayIsArray,
            args: vec![cand.arr],
        });
        b.set_terminator(Terminator::Branch {
            condition: is_arr,
            true_block: fast,
            false_block: slow,
        });
        blocks.push(b);
    }

    // ── slow：原 builtin 调用（结果 = slow_result）──
    {
        let mut b = BasicBlock::new(slow);
        b.push_instruction(Instruction::CallBuiltin {
            dest: Some(slow_result),
            builtin: cand.builtin,
            args: cand.args.clone(),
        });
        b.set_terminator(Terminator::Jump { target: merge });
        blocks.push(b);
    }

    // ── fast：length 快照 + 常量 + 结果容器 + 反向初值 ──
    let fast_len;
    let fast_zero;
    let fast_one;
    let fast_neg_one;
    let fast_undef;
    let fast_true;
    let fast_false;
    let fast_result;
    let i_start;
    {
        let mut b = BasicBlock::new(fast);
        let len_key = vg.fresh();
        let len = vg.fresh();
        b.push_instruction(Instruction::Const {
            dest: len_key,
            constant: str_length,
        });
        b.push_instruction(Instruction::GetProp {
            dest: len,
            object: cand.arr,
            key: len_key,
        });
        let zero = vg.fresh();
        let one = vg.fresh();
        let neg_one = vg.fresh();
        let undef = vg.fresh();
        let true_v = vg.fresh();
        let false_v = vg.fresh();
        b.push_instruction(Instruction::Const {
            dest: zero,
            constant: zero_const,
        });
        b.push_instruction(Instruction::Const {
            dest: one,
            constant: one_const,
        });
        b.push_instruction(Instruction::Const {
            dest: neg_one,
            constant: neg_one_const,
        });
        b.push_instruction(Instruction::Const {
            dest: undef,
            constant: undef_const,
        });
        b.push_instruction(Instruction::Const {
            dest: true_v,
            constant: true_const,
        });
        b.push_instruction(Instruction::Const {
            dest: false_v,
            constant: false_const,
        });
        let result = match cand.kind {
            Kind::Map => {
                let result = vg.fresh();
                b.push_instruction(Instruction::CallBuiltin {
                    dest: Some(result),
                    builtin: Builtin::ArrayAllocate,
                    args: vec![len],
                });
                Some(result)
            }
            Kind::Filter => {
                let result = vg.fresh();
                b.push_instruction(Instruction::NewArray {
                    dest: result,
                    capacity: 0,
                });
                Some(result)
            }
            _ => None,
        };
        let start = if cand.kind.reverse() {
            let start = vg.fresh();
            b.push_instruction(Instruction::Binary {
                dest: start,
                op: BinaryOp::Sub,
                lhs: len,
                rhs: one,
            });
            start
        } else {
            zero
        };
        b.set_terminator(Terminator::Jump { target: header });
        fast_len = len;
        fast_zero = zero;
        fast_one = one;
        fast_neg_one = neg_one;
        fast_undef = undef;
        fast_true = true_v;
        fast_false = false_v;
        fast_result = result;
        i_start = start;
        blocks.push(b);
    }

    // ── header：索引 phi + 累加器 phi（reduce）+ 循环条件 ──
    let loop_index;
    let loop_acc;
    {
        let mut b = BasicBlock::new(header);
        let i = vg.fresh();
        b.push_instruction(Instruction::Phi {
            dest: i,
            sources: vec![
                PhiSource {
                    predecessor: fast,
                    value: i_start,
                },
                PhiSource {
                    predecessor: next,
                    value: i2,
                },
            ],
        });
        let acc = if let Some(initial) = cand.initial {
            let acc = vg.fresh();
            b.push_instruction(Instruction::Phi {
                dest: acc,
                sources: vec![
                    PhiSource {
                        predecessor: fast,
                        value: initial,
                    },
                    PhiSource {
                        predecessor: next,
                        value: acc2.expect("reduce 预分配 acc2"),
                    },
                ],
            });
            Some(acc)
        } else {
            None
        };
        // 循环条件：正向 i < len；反向 i >= 0。
        let (cmp_rhs, cmp_rev, cmp_inv) = if cand.kind.reverse() {
            (fast_zero, fast_false, fast_true)
        } else {
            (fast_len, fast_false, fast_false)
        };
        let cond = vg.fresh();
        b.push_instruction(Instruction::CallBuiltin {
            dest: Some(cond),
            builtin: Builtin::AbstractCompare,
            args: vec![i, cmp_rhs, cmp_rev, cmp_inv],
        });
        b.set_terminator(Terminator::Branch {
            condition: cond,
            true_block: body,
            false_block: exit,
        });
        loop_index = i;
        loop_acc = acc;
        blocks.push(b);
    }

    // ── body：hole 检查 ──
    {
        let mut b = BasicBlock::new(body);
        let has = vg.fresh();
        b.push_instruction(Instruction::CallBuiltin {
            dest: Some(has),
            builtin: Builtin::ArrayHasElement,
            args: vec![cand.arr, loop_index],
        });
        b.set_terminator(Terminator::Branch {
            condition: has,
            true_block: call_blk,
            false_block: skip_blk,
        });
        blocks.push(b);
    }

    // ── call_blk：读取元素 + 调用回调 + 异常检查 ──
    let elem;
    let call_result;
    {
        let mut b = BasicBlock::new(call_blk);
        let e = vg.fresh();
        b.push_instruction(Instruction::GetElem {
            dest: e,
            object: cand.arr,
            index: loop_index,
        });
        let r = vg.fresh();
        let this_val = cand.this_val.unwrap_or(fast_undef);
        let call_args: Vec<ValueId> = if let Some(acc) = loop_acc {
            vec![acc, e, loop_index, cand.arr]
        } else {
            vec![e, loop_index, cand.arr]
        };
        b.push_instruction(Instruction::Call {
            dest: Some(r),
            callee: cand.cb,
            this_val,
            args: call_args,
        });
        let is_exc = vg.fresh();
        b.push_instruction(Instruction::IsException {
            dest: is_exc,
            value: r,
        });
        b.set_terminator(Terminator::Branch {
            condition: is_exc,
            true_block: exc_blk,
            false_block: ok_blk,
        });
        elem = e;
        call_result = r;
        blocks.push(b);
    }

    // ── ok_blk：按 kind 处理回调结果 ──
    {
        let mut b = BasicBlock::new(ok_blk);
        let term = match cand.kind {
            Kind::ForEach => Terminator::Jump { target: next },
            Kind::Map => {
                let dest = vg.fresh();
                b.push_instruction(Instruction::SetElem {
                    dest,
                    object: fast_result.expect("map 有 result"),
                    index: loop_index,
                    value: call_result,
                    strict: false,
                });
                Terminator::Jump { target: next }
            }
            Kind::Filter => {
                let t = vg.fresh();
                b.push_instruction(Instruction::CallBuiltin {
                    dest: Some(t),
                    builtin: Builtin::ToBoolean,
                    args: vec![call_result],
                });
                Terminator::Branch {
                    condition: t,
                    true_block: push_blk.expect("filter 有 push_blk"),
                    false_block: next_blk.expect("filter 有 next_blk"),
                }
            }
            Kind::Find | Kind::FindIndex | Kind::FindLast | Kind::FindLastIndex | Kind::Some => {
                let t = vg.fresh();
                b.push_instruction(Instruction::CallBuiltin {
                    dest: Some(t),
                    builtin: Builtin::ToBoolean,
                    args: vec![call_result],
                });
                Terminator::Branch {
                    condition: t,
                    true_block: found_blk.expect("exits_on_truthy 有 found_blk"),
                    false_block: next_blk.expect("exits_on_truthy 有 next_blk"),
                }
            }
            Kind::Every => {
                let t = vg.fresh();
                b.push_instruction(Instruction::CallBuiltin {
                    dest: Some(t),
                    builtin: Builtin::ToBoolean,
                    args: vec![call_result],
                });
                Terminator::Branch {
                    condition: t,
                    true_block: next_blk.expect("every 有 next_blk"),
                    false_block: fail_blk.expect("every 有 fail_blk"),
                }
            }
            Kind::Reduce | Kind::ReduceRight => Terminator::Jump { target: next },
        };
        b.set_terminator(term);
        blocks.push(b);
    }

    // ── 辅助块（id 顺序）：push_blk / found_blk / fail_blk / next_blk ──
    if let Some(push_blk) = push_blk {
        let mut b = BasicBlock::new(push_blk);
        b.push_instruction(Instruction::CallBuiltin {
            dest: None,
            builtin: Builtin::ArrayPush,
            args: vec![fast_result.expect("filter 有 result"), elem],
        });
        b.set_terminator(Terminator::Jump { target: next });
        blocks.push(b);
    }
    if let Some(found_blk) = found_blk {
        let mut b = BasicBlock::new(found_blk);
        b.set_terminator(Terminator::Jump { target: merge });
        blocks.push(b);
    }
    if let Some(fail_blk) = fail_blk {
        let mut b = BasicBlock::new(fail_blk);
        b.set_terminator(Terminator::Jump { target: merge });
        blocks.push(b);
    }
    if let Some(next_blk) = next_blk {
        let mut b = BasicBlock::new(next_blk);
        b.set_terminator(Terminator::Jump { target: next });
        blocks.push(b);
    }

    // ── skip_blk：hole 跳过 ──
    {
        let mut b = BasicBlock::new(skip_blk);
        b.set_terminator(Terminator::Jump { target: next });
        blocks.push(b);
    }

    // ── exc_blk：回调异常传播 ──
    {
        let mut b = BasicBlock::new(exc_blk);
        let thrown = vg.fresh();
        b.push_instruction(Instruction::CallBuiltin {
            dest: Some(thrown),
            builtin: Builtin::ExceptionValue,
            args: vec![call_result],
        });
        match &cand.exc {
            Some((tmp_name, catch_target)) => {
                b.push_instruction(Instruction::StoreVar {
                    name: tmp_name.clone(),
                    value: thrown,
                });
                b.set_terminator(Terminator::Jump {
                    target: *catch_target,
                });
            }
            None => {
                b.set_terminator(Terminator::Throw { value: thrown });
            }
        }
        blocks.push(b);
    }

    // ── next：累加器 phi（reduce）+ 索引自增/自减 ──
    {
        let mut b = BasicBlock::new(next);
        if let Some(acc2) = acc2 {
            b.push_instruction(Instruction::Phi {
                dest: acc2,
                sources: vec![
                    PhiSource {
                        predecessor: ok_blk,
                        value: call_result,
                    },
                    PhiSource {
                        predecessor: skip_blk,
                        value: loop_acc.expect("reduce 有 acc"),
                    },
                ],
            });
        }
        let op = if cand.kind.reverse() {
            BinaryOp::Sub
        } else {
            BinaryOp::Add
        };
        b.push_instruction(Instruction::Binary {
            dest: i2,
            op,
            lhs: loop_index,
            rhs: fast_one,
        });
        b.set_terminator(Terminator::Jump { target: header });
        blocks.push(b);
    }

    // ── exit：循环正常结束 → merge ──
    {
        let mut b = BasicBlock::new(exit);
        b.set_terminator(Terminator::Jump { target: merge });
        blocks.push(b);
    }

    // ── merge：d = phi(slow, 快路径) → B_post ──
    {
        let mut b = BasicBlock::new(merge);
        let mut sources = vec![PhiSource {
            predecessor: slow,
            value: slow_result,
        }];
        if let Some(found) = found_blk {
            let found_value = match cand.kind {
                Kind::Find | Kind::FindLast => elem,
                Kind::FindIndex | Kind::FindLastIndex => loop_index,
                Kind::Some => fast_true,
                _ => unreachable!("found_blk 仅 exits_on_truthy"),
            };
            sources.push(PhiSource {
                predecessor: found,
                value: found_value,
            });
        } else if let Some(fail) = fail_blk {
            sources.push(PhiSource {
                predecessor: fail,
                value: fast_false,
            });
        }
        let exit_value = match cand.kind {
            Kind::ForEach => fast_undef,
            Kind::Map | Kind::Filter => fast_result.expect("容器 kind 有 result"),
            Kind::Reduce | Kind::ReduceRight => loop_acc.expect("reduce 有 acc"),
            Kind::Find | Kind::FindLast => fast_undef,
            Kind::FindIndex | Kind::FindLastIndex => fast_neg_one,
            Kind::Some => fast_false,
            Kind::Every => fast_true,
        };
        sources.push(PhiSource {
            predecessor: exit,
            value: exit_value,
        });
        b.push_instruction(Instruction::Phi {
            dest: cand.dest,
            sources,
        });
        b.set_terminator(Terminator::Jump { target: b_post });
        blocks.push(b);
    }

    // ── B_post：调用后指令 + 原终止器 ──
    {
        let mut b = BasicBlock::new(b_post);
        for ins in post_instructions {
            b.push_instruction(ins);
        }
        b.set_terminator(orig_terminator);
        blocks.push(b);
    }

    // ── 应用：重写原块 + 追加新块 ──
    current_max_value[func_idx] = vg.next - 1;
    {
        let caller = module
            .function_mut(caller_id)
            .expect("caller function must exist");
        let b = &mut caller.blocks_mut()[block_idx];
        *b.instructions_mut() = pre_instructions;
        b.set_terminator(Terminator::Jump { target: guard });
        for block in blocks {
            caller.push_block(block);
        }
        // 原终止器已迁入 b_post：后继块中以原块为前驱的 phi source 必须改指
        // b_post，否则 phi 清洗会把这条边当死边剔除，塌缩出支配性破坏。
        let orig_block = BasicBlockId(cand.block_idx);
        for succ in super::cfg_fold::terminator_successors(
            caller.blocks()[b_post.0 as usize].terminator(),
        ) {
            for instr in caller.blocks_mut()[succ.0 as usize].instructions_mut() {
                if let Instruction::Phi { sources, .. } = instr {
                    for source in sources {
                        if source.predecessor == orig_block {
                            source.predecessor = b_post;
                        }
                    }
                }
            }
        }
    }
}
