use std::collections::{HashMap, HashSet};

use wjsm_ir::variable_ssa::{VarDef, VariableSsa};
use wjsm_ir::{
    Builtin, Constant, Function, FunctionId, Instruction, Program, Terminator, UnaryOp, ValueId,
};
use wjsm_native_abi::NativeHostSymbol;

use crate::call_graph::collect_direct_calls;

pub(crate) fn infer_f64_values(program: &Program) -> HashMap<FunctionId, HashSet<ValueId>> {
    infer_f64_values_with_param_seeds(program, &HashMap::new())
}

/// 以「调用方保证第 i 个 JS 参数是 number」的种子运行同一套 fixpoint 分析。
///
/// 种子只把更多参数标记为 f64 起点，不改变传播规则；用于运行时特化编译时，
/// 让 wrapper 的入口 tag 守卫背书下的参数衍生值获得 f64 证明。base 编译
/// （无种子）与 overlay 编译（有种子）共享同一分析实现。
pub(crate) fn infer_f64_values_with_param_seeds(
    program: &Program,
    seeds: &HashMap<FunctionId, Vec<bool>>,
) -> HashMap<FunctionId, HashSet<ValueId>> {
    let call_info = collect_direct_calls(program);
    let function_count = program.functions().len();
    let frame_locals = program.frame_local_variable_names_by_function();
    // SSA 视图只依赖 CFG 与变量名，与种子无关，故在不动点之外构造一次。
    let variable_ssa: Vec<VariableSsa<'_>> = program
        .functions()
        .iter()
        .zip(&frame_locals)
        .map(|(function, names)| VariableSsa::build(function, names))
        .collect();
    let mut f64_params: Vec<Vec<bool>> = program
        .functions()
        .iter()
        .enumerate()
        .map(|(index, function)| {
            let empty = vec![false; function.params().len().saturating_sub(2)];
            let function_id = FunctionId(u32::try_from(index).expect("function index fits u32"));
            match seeds.get(&function_id) {
                Some(seed) => seed.clone(),
                None => empty,
            }
        })
        .collect();
    let mut return_f64 = vec![false; function_count];
    let mut inferred = vec![HashSet::new(); function_count];

    loop {
        let next_inferred: Vec<HashSet<ValueId>> = program
            .functions()
            .iter()
            .enumerate()
            .map(|(index, function)| {
                analyze_function(
                    program,
                    function,
                    &variable_ssa[index],
                    &frame_locals[index],
                    &f64_params[index],
                    &call_info.targets_by_callee_value[index],
                    &return_f64,
                )
            })
            .collect();
        let next_return_f64: Vec<bool> = program
            .functions()
            .iter()
            .enumerate()
            .map(|(index, function)| function_returns_f64(function, &next_inferred[index]))
            .collect();
        let mut next_params = f64_params.clone();
        for (target, calls) in call_info.calls_by_target.iter().enumerate() {
            if call_info.escaped_target[target] || calls.is_empty() {
                continue;
            }
            for parameter in 0..next_params[target].len() {
                if next_params[target][parameter]
                    || calls.iter().all(|call| {
                        call.args
                            .get(parameter)
                            .is_some_and(|value| next_inferred[call.caller].contains(value))
                    })
                {
                    next_params[target][parameter] = true;
                }
            }
        }

        let stable =
            next_inferred == inferred && next_return_f64 == return_f64 && next_params == f64_params;
        inferred = next_inferred;
        return_f64 = next_return_f64;
        f64_params = next_params;
        if stable {
            break;
        }
    }

    inferred
        .into_iter()
        .enumerate()
        .map(|(index, values)| (FunctionId(index as u32), values))
        .collect()
}

/// 一条指令产出值的 f64 资格：形状可否产出 f64，以及其操作数条件是否成立。
struct F64Rule {
    dest: ValueId,
    holds: bool,
}

