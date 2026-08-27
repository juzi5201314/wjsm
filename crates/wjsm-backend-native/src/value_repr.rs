//! 已证明 f64 值的机器表示规划与跨表示转换。
//!
//! 循环体里的纯数值归纳变量若每条指令后都做一次 NaN-Box 打标
//! （[`box_f64_result`]）、下一条指令再 `bitcast` 拆包，寄存器分配就被迫在
//! 通用寄存器与浮点寄存器之间来回搬运。本模块把这类值的 Cranelift `Variable`
//! 直接声明成 [`types::F64`]，只在跨表示边界插入转换：进入循环前拆一次包，
//! 逃逸出循环时打一次标。循环携带的归纳变量因此整轮迭代常驻浮点寄存器。
//!
//! 表示约定：
//!
//! - typed 变量（`types::F64`）持有**原始机器浮点位**，逐条指令不做 NaN 规范化。
//! - boxed 变量（`types::I64`）持有 NaN-Box 编码值，NaN 必须是 `value::encode_f64`
//!   的规范形态，否则会与 `BOX_BASE` 前缀撞车被误判成句柄。
//! - 因此 typed → boxed 的每一次转换都必须规范化 NaN（[`use_value_boxed`]），
//!   boxed → typed 只需 `bitcast`（位模式本就是 double）。

use std::collections::{BTreeSet, HashMap, HashSet};

use anyhow::{Context, Result};
use cranelift_codegen::ir::{self, InstBuilder, types};
use cranelift_frontend::{FunctionBuilder, Variable};
use wjsm_ir::{BinaryOp, Builtin, Instruction, UnaryOp, ValueId, value};
use wjsm_native_abi::NativeHostSymbol;

/// SSA 值与帧局部的机器表示计划，兼作「ValueId → Cranelift 变量」索引。
pub(crate) struct ValueRepr {
    variables: HashMap<ValueId, Variable>,
    typed_f64_ssa: HashSet<ValueId>,
    typed_f64_locals: HashSet<String>,
}

