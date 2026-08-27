//! tail_self_loop pass：把「自递归尾调用」原地改写成跳回函数入口的循环。
//!
//! 背景：`return f(...)` 形式的自递归每层都占一个 native 栈帧，深度受栈上限约束，
//! 与 ECMAScript 的 proper tail call 语义不符。本 pass 在 `direct_call` 之后运行，
//! 识别「函数体末尾调用自身且调用结果被直接返回」的调用点，改写为：
//!
//! - 删除该 `Call` 指令；
//! - 按实参顺序对每个 JS 形参槽发射 `StoreVar`（实参已是求值完成的 SSA 值，
//!   逐个写回不会互相污染）；
//! - 把该块的 `Return` 终止器换成 `Jump { target: f.entry() }`。
//!
//! 跳回入口而不是跳到函数体首块，是因为入口块承载 hoisted var 初始化、`$shared_env`
//! 复位与形参默认值/解构初始化——这些正是「一次新调用」应当重做的活动记录初始化。
//!
//! callee 识别覆盖两种形式：`Const(FunctionRef(self))`（`direct_call` 已静态解析），
//! 以及 `GetProp($env, "<自身函数声明绑定名>")`。后者是自递归的常态：函数体引用自身
//! 名字会把该名字记入 `captured_names`，于是 `direct_call` 不会把它标成
//! `direct_callable`，读取仍走 env。该读取安全等价于 `FunctionRef(self)` 的条件与
//! `direct_call` 完全一致——绑定来自函数声明（`known_callee_vars`）且全模块只被
//! `StoreVar` 一次，且模块内无 eval。
//!
//! v1 保守性：只处理不依赖「本次调用独有的活动记录」的函数。含 `Suspend` /
//! `GeneratorSuspend`（async / generator 的恢复点按函数入口分派）、`CollectRestArgs`
//! （`arguments` 绑定当次实参）、`SuperCall` / `new.target`（依赖构造调用上下文）、
//! 读写 `$this`（尾调用以 `this = undefined` 发起，循环会错误地沿用当前 `this`）
//! 的函数一律跳过；形参必须全部是本帧局部槽，否则写回会越过函数边界。

use std::collections::{HashMap, HashSet};

use wjsm_ir::{
    BasicBlockId, Builtin, Constant, Function, FunctionId, Instruction, Module, Terminator, ValueId,
};

use super::direct_call::{instr_uses, instruction_dest, is_env_name, terminator_uses};

/// 是否为 this 变量 IR 名（`$this` 或 `${scope}.$this`）。
fn is_this_name(name: &str) -> bool {
    name == "$this" || name.ends_with(".$this")
}

/// 一个待改写的尾调用点。
struct TailSite {
    block: BasicBlockId,
    /// `Call` 指令在块内的下标。
    instruction_index: usize,
    /// 调用实参，按 JS 形参顺序。
    args: Vec<ValueId>,
}

/// 全模块「不可变函数声明绑定」表：名字 → 目标函数。
///
/// 与 `direct_call` 的 `immutable` 同源：`known_callee_vars` 记录的是函数/类声明的
/// 初始绑定，全模块只有一次 `StoreVar` 即证明该绑定不可重赋。
fn immutable_function_bindings(module: &Module) -> HashMap<String, FunctionId> {
    let mut store_count: HashMap<&str, u32> = HashMap::new();
    for function in module.functions() {
        for block in function.blocks() {
            for instruction in block.instructions() {
                if let Instruction::StoreVar { name, .. } = instruction {
                    *store_count.entry(name.as_str()).or_insert(0) += 1;
                }
            }
        }
    }
    let mut bindings = HashMap::new();
    for function in module.functions() {
        for (name, function_id) in function.known_callee_vars() {
            if store_count.get(name.as_str()) == Some(&1) {
                bindings.insert(name.clone(), *function_id);
            }
        }
    }
    bindings
}

/// 函数内 `dest → 定义指令` 表。ValueId 每函数独立编号，必须按函数隔离。
fn function_defs(function: &Function) -> HashMap<ValueId, &Instruction> {
    let mut defs = HashMap::new();
    for block in function.blocks() {
        for instruction in block.instructions() {
            if let Some(dest) = instruction_dest(instruction) {
                defs.insert(dest, instruction);
            }
        }
    }
    defs
}

