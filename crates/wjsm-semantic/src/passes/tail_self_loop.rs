//! tail_self_loop pass：把自递归尾调用改写为函数内的回边循环（loopification）。
//!
//! 背景：`function count(n, acc) { ...; return count(n - 1, acc + 1); }` 每层递归都要走
//! 一次完整的 JS 调用（callee 求值、参数装帧、native 栈帧），深递归会线性消耗调用栈直到
//! `RangeError: Maximum call stack size exceeded`。
//!
//! 改写：尾位置的自调用等价于「把实参写回形参槽 + 跳回函数入口」，因为一次新调用相对
//! 当前帧只改变形参绑定；其余每次调用都要重做的初始化（hoisted var 置 undefined、闭包
//! env 分配、参数默认值……）本来就位于入口块之后，回边会原样重跑一遍。于是：
//!
//! ```text
//!   %d = call %self, this=undefined, args=[%a0, %a1]      store var $1.n,   %a0
//!   return %d                                        =>   store var $1.acc, %a1
//!                                                         jump bb0
//! ```
//!
//! 后端零改动：IR 入口块在 Cranelift 里本就被映射成独立的 `entry_body` 块（真正的
//! CLIF entry 负责 prologue），入口块因此可以有前驱；SSA 由 cranelift-frontend 的
//! variable 机制在 `seal_all_blocks` 时重建。
//!
//! 适用范围（v1）：仅自递归，且函数不依赖「每次调用由宿主重新建立、回边无法复现」的
//! 状态——`this`、`new.target`、`arguments` / rest 参数、async / generator 的
//! resume 状态。形参必须能提升为栈帧局部（被内层闭包捕获的形参留在共享 env 里，
//! 真实递归会为每层建立独立绑定，回边做不到，必须排除）。
//!
//! 本 pass 排在 `direct_call` 之后：callee 既可能已被替换成 `Const(FunctionRef)`，
//! 也可能仍是不可变函数声明绑定的 `LoadVar` / `GetProp(env, name)`（自递归函数会把
//! 自身名字捕获进 env，`direct_call` 对这种形状保守跳过），两种形状都要能识别。

use std::collections::{HashMap, HashSet};

use wjsm_ir::{
    BasicBlockId, Builtin, Constant, Function, FunctionId, Instruction, Module, Terminator, ValueId,
};

use super::direct_call::{
    collect_uses, immutable_function_bindings, instruction_dest, is_env_name, terminator_uses,
};

/// 一处已确认可改写的自递归尾调用。
#[derive(Debug, Clone, PartialEq, Eq)]
struct TailSite {
    /// 尾调用所在块；该块的终止器必定是 `Return { value: Some(call_dest) }`。
    block: BasicBlockId,
    /// `Call` 指令在块内的下标。
    index: usize,
    /// 实参 ValueId，按形参顺序（求值已在 `Call` 之前完成，回写不存在别名问题）。
    args: Vec<ValueId>,
}

/// 运行 tail_self_loop pass。任一函数含 eval 时全局禁用（eval 可动态改写绑定，
/// callee 解析的不可变前提失效）。
pub fn run(module: &mut Module) {
    if module.functions().iter().any(Function::has_eval) {
        return;
    }

    let immutable = immutable_function_bindings(module);
    // 形参必须已是栈帧局部，回写才等价于重新绑定形参；借用需在改写前结束。
    let frame_locals: Vec<HashSet<String>> = module
        .frame_local_variable_names_by_function()
        .into_iter()
        .map(|names| names.into_iter().map(str::to_owned).collect())
        .collect();

    let mut plans: Vec<(FunctionId, Vec<TailSite>)> = Vec::new();
    for (index, function) in module.functions().iter().enumerate() {
        let function_id = FunctionId(index as u32);
        if !function_is_loopifiable(function, &frame_locals[index]) {
            continue;
        }
        let sites = collect_tail_sites(module, function, function_id, &immutable);
        if !sites.is_empty() {
            plans.push((function_id, sites));
        }
    }

    for (function_id, sites) in plans {
        if let Some(function) = module.function_mut(function_id) {
            rewrite_tail_sites(function, &sites);
            drop_dead_env_reads(function);
        }
    }
}

