//! cfg_fold pass：IR 级控制流与常量折叠。
//!
//! 在多块内联（inline_for_ea）与逃逸分析/标量替换（escape_scalar）之后运行，
//! 把内联/EA 暴露的死指令、死块与可折叠分支清理干净。规则按序应用到不动点
//! （每函数每轮全量重扫，上限 8 轮）：
//!
//! 1. **IsException 折叠**：`is_exception(value)` 在 value 可证明非异常时恒
//!    false——Const/NewObject/Compare（===/!== 永不抛）/NewArray，及所有
//!    source 可证明非异常的 Phi（传递闭包）。NewObject 分配成功即普通对象；
//!    失败软上限 RangeError 的语义取舍见计划 Assumptions。Binary/Unary 可能
//!    抛异常（bigint 混合算术），不折叠。
//! 2. **IsJsObject 折叠**：`is_js_object(new_object)` 恒 true（从 inline_for_ea
//!    阶段 B 迁移，行为不变）。
//! 3. **常量分支折叠**：Const 条件的 `Branch` / Const 值的 `Switch` → `Jump`。
//! 4. **死块中和**：不可达块清空指令、终止器置 `Unreachable`（块索引与 id 不变，
//!    沿用后端中和约定；绝不从 blocks 向量移除）。
//! 5. **Phi 清洗**：剔除前驱边已死的 source；单值/全同值 phi 塌缩为直接值；
//!    0 源 phi（防御）替换为 undefined。
//! 6. **DCE**（白名单）：全函数零 use 的 `Const`/`LoadVar`/`Phi`/`IsException`；
//!    专用规则：`get_prototype_from_constructor`（构造器原型为不可配置数据属性，
//!    读取无副作用）在目标 `needs_prototype() == false` 时无条件删除。

use std::collections::{HashMap, HashSet};

use wjsm_ir::{
    BasicBlockId, Builtin, Constant, ConstantId, FunctionId, Instruction, Module, Terminator,
    ValueId,
};

use super::direct_call::{instr_uses, instruction_dest, terminator_uses};
use super::inline_for_ea::replace_all_uses_of;

/// 判断值是否可证明非异常（其 def 不可能产生 TAG_EXCEPTION）。
/// 仅覆盖恒不抛异常的指令：Const/NewObject（分配成功即普通对象/数组）、
/// Compare（===/!== 严格相等永不抛）、NewArray（数组分配），以及所有 source
/// 都可证明非异常的 Phi（传递闭包）；循环 Phi 保守返回 false。
///
/// 注意：Binary/Unary 不可折叠——bigint 混合算术（`7n + 1`）、一元 `+bigint`、
/// `delete`（严格模式不可配置属性）等都可能抛异常。
fn is_provably_non_exception(defs: &HashMap<ValueId, Instruction>, value: ValueId) -> bool {
    fn inner(
        defs: &HashMap<ValueId, Instruction>,
        value: ValueId,
        visiting: &mut HashSet<ValueId>,
    ) -> bool {
        let Some(instr) = defs.get(&value) else {
            return false;
        };
        match instr {
            Instruction::Const { .. }
            | Instruction::NewObject { .. }
            | Instruction::Compare { .. }
            | Instruction::NewArray { .. } => true,
            Instruction::Phi { sources, .. } => {
                if !visiting.insert(value) {
                    return false;
                }
                let all_non_exception = sources
                    .iter()
                    .all(|source| inner(defs, source.value, visiting));
                visiting.remove(&value);
                all_non_exception
            }
            _ => false,
        }
    }
    inner(defs, value, &mut HashSet::new())
}

/// 在模块常量池中查找指定常量；缺失时追加并返回新 ID。
fn const_id_or_add(module: &mut Module, constant: Constant) -> ConstantId {
    for (i, c) in module.constants().iter().enumerate() {
        if *c == constant {
            return ConstantId(i as u32);
        }
    }
    module.add_constant(constant)
}