/// 常量下标对应的字符串常量。
fn constant_string(module: &Module, constant: wjsm_ir::ConstantId) -> Option<&str> {
    match module.constants().get(constant.0 as usize)? {
        Constant::String(text) => Some(text.as_str()),
        _ => None,
    }
}

/// `value` 的定义是否为 `Const(undefined)`。
fn is_const_undefined(
    module: &Module,
    defs: &HashMap<ValueId, &Instruction>,
    value: ValueId,
) -> bool {
    let Some(Instruction::Const { constant, .. }) = defs.get(&value) else {
        return false;
    };
    matches!(
        module.constants().get(constant.0 as usize),
        Some(Constant::Undefined)
    )
}

/// `callee` 是否静态等价于「本函数自身」。
fn callee_is_self(
    module: &Module,
    defs: &HashMap<ValueId, &Instruction>,
    bindings: &HashMap<String, FunctionId>,
    self_id: FunctionId,
    callee: ValueId,
) -> bool {
    match defs.get(&callee) {
        // direct_call 已把绑定读取替换为静态函数引用。
        Some(Instruction::Const { constant, .. }) => matches!(
            module.constants().get(constant.0 as usize),
            Some(Constant::FunctionRef(target)) if *target == self_id
        ),
        // 自递归常态：`GetProp($env, "<自身声明绑定名>")`。绑定不可重赋时恒等于自身。
        Some(Instruction::GetProp { object, key, .. }) => {
            let Some(Instruction::LoadVar { name, .. }) = defs.get(object) else {
                return false;
            };
            if !is_env_name(name) {
                return false;
            }
            let Some(Instruction::Const { constant, .. }) = defs.get(key) else {
                return false;
            };
            let Some(key_text) = constant_string(module, *constant) else {
                return false;
            };
            bindings.get(key_text) == Some(&self_id)
        }
        _ => false,
    }
}

/// 函数是否允许把自递归尾调用降为循环。
fn function_is_eligible(
    function: &Function,
    bindings: &HashMap<String, FunctionId>,
    frame_locals: &HashSet<&str>,
) -> bool {
    if function.has_eval() || function.params().len() < 2 {
        return false;
    }
    // 捕获名必须都是不可变函数声明绑定：这类捕获只是「读到一个恒定的函数值」，
    // 不构成本次调用独有的活动记录状态；其余捕获形态 v1 一律保守跳过。
    if !function
        .captured_names()
        .iter()
        .all(|name| bindings.contains_key(name.as_str()))
    {
        return false;
    }
    for block in function.blocks() {
        for instruction in block.instructions() {
            match instruction {
                Instruction::Suspend { .. }
                | Instruction::GeneratorSuspend { .. }
                | Instruction::CollectRestArgs { .. }
                | Instruction::SuperCall { .. } => return false,
                Instruction::CallBuiltin {
                    builtin: Builtin::NewTarget,
                    ..
                } => return false,
                Instruction::LoadVar { name, .. } | Instruction::StoreVar { name, .. }
                    if is_this_name(name) =>
                {
                    return false;
                }
                _ => {}
            }
        }
    }
    function
        .params()
        .iter()
        .skip(2)
        .all(|param| frame_locals.contains(param.as_str()))
}

/// 收集函数内全部可改写的自递归尾调用点。
fn collect_tail_sites(
    module: &Module,
    function: &Function,
    self_id: FunctionId,
    bindings: &HashMap<String, FunctionId>,
) -> Vec<TailSite> {
    let defs = function_defs(function);
    let js_param_count = function.params().len() - 2;
    let mut sites = Vec::new();
    for block in function.blocks() {
        let Terminator::Return {
            value: Some(returned),
        } = block.terminator()
        else {
            continue;
        };
        // 末尾的 DebugCheck 只是断点锚点，不影响尾位置判定。
        let Some((index, instruction)) = block
            .instructions()
            .iter()
            .enumerate()
            .rev()
            .find(|(_, instruction)| !matches!(instruction, Instruction::DebugCheck { .. }))
        else {
            continue;
        };
        let Instruction::Call {
            dest: Some(dest),
            callee,
            this_val,
            args,
        } = instruction
        else {
            continue;
        };
        if dest != returned || args.len() != js_param_count {
            continue;
        }
        // 尾调用必须是 `f(...)` 形式：`this` 为 undefined，循环才不会改变 this 绑定。
        if !is_const_undefined(module, &defs, *this_val) {
            continue;
        }
        if !callee_is_self(module, &defs, bindings, self_id, *callee) {
            continue;
        }
        sites.push(TailSite {
            block: block.id(),
            instruction_index: index,
            args: args.clone(),
        });
    }
    sites
}

