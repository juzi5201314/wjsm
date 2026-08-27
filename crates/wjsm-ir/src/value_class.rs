//! Number / Int32 值类与可抛性的函数内乐观不动点。
//!
//! 与后端历史 `f64_analysis` 同一循环 φ 思想：先把形状上可能产出 Number 的
//! 值全部纳入，再剔除操作数条件不成立者。种子来自常量、参数承诺或 overlay
//! 反馈；未证明的 `Binary`（含 bigint 混合）保持未知，因而 `is_exception` 不折叠。

use std::collections::{BTreeSet, HashMap, HashSet};

use crate::variable_ssa::{VarDef, VariableSsa};
use crate::{BinaryOp, Constant, Function, Instruction, Program, UnaryOp, ValueId};

/// 值类 SSA 可见变量：帧局部 + 本函数内 load/store 都出现的 `$0.*` 模块绑定。
///
/// host 槽提升仍只用 [`Program::frame_local_variable_names`]；此处额外纳入
/// `$0.i` 这类模块级循环计数器，供 VariableSsa 做到达定义分析。
fn ssa_variable_names<'a>(
    function: &'a Function,
    frame_locals: &BTreeSet<&'a str>,
) -> BTreeSet<&'a str> {
    let mut names = frame_locals.clone();
    let captured: BTreeSet<&str> = function.captured_names.iter().map(String::as_str).collect();
    let mut loaded = BTreeSet::new();
    let mut stored = BTreeSet::new();
    for block in function.blocks() {
        for instruction in block.instructions() {
            match instruction {
                Instruction::LoadVar { name, .. } => {
                    if name.starts_with("$0.")
                        && !captured.contains(name.as_str())
                        && !matches!(name.as_str(), "$0.$global" | "$0.$shared_env")
                    {
                        loaded.insert(name.as_str());
                    }
                }
                Instruction::StoreVar { name, .. } => {
                    if name.starts_with("$0.")
                        && !captured.contains(name.as_str())
                        && !matches!(name.as_str(), "$0.$global" | "$0.$shared_env")
                    {
                        stored.insert(name.as_str());
                    }
                }
                _ => {}
            }
        }
    }
    for name in loaded.intersection(&stored) {
        names.insert(name);
    }
    names
}

/// 单函数值类分析结果。
#[derive(Clone, Debug, Default)]
pub struct ValueClassSet {
    pub numbers: HashSet<ValueId>,
    pub int32s: HashSet<ValueId>,
}

impl ValueClassSet {
    /// 该 SSA 值的 def 不可能产出 `TAG_EXCEPTION`。
    pub fn cannot_throw(&self, value: ValueId) -> bool {
        self.numbers.contains(&value) || self.int32s.contains(&value)
    }
}

/// 覆盖参数与额外 SSA 种子（overlay 把稳定 Number 运算的操作数标进来）。
#[derive(Clone, Debug, Default)]
pub struct FunctionSeeds {
    pub param_is_number: Vec<bool>,
    pub extra_numbers: HashSet<ValueId>,
}

/// 对单个函数求解值类。`frame_locals` 应使用模块级可提升名。
pub fn infer_function(
    program: &Program,
    function: &Function,
    frame_locals: &BTreeSet<&str>,
    seeds: &FunctionSeeds,
) -> ValueClassSet {
    let ssa_names = ssa_variable_names(function, frame_locals);
    let variables = VariableSsa::build(function, &ssa_names);
    let js_params = function.params().len().saturating_sub(2);
    let mut param_flags = vec![false; js_params];
    for (index, flag) in seeds.param_is_number.iter().copied().enumerate() {
        if let Some(slot) = param_flags.get_mut(index) {
            *slot = flag;
        }
    }
    analyze(
        program,
        function,
        &variables,
        &ssa_names,
        &param_flags,
        &seeds.extra_numbers,
    )
}

/// 无种子的整包分析（AOT）。
pub fn infer_program(program: &Program) -> HashMap<u32, ValueClassSet> {
    infer_program_with_seeds(program, &HashMap::new())
}

/// 带每函数种子的整包分析。
pub fn infer_program_with_seeds(
    program: &Program,
    seeds: &HashMap<u32, FunctionSeeds>,
) -> HashMap<u32, ValueClassSet> {
    let frame_locals = program.frame_local_variable_names_by_function();
    program
        .functions()
        .iter()
        .enumerate()
        .map(|(index, function)| {
            let names = frame_locals.get(index).cloned().unwrap_or_default();
            let empty = FunctionSeeds::default();
            let function_seeds = seeds.get(&(index as u32)).unwrap_or(&empty);
            (
                index as u32,
                infer_function(program, function, &names, function_seeds),
            )
        })
        .collect()
}