impl ValueRepr {
    /// 规划哪些已证明 f64 的 SSA 值与帧局部可以常驻浮点寄存器。
    ///
    /// `f64_values` 必须是**可靠**证明集合（不含运行时反馈推测出来的 number），
    /// typed 变量没有守卫，位模式一旦不是 double 就会被后续规范化改写。
    ///
    /// 在此之上还要求「值确实由浮点指令产出」：`Call` 与 dispatcher 返回值在 ABI
    /// 上可能是 exception 编码（一个 NaN-Box 值），把它搬进浮点寄存器后逃逸点的
    /// NaN 规范化会将其改写成 `NaN`，随后的 `IsException` 就再也识别不出来。
    /// 因此只有 f64 分析证明、**并且**定义形态是浮点产出或 typed 之间搬运的值才提升。
    ///
    /// 帧局部的资格比 SSA 值再严格一层，见 [`candidate_locals`]：要求它被读过、
    /// 被写过、**全部 load 都已证明 f64**，且**全部 store 的源值本身 typed**。
    ///
    /// 两个集合互相依赖（`i = φ(0, i + 1)` 经帧局部往返），故取最大不动点：先全部
    /// 纳入候选，再迭代剔除条件不成立者。最小不动点会因自依赖把整条循环链剔光。
    pub(crate) fn plan(
        function: &wjsm_ir::Function,
        f64_values: &HashSet<ValueId>,
        frame_local_names: &BTreeSet<&str>,
        has_suspend: bool,
    ) -> Self {
        let mut repr = Self {
            variables: HashMap::new(),
            typed_f64_ssa: HashSet::new(),
            typed_f64_locals: HashSet::new(),
        };
        // 挂起函数的活跃值经宿主 continuation 以 boxed 形态往返，恢复路径不在
        // 本模块的转换点覆盖范围内，整体退回 boxed。
        if has_suspend {
            return repr;
        }

        let definitions: Vec<(ValueId, TypedDef<'_>)> = function
            .blocks()
            .iter()
            .flat_map(|block| block.instructions())
            .filter_map(|instruction| classify_definition(instruction, frame_local_names))
            .filter(|(dest, _)| f64_values.contains(dest))
            .collect();
        let mut typed_ssa: HashSet<ValueId> = definitions.iter().map(|(dest, _)| *dest).collect();
        let mut typed_locals = candidate_locals(function, f64_values, frame_local_names);

        let mut changed = true;
        while changed {
            changed = false;
            for (dest, definition) in &definitions {
                if !typed_ssa.contains(dest) {
                    continue;
                }
                let holds = match definition {
                    TypedDef::Producer => true,
                    TypedDef::Copy(sources) => {
                        sources.iter().all(|source| typed_ssa.contains(source))
                    }
                    TypedDef::LoadOf(name) => typed_locals.contains(*name),
                };
                if !holds {
                    typed_ssa.remove(dest);
                    changed = true;
                }
            }
            for block in function.blocks() {
                for instruction in block.instructions() {
                    if let Instruction::StoreVar { name, value } = instruction
                        && !typed_ssa.contains(value)
                        && typed_locals.remove(name.as_str())
                    {
                        changed = true;
                    }
                }
            }
        }

        repr.typed_f64_ssa = typed_ssa;
        repr.typed_f64_locals = typed_locals.into_iter().map(str::to_owned).collect();
        repr
    }

    /// 按计划为每个 SSA 值声明 Cranelift 变量：typed 用 `F64`，其余用 `I64`。
    pub(crate) fn declare_values(
        &mut self,
        builder: &mut FunctionBuilder<'_>,
        values: &HashSet<ValueId>,
    ) {
        self.typed_f64_ssa.retain(|value| values.contains(value));
        for value in values {
            let ty = if self.typed_f64_ssa.contains(value) {
                types::F64
            } else {
                types::I64
            };
            self.variables.insert(*value, builder.declare_var(ty));
        }
    }

    pub(crate) fn is_typed_value(&self, value: ValueId) -> bool {
        self.typed_f64_ssa.contains(&value)
    }

    pub(crate) fn is_typed_local(&self, name: &str) -> bool {
        self.typed_f64_locals.contains(name)
    }

    /// 帧局部变量应声明的机器类型。
    pub(crate) fn local_type(&self, name: &str) -> ir::Type {
        if self.is_typed_local(name) {
            types::F64
        } else {
            types::I64
        }
    }

    fn variable(&self, value: ValueId) -> Result<Variable> {
        self.variables
            .get(&value)
            .copied()
            .with_context(|| format!("value {} has no native variable", value.0))
    }
}

/// 一个已证明 f64 的定义在机器层的产出形态，决定它能否常驻浮点寄存器。
enum TypedDef<'a> {
    /// 定义点直接发浮点指令（`f64const` / `fadd` / `sqrt` / typed math thunk），
    /// 产出必然是货真价实的 double，与操作数当下的表示无关。
    Producer,
    /// 定义点只是搬运，表示随源值而定。
    Copy(Vec<ValueId>),
    /// 从帧局部读出，资格随该局部。
    LoadOf(&'a str),
}

/// 判定一条指令的产出值是否有资格常驻浮点寄存器，并给出它依赖的前提。
///
/// 返回 `None` 表示无条件不合格。关键是 [`Instruction::Call`]：即便 f64 分析
/// 证明被调函数的每条 `Return` 都返回 number，抛出路径仍会按 ABI 返回 exception
/// 编码（一个 NaN-Box 值），而调用点的 `IsException` 是独立指令、不在定义点拦截。
/// 把它搬进浮点寄存器，逃逸时的 NaN 规范化就会将其改写成 `NaN`，异常被静默吞掉。
///
/// 各分支与 [`crate::lower`] 的 lowering arm 一一对应：只有那里真的发浮点指令的
/// 形态才算 `Producer`，落到宿主 dispatcher 的形态（`%`、`**`、位运算等）产出
/// boxed 结果，一律不合格。
fn classify_definition<'a>(
    instruction: &'a Instruction,
    frame_local_names: &BTreeSet<&str>,
) -> Option<(ValueId, TypedDef<'a>)> {
    match instruction {
        // 非 number 常量不会进入 f64 集合，调用方已按 f64 集合过滤。
        Instruction::Const { dest, .. } => Some((*dest, TypedDef::Producer)),
        Instruction::Binary { dest, op, .. }
            if matches!(
                op,
                BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div
            ) =>
        {
            Some((*dest, TypedDef::Producer))
        }
        Instruction::Unary {
            dest,
            op: UnaryOp::Neg,
            ..
        } => Some((*dest, TypedDef::Producer)),
        // 一元 `+` 对已证明 number 是恒等运算，按目标表示原样搬运。
        Instruction::Unary {
            dest,
            op: UnaryOp::Pos,
            value,
        } => Some((*dest, TypedDef::Copy(vec![*value]))),
        Instruction::Phi { dest, sources } if !sources.is_empty() => Some((
            *dest,
            TypedDef::Copy(sources.iter().map(|source| source.value).collect()),
        )),
        // 非帧局部（宿主共享槽）经运行时 `LoadVar` 往返，只有 boxed 表示。
        Instruction::LoadVar { dest, name } => frame_local_names
            .contains(name.as_str())
            .then(|| (*dest, TypedDef::LoadOf(name.as_str()))),
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
        } if args.len() == 1 => Some((*dest, TypedDef::Producer)),
        Instruction::CallBuiltin {
            dest: Some(dest),
            builtin,
            args,
        } if NativeHostSymbol::for_builtin(*builtin).is_some_and(|symbol| {
            args.len() == usize::from(symbol.signature().argument_count())
        }) =>
        {
            Some((*dest, TypedDef::Producer))
        }
        _ => None,
    }
}