/// 就地改写：`Call` → 形参写回，`Return` → 跳回入口。
fn rewrite_tail_sites(function: &mut Function, sites: &[TailSite]) {
    let entry = function.entry();
    let param_names: Vec<String> = function.params().iter().skip(2).cloned().collect();
    for site in sites {
        let Some(block) = function.block_by_id_mut(site.block) else {
            continue;
        };
        let instructions = block.instructions_mut();
        instructions.remove(site.instruction_index);
        let stores =
            param_names
                .iter()
                .zip(&site.args)
                .map(|(name, value)| Instruction::StoreVar {
                    name: name.clone(),
                    value: *value,
                });
        instructions.splice(site.instruction_index..site.instruction_index, stores);
        block.set_terminator(Terminator::Jump { target: entry });
    }
    remove_dead_callee_reads(function);
}

/// 清理改写后失去使用者的 callee 物化链。
///
/// `GetProp($env, "<函数声明名>")` 读的是内部 env 记录（普通数据属性，无 getter），
/// 删除无副作用；但它不在 `cfg_fold` 的 DCE 白名单里，留在原地会让每轮循环多一次
/// 属性查找。这里只删「零使用」的 `Const`、`LoadVar` 与 env `GetProp`，迭代到不动点。
fn remove_dead_callee_reads(function: &mut Function) {
    loop {
        let mut used: HashSet<ValueId> = HashSet::new();
        let mut env_loads: HashSet<ValueId> = HashSet::new();
        for block in function.blocks() {
            for instruction in block.instructions() {
                used.extend(instr_uses(instruction));
                if let Instruction::Phi { sources, .. } = instruction {
                    used.extend(sources.iter().map(|source| source.value));
                }
                if let Instruction::LoadVar { dest, name } = instruction
                    && is_env_name(name)
                {
                    env_loads.insert(*dest);
                }
            }
            used.extend(terminator_uses(block.terminator()));
        }

        let mut removed = false;
        for block in function.blocks_mut() {
            let before = block.instructions().len();
            block.instructions_mut().retain(|instruction| {
                let dead_candidate = match instruction {
                    Instruction::Const { dest, .. } | Instruction::LoadVar { dest, .. } => *dest,
                    Instruction::GetProp { dest, object, .. } if env_loads.contains(object) => {
                        *dest
                    }
                    _ => return true,
                };
                used.contains(&dead_candidate)
            });
            removed |= block.instructions().len() != before;
        }
        if !removed {
            return;
        }
    }
}

/// 运行 tail_self_loop pass。任一函数含 eval 时全局禁用（eval 可动态改写绑定）。
pub fn run(module: &mut Module) {
    if module.functions().iter().any(Function::has_eval) {
        return;
    }
    let bindings = immutable_function_bindings(module);
    let frame_locals: Vec<HashSet<String>> = module
        .frame_local_variable_names_by_function()
        .into_iter()
        .map(|names| names.into_iter().map(str::to_string).collect())
        .collect();

    let mut plan: Vec<(FunctionId, Vec<TailSite>)> = Vec::new();
    for (index, function) in module.functions().iter().enumerate() {
        let self_id = FunctionId(index as u32);
        let locals: HashSet<&str> = frame_locals[index].iter().map(String::as_str).collect();
        if !function_is_eligible(function, &bindings, &locals) {
            continue;
        }
        let sites = collect_tail_sites(module, function, self_id, &bindings);
        if !sites.is_empty() {
            plan.push((self_id, sites));
        }
    }

    for (function_id, sites) in plan {
        if let Some(function) = module.function_mut(function_id) {
            rewrite_tail_sites(function, &sites);
        }
    }
}

#[cfg(test)]
mod tests {
    use wjsm_ir::{BasicBlock, Program};

    use super::*;