/// 终止器的后继块（Jump/Branch/Switch）。
pub(crate) fn terminator_successors(terminator: &Terminator) -> Vec<BasicBlockId> {
    match terminator {
        Terminator::Jump { target } => vec![*target],
        Terminator::Branch {
            true_block,
            false_block,
            ..
        } => vec![*true_block, *false_block],
        Terminator::Switch {
            cases,
            default_block,
            exit_block,
            ..
        } => {
            let mut succs = Vec::with_capacity(cases.len() + 2);
            for case in cases {
                succs.push(case.target);
            }
            succs.push(*default_block);
            succs.push(*exit_block);
            succs
        }
        Terminator::Return { .. } | Terminator::Throw { .. } | Terminator::Unreachable => vec![],
    }
}

/// 运行 cfg_fold pass。
pub(crate) fn run(module: &mut Module) {
    let mut round = 0;
    let mut any_change = true;
    while any_change && round < 8 {
        any_change = false;
        round += 1;

        for fid in 0..module.functions().len() {
            let function_id = FunctionId(fid as u32);

            // ── 只读快照：def 表与常量 ID ──
            // （后续持有 function 可变借用时不能再查 module，先全部快照。）
            let constants_snapshot: Vec<Constant> = module.constants().to_vec();
            let defs: HashMap<ValueId, Instruction> = {
                let function = &module.functions()[fid];
                let mut defs = HashMap::new();
                for block in function.blocks() {
                    for instr in block.instructions() {
                        if let Some(dest) = instruction_dest(instr) {
                            defs.insert(dest, instr.clone());
                        }
                    }
                }
                defs
            };
            let (bool_true, bool_false, undefined) = {
                let bt = const_id_or_add(module, Constant::Bool(true));
                let bf = const_id_or_add(module, Constant::Bool(false));
                let undef = const_id_or_add(module, Constant::Undefined);
                (bt, bf, undef)
            };

            // ── 规则 1 + 2 + 6 专用：指令级折叠 ──
            let function = module
                .function_mut(function_id)
                .expect("function id must be valid");
            let mut folded_any = false;
            for block in function.blocks_mut() {
                let mut replace_sites: Vec<(usize, Instruction)> = Vec::new();
                for (idx, instr) in block.instructions().iter().enumerate() {
                    // 规则 1：is_exception(可证明非异常值) → false。
                    if let Instruction::IsException { dest, value } = instr {
                        let foldable = is_provably_non_exception(&defs, *value);
                        if foldable {
                            replace_sites.push((
                                idx,
                                Instruction::Const {
                                    dest: *dest,
                                    constant: bool_false,
                                },
                            ));
                        }
                        continue;
                    }
                    // 规则 2：is_js_object(NewObject) → true。
                    if let Instruction::CallBuiltin {
                        dest: Some(dest),
                        builtin: Builtin::IsJsObject,
                        args,
                    } = instr
                    {
                        if args.len() == 1
                            && matches!(defs.get(&args[0]), Some(Instruction::NewObject { .. }))
                        {
                            replace_sites.push((
                                idx,
                                Instruction::Const {
                                    dest: *dest,
                                    constant: bool_true,
                                },
                            ));
                        }
                        continue;
                    }
                }
                if !replace_sites.is_empty() {
                    for (idx, new_instr) in replace_sites {
                        block.instructions_mut()[idx] = new_instr;
                    }
                    folded_any = true;
                }
            }
            any_change |= folded_any;

            // ── 规则 3：常量分支折叠 ──
            let mut branch_folded = false;
            for block in function.blocks_mut() {
                let terminator = block.terminator_mut();
                match terminator {
                    Terminator::Branch {
                        condition,
                        true_block,
                        false_block,
                    } => {
                        if let Some(Instruction::Const { constant, .. }) = defs.get(condition)
                            && let Some(Constant::Bool(b)) =
                                constants_snapshot.get(constant.0 as usize)
                        {
                            let target = if *b { *true_block } else { *false_block };
                            *terminator = Terminator::Jump { target };
                            branch_folded = true;
                        }
                    }
                    Terminator::Switch {
                        value,
                        cases,
                        default_block,
                        ..
                    } => {
                        if let Some(Instruction::Const { constant, .. }) = defs.get(value) {
                            let target = cases
                                .iter()
                                .find(|case| case.constant == *constant)
                                .map_or(*default_block, |case| case.target);
                            *terminator = Terminator::Jump { target };
                            branch_folded = true;
                        }
                    }
                    _ => {}
                }
            }
            any_change |= branch_folded;

            // ── 规则 4：死块中和 ──
            let mut reachable = vec![false; function.blocks().len()];
            let mut stack = vec![function.entry()];
            while let Some(id) = stack.pop() {
                let idx = id.0 as usize;
                if idx >= reachable.len() || reachable[idx] {
                    continue;
                }
                reachable[idx] = true;
                let succs = terminator_successors(function.blocks()[idx].terminator());
                for succ in succs {
                    stack.push(succ);
                }
            }
            let mut neutralized = false;
            for (idx, block) in function.blocks_mut().iter_mut().enumerate() {
                if !reachable[idx] {
                    block.instructions_mut().clear();
                    block.set_terminator(Terminator::Unreachable);
                    neutralized = true;
                }
            }
            any_change |= neutralized;

            // ── 规则 5：Phi 清洗 ──
            // 前驱集合（基于中和后的终止器）。
            let block_count = function.blocks().len();
            let mut preds: Vec<Vec<BasicBlockId>> = vec![Vec::new(); block_count];
            for (i, block) in function.blocks().iter().enumerate() {
                for succ in terminator_successors(block.terminator()) {
                    if (succ.0 as usize) < block_count {
                        preds[succ.0 as usize].push(BasicBlockId(i as u32));
                    }
                }
            }

            let mut phi_folded = false;
            for (block_idx, block_preds) in preds.iter().enumerate() {
                // 收集（只读借用）→ 应用（可变借用），避免借用冲突。
                let mut collapses: Vec<(usize, ValueId, ValueId)> = Vec::new();
                let mut undefs: Vec<(usize, ValueId)> = Vec::new();
                {
                    let block = &function.blocks()[block_idx];
                    for (idx, instr) in block.instructions().iter().enumerate() {
                        if let Instruction::Phi { dest, sources } = instr {
                            let live_sources: Vec<wjsm_ir::PhiSource> = sources
                                .iter()
                                .filter(|s| block_preds.contains(&s.predecessor))
                                .cloned()
                                .collect();
                            if !live_sources.is_empty()
                                && live_sources
                                    .iter()
                                    .all(|s| s.value == live_sources[0].value)
                            {
                                collapses.push((idx, *dest, live_sources[0].value));
                            } else if live_sources.is_empty() {
                                // 防御：可达非入口块不应出现 0 源 phi；替换为 undefined。
                                undefs.push((idx, *dest));
                            }
                        }
                    }
                }
                for (_, dest, value) in &collapses {
                    replace_all_uses_of(function, *dest, *value);
                    phi_folded = true;
                }
                for (idx, dest) in &undefs {
                    function.blocks_mut()[block_idx].instructions_mut()[*idx] =
                        Instruction::Const {
                            dest: *dest,
                            constant: undefined,
                        };
                    phi_folded = true;
                }
                // 删除塌缩的 Phi（按索引降序，避免偏移）。
                let mut collapse_idxs: Vec<usize> =
                    collapses.into_iter().map(|(idx, _, _)| idx).collect();
                collapse_idxs.sort_unstable_by(|a, b| b.cmp(a));
                for idx in collapse_idxs {
                    function.blocks_mut()[block_idx]
                        .instructions_mut()
                        .remove(idx);
                }
            }
            any_change |= phi_folded;

            // ── 规则 6：DCE（白名单）──
            let mut use_count: HashMap<ValueId, usize> = HashMap::new();
            for block in function.blocks() {
                for instr in block.instructions() {
                    let mut used = instr_uses(instr);
                    if let Instruction::Phi { sources, .. } = instr {
                        used.extend(sources.iter().map(|s| s.value));
                    }
                    for v in used {
                        *use_count.entry(v).or_insert(0) += 1;
                    }
                }
                for v in terminator_uses(block.terminator()) {
                    *use_count.entry(v).or_insert(0) += 1;
                }
            }
            let mut dce_deletions: Vec<(usize, Vec<usize>)> = Vec::new();
            for (block_idx, block) in function.blocks().iter().enumerate() {
                let mut sites = Vec::new();
                for (idx, instr) in block.instructions().iter().enumerate() {
                    // 白名单：dest 零 use 即可删（纯指令 / 读取无副作用）。
                    let whitelisted = matches!(
                        instr,
                        Instruction::Const { .. }
                            | Instruction::LoadVar { .. }
                            | Instruction::Phi { .. }
                            | Instruction::IsException { .. }
                    ) || matches!(
                        instr,
                        Instruction::CallBuiltin {
                            builtin: Builtin::GetPrototypeFromConstructor,
                            ..
                        }
                    );
                    // 专用规则：get_prototype_from_constructor 读取构造器 prototype
                    // （不可配置数据属性）无副作用，但 dest 被 set_proto 等使用时必须
                    // 保留（删除会使 use 悬空）。仅零 use 时删除。
                    if whitelisted
                        && let Some(dest) = instruction_dest(instr)
                        && use_count.get(&dest).copied().unwrap_or(0) == 0
                    {
                        sites.push(idx);
                    }
                }
                if !sites.is_empty() {
                    dce_deletions.push((block_idx, sites));
                }
            }
            let has_dce = !dce_deletions.is_empty();
            for (block_idx, sites) in dce_deletions {
                let block = &mut function.blocks_mut()[block_idx];
                for idx in sites.into_iter().rev() {
                    block.instructions_mut().remove(idx);
                }
            }
            any_change |= has_dce;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wjsm_ir::{BasicBlock, Function};

    fn bool_true_id(module: &mut Module) -> ConstantId {
        const_id_or_add(module, Constant::Bool(true))
    }

    fn bool_false_id(module: &mut Module) -> ConstantId {
        const_id_or_add(module, Constant::Bool(false))
    }

    fn number_id(module: &mut Module, n: f64) -> ConstantId {
        const_id_or_add(module, Constant::Number(n))
    }

    fn block(id: u32) -> BasicBlock {
        BasicBlock::new(BasicBlockId(id))
    }

    #[test]
    fn folds_is_exception_on_new_object() {
        let mut module = Module::new();
        let obj = ValueId(0);
        let ex = ValueId(1);
        let b0 = block(0);
        let mut b1 = block(1);
        b1.push_instruction(Instruction::NewObject {
            dest: obj,
            capacity: 4,
        });
        b1.push_instruction(Instruction::IsException {
            dest: ex,
            value: obj,
        });
        b1.set_terminator(Terminator::Return { value: Some(ex) });
        module.push_function({
            let mut f = Function::new("ctor", BasicBlockId(1));
            f.set_has_eval(false);
            f.push_block(b0);
            f.push_block(b1);
            f
        });

        run(&mut module);

        let text = module.dump_text();
        // is_exception 已折叠为 const false；NewObject 的 dest 仍被 return 引用。
        assert!(!text.contains("is_exception"), "got:\n{text}");
        assert!(text.contains("= const c"), "折叠常量缺失:\n{text}");
    }

    #[test]
    fn folds_is_exception_on_compare() {
        let mut module = Module::new();
        let lhs = ValueId(0);
        let rhs = ValueId(1);
        let cmp = ValueId(2);
        let ex = ValueId(3);
        let c2 = number_id(&mut module, 2.0);
        let c3 = number_id(&mut module, 3.0);
        let b0 = block(0);
        let mut b1 = block(1);
        b1.push_instruction(Instruction::Const {
            dest: lhs,
            constant: c2,
        });
        b1.push_instruction(Instruction::Const {
            dest: rhs,
            constant: c3,
        });
        b1.push_instruction(Instruction::Compare {
            dest: cmp,
            op: wjsm_ir::CompareOp::StrictEq,
            lhs,
            rhs,
        });
        b1.push_instruction(Instruction::IsException {
            dest: ex,
            value: cmp,
        });
        b1.set_terminator(Terminator::Return { value: Some(ex) });
        module.push_function({
            let mut f = Function::new("cmp", BasicBlockId(1));
            f.set_has_eval(false);
            f.push_block(b0);
            f.push_block(b1);
            f
        });

        run(&mut module);

        let text = module.dump_text();
        assert!(!text.contains("is_exception"), "Compare 未折叠:\n{text}");
        assert!(text.contains("= const c"), "折叠常量缺失:\n{text}");
    }

    #[test]
    fn does_not_fold_is_exception_on_binary() {
        let mut module = Module::new();
        let lhs = ValueId(0);
        let rhs = ValueId(1);
        let bin = ValueId(2);
        let ex = ValueId(3);
        let c2 = number_id(&mut module, 2.0);
        let c3 = number_id(&mut module, 3.0);
        let b0 = block(0);
        let mut b1 = block(1);
        b1.push_instruction(Instruction::Const {
            dest: lhs,
            constant: c2,
        });
        b1.push_instruction(Instruction::Const {
            dest: rhs,
            constant: c3,
        });
        b1.push_instruction(Instruction::Binary {
            dest: bin,
            op: wjsm_ir::BinaryOp::Mul,
            lhs,
            rhs,
        });
        b1.push_instruction(Instruction::IsException {
            dest: ex,
            value: bin,
        });
        b1.set_terminator(Terminator::Return { value: Some(ex) });
        module.push_function({
            let mut f = Function::new("bin", BasicBlockId(1));
            f.set_has_eval(false);
            f.push_block(b0);
            f.push_block(b1);
            f
        });

        run(&mut module);

        let text = module.dump_text();
        assert!(text.contains("is_exception"), "Binary 不应被折叠:\n{text}");
    }

    #[test]
    fn folds_is_exception_on_phi_of_pure_values() {
        let mut module = Module::new();
        let a = ValueId(0);
        let b = ValueId(1);
        let phi = ValueId(2);
        let ex = ValueId(3);
        let ca = number_id(&mut module, 1.0);
        let cb = number_id(&mut module, 2.0);
        let mut b0 = block(0);
        let mut b1 = block(1);
        let mut b2 = block(2);
        let mut b3 = block(3);
        b0.set_terminator(Terminator::Branch {
            condition: ValueId(9),
            true_block: BasicBlockId(1),
            false_block: BasicBlockId(2),
        });
        b1.push_instruction(Instruction::Const {
            dest: a,
            constant: ca,
        });
        b1.set_terminator(Terminator::Jump {
            target: BasicBlockId(3),
        });
        b2.push_instruction(Instruction::Const {
            dest: b,
            constant: cb,
        });
        b2.set_terminator(Terminator::Jump {
            target: BasicBlockId(3),
        });
        b3.push_instruction(Instruction::Phi {
            dest: phi,
            sources: vec![
                wjsm_ir::PhiSource {
                    predecessor: BasicBlockId(1),
                    value: a,
                },
                wjsm_ir::PhiSource {
                    predecessor: BasicBlockId(2),
                    value: b,
                },
            ],
        });
        b3.push_instruction(Instruction::IsException {
            dest: ex,
            value: phi,
        });
        b3.set_terminator(Terminator::Return { value: Some(ex) });
        module.push_function({
            let mut f = Function::new("phi_exc", BasicBlockId(0));
            f.set_has_eval(false);
            f.push_block(b0);
            f.push_block(b1);
            f.push_block(b2);
            f.push_block(b3);
            f
        });

        run(&mut module);

        let text = module.dump_text();
        assert!(!text.contains("is_exception"), "Phi 未折叠:\n{text}");
        assert!(text.contains("= const c"), "折叠常量缺失:\n{text}");
    }

    #[test]
    fn does_not_fold_is_exception_on_call() {
        let mut module = Module::new();
        let callee = ValueId(0);
        let call = ValueId(1);
        let ex = ValueId(2);
        let b0 = block(0);
        let mut b1 = block(1);
        b1.push_instruction(Instruction::Const {
            dest: callee,
            constant: const_id_or_add(&mut module, Constant::Undefined),
        });
        b1.push_instruction(Instruction::Call {
            dest: Some(call),
            callee,
            this_val: callee,
            args: vec![],
        });
        b1.push_instruction(Instruction::IsException {
            dest: ex,
            value: call,
        });
        b1.set_terminator(Terminator::Return { value: Some(ex) });
        module.push_function({
            let mut f = Function::new("call_exc", BasicBlockId(1));
            f.set_has_eval(false);
            f.push_block(b0);
            f.push_block(b1);
            f
        });

        run(&mut module);

        let text = module.dump_text();
        assert!(
            text.contains("is_exception"),
            "Call 结果不应被折叠:\n{text}"
        );
    }

    #[test]
    fn folds_constant_branch_to_jump() {
        let mut module = Module::new();
        let b0 = block(0);
        let mut b1 = block(1);
        let mut b2 = block(2);
        let mut b3 = block(3);
        let cond = ValueId(0);
        let c_true = bool_true_id(&mut module);
        b1.push_instruction(Instruction::Const {
            dest: cond,
            constant: c_true,
        });
        b1.set_terminator(Terminator::Branch {
            condition: cond,
            true_block: BasicBlockId(2),
            false_block: BasicBlockId(3),
        });
        b2.set_terminator(Terminator::Return { value: None });
        b3.set_terminator(Terminator::Return { value: None });
        module.push_function({
            let mut f = Function::new("br", BasicBlockId(1));
            f.set_has_eval(false);
            f.push_block(b0);
            f.push_block(b1);
            f.push_block(b2);
            f.push_block(b3);
            f
        });

        run(&mut module);

        let text = module.dump_text();
        assert!(!text.contains("branch"), "分支未折叠:\n{text}");
        assert!(text.contains("jump bb2"), "应跳转到 true_block:\n{text}");
        // false_block（bb3）成为死块被中和。
        assert!(text.contains("bb3:"), "块索引必须保留:\n{text}");
    }

    #[test]
    fn collapses_three_source_phi() {
        let mut module = Module::new();
        let v = ValueId(0);
        let phi = ValueId(1);
        let c = number_id(&mut module, 42.0);
        // 菱形嵌套：entry → bb1（二路分支）→ bb3/bb4，entry → bb2；
        // bb2/bb3/bb4 都是汇合块 bb5 的前驱（三源 phi）。
        let mut b0 = block(0);
        let mut b1 = block(1);
        let mut b2 = block(2);
        let mut b3 = block(3);
        let mut b4 = block(4);
        let mut b5 = block(5);
        b0.set_terminator(Terminator::Jump {
            target: BasicBlockId(1),
        });
        b1.push_instruction(Instruction::Const {
            dest: v,
            constant: c,
        });
        b1.set_terminator(Terminator::Branch {
            condition: ValueId(9),
            true_block: BasicBlockId(3),
            false_block: BasicBlockId(4),
        });
        b2.push_instruction(Instruction::Const {
            dest: v,
            constant: c,
        });
        b2.set_terminator(Terminator::Jump {
            target: BasicBlockId(5),
        });
        b3.push_instruction(Instruction::Const {
            dest: v,
            constant: c,
        });
        b3.set_terminator(Terminator::Jump {
            target: BasicBlockId(5),
        });
        b4.push_instruction(Instruction::Const {
            dest: v,
            constant: c,
        });
        b4.set_terminator(Terminator::Jump {
            target: BasicBlockId(5),
        });
        b5.push_instruction(Instruction::Phi {
            dest: phi,
            sources: vec![
                wjsm_ir::PhiSource {
                    predecessor: BasicBlockId(2),
                    value: v,
                },
                wjsm_ir::PhiSource {
                    predecessor: BasicBlockId(3),
                    value: v,
                },
                wjsm_ir::PhiSource {
                    predecessor: BasicBlockId(4),
                    value: v,
                },
            ],
        });
        b5.set_terminator(Terminator::Return { value: Some(phi) });
        module.push_function({
            let mut f = Function::new("phi", BasicBlockId(0));
            f.set_has_eval(false);
            f.push_block(b0);
            f.push_block(b1);
            f.push_block(b2);
            f.push_block(b3);
            f.push_block(b4);
            f.push_block(b5);
            f
        });

        run(&mut module);

        let text = module.dump_text();
        assert!(!text.contains("= phi"), "phi 未塌缩:\n{text}");
        assert!(
            text.contains("return %0"),
            "phi dest 应替换为唯一值:\n{text}"
        );
    }

    #[test]
    fn dead_block_neutralization_prunes_phi_source() {
        let mut module = Module::new();
        let v = ValueId(0);
        let phi = ValueId(1);
        let c = number_id(&mut module, 7.0);
        // entry → bb1 (branch false→bb2, true→bb4 死块)；bb2 → bb3；bb3 是 phi 汇合。
        let mut b0 = block(0);
        let mut b1 = block(1);
        let mut b2 = block(2);
        let mut b3 = block(3);
        let mut b4 = block(4);
        let cond = ValueId(2);
        let c_false = bool_false_id(&mut module);
        b0.set_terminator(Terminator::Jump {
            target: BasicBlockId(1),
        });
        b1.push_instruction(Instruction::Const {
            dest: cond,
            constant: c_false,
        });
        b1.set_terminator(Terminator::Branch {
            condition: cond,
            true_block: BasicBlockId(4),
            false_block: BasicBlockId(2),
        });
        b2.push_instruction(Instruction::Const {
            dest: v,
            constant: c,
        });
        b2.set_terminator(Terminator::Jump {
            target: BasicBlockId(3),
        });
        // bb3 的 phi 有两个 source：来自 bb2（活）与 bb4（死）。
        b3.push_instruction(Instruction::Phi {
            dest: phi,
            sources: vec![
                wjsm_ir::PhiSource {
                    predecessor: BasicBlockId(2),
                    value: v,
                },
                wjsm_ir::PhiSource {
                    predecessor: BasicBlockId(4),
                    value: v,
                },
            ],
        });
        b3.set_terminator(Terminator::Return { value: Some(phi) });
        b4.set_terminator(Terminator::Return { value: Some(v) });
        module.push_function({
            let mut f = Function::new("dead", BasicBlockId(0));
            f.set_has_eval(false);
            f.push_block(b0);
            f.push_block(b1);
            f.push_block(b2);
            f.push_block(b3);
            f.push_block(b4);
            f
        });

        run(&mut module);

        let text = module.dump_text();
        // bb4 死块被中和（branch 折叠后其 predecessor 消失，phi 单源塌缩）。
        assert!(!text.contains("(bb4"), "死前驱 phi source 未剔除:\n{text}");
        assert!(!text.contains("= phi"), "单源 phi 未塌缩:\n{text}");
    }

    #[test]
    fn folds_switch_on_constant() {
        let mut module = Module::new();
        let v = ValueId(0);
        let c1 = number_id(&mut module, 1.0);
        let c2 = number_id(&mut module, 2.0);
        let b0 = block(0);
        let mut b1 = block(1);
        let mut b2 = block(2);
        let mut b3 = block(3);
        let mut b4 = block(4);
        b1.push_instruction(Instruction::Const {
            dest: v,
            constant: c2,
        });
        b1.set_terminator(Terminator::Switch {
            value: v,
            cases: vec![
                wjsm_ir::SwitchCaseTarget {
                    constant: c1,
                    target: BasicBlockId(2),
                },
                wjsm_ir::SwitchCaseTarget {
                    constant: c2,
                    target: BasicBlockId(3),
                },
            ],
            default_block: BasicBlockId(4),
            exit_block: BasicBlockId(4),
        });
        b2.set_terminator(Terminator::Return { value: None });
        b3.set_terminator(Terminator::Return { value: None });
        b4.set_terminator(Terminator::Return { value: None });
        module.push_function({
            let mut f = Function::new("sw", BasicBlockId(1));
            f.set_has_eval(false);
            f.push_block(b0);
            f.push_block(b1);
            f.push_block(b2);
            f.push_block(b3);
            f.push_block(b4);
            f
        });

        run(&mut module);

        let text = module.dump_text();
        assert!(!text.contains("switch"), "switch 未折叠:\n{text}");
        assert!(text.contains("jump bb3"), "应跳转到命中 case:\n{text}");
    }
}
