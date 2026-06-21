// IR 基础类型：ID 类型、运算符枚举、控制流终止器

use std::fmt;

// ── ID 类型 ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ConstantId(pub u32);

impl fmt::Display for ConstantId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "c{}", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FunctionId(pub u32);

impl fmt::Display for FunctionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BasicBlockId(pub u32);

impl fmt::Display for BasicBlockId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "bb{}", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ValueId(pub u32);

impl fmt::Display for ValueId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "%{}", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ModuleId(pub u32);

impl fmt::Display for ModuleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "mod{}", self.0)
    }
}

// ── 运算符枚举 ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Exp,
    // 位运算
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    UShr,
}

impl fmt::Display for BinaryOp {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Add => "add",
            Self::Sub => "sub",
            Self::Mul => "mul",
            Self::Div => "div",
            Self::Mod => "mod",
            Self::Exp => "exp",
            Self::BitAnd => "bitand",
            Self::BitOr => "bitor",
            Self::BitXor => "bitxor",
            Self::Shl => "shl",
            Self::Shr => "shr",
            Self::UShr => "ushr",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Not,
    Neg,
    Pos,
    BitNot,
    Void,
    IsNullish,
    Delete,
}

impl fmt::Display for UnaryOp {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Not => "not",
            Self::Neg => "neg",
            Self::Pos => "pos",
            Self::BitNot => "bitnot",
            Self::Void => "void",
            Self::IsNullish => "is_nullish",
            Self::Delete => "delete",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompareOp {
    StrictEq,
    StrictNotEq,
}

impl fmt::Display for CompareOp {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::StrictEq => "stricteq",
            Self::StrictNotEq => "strictneq",
        })
    }
}

// ── 控制流终止器 ────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum Terminator {
    Return {
        value: Option<ValueId>,
    },
    Jump {
        target: BasicBlockId,
    },
    Branch {
        condition: ValueId,
        true_block: BasicBlockId,
        false_block: BasicBlockId,
    },
    Switch {
        value: ValueId,
        cases: Vec<SwitchCaseTarget>,
        default_block: BasicBlockId,
        exit_block: BasicBlockId,
    },
    Throw {
        value: ValueId,
    },
    Unreachable,
}

impl fmt::Display for Terminator {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Return { value: Some(value) } => write!(formatter, "return {value}"),
            Self::Return { value: None } => formatter.write_str("return"),
            Self::Jump { target } => write!(formatter, "jump {target}"),
            Self::Branch {
                condition,
                true_block,
                false_block,
            } => {
                write!(formatter, "branch {condition}, {true_block}, {false_block}")
            }
            Self::Switch {
                value,
                cases,
                default_block,
                exit_block,
            } => {
                write!(formatter, "switch {value} [")?;
                for (i, case) in cases.iter().enumerate() {
                    if i > 0 {
                        formatter.write_str(", ")?;
                    }
                    write!(formatter, "case {case}")?;
                }
                write!(formatter, "], default {default_block}, exit {exit_block}")
            }
            Self::Throw { value } => write!(formatter, "throw {value}"),
            Self::Unreachable => formatter.write_str("unreachable"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SwitchCaseTarget {
    pub constant: ConstantId,
    pub target: BasicBlockId,
}

impl fmt::Display for SwitchCaseTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "c{} -> {}", self.constant.0, self.target)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PhiSource {
    pub predecessor: BasicBlockId,
    pub value: ValueId,
}

impl fmt::Display for PhiSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "({}, {})", self.predecessor, self.value)
    }
}