fn analyze_function(
    program: &Program,
    function: &Function,
    variables: &VariableSsa<'_>,
    frame_locals: &std::collections::BTreeSet<&str>,
    f64_params: &[bool],
    direct_targets: &HashMap<ValueId, FunctionId>,
    return_f64: &[bool],
) -> HashSet<ValueId> {
    let parameter_names: HashSet<&str> = function
        .params()
        .iter()
        .skip(2)
        .zip(f64_params)
        .filter_map(|(name, is_f64)| is_f64.then_some(name.as_str()))
        .collect();
    // 非帧局部（宿主共享槽 / 捕获名）没有 SSA 视图，只能沿用「本函数未写过的
    // 已证明参数」这条保守规则。
    let modified_parameters: HashSet<&str> = function
        .blocks()
        .iter()
        .flat_map(|block| block.instructions())
        .filter_map(|instruction| match instruction {
            Instruction::StoreVar { name, .. } if parameter_names.contains(name.as_str()) => {
                Some(name.as_str())
            }
            _ => None,
        })
        .collect();

    // 最大不动点（乐观求解）：先把所有「形状上可能产出 f64」的值与 φ 全部纳入，
    // 再迭代剔除操作数条件不成立者。
    //
    // 必须乐观而非悲观：循环携带的变量满足 `i = φ(0, i + 1)`，自依赖使得从空集
    // 出发的最小不动点永远无法建立 φ，整条链会退回 boxed。
    let mut f64_values: HashSet<ValueId> = HashSet::new();
    for block in function.blocks() {
        for instruction in block.instructions() {
            if let Some(rule) = f64_rule(
                program,
                instruction,
                variables,
                frame_locals,
                &parameter_names,
                &modified_parameters,
                direct_targets,
                return_f64,
                &f64_values,
                &HashSet::new(),
                true,
            ) {
                f64_values.insert(rule.dest);
            }
        }
    }
    let mut f64_phis: HashSet<u32> =
        (0..u32::try_from(variables.phi_count()).expect("phi 数在 u32 内")).collect();

    let mut changed = true;
    while changed {
        changed = false;
        // φ：任一输入不可证为 f64 即剔除。空输入（不可达合流）也不成立。
        let doomed: Vec<u32> = f64_phis
            .iter()
            .copied()
            .filter(|phi| {
                let sources = variables.phi_sources(*phi);
                let entry_is_f64 = variables
                    .phi_variable(*phi)
                    .is_some_and(|name| parameter_names.contains(name));
                sources.is_empty()
                    || !sources.iter().all(|source| {
                        definition_is_f64(*source, &f64_values, &f64_phis, entry_is_f64)
                    })
            })
            .collect();
        for phi in doomed {
            f64_phis.remove(&phi);
            changed = true;
        }

        for block in function.blocks() {
            for instruction in block.instructions() {
                let Some(rule) = f64_rule(
                    program,
                    instruction,
                    variables,
                    frame_locals,
                    &parameter_names,
                    &modified_parameters,
                    direct_targets,
                    return_f64,
                    &f64_values,
                    &f64_phis,
                    false,
                ) else {
                    continue;
                };
                if !rule.holds && f64_values.remove(&rule.dest) {
                    changed = true;
                }
            }
        }
    }
    f64_values
}