/// 函数级门槛：函数体不得依赖「回边无法复现的每次调用状态」。
fn function_is_loopifiable(function: &Function, frame_locals: &HashSet<String>) -> bool {
    // async / generator 的 resume dispatch 以入口块为状态机分发点，回边会破坏恢复语义。
    if is_resumable_function_name(function.name()) {
        return false;
    }
    // 约定：params[0] = $env，params[1] = $this，其后才是 JS 形参。
    if function.params().len() < 2 {
        return false;
    }

    for block in function.blocks() {
        for instruction in block.instructions() {
            match instruction {
                // Suspend / GeneratorSuspend：同上，函数是状态机。
                Instruction::Suspend { .. }
                | Instruction::GeneratorSuspend { .. }
                // CollectRestArgs：rest 参数与 arguments 对象读的是宿主 activation 里的
                // 原始实参，回边改写形参槽不会更新它。
                | Instruction::CollectRestArgs { .. }
                // SuperCall / new.target：由调用约定在每次调用时建立。
                | Instruction::SuperCall { .. }
                | Instruction::CallBuiltin {
                    builtin: Builtin::NewTarget,
                    ..
                } => return false,
                // this 绑定同样由调用方传入；尾调用点 this=undefined 与当前帧的 this 未必一致。
                Instruction::LoadVar { name, .. } if is_this_name(name) => return false,
                _ => {}
            }
        }
    }

    let js_params = &function.params()[2..];
    // 重名形参（非严格模式 `function f(a, a)`）的槽位共享，顺序回写语义不清晰，直接排除。
    let unique: HashSet<&str> = js_params.iter().map(String::as_str).collect();
    if unique.len() != js_params.len() {
        return false;
    }
    js_params
        .iter()
        .all(|param| frame_locals.contains(param.as_str()))
}

/// async / generator 降级后的函数体名（`f$async` / `f$asyncgen` / `f$gen`）。
fn is_resumable_function_name(name: &str) -> bool {
    name.ends_with("$async") || name.ends_with("$asyncgen") || name.ends_with("$gen")
}

/// 是否为 this 变量 IR 名（`$this` 或 `${scope}.$this`）。
fn is_this_name(name: &str) -> bool {
    name == "$this" || name.ends_with(".$this")
}

/// 扫描函数内全部尾位置自调用。
fn collect_tail_sites(
    module: &Module,
    function: &Function,
    function_id: FunctionId,
    immutable: &HashMap<String, FunctionId>,
) -> Vec<TailSite> {
    let (defs, const_strings) = function_defs(module, function);
    let arity = function.params().len() - 2;

    let mut sites = Vec::new();
    for block in function.blocks() {
        let Terminator::Return {
            value: Some(returned),
        } = block.terminator()
        else {
            continue;
        };
        let Some(index) = last_significant_instruction(block.instructions()) else {
            continue;
        };
        let Instruction::Call {
            dest: Some(dest),
            callee,
            this_val,
            args,
        } = &block.instructions()[index]
        else {
            continue;
        };
        if dest != returned || args.len() != arity {
            continue;
        }
        if resolve_callee_function(module, &defs, &const_strings, immutable, *callee)
            != Some(function_id)
        {
            continue;
        }
        if !is_undefined_const(module, &defs, *this_val) {
            continue;
        }
        // 调用结果只被这条 return 消费；否则删除 Call 会留下悬空 use。
        if value_is_used(function, *dest, Some(block.id())) {
            continue;
        }
        sites.push(TailSite {
            block: block.id(),
            index,
            args: args.clone(),
        });
    }
    sites
}

/// 函数内 def 表与 `Const(String)` 字面量表（ValueId 每函数独立编号）。
fn function_defs<'a>(
    module: &Module,
    function: &'a Function,
) -> (HashMap<ValueId, &'a Instruction>, HashMap<ValueId, String>) {
    let mut defs = HashMap::new();
    let mut const_strings = HashMap::new();
    for block in function.blocks() {
        for instruction in block.instructions() {
            if let Some(dest) = instruction_dest(instruction) {
                defs.insert(dest, instruction);
            }
            if let Instruction::Const { dest, constant } = instruction
                && let Some(Constant::String(text)) = module.constants().get(constant.0 as usize)
            {
                const_strings.insert(*dest, text.clone());
            }
        }
    }
    (defs, const_strings)
}