fn analyze(
    program: &Program,
    function: &Function,
    variables: &VariableSsa<'_>,
    ssa_names: &BTreeSet<&str>,
    f64_params: &[bool],
    extra_numbers: &HashSet<ValueId>,
) -> ValueClassSet {
    let parameter_names: HashSet<&str> = function
        .params()
        .iter()
        .skip(2)
        .zip(f64_params)
        .filter_map(|(name, is_f64)| is_f64.then_some(name.as_str()))
        .collect();
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

    let mut numbers: HashSet<ValueId> = extra_numbers.clone();
    for block in function.blocks() {
        for instruction in block.instructions() {
            if let Some(dest) = number_dest_shape(program, instruction) {
                numbers.insert(dest);
            }
        }
    }
    let mut number_phis: HashSet<u32> =
        (0..u32::try_from(variables.phi_count()).expect("phi 数在 u32 内")).collect();

    let mut changed = true;
    while changed {
        changed = false;
        let doomed: Vec<u32> = number_phis
            .iter()
            .copied()
            .filter(|phi| {
                let sources = variables.phi_sources(*phi);
                let entry_is_number = variables
                    .phi_variable(*phi)
                    .is_some_and(|name| parameter_names.contains(name));
                sources.is_empty()
                    || !sources.iter().all(|source| {
                        definition_is_number(*source, &numbers, &number_phis, entry_is_number)
                    })
            })
            .collect();
        for phi in doomed {
            number_phis.remove(&phi);
            changed = true;
        }
        for block in function.blocks() {
            for instruction in block.instructions() {
                let Some(dest) = number_dest_shape(program, instruction) else {
                    continue;
                };
                let holds = number_holds(
                    instruction,
                    variables,
                    &parameter_names,
                    &modified_parameters,
                    extra_numbers,
                    &numbers,
                    &number_phis,
                );
                if !holds && numbers.remove(&dest) {
                    changed = true;
                }
            }
        }
    }

    let int32s = infer_int32(program, function, ssa_names, &numbers);
    ValueClassSet { numbers, int32s }
}

fn number_dest_shape(program: &Program, instruction: &Instruction) -> Option<ValueId> {
    match instruction {
        Instruction::Const { dest, constant } => matches!(
            program.constants().get(constant.0 as usize),
            Some(Constant::Number(_))
        )
        .then_some(*dest),
        Instruction::LoadVar { dest, .. }
        | Instruction::Phi { dest, .. }
        | Instruction::Unary {
            dest,
            op: UnaryOp::Neg | UnaryOp::Pos,
            ..
        } => Some(*dest),
        Instruction::Binary { dest, op, .. } if number_binary(*op) => Some(*dest),
        _ => None,
    }
}

fn number_binary(op: BinaryOp) -> bool {
    matches!(
        op,
        BinaryOp::Add
            | BinaryOp::Sub
            | BinaryOp::Mul
            | BinaryOp::Div
            | BinaryOp::Mod
            | BinaryOp::Exp
            | BinaryOp::BitAnd
            | BinaryOp::BitOr
            | BinaryOp::BitXor
            | BinaryOp::Shl
            | BinaryOp::Shr
            | BinaryOp::UShr
    )
}

fn number_holds(
    instruction: &Instruction,
    variables: &VariableSsa<'_>,
    parameter_names: &HashSet<&str>,
    modified_parameters: &HashSet<&str>,
    extra_numbers: &HashSet<ValueId>,
    numbers: &HashSet<ValueId>,
    number_phis: &HashSet<u32>,
) -> bool {
    match instruction {
        Instruction::Const { .. } => true,
        Instruction::LoadVar { dest, name } => {
            if extra_numbers.contains(dest) {
                return true;
            }
            match variables.load_definition(*dest) {
                Some(definition) => definition_is_number(
                    definition,
                    numbers,
                    number_phis,
                    parameter_names.contains(name.as_str()),
                ),
                None => {
                    // 非帧槽（含 `$sroa.*`）没有函数内 SSA 到达定义：入口
                    // LoadVar 可能读的是别的函数写来的值，不能用「本函数后
                    // 续 StoreVar 都是 Number」证明入口已经是 Number。
                    parameter_names.contains(name.as_str())
                        && !modified_parameters.contains(name.as_str())
                }
            }
        }
        Instruction::Binary { lhs, rhs, .. } => {
            (numbers.contains(lhs) && numbers.contains(rhs))
                || (extra_numbers.contains(lhs) && extra_numbers.contains(rhs))
        }
        Instruction::Unary { value, .. } => {
            extra_numbers.contains(value) || numbers.contains(value)
        }
        Instruction::Phi { dest, sources } => {
            extra_numbers.contains(dest)
                || (!sources.is_empty()
                    && sources.iter().all(|source| numbers.contains(&source.value)))
        }
        _ => false,
    }
}