    /// 构造 `function self_rec($env, $this, n)`：
    /// bb0 分支到 bb1（`return n`）或 bb2（`return self_rec(n)`）。
    /// `callee_via_env` 决定尾调用 callee 走 `GetProp($env, "$0.self_rec")` 还是
    /// `Const(FunctionRef)`。
    fn build_self_recursive(callee_via_env: bool) -> Program {
        let mut program = Program::new();
        let undefined = program.add_constant(Constant::Undefined);
        let zero = program.add_constant(Constant::Number(0.0));
        let name = program.add_constant(Constant::String("$0.self_rec".into()));

        let mut function = Function::new("self_rec", BasicBlockId(0));
        function.set_params(vec!["$1.$env".into(), "$1.$this".into(), "$1.n".into()]);

        let mut entry = BasicBlock::new(BasicBlockId(0));
        entry.push_instruction(Instruction::LoadVar {
            dest: ValueId(0),
            name: "$1.n".into(),
        });
        entry.push_instruction(Instruction::Const {
            dest: ValueId(1),
            constant: zero,
        });
        entry.push_instruction(Instruction::Compare {
            dest: ValueId(2),
            op: wjsm_ir::CompareOp::StrictEq,
            lhs: ValueId(0),
            rhs: ValueId(1),
        });
        entry.set_terminator(Terminator::Branch {
            condition: ValueId(2),
            true_block: BasicBlockId(1),
            false_block: BasicBlockId(2),
        });
        function.push_block(entry);

        let mut base = BasicBlock::new(BasicBlockId(1));
        base.push_instruction(Instruction::LoadVar {
            dest: ValueId(3),
            name: "$1.n".into(),
        });
        base.set_terminator(Terminator::Return {
            value: Some(ValueId(3)),
        });
        function.push_block(base);

        let mut tail = BasicBlock::new(BasicBlockId(2));
        tail.push_instruction(Instruction::Const {
            dest: ValueId(4),
            constant: undefined,
        });
        if callee_via_env {
            tail.push_instruction(Instruction::LoadVar {
                dest: ValueId(5),
                name: "$env".into(),
            });
            tail.push_instruction(Instruction::Const {
                dest: ValueId(6),
                constant: name,
            });
            tail.push_instruction(Instruction::GetProp {
                dest: ValueId(7),
                object: ValueId(5),
                key: ValueId(6),
            });
            function.set_captured_names(vec!["$0.self_rec".into()]);
        } else {
            let function_ref = program.add_constant(Constant::FunctionRef(FunctionId(0)));
            tail.push_instruction(Instruction::Const {
                dest: ValueId(7),
                constant: function_ref,
            });
        }
        tail.push_instruction(Instruction::LoadVar {
            dest: ValueId(8),
            name: "$1.n".into(),
        });
        tail.push_instruction(Instruction::Const {
            dest: ValueId(9),
            constant: zero,
        });
        tail.push_instruction(Instruction::Binary {
            dest: ValueId(10),
            op: wjsm_ir::BinaryOp::Sub,
            lhs: ValueId(8),
            rhs: ValueId(9),
        });
        tail.push_instruction(Instruction::Call {
            dest: Some(ValueId(11)),
            callee: ValueId(7),
            this_val: ValueId(4),
            args: vec![ValueId(10)],
        });
        tail.set_terminator(Terminator::Return {
            value: Some(ValueId(11)),
        });
        function.push_block(tail);

        program.push_function(function);

        // 模块入口：存储函数声明绑定，使 `$0.self_rec` 成为不可变函数绑定。
        let mut main = Function::new(wjsm_ir::MODULE_ENTRY_IR_NAME, BasicBlockId(0));
        let function_ref = program.add_constant(Constant::FunctionRef(FunctionId(0)));
        let mut main_entry = BasicBlock::new(BasicBlockId(0));
        main_entry.push_instruction(Instruction::Const {
            dest: ValueId(0),
            constant: function_ref,
        });
        main_entry.push_instruction(Instruction::StoreVar {
            name: "$0.self_rec".into(),
            value: ValueId(0),
        });
        main_entry.set_terminator(Terminator::Return { value: None });
        main.push_block(main_entry);
        main.record_known_callee("$0.self_rec".into(), FunctionId(0));
        program.push_function(main);

        program
    }

    fn tail_block(program: &Program) -> &BasicBlock {
        &program.functions()[0].blocks()[2]
    }

    fn call_count(function: &Function) -> usize {
        function
            .blocks()
            .iter()
            .flat_map(|block| block.instructions())
            .filter(|instruction| matches!(instruction, Instruction::Call { .. }))
            .count()
    }