/// 块内最后一条「语义相关」指令的下标；`DebugCheck` 只是断点映射标记，跳过。
fn last_significant_instruction(instructions: &[Instruction]) -> Option<usize> {
    instructions
        .iter()
        .rposition(|instruction| !matches!(instruction, Instruction::DebugCheck { .. }))
}

/// 把 callee 值解析回静态函数。识别 `direct_call` 之后可能出现的三种形状。
fn resolve_callee_function(
    module: &Module,
    defs: &HashMap<ValueId, &Instruction>,
    const_strings: &HashMap<ValueId, String>,
    immutable: &HashMap<String, FunctionId>,
    callee: ValueId,
) -> Option<FunctionId> {
    match defs.get(&callee)? {
        // direct_call 已把不可变绑定读取替换成 FunctionRef。
        Instruction::Const { constant, .. } => match module.constants().get(constant.0 as usize)? {
            Constant::FunctionRef(function_id) => Some(*function_id),
            _ => None,
        },
        // 未被替换的不可变函数声明绑定读取。
        Instruction::LoadVar { name, .. } => immutable.get(name.as_str()).copied(),
        // 自递归函数会把自身名字捕获进共享 env，callee 是 `GetProp(env, "$N.name")`。
        Instruction::GetProp { object, key, .. } => {
            let Instruction::LoadVar { name, .. } = defs.get(object)? else {
                return None;
            };
            if !is_env_name(name) {
                return None;
            }
            immutable.get(const_strings.get(key)?.as_str()).copied()
        }
        _ => None,
    }
}