/// 判定一条指令的产出值是否有 f64 资格。
///
/// `optimistic` 为真时（初始化阶段）跳过操作数检查，只按指令形状纳入候选；
/// 为假时（剔除阶段）按当前集合求解操作数条件。形状本身不可能产出 f64 的指令
/// 一律返回 `None`，其产出值因此永不进入结果集。
#[allow(clippy::too_many_arguments)]
fn f64_rule(
    program: &Program,
    instruction: &Instruction,
    variables: &VariableSsa<'_>,
    frame_locals: &std::collections::BTreeSet<&str>,
    parameter_names: &HashSet<&str>,
    modified_parameters: &HashSet<&str>,
    direct_targets: &HashMap<ValueId, FunctionId>,
    return_f64: &[bool],
    f64_values: &HashSet<ValueId>,
    f64_phis: &HashSet<u32>,
    optimistic: bool,
) -> Option<F64Rule> {
    match instruction {
        Instruction::Const { dest, constant } => matches!(
            program.constants().get(constant.0 as usize),
            Some(Constant::Number(_))
        )
        .then_some(F64Rule {
            dest: *dest,
            holds: true,
        }),
        Instruction::LoadVar { dest, name } => {
            let holds = match variables.load_definition(*dest) {
                Some(definition) => {
                    optimistic
                        || definition_is_f64(
                            definition,
                            f64_values,
                            f64_phis,
                            parameter_names.contains(name.as_str()),
                        )
                }
                // SSA 未覆盖：只有非帧局部会走到这里，条件与集合无关，乐观阶段
                // 也照常求解，避免把宿主共享槽误纳入候选。
                None => {
                    !frame_locals.contains(name.as_str())
                        && parameter_names.contains(name.as_str())
                        && !modified_parameters.contains(name.as_str())
                }
            };
            Some(F64Rule { dest: *dest, holds })
        }
        Instruction::Binary { dest, lhs, rhs, .. } => Some(F64Rule {
            dest: *dest,
            holds: every_f64([*lhs, *rhs], f64_values, optimistic),
        }),
        Instruction::Unary {
            dest,
            value,
            op: UnaryOp::Neg | UnaryOp::Pos,
        } => Some(F64Rule {
            dest: *dest,
            holds: every_f64([*value], f64_values, optimistic),
        }),
        Instruction::Call {
            dest: Some(dest),
            callee,
            ..
        } => direct_targets
            .get(callee)
            .is_some_and(|target| return_f64.get(target.0 as usize).copied().unwrap_or(false))
            .then_some(F64Rule {
                dest: *dest,
                holds: true,
            }),
        Instruction::CallBuiltin {
            dest: Some(dest),
            builtin:
                Builtin::MathAbs
                | Builtin::MathSqrt
                | Builtin::MathCeil
                | Builtin::MathFloor
                | Builtin::MathTrunc
                | Builtin::MathFround,
            args,
        } => (args.len() == 1).then(|| F64Rule {
            dest: *dest,
            holds: every_f64(args.iter().copied(), f64_values, optimistic),
        }),
        Instruction::CallBuiltin {
            dest: Some(dest),
            builtin,
            args,
        } => NativeHostSymbol::for_builtin(*builtin)
            .filter(|symbol| args.len() == usize::from(symbol.signature().argument_count()))
            .map(|_| F64Rule {
                dest: *dest,
                holds: every_f64(args.iter().copied(), f64_values, optimistic),
            }),
        Instruction::Phi { dest, sources } => Some(F64Rule {
            dest: *dest,
            holds: !sources.is_empty()
                && every_f64(
                    sources.iter().map(|source| source.value),
                    f64_values,
                    optimistic,
                ),
        }),
        _ => None,
    }
}

/// 一组操作数是否全部可证为 f64；乐观阶段跳过检查。
fn every_f64(
    values: impl IntoIterator<Item = ValueId>,
    f64_values: &HashSet<ValueId>,
    optimistic: bool,
) -> bool {
    optimistic || values.into_iter().all(|value| f64_values.contains(&value))
}

/// 一个到达定义是否可证为 f64。`entry_is_f64` 表示该变量的入口定义
/// （非参数局部为 `undefined`，参数为入参初值）是否已被证明。
fn definition_is_f64(
    definition: VarDef,
    f64_values: &HashSet<ValueId>,
    f64_phis: &HashSet<u32>,
    entry_is_f64: bool,
) -> bool {
    match definition {
        VarDef::Value(value) => f64_values.contains(&value),
        VarDef::Phi(phi) => f64_phis.contains(&phi),
        VarDef::Entry => entry_is_f64,
    }
}

fn function_returns_f64(function: &Function, values: &HashSet<ValueId>) -> bool {
    let mut saw_return = false;
    for block in function.blocks() {
        match block.terminator() {
            Terminator::Return { value: Some(value) } => {
                saw_return = true;
                if !values.contains(value) {
                    return false;
                }
            }
            Terminator::Return { value: None } | Terminator::Throw { .. } => return false,
            _ => {}
        }
    }
    saw_return
}