fn number_var_names(function: &Function, numbers: &HashSet<ValueId>) -> HashSet<String> {
    let mut stores: HashMap<String, Vec<ValueId>> = HashMap::new();
    for block in function.blocks() {
        for instruction in block.instructions() {
            if let Instruction::StoreVar { name, value } = instruction {
                stores.entry(name.clone()).or_default().push(*value);
            }
        }
    }
    stores
        .into_iter()
        .filter_map(|(name, values)| {
            (!values.is_empty() && values.iter().all(|value| numbers.contains(value)))
                .then_some(name)
        })
        .collect()
}

fn definition_is_number(
    definition: VarDef,
    numbers: &HashSet<ValueId>,
    number_phis: &HashSet<u32>,
    entry_is_number: bool,
) -> bool {
    match definition {
        VarDef::Value(value) => numbers.contains(&value),
        VarDef::Phi(phi) => number_phis.contains(&phi),
        VarDef::Entry => entry_is_number,
    }
}

fn infer_int32(
    program: &Program,
    function: &Function,
    ssa_names: &BTreeSet<&str>,
    numbers: &HashSet<ValueId>,
) -> HashSet<ValueId> {
    let mut int32s = HashSet::new();
    for block in function.blocks() {
        for instruction in block.instructions() {
            if let Instruction::Const { dest, constant } = instruction
                && numbers.contains(dest)
                && matches!(
                    program.constants().get(constant.0 as usize),
                    Some(Constant::Number(value)) if is_int32_number(*value)
                )
            {
                int32s.insert(*dest);
            }
        }
    }
    let mut name_int32s = number_var_names(function, &int32s);
    let mut changed = true;
    while changed {
        changed = false;
        for block in function.blocks() {
            for instruction in block.instructions() {
                let dest = match instruction {
                    Instruction::LoadVar { dest, name }
                        if numbers.contains(dest)
                            && ssa_names.contains(name.as_str())
                            && name_int32s.contains(name) =>
                    {
                        *dest
                    }
                    Instruction::Binary {
                        dest,
                        op: BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul,
                        lhs,
                        rhs,
                    } if numbers.contains(dest) && int32s.contains(lhs) && int32s.contains(rhs) => {
                        *dest
                    }
                    Instruction::Unary {
                        dest,
                        value,
                        op: UnaryOp::Neg | UnaryOp::Pos,
                    } if numbers.contains(dest) && int32s.contains(value) => *dest,
                    Instruction::Phi { dest, sources }
                        if numbers.contains(dest)
                            && !sources.is_empty()
                            && sources.iter().all(|source| int32s.contains(&source.value)) =>
                    {
                        *dest
                    }
                    _ => continue,
                };
                if int32s.insert(dest) {
                    changed = true;
                }
            }
        }
        let next_names = number_var_names(function, &int32s);
        if next_names != name_int32s {
            name_int32s = next_names;
            changed = true;
        }
    }
    int32s
}

/// 有限、非 `-0`、等于某个 i32 的 IEEE 值。
pub fn is_int32_number(value: f64) -> bool {
    if !value.is_finite() || value == -0.0 && value.is_sign_negative() {
        return false;
    }
    let truncated = value as i32;
    f64::from(truncated) == value
}

#[cfg(test)]
mod tests {
    use super::{FunctionSeeds, infer_function, is_int32_number};
    use crate::{
        BasicBlock, BasicBlockId, Constant, Function, Instruction, Program, Terminator, ValueId,
    };
    use std::collections::BTreeSet;

    #[test]
    fn int32_number_rejects_fraction_and_neg_zero() {
        assert!(is_int32_number(0.0));
        assert!(is_int32_number(128.0));
        assert!(is_int32_number(-1.0));
        assert!(!is_int32_number(1.5));
        assert!(!is_int32_number(f64::NAN));
        assert!(!is_int32_number(-0.0));
    }

    #[test]
    fn shared_slot_entry_load_is_not_number_from_later_stores() {
        let mut program = Program::new();
        let zero = program.add_constant(Constant::Number(0.0));
        let mut function = Function::new("arrow", BasicBlockId(0));
        let mut block = BasicBlock::new(BasicBlockId(0));
        block.push_instruction(Instruction::LoadVar {
            dest: ValueId(0),
            name: "$sroa.$0.state.count".into(),
        });
        block.push_instruction(Instruction::Const {
            dest: ValueId(1),
            constant: zero,
        });
        block.push_instruction(Instruction::StoreVar {
            name: "$sroa.$0.state.count".into(),
            value: ValueId(1),
        });
        block.set_terminator(Terminator::Return {
            value: Some(ValueId(0)),
        });
        function.push_block(block);
        let classes = infer_function(
            &program,
            &function,
            &BTreeSet::new(),
            &FunctionSeeds::default(),
        );
        assert!(
            !classes.numbers.contains(&ValueId(0)),
            "entry load of a shared slot must not inherit later Number stores"
        );
        assert!(classes.numbers.contains(&ValueId(1)));
    }
}