/// 帧局部的 typed 候选集：被读过、被写过，且每条 `LoadVar` 的产出都已证明 f64。
///
/// 「全部 load 已证明」是入口初值的安全条件：入口把局部定义成 `undefined`
/// （一个 NaN-Box 值），若它能被某条 load 观察到，规范化就会把 `undefined` 改写
/// 成 `NaN`。该条件等价于 [`wjsm_ir::variable_ssa`] 解出的入口定义到不了任何 load。
///
/// 「被写过」不是收益判断而是 GC 安全条件：[`crate::lower::boxed_frame_local_names`]
/// 只把「全部 store 都写入已证明 f64」的局部排除出 root 槽，零 store 的局部仍会
/// 被钉在 root 槽里，而 typed 局部写回的是原始浮点位——硬件 NaN 的位模式恰好是
/// `BOX_BASE` 前缀，会被 GC 当成句柄扫描。
fn candidate_locals<'a>(
    function: &'a wjsm_ir::Function,
    f64_values: &HashSet<ValueId>,
    frame_local_names: &BTreeSet<&str>,
) -> HashSet<&'a str> {
    let mut loaded: HashSet<&'a str> = HashSet::new();
    let mut stored: HashSet<&'a str> = HashSet::new();
    let mut rejected: HashSet<&'a str> = HashSet::new();
    for block in function.blocks() {
        for instruction in block.instructions() {
            match instruction {
                Instruction::StoreVar { name, .. } => {
                    stored.insert(name.as_str());
                }
                Instruction::LoadVar { dest, name } => {
                    loaded.insert(name.as_str());
                    if !f64_values.contains(dest) {
                        rejected.insert(name.as_str());
                    }
                }
                _ => {}
            }
        }
    }
    loaded
        .into_iter()
        .filter(|name| {
            frame_local_names.contains(*name) && stored.contains(name) && !rejected.contains(name)
        })
        .collect()
}