    #[test]
    fn rewrites_env_resolved_self_tail_call_to_entry_jump() {
        let mut program = build_self_recursive(true);
        run(&mut program);

        let function = &program.functions()[0];
        assert_eq!(call_count(function), 0, "self tail call must be removed");
        let tail = tail_block(&program);
        assert!(matches!(
            tail.terminator(),
            Terminator::Jump {
                target: BasicBlockId(0)
            }
        ));
        assert!(
            tail.instructions().iter().any(|instruction| matches!(
                instruction,
                Instruction::StoreVar { name, value: ValueId(10) } if name == "$1.n"
            )),
            "argument must be written back to the parameter slot"
        );
    }

    #[test]
    fn rewrites_static_function_ref_self_tail_call() {
        let mut program = build_self_recursive(false);
        run(&mut program);

        assert_eq!(call_count(&program.functions()[0]), 0);
        assert!(matches!(
            tail_block(&program).terminator(),
            Terminator::Jump {
                target: BasicBlockId(0)
            }
        ));
    }

    #[test]
    fn keeps_call_when_result_is_not_returned_directly() {
        let mut program = build_self_recursive(true);
        // `return self_rec(n) - 0;` 不是尾调用：结果参与运算后才返回。
        {
            let function = program.function_mut(FunctionId(0)).expect("function");
            let tail = function.block_by_id_mut(BasicBlockId(2)).expect("block");
            tail.push_instruction(Instruction::Binary {
                dest: ValueId(12),
                op: wjsm_ir::BinaryOp::Sub,
                lhs: ValueId(11),
                rhs: ValueId(9),
            });
            tail.set_terminator(Terminator::Return {
                value: Some(ValueId(12)),
            });
        }
        run(&mut program);

        assert_eq!(call_count(&program.functions()[0]), 1);
        assert!(matches!(
            tail_block(&program).terminator(),
            Terminator::Return { .. }
        ));
    }

    #[test]
    fn keeps_call_when_this_is_not_undefined() {
        let mut program = build_self_recursive(true);
        // `obj.self_rec(n)` 形态：this 非 undefined，循环会改变 this 绑定。
        {
            let function = program.function_mut(FunctionId(0)).expect("function");
            let tail = function.block_by_id_mut(BasicBlockId(2)).expect("block");
            for instruction in tail.instructions_mut() {
                if let Instruction::Call { this_val, .. } = instruction {
                    *this_val = ValueId(8);
                }
            }
        }
        run(&mut program);

        assert_eq!(call_count(&program.functions()[0]), 1);
    }

    #[test]
    fn keeps_call_when_body_reads_this() {
        let mut program = build_self_recursive(true);
        {
            let function = program.function_mut(FunctionId(0)).expect("function");
            let base = function.block_by_id_mut(BasicBlockId(1)).expect("block");
            base.instructions_mut()[0] = Instruction::LoadVar {
                dest: ValueId(3),
                name: "$1.$this".into(),
            };
        }
        run(&mut program);

        assert_eq!(call_count(&program.functions()[0]), 1);
    }

    #[test]
    fn keeps_call_when_arguments_object_is_materialised() {
        let mut program = build_self_recursive(true);
        {
            let function = program.function_mut(FunctionId(0)).expect("function");
            let entry = function.block_by_id_mut(BasicBlockId(0)).expect("block");
            entry.push_instruction(Instruction::CollectRestArgs {
                dest: ValueId(12),
                skip: 0,
            });
        }
        run(&mut program);

        assert_eq!(call_count(&program.functions()[0]), 1);
    }

    #[test]
    fn keeps_call_when_module_uses_eval() {
        let mut program = build_self_recursive(true);
        program
            .function_mut(FunctionId(1))
            .expect("module entry")
            .set_has_eval(true);
        run(&mut program);

        assert_eq!(call_count(&program.functions()[0]), 1);
    }

    #[test]
    fn keeps_call_when_binding_is_reassigned() {
        let mut program = build_self_recursive(true);
        // 第二次 StoreVar 意味着绑定可重赋，env 读取不再恒等于自身。
        {
            let main = program.function_mut(FunctionId(1)).expect("module entry");
            let entry = main.block_by_id_mut(BasicBlockId(0)).expect("block");
            entry.push_instruction(Instruction::StoreVar {
                name: "$0.self_rec".into(),
                value: ValueId(0),
            });
        }
        run(&mut program);

        assert_eq!(call_count(&program.functions()[0]), 1);
    }

    #[test]
    fn rewritten_function_still_verifies() {
        let mut program = build_self_recursive(true);
        run(&mut program);
        program
            .verify()
            .expect("rewritten module must satisfy IR verification");
    }
}