/// `value` 的 def 是否为 `Const(Undefined)`。
fn is_undefined_const(
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

/// `value` 在函数内是否还有使用者（含 Phi source 与终止器）。
///
/// `ignored_terminator` 指定的块的终止器不计入：判定尾调用时，那条即将被回边替换掉的
/// `Return` 不算「别的使用者」。
fn value_is_used(
    function: &Function,
    value: ValueId,
    ignored_terminator: Option<BasicBlockId>,
) -> bool {
    !collect_uses(function, value).is_empty()
        || function.blocks().iter().any(|block| {
            Some(block.id()) != ignored_terminator
                && terminator_uses(block.terminator()).contains(&value)
        })
}

/// 就地改写：删除 `Call`，按形参顺序回写实参，终止器换成跳回入口的回边。
fn rewrite_tail_sites(function: &mut Function, sites: &[TailSite]) {
    let entry = function.entry();
    let params: Vec<String> = function.params()[2..].to_vec();
    // 每个 site 位于独立的块（一个块只有一个 Return 终止器），下标互不影响。
    for site in sites {
        let Some(block) = function.block_by_id_mut(site.block) else {
            continue;
        };
        let instructions = block.instructions_mut();
        instructions.remove(site.index);
        for (offset, (name, value)) in params.iter().zip(&site.args).enumerate() {
            instructions.insert(
                site.index + offset,
                Instruction::StoreVar {
                    name: name.clone(),
                    value: *value,
                },
            );
        }
        block.set_terminator(Terminator::Jump { target: entry });
    }
}

/// 删除改写后零 use 的 `GetProp(env, ...)`。
///
/// 被改写的自递归调用点上，callee 往往是 `GetProp(env, "$N.name")`——env 对象是编译器
/// 生成的内部记录，没有 getter，读取无副作用，删除安全。留着它会让每轮回边都多走一次
/// 属性读取，正好抵消掉 loopification 的收益。剩下的 `LoadVar $env` 与键常量由
/// cfg_fold 的通用 DCE 白名单回收。
fn drop_dead_env_reads(function: &mut Function) {
    let env_values: HashSet<ValueId> = function
        .blocks()
        .iter()
        .flat_map(|block| block.instructions())
        .filter_map(|instruction| match instruction {
            Instruction::LoadVar { dest, name } if is_env_name(name) => Some(*dest),
            _ => None,
        })
        .collect();

    let dead: Vec<(BasicBlockId, usize)> = function
        .blocks()
        .iter()
        .flat_map(|block| {
            block
                .instructions()
                .iter()
                .enumerate()
                .filter(|(_, instruction)| match instruction {
                    Instruction::GetProp { dest, object, .. } => {
                        env_values.contains(object) && !value_is_used(function, *dest, None)
                    }
                    _ => false,
                })
                .map(|(index, _)| (block.id(), index))
                .collect::<Vec<_>>()
        })
        .collect();

    for (block_id, index) in dead.into_iter().rev() {
        if let Some(block) = function.block_by_id_mut(block_id) {
            block.instructions_mut().remove(index);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wjsm_ir::BasicBlock;

    /// 构造 `function self(n) { ... }`：params = [$1.$env, $1.$this, $1.n]。
    fn self_recursive_module(body: impl FnOnce(&mut Module, &mut Function)) -> Module {
        let mut module = Module::new();
        let mut function = Function::new("self", BasicBlockId(0));
        function.set_params(vec![
            "$1.$env".to_string(),
            "$1.$this".to_string(),
            "$1.n".to_string(),
        ]);
        body(&mut module, &mut function);
        module.push_function(function);
        module
    }

    /// `bb0: %0 = FunctionRef(@0); %1 = undefined; %2 = load n; %3 = call %0(%2); return %3`
    fn tail_call_block(module: &mut Module, arg_count: usize) -> BasicBlock {
        let function_ref = module.add_constant(Constant::FunctionRef(FunctionId(0)));
        let undefined = module.add_constant(Constant::Undefined);
        let mut block = BasicBlock::new(BasicBlockId(0));
        block.push_instruction(Instruction::Const {
            dest: ValueId(0),
            constant: function_ref,
        });
        block.push_instruction(Instruction::Const {
            dest: ValueId(1),
            constant: undefined,
        });
        let args: Vec<ValueId> = (0..arg_count)
            .map(|index| {
                let dest = ValueId(2 + index as u32);
                block.push_instruction(Instruction::LoadVar {
                    dest,
                    name: "$1.n".to_string(),
                });
                dest
            })
            .collect();
        let dest = ValueId(2 + arg_count as u32);
        block.push_instruction(Instruction::Call {
            dest: Some(dest),
            callee: ValueId(0),
            this_val: ValueId(1),
            args,
        });
        block.set_terminator(Terminator::Return { value: Some(dest) });
        block
    }

    /// `$1.n` 需同时被 load 与 store 才算栈帧局部候选；形参已计入 store。
    fn assert_rewritten(module: &Module, block: usize, expected_store_values: &[ValueId]) {
        let function = &module.functions()[0];
        let block = &function.blocks()[block];
        assert!(
            !block
                .instructions()
                .iter()
                .any(|instruction| matches!(instruction, Instruction::Call { .. })),
            "尾调用应被删除：{block:?}"
        );
        let stores: Vec<ValueId> = block
            .instructions()
            .iter()
            .filter_map(|instruction| match instruction {
                Instruction::StoreVar { name, value } if name == "$1.n" => Some(*value),
                _ => None,
            })
            .collect();
        assert_eq!(stores, expected_store_values);
        assert_eq!(
            block.terminator(),
            &Terminator::Jump {
                target: BasicBlockId(0)
            }
        );
    }

    #[test]
    fn rewrites_simple_self_tail_call() {
        let mut module = self_recursive_module(|module, function| {
            let block = tail_call_block(module, 1);
            function.push_block(block);
        });
        run(&mut module);
        assert_rewritten(&module, 0, &[ValueId(2)]);
    }

    #[test]
    fn rewrites_multiple_tail_sites() {
        let mut module = self_recursive_module(|module, function| {
            // bb0: branch %0, bb1, bb2（条件用 load 的值即可，pass 不看条件类型）
            let mut entry = BasicBlock::new(BasicBlockId(0));
            entry.push_instruction(Instruction::LoadVar {
                dest: ValueId(0),
                name: "$1.n".to_string(),
            });
            entry.set_terminator(Terminator::Branch {
                condition: ValueId(0),
                true_block: BasicBlockId(1),
                false_block: BasicBlockId(2),
            });
            function.push_block(entry);

            let function_ref = module.add_constant(Constant::FunctionRef(FunctionId(0)));
            let undefined = module.add_constant(Constant::Undefined);
            for (index, base) in [(1u32, 10u32), (2, 20)] {
                let mut block = BasicBlock::new(BasicBlockId(index));
                block.push_instruction(Instruction::Const {
                    dest: ValueId(base),
                    constant: function_ref,
                });
                block.push_instruction(Instruction::Const {
                    dest: ValueId(base + 1),
                    constant: undefined,
                });
                block.push_instruction(Instruction::LoadVar {
                    dest: ValueId(base + 2),
                    name: "$1.n".to_string(),
                });
                block.push_instruction(Instruction::Call {
                    dest: Some(ValueId(base + 3)),
                    callee: ValueId(base),
                    this_val: ValueId(base + 1),
                    args: vec![ValueId(base + 2)],
                });
                block.set_terminator(Terminator::Return {
                    value: Some(ValueId(base + 3)),
                });
                function.push_block(block);
            }
        });
        run(&mut module);
        assert_rewritten(&module, 1, &[ValueId(12)]);
        assert_rewritten(&module, 2, &[ValueId(22)]);
    }

    #[test]
    fn keeps_env_captured_self_call() {
        // callee = GetProp($env, "$0.self")：自递归函数捕获自身名字时的真实形状。
        let mut module = Module::new();
        let key = module.add_constant(Constant::String("$0.self".to_string()));
        let undefined = module.add_constant(Constant::Undefined);

        let mut function = Function::new("self", BasicBlockId(0));
        function.set_params(vec![
            "$1.$env".to_string(),
            "$1.$this".to_string(),
            "$1.n".to_string(),
        ]);
        let mut block = BasicBlock::new(BasicBlockId(0));
        block.push_instruction(Instruction::LoadVar {
            dest: ValueId(0),
            name: "$env".to_string(),
        });
        block.push_instruction(Instruction::Const {
            dest: ValueId(1),
            constant: key,
        });
        block.push_instruction(Instruction::GetProp {
            dest: ValueId(2),
            object: ValueId(0),
            key: ValueId(1),
        });
        block.push_instruction(Instruction::Const {
            dest: ValueId(3),
            constant: undefined,
        });
        block.push_instruction(Instruction::LoadVar {
            dest: ValueId(4),
            name: "$1.n".to_string(),
        });
        block.push_instruction(Instruction::Call {
            dest: Some(ValueId(5)),
            callee: ValueId(2),
            this_val: ValueId(3),
            args: vec![ValueId(4)],
        });
        block.set_terminator(Terminator::Return {
            value: Some(ValueId(5)),
        });
        function.push_block(block);
        // 声明侧：`$0.self` 只被 store 一次，且登记为 known callee → 不可变绑定。
        module.push_function(function);

        let mut caller = Function::new("$module_main", BasicBlockId(0));
        let mut caller_entry = BasicBlock::new(BasicBlockId(0));
        let self_ref = module.add_constant(Constant::FunctionRef(FunctionId(0)));
        caller_entry.push_instruction(Instruction::Const {
            dest: ValueId(0),
            constant: self_ref,
        });
        caller_entry.push_instruction(Instruction::StoreVar {
            name: "$0.self".to_string(),
            value: ValueId(0),
        });
        caller_entry.set_terminator(Terminator::Return { value: None });
        caller.push_block(caller_entry);
        caller.record_known_callee("$0.self".to_string(), FunctionId(0));
        module.push_function(caller);

        run(&mut module);
        assert_rewritten(&module, 0, &[ValueId(4)]);
        assert!(
            !module.functions()[0].blocks()[0]
                .instructions()
                .iter()
                .any(|instruction| matches!(instruction, Instruction::GetProp { .. })),
            "改写后 callee 的 env 读取应一并删除，避免每轮回边多一次属性读"
        );
    }

    /// pass 不改写时，`Call` 与 `Return` 都应原样保留。
    fn assert_untouched(module: &Module, block: usize) {
        let block = &module.functions()[0].blocks()[block];
        assert!(
            block
                .instructions()
                .iter()
                .any(|instruction| matches!(instruction, Instruction::Call { .. })),
            "不满足条件时尾调用必须保留：{block:?}"
        );
        assert!(matches!(block.terminator(), Terminator::Return { .. }));
    }

    #[test]
    fn skips_call_to_other_function() {
        let mut module = self_recursive_module(|module, function| {
            let other = module.add_constant(Constant::FunctionRef(FunctionId(1)));
            let mut block = tail_call_block(module, 1);
            block.instructions_mut()[0] = Instruction::Const {
                dest: ValueId(0),
                constant: other,
            };
            function.push_block(block);
        });
        module.push_function(Function::new("other", BasicBlockId(0)));
        run(&mut module);
        assert_untouched(&module, 0);
    }

    #[test]
    fn skips_function_reading_this() {
        let mut module = self_recursive_module(|module, function| {
            let mut block = tail_call_block(module, 1);
            block.instructions_mut().insert(
                0,
                Instruction::LoadVar {
                    dest: ValueId(9),
                    name: "$1.$this".to_string(),
                },
            );
            function.push_block(block);
        });
        run(&mut module);
        assert_untouched(&module, 0);
    }

    #[test]
    fn skips_function_collecting_rest_args() {
        let mut module = self_recursive_module(|module, function| {
            let mut block = tail_call_block(module, 1);
            block.instructions_mut().insert(
                0,
                Instruction::CollectRestArgs {
                    dest: ValueId(9),
                    skip: 0,
                },
            );
            function.push_block(block);
        });
        run(&mut module);
        assert_untouched(&module, 0);
    }

    #[test]
    fn skips_arity_mismatch() {
        let mut module = self_recursive_module(|module, function| {
            let block = tail_call_block(module, 2);
            function.push_block(block);
        });
        run(&mut module);
        assert_untouched(&module, 0);
    }

    #[test]
    fn skips_call_that_is_not_last_instruction() {
        let mut module = self_recursive_module(|module, function| {
            let mut block = tail_call_block(module, 1);
            let extra = module.add_constant(Constant::Undefined);
            block.push_instruction(Instruction::Const {
                dest: ValueId(9),
                constant: extra,
            });
            function.push_block(block);
        });
        run(&mut module);
        assert_untouched(&module, 0);
    }

    #[test]
    fn skips_return_of_other_value() {
        let mut module = self_recursive_module(|module, function| {
            let mut block = tail_call_block(module, 1);
            block.set_terminator(Terminator::Return {
                value: Some(ValueId(2)),
            });
            function.push_block(block);
        });
        run(&mut module);
        assert_untouched(&module, 0);
    }

    #[test]
    fn skips_function_with_suspend() {
        let mut module = self_recursive_module(|module, function| {
            let mut block = tail_call_block(module, 1);
            block.instructions_mut().insert(
                0,
                Instruction::Suspend {
                    promise: ValueId(1),
                    state: 1,
                },
            );
            function.push_block(block);
        });
        run(&mut module);
        assert_untouched(&module, 0);
    }

    #[test]
    fn skips_param_that_is_not_frame_local() {
        // `$0.*` 是模块级共享槽，永远不是栈帧局部。
        let mut module = Module::new();
        let mut function = Function::new("self", BasicBlockId(0));
        function.set_params(vec![
            "$1.$env".to_string(),
            "$1.$this".to_string(),
            "$0.n".to_string(),
        ]);
        let mut block = tail_call_block(&mut module, 1);
        block.instructions_mut()[2] = Instruction::LoadVar {
            dest: ValueId(2),
            name: "$0.n".to_string(),
        };
        function.push_block(block);
        module.push_function(function);
        run(&mut module);
        assert_untouched(&module, 0);
    }

    #[test]
    fn skips_async_function_body() {
        let mut module = Module::new();
        let mut function = Function::new("self$async", BasicBlockId(0));
        function.set_params(vec![
            "$1.$env".to_string(),
            "$1.$this".to_string(),
            "$1.n".to_string(),
        ]);
        let block = tail_call_block(&mut module, 1);
        function.push_block(block);
        module.push_function(function);
        run(&mut module);
        assert_untouched(&module, 0);
    }
}