/// 读出一个 SSA 值的 NaN-Box 编码形态。typed 变量在此规范化 NaN。
pub(crate) fn use_value_boxed(
    builder: &mut FunctionBuilder<'_>,
    repr: &ValueRepr,
    value: ValueId,
) -> Result<ir::Value> {
    let native = builder.use_var(repr.variable(value)?);
    Ok(if repr.is_typed_value(value) {
        box_f64_result(builder, native)
    } else {
        native
    })
}

/// 读出一个 SSA 值的原始浮点形态。boxed 变量在此 `bitcast` 拆包。
pub(crate) fn use_value_f64(
    builder: &mut FunctionBuilder<'_>,
    repr: &ValueRepr,
    value: ValueId,
) -> Result<ir::Value> {
    let native = builder.use_var(repr.variable(value)?);
    Ok(if repr.is_typed_value(value) {
        native
    } else {
        unbox_f64(builder, native)
    })
}

/// 以 NaN-Box 编码形态定义一个 SSA 值。typed 变量在此 `bitcast` 拆包。
pub(crate) fn define_value_boxed(
    builder: &mut FunctionBuilder<'_>,
    repr: &ValueRepr,
    value: ValueId,
    native: ir::Value,
) -> Result<()> {
    let variable = repr.variable(value)?;
    let native = if repr.is_typed_value(value) {
        unbox_f64(builder, native)
    } else {
        native
    };
    builder.def_var(variable, native);
    Ok(())
}

/// 以原始浮点形态定义一个 SSA 值。boxed 变量在此规范化 NaN 后打标。
pub(crate) fn define_value_f64(
    builder: &mut FunctionBuilder<'_>,
    repr: &ValueRepr,
    value: ValueId,
    native: ir::Value,
) -> Result<()> {
    let variable = repr.variable(value)?;
    let native = if repr.is_typed_value(value) {
        native
    } else {
        box_f64_result(builder, native)
    };
    builder.def_var(variable, native);
    Ok(())
}

/// 把 `source` 读成 `dest` 所需的表示，用于 φ 边与 `StoreVar` 的成对转换。
///
/// 两端表示一致时不产出任何转换指令；循环回边上的归纳变量因此保持在浮点寄存器里。
pub(crate) fn use_value_as(
    builder: &mut FunctionBuilder<'_>,
    repr: &ValueRepr,
    dest_is_typed: bool,
    source: ValueId,
) -> Result<ir::Value> {
    if dest_is_typed {
        use_value_f64(builder, repr, source)
    } else {
        use_value_boxed(builder, repr, source)
    }
}

/// [`use_value_as`] 的对偶：`native` 已按 `dest` 的表示物化。
pub(crate) fn define_value_as(
    builder: &mut FunctionBuilder<'_>,
    repr: &ValueRepr,
    dest: ValueId,
    native: ir::Value,
) -> Result<()> {
    if repr.is_typed_value(dest) {
        define_value_f64(builder, repr, dest, native)
    } else {
        define_value_boxed(builder, repr, dest, native)
    }
}

/// NaN-Box 打标：硬件默认 QNaN 的位模式恰好等于 `BOX_BASE`，必须换成规范 NaN。
pub(crate) fn box_f64_result(builder: &mut FunctionBuilder<'_>, result: ir::Value) -> ir::Value {
    let is_nan = builder
        .ins()
        .fcmp(ir::condcodes::FloatCC::Unordered, result, result);
    let bits = builder
        .ins()
        .bitcast(types::I64, ir::MemFlagsData::new(), result);
    let canonical_nan = builder
        .ins()
        .iconst(types::I64, value::encode_f64(f64::NAN));
    builder.ins().select(is_nan, canonical_nan, bits)
}

