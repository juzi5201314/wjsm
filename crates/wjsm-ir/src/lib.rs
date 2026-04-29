pub mod value;

use std::fmt::{self, Write};

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Module {
    constants: Vec<Constant>,
    functions: Vec<Function>,
}

pub type Program = Module;

impl Module {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_constant(&mut self, constant: Constant) -> ConstantId {
        let id = ConstantId(self.constants.len() as u32);
        self.constants.push(constant);
        id
    }

    pub fn push_function(&mut self, function: Function) -> FunctionId {
        let id = FunctionId(self.functions.len() as u32);
        self.functions.push(function);
        id
    }

    pub fn constants(&self) -> &[Constant] {
        &self.constants
    }

    pub fn functions(&self) -> &[Function] {
        &self.functions
    }

    pub fn dump_text(&self) -> String {
        let mut out = String::from("module {\n");

        if self.constants.is_empty() {
            out.push_str("  constants: []\n");
        } else {
            out.push_str("  constants:\n");
            for (index, constant) in self.constants.iter().enumerate() {
                let _ = writeln!(out, "    c{index} = {constant}");
            }
        }

        out.push('\n');

        for (index, function) in self.functions.iter().enumerate() {
            if index > 0 {
                out.push('\n');
            }
            function.dump_into(&mut out);
        }

        out.push_str("}\n");
        out
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Constant {
    Number(f64),
    String(String),
    Bool(bool),
    Null,
    Undefined,
    FunctionRef(FunctionId),
}

impl fmt::Display for Constant {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Number(value) => write!(formatter, "number({value})"),
            Self::String(value) => write!(formatter, "string({value:?})"),
            Self::Bool(value) => write!(formatter, "bool({value})"),
            Self::Null => formatter.write_str("null"),
            Self::Undefined => formatter.write_str("undefined"),
            Self::FunctionRef(id) => write!(formatter, "functionref(@{id})"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Function {
    name: String,
    params: Vec<String>,
    entry: BasicBlockId,
    blocks: Vec<BasicBlock>,
}

impl Function {
    pub fn new(name: impl Into<String>, entry: BasicBlockId) -> Self {
        Self {
            name: name.into(),
            params: Vec::new(),
            entry,
            blocks: Vec::new(),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn params(&self) -> &[String] {
        &self.params
    }

    pub fn set_params(&mut self, params: Vec<String>) {
        self.params = params;
    }

    pub fn entry(&self) -> BasicBlockId {
        self.entry
    }

    pub fn push_block(&mut self, block: BasicBlock) {
        self.blocks.push(block);
    }

    pub fn blocks(&self) -> &[BasicBlock] {
        &self.blocks
    }

    pub fn blocks_mut(&mut self) -> &mut [BasicBlock] {
        &mut self.blocks
    }

    pub fn block_by_id(&self, id: BasicBlockId) -> Option<&BasicBlock> {
        self.blocks.iter().find(|b| b.id == id)
    }

    pub fn block_by_id_mut(&mut self, id: BasicBlockId) -> Option<&mut BasicBlock> {
        self.blocks.iter_mut().find(|b| b.id == id)
    }

    fn dump_into(&self, out: &mut String) {
        if self.params.is_empty() {
            let _ = writeln!(out, "  fn @{} [entry={}]:", self.name, self.entry);
        } else {
            let _ = write!(out, "  fn @{} [params: ", self.name);
            for (i, param) in self.params.iter().enumerate() {
                if i > 0 {
                    let _ = write!(out, ", ");
                }
                let _ = write!(out, "{param}");
            }
            let _ = writeln!(out, "] [entry={}]:", self.entry);
        }

        for block in &self.blocks {
            let _ = writeln!(out, "    {}:", block.id);

            for instruction in &block.instructions {
                let _ = writeln!(out, "      {instruction}");
            }

            let _ = writeln!(out, "      {}", block.terminator);
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct BasicBlock {
    id: BasicBlockId,
    instructions: Vec<Instruction>,
    terminator: Terminator,
}

impl BasicBlock {
    pub fn new(id: BasicBlockId) -> Self {
        Self {
            id,
            instructions: Vec::new(),
            terminator: Terminator::Unreachable,
        }
    }

    pub fn new_with_terminator(id: BasicBlockId, terminator: Terminator) -> Self {
        Self {
            id,
            instructions: Vec::new(),
            terminator,
        }
    }

    pub fn id(&self) -> BasicBlockId {
        self.id
    }

    pub fn push_instruction(&mut self, instruction: Instruction) {
        self.instructions.push(instruction);
    }

    pub fn instructions(&self) -> &[Instruction] {
        &self.instructions
    }

    pub fn terminator(&self) -> &Terminator {
        &self.terminator
    }

    pub fn set_terminator(&mut self, terminator: Terminator) {
        self.terminator = terminator;
    }

    pub fn terminator_mut(&mut self) -> &mut Terminator {
        &mut self.terminator
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Instruction {
    Const {
        dest: ValueId,
        constant: ConstantId,
    },
    Binary {
        dest: ValueId,
        op: BinaryOp,
        lhs: ValueId,
        rhs: ValueId,
    },
    Unary {
        dest: ValueId,
        op: UnaryOp,
        value: ValueId,
    },
    Compare {
        dest: ValueId,
        op: CompareOp,
        lhs: ValueId,
        rhs: ValueId,
    },
    Phi {
        dest: ValueId,
        sources: Vec<PhiSource>,
    },
    CallBuiltin {
        dest: Option<ValueId>,
        builtin: Builtin,
        args: Vec<ValueId>,
    },
    LoadVar {
        dest: ValueId,
        name: String,
    },
    StoreVar {
        name: String,
        value: ValueId,
    },
    Call {
        dest: Option<ValueId>,
        callee: ValueId,
        this_val: ValueId,
        args: Vec<ValueId>,
    },
    NewObject {
        dest: ValueId,
    },
    GetProp {
        dest: ValueId,
        object: ValueId,
        key: ValueId,
    },
    SetProp {
        object: ValueId,
        key: ValueId,
        value: ValueId,
    },
}

impl fmt::Display for Instruction {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Const { dest, constant } => write!(formatter, "{dest} = const {constant}"),
            Self::Binary { dest, op, lhs, rhs } => {
                write!(formatter, "{dest} = {op} {lhs}, {rhs}")
            }
            Self::Unary { dest, op, value } => {
                write!(formatter, "{dest} = {op} {value}")
            }
            Self::Compare { dest, op, lhs, rhs } => {
                write!(formatter, "{dest} = {op} {lhs}, {rhs}")
            }
            Self::Phi { dest, sources } => {
                write!(formatter, "{dest} = phi [")?;
                for (index, source) in sources.iter().enumerate() {
                    if index > 0 {
                        formatter.write_str(", ")?;
                    }
                    write!(formatter, "({}, {})", source.predecessor, source.value)?;
                }
                formatter.write_char(']')
            }
            Self::CallBuiltin {
                dest,
                builtin,
                args,
            } => {
                if let Some(dest) = dest {
                    write!(formatter, "{dest} = ")?;
                }

                write!(formatter, "call builtin.{builtin}(")?;
                for (index, arg) in args.iter().enumerate() {
                    if index > 0 {
                        formatter.write_str(", ")?;
                    }
                    write!(formatter, "{arg}")?;
                }
                formatter.write_char(')')
            }
            Self::LoadVar { dest, name } => {
                write!(formatter, "{dest} = load var {name}")
            }
            Self::StoreVar { name, value } => {
                write!(formatter, "store var {name}, {value}")
            }
            Self::Call {
                dest,
                callee,
                this_val,
                args,
            } => {
                if let Some(dest) = dest {
                    write!(formatter, "{dest} = ")?;
                }
                write!(formatter, "call {callee}, this={this_val}")?;
                if !args.is_empty() {
                    formatter.write_str(", args=[")?;
                    for (index, arg) in args.iter().enumerate() {
                        if index > 0 {
                            formatter.write_str(", ")?;
                        }
                        write!(formatter, "{arg}")?;
                    }
                    formatter.write_char(']')?;
                }
                Ok(())
            }
            Self::NewObject { dest } => {
                write!(formatter, "{dest} = new_object")
            }
            Self::GetProp { dest, object, key } => {
                write!(formatter, "{dest} = get_prop {object}, {key}")
            }
            Self::SetProp {
                object,
                key,
                value,
            } => {
                write!(formatter, "set_prop {object}, {key}, {value}")
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Exp,
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
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompareOp {
    Eq,
    NotEq,
    StrictEq,
    StrictNotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
}

impl fmt::Display for CompareOp {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Eq => "eq",
            Self::NotEq => "neq",
            Self::StrictEq => "stricteq",
            Self::StrictNotEq => "strictneq",
            Self::Lt => "lt",
            Self::LtEq => "lteq",
            Self::Gt => "gt",
            Self::GtEq => "gteq",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Builtin {
    ConsoleLog,
    Debugger,
    Throw,
    F64Mod,
    F64Exp,
    BeginTry,
    EndTry,
    BeginFinally,
    EndFinally,
    IteratorFrom,
    IteratorNext,
    IteratorClose,
    IteratorValue,
    IteratorDone,
    EnumeratorFrom,
    EnumeratorNext,
    EnumeratorKey,
    EnumeratorDone,
}

impl fmt::Display for Builtin {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ConsoleLog => "console.log",
            Self::Debugger => "debugger",
            Self::Throw => "throw",
            Self::F64Mod => "f64.mod",
            Self::F64Exp => "f64.exp",
            Self::BeginTry => "begin_try",
            Self::EndTry => "end_try",
            Self::BeginFinally => "begin_finally",
            Self::EndFinally => "end_finally",
            Self::IteratorFrom => "iterator.from",
            Self::IteratorNext => "iterator.next",
            Self::IteratorClose => "iterator.close",
            Self::IteratorValue => "iterator.value",
            Self::IteratorDone => "iterator.done",
            Self::EnumeratorFrom => "enumerator.from",
            Self::EnumeratorNext => "enumerator.next",
            Self::EnumeratorKey => "enumerator.key",
            Self::EnumeratorDone => "enumerator.done",
        })
    }
}

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