/// NaN-Box 拆包：已证明 number 的编码值位模式就是 double。
pub(crate) fn unbox_f64(builder: &mut FunctionBuilder<'_>, bits: ir::Value) -> ir::Value {
    builder
        .ins()
        .bitcast(types::F64, ir::MemFlagsData::new(), bits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wjsm_ir::{BasicBlock, BasicBlockId, ConstantId, Terminator};

    /// `let i = 0; while (…) { i = i + 1; }` 的帧局部形状：全部 store/load 都已证明。
    fn numeric_loop() -> wjsm_ir::Function {
        numeric_loop_with_step(Instruction::Binary {
            dest: ValueId(2),
            op: BinaryOp::Add,
            lhs: ValueId(1),
            rhs: ValueId(0),
        })
    }

    /// 同上，但循环体内递增值由 `step` 定义，用于考察不同定义形态的资格。
    fn numeric_loop_with_step(step: Instruction) -> wjsm_ir::Function {
        let mut function = wjsm_ir::Function::new("loop", BasicBlockId(0));
        let mut bb0 = BasicBlock::new(BasicBlockId(0));
        bb0.push_instruction(Instruction::Const {
            dest: ValueId(0),
            constant: ConstantId(0),
        });
        bb0.push_instruction(Instruction::StoreVar {
            name: "$1.i".into(),
            value: ValueId(0),
        });
        bb0.set_terminator(Terminator::Jump {
            target: BasicBlockId(1),
        });
        let mut bb1 = BasicBlock::new(BasicBlockId(1));
        bb1.push_instruction(Instruction::LoadVar {
            dest: ValueId(1),
            name: "$1.i".into(),
        });
        bb1.push_instruction(step);
        bb1.push_instruction(Instruction::StoreVar {
            name: "$1.i".into(),
            value: ValueId(2),
        });
        bb1.set_terminator(Terminator::Return { value: None });
        function.push_block(bb0);
        function.push_block(bb1);
        function
    }

    fn all_values() -> HashSet<ValueId> {
        HashSet::from([ValueId(0), ValueId(1), ValueId(2)])
    }

    #[test]
    fn all_proven_local_becomes_typed() {
        let function = numeric_loop();
        let names = BTreeSet::from(["$1.i"]);
        let repr = ValueRepr::plan(&function, &all_values(), &names, false);
        assert!(repr.is_typed_local("$1.i"));
        assert_eq!(repr.local_type("$1.i"), types::F64);
        assert!(repr.is_typed_value(ValueId(2)));
    }

    /// 调用返回值在 ABI 上可能是 exception 编码，即便 f64 分析证明它是 number
    /// 也不得进浮点寄存器；整条依赖链随之退回 boxed。
    #[test]
    fn call_result_is_never_typed() {
        let function = numeric_loop_with_step(Instruction::Call {
            dest: Some(ValueId(2)),
            callee: ValueId(1),
            this_val: ValueId(1),
            args: Vec::new(),
        });
        let names = BTreeSet::from(["$1.i"]);
        let repr = ValueRepr::plan(&function, &all_values(), &names, false);
        assert!(!repr.is_typed_value(ValueId(2)));
        assert!(!repr.is_typed_local("$1.i"));
        assert!(!repr.is_typed_value(ValueId(1)));
    }

    /// `%` 落到宿主 dispatcher，返回的是 boxed 结果，不是浮点寄存器里的 double。
    #[test]
    fn dispatcher_backed_operator_is_never_typed() {
        let function = numeric_loop_with_step(Instruction::Binary {
            dest: ValueId(2),
            op: BinaryOp::Mod,
            lhs: ValueId(1),
            rhs: ValueId(0),
        });
        let names = BTreeSet::from(["$1.i"]);
        let repr = ValueRepr::plan(&function, &all_values(), &names, false);
        assert!(!repr.is_typed_value(ValueId(2)));
        assert!(!repr.is_typed_local("$1.i"));
    }

    /// typed 常量与浮点算子照常提升，即便所在局部因别的原因退回 boxed。
    #[test]
    fn producer_stays_typed_when_local_is_rejected() {
        let function = numeric_loop_with_step(Instruction::Binary {
            dest: ValueId(2),
            op: BinaryOp::Mod,
            lhs: ValueId(1),
            rhs: ValueId(0),
        });
        let names = BTreeSet::from(["$1.i"]);
        let repr = ValueRepr::plan(&function, &all_values(), &names, false);
        assert!(repr.is_typed_value(ValueId(0)));
    }

    #[test]
    fn unproven_load_keeps_local_boxed() {
        let function = numeric_loop();
        // 少了 ValueId(1)：某条 load 未被证明，入口 undefined 可能被观察到。
        let f64_values = HashSet::from([ValueId(0), ValueId(2)]);
        let names = BTreeSet::from(["$1.i"]);
        let repr = ValueRepr::plan(&function, &f64_values, &names, false);
        assert!(!repr.is_typed_local("$1.i"));
        assert_eq!(repr.local_type("$1.i"), types::I64);
    }

    #[test]
    fn unproven_store_keeps_local_boxed() {
        let function = numeric_loop();
        let f64_values = HashSet::from([ValueId(0), ValueId(1)]);
        let names = BTreeSet::from(["$1.i"]);
        let repr = ValueRepr::plan(&function, &f64_values, &names, false);
        assert!(!repr.is_typed_local("$1.i"));
    }

    /// 只写不读的局部没有收益，且入口初值语义无从验证，保持 boxed。
    #[test]
    fn never_loaded_local_stays_boxed() {
        let mut function = wjsm_ir::Function::new("store_only", BasicBlockId(0));
        let mut block = BasicBlock::new(BasicBlockId(0));
        block.push_instruction(Instruction::StoreVar {
            name: "$1.x".into(),
            value: ValueId(0),
        });
        block.set_terminator(Terminator::Return { value: None });
        function.push_block(block);
        let repr = ValueRepr::plan(
            &function,
            &HashSet::from([ValueId(0)]),
            &BTreeSet::from(["$1.x"]),
            false,
        );
        assert!(!repr.is_typed_local("$1.x"));
    }

    /// 只读不写的局部（如只读参数）仍被钉在 GC root 槽里，提升会把原始浮点位
    /// 写进 root 槽，保持 boxed。
    #[test]
    fn never_stored_local_stays_boxed() {
        let mut function = wjsm_ir::Function::new("load_only", BasicBlockId(0));
        let mut block = BasicBlock::new(BasicBlockId(0));
        block.push_instruction(Instruction::LoadVar {
            dest: ValueId(0),
            name: "$1.x".into(),
        });
        block.set_terminator(Terminator::Return { value: None });
        function.push_block(block);
        let repr = ValueRepr::plan(
            &function,
            &HashSet::from([ValueId(0)]),
            &BTreeSet::from(["$1.x"]),
            false,
        );
        assert!(!repr.is_typed_local("$1.x"));
        assert!(!repr.is_typed_value(ValueId(0)));
    }

    /// 宿主共享槽不在帧局部集合里，即使全部 load/store 已证明也不得提升。
    #[test]
    fn non_frame_local_is_never_typed() {
        let function = numeric_loop();
        let f64_values = HashSet::from([ValueId(0), ValueId(1), ValueId(2)]);
        let repr = ValueRepr::plan(&function, &f64_values, &BTreeSet::new(), false);
        assert!(!repr.is_typed_local("$1.i"));
    }

    /// 挂起函数整体退回 boxed：恢复路径不经本模块的转换点。
    #[test]
    fn suspending_function_disables_typed_representation() {
        let function = numeric_loop();
        let f64_values = HashSet::from([ValueId(0), ValueId(1), ValueId(2)]);
        let names = BTreeSet::from(["$1.i"]);
        let repr = ValueRepr::plan(&function, &f64_values, &names, true);
        assert!(!repr.is_typed_local("$1.i"));
        assert!(!repr.is_typed_value(ValueId(0)));
    }
}
