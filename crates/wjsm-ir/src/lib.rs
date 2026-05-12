pub mod constants;
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
    /// 运行时原生可调用对象；当前用于全局 eval 被作为值读取时。
    NativeCallableEval,
    /// BigInt 字面量（十进制字符串）
    BigInt(String),
    /// RegExp 字面量（pattern 和 flags）
    RegExp {
        pattern: String,
        flags: String,
    },
    /// AOT 解析的模块 ID（用于动态 import）
    ModuleId(ModuleId),
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
            Self::NativeCallableEval => formatter.write_str("native_callable(eval)"),
            Self::BigInt(value) => write!(formatter, "bigint({value})"),
            Self::RegExp { pattern, flags } => {
                write!(formatter, "regex(/{pattern}/{flags})")
            }
            Self::ModuleId(id) => write!(formatter, "moduleid({id})"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Function {
    name: String,
    params: Vec<String>,
    entry: BasicBlockId,
    blocks: Vec<BasicBlock>,
    /// 函数体是否包含 direct eval。后端据此降低局部变量优化强度。
    has_eval: bool,
    /// 该函数捕获的外层变量名列表（闭包用）。
    /// 语义层逃逸分析后填入，后端用于 env 对象的属性名。
    captured_names: Vec<String>,
    /// 类方法绑定的构造函数 FunctionId，用于 super 属性访问。
    /// 非类方法（普通函数、箭头函数等）为 None。
    /// 对于静态方法，home_object 设置为 None（静态方法无 super）。
    pub home_object: Option<FunctionId>,
}

impl Function {
    pub fn new(name: impl Into<String>, entry: BasicBlockId) -> Self {
        Self {
            name: name.into(),
            params: Vec::new(),
            entry,
            blocks: Vec::new(),
            has_eval: false,
            captured_names: Vec::new(),
            home_object: None,
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

    pub fn has_eval(&self) -> bool {
        self.has_eval
    }

    pub fn set_has_eval(&mut self, has_eval: bool) {
        self.has_eval = has_eval;
    }

    pub fn captured_names(&self) -> &[String] {
        &self.captured_names
    }

    pub fn set_captured_names(&mut self, names: Vec<String>) {
        self.captured_names = names;
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

    /// O(1) 通过 id 获取 block 引用。
    ///
    /// # 性能优化
    /// 由于 block id 等于其在 blocks 向量中的索引（由 FunctionBuilder::new_block 保证），
    /// 使用直接索引访问而非 iter().find()，将 O(n) 降为 O(1)。
    pub fn block_by_id(&self, id: BasicBlockId) -> Option<&BasicBlock> {
        self.blocks.get(id.0 as usize)
    }

    /// O(1) 通过 id 获取 block 可变引用。
    ///
    /// # 性能优化
    /// 由于 block id 等于其在 blocks 向量中的索引（由 FunctionBuilder::new_block 保证），
    /// 使用直接索引访问而非 iter().find()，将 O(n) 降为 O(1)。
    pub fn block_by_id_mut(&mut self, id: BasicBlockId) -> Option<&mut BasicBlock> {
        self.blocks.get_mut(id.0 as usize)
    }

    fn dump_into(&self, out: &mut String) {
        let _ = write!(out, "  fn @{}", self.name);
        if let Some(home) = self.home_object {
            let _ = write!(out, " [home_object=@{}]", home.0);
        }
        if self.has_eval {
            let _ = write!(out, " [has_eval]");
        }
        if !self.captured_names.is_empty() {
            let _ = write!(out, " [captures: ");
            for (i, name) in self.captured_names.iter().enumerate() {
                if i > 0 {
                    let _ = write!(out, ", ");
                }
                let _ = write!(out, "{name}");
            }
            let _ = write!(out, "]");
        }
        if self.params.is_empty() {
            let _ = writeln!(out, " [entry={}]:", self.entry);
        } else {
            let _ = write!(out, " [params: ");
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
    StringConcatVa {
        dest: ValueId,
        parts: Vec<ValueId>,
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
        capacity: u32,
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
    /// 删除对象属性，返回布尔值表示是否成功删除
    DeleteProp {
        dest: ValueId,
        object: ValueId,
        key: ValueId,
    },
    /// 直接设置对象的 __proto__ 槽位（offset 0），用于原型链构建。
    SetProto {
        object: ValueId,
        value: ValueId,
    },
    /// 创建 TAG_ARRAY 数组对象
    NewArray {
        dest: ValueId,
        capacity: u32,
    },
    /// 按数字索引读取数组元素
    GetElem {
        dest: ValueId,
        object: ValueId,
        index: ValueId,
    },
    /// 按数字索引写入数组元素
    SetElem {
        object: ValueId,
        index: ValueId,
        value: ValueId,
    },
    /// 可选链属性访问：object?.key，object 为 null/undefined 时返回 undefined
    OptionalGetProp {
        dest: ValueId,
        object: ValueId,
        key: ValueId,
    },
    /// 可选链索引访问：object?.[expr]
    OptionalGetElem {
        dest: ValueId,
        object: ValueId,
        key: ValueId,
    },
    /// 可选链调用：callee?.(...args)，callee 为 null/undefined 时返回 undefined
    OptionalCall {
        dest: ValueId,
        callee: ValueId,
        this_val: ValueId,
        args: Vec<ValueId>,
    },
    /// 对象 spread：将 source 的 own enumerable 属性复制到 dest
    ObjectSpread {
        dest: ValueId,
        source: ValueId,
    },
    /// 获取 super 基类：从 home_object 的 proto header offset 0 读取原型对象
    GetSuperBase {
        dest: ValueId,
    },
    NewPromise {
        dest: ValueId,
    },
    PromiseResolve {
        promise: ValueId,
        value: ValueId,
    },
    PromiseReject {
        promise: ValueId,
        reason: ValueId,
    },
    Suspend {
        promise: ValueId,
        state: u32,
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
            Self::StringConcatVa { dest, parts } => {
                write!(formatter, "{dest} = string_concat_va [")?;
                for (index, part) in parts.iter().enumerate() {
                    if index > 0 {
                        formatter.write_str(", ")?;
                    }
                    write!(formatter, "{part}")?;
                }
                formatter.write_char(']')
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
            Self::NewObject { dest, capacity } => {
                write!(formatter, "{dest} = new_object(capacity={capacity})")
            }
            Self::GetProp { dest, object, key } => {
                write!(formatter, "{dest} = get_prop {object}, {key}")
            }
            Self::SetProp { object, key, value } => {
                write!(formatter, "set_prop {object}, {key}, {value}")
            }
            Self::DeleteProp { dest, object, key } => {
                write!(formatter, "{dest} = delete_prop {object}, {key}")
            }
            Self::SetProto { object, value } => {
                write!(formatter, "set_proto {object}, {value}")
            }
            Self::NewArray { dest, capacity } => {
                write!(formatter, "{dest} = new_array(capacity={capacity})")
            }
            Self::GetElem {
                dest,
                object,
                index,
            } => {
                write!(formatter, "{dest} = get_elem {object}, {index}")
            }
            Self::SetElem {
                object,
                index,
                value,
            } => {
                write!(formatter, "set_elem {object}, {index}, {value}")
            }
            Self::OptionalGetProp { dest, object, key } => {
                write!(formatter, "{dest} = optional_get_prop {object}, {key}")
            }
            Self::OptionalGetElem { dest, object, key } => {
                write!(formatter, "{dest} = optional_get_elem {object}, {key}")
            }
            Self::OptionalCall {
                dest,
                callee,
                this_val,
                args,
            } => {
                write!(
                    formatter,
                    "{dest} = optional_call {callee}, this={this_val}"
                )?;
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
            Self::ObjectSpread { dest, source } => {
                write!(formatter, "{dest} = object_spread {source}")
            }
            Self::GetSuperBase { dest } => {
                write!(formatter, "{dest} = get_super_base")
            }
            Self::NewPromise { dest } => write!(formatter, "{dest} = new_promise"),
            Self::PromiseResolve { promise, value } => {
                write!(formatter, "promise_resolve {promise}, {value}")
            }
            Self::PromiseReject { promise, reason } => {
                write!(formatter, "promise_reject {promise}, {reason}")
            }
            Self::Suspend { promise, state } => {
                write!(formatter, "suspend {promise}, state={state}")
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Builtin {
    ConsoleLog,
    ConsoleError,
    ConsoleWarn,
    ConsoleInfo,
    ConsoleDebug,
    ConsoleTrace,
    Debugger,
    Throw,
    AbortShadowStackOverflow,
    F64Mod,
    F64Exp,
    IteratorFrom,
    IteratorNext,
    IteratorClose,
    IteratorValue,
    IteratorDone,
    EnumeratorFrom,
    EnumeratorNext,
    EnumeratorKey,
    EnumeratorDone,
    // 运算符
    TypeOf,
    In,
    InstanceOf,
    AbstractEq,
    AbstractCompare,
    // 对象属性描述符
    DefineProperty,
    GetOwnPropDesc,
    // 宿主 API
    SetTimeout,
    ClearTimeout,
    SetInterval,
    ClearInterval,
    Fetch,
    Eval,
    EvalIndirect,
    EvalResult,
    JsonStringify,
    JsonParse,
    CreateClosure,
    // 数组方法
    ArrayPush,
    ArrayPop,
    ArrayIncludes,
    ArrayIndexOf,
    ArrayJoin,
    ArrayConcat,
    ArraySlice,
    ArrayFill,
    ArrayReverse,
    ArrayFlat,
    ArrayInitLength,
    ArrayGetLength,
    // ── 新数组方法 ──
    ArrayShift,
    ArrayUnshiftVa,
    ArraySort,
    ArrayAt,
    ArrayCopyWithin,
    ArrayForEach,
    ArrayMap,
    ArrayFilter,
    ArrayReduce,
    ArrayReduceRight,
    ArrayFind,
    ArrayFindIndex,
    ArraySome,
    ArrayEvery,
    ArrayFlatMap,
    ArrayIsArray,
    ArraySpliceVa,
    ArrayConcatVa,
    // ── 函数原型方法 ──
    FuncCall,
    FuncApply,
    FuncBind,
    // ── 对象解构 rest ──
    ObjectRest,
    // ── new.prototype 修复 ──
    GetPrototypeFromConstructor,
    // ── Object 原型方法 ──
    HasOwnProperty,
    ObjectProtoToString,
    ObjectProtoValueOf,
    // ── Object 静态方法 ──
    ObjectKeys,
    ObjectValues,
    ObjectEntries,
    ObjectAssign,
    ObjectCreate,
    ObjectGetPrototypeOf,
    ObjectSetPrototypeOf,
    ObjectGetOwnPropertyNames,
    ObjectIs,
    // ── BigInt operations ──────────────────────────────────────────────
    BigIntFromLiteral,
    BigIntAdd,
    BigIntSub,
    BigIntMul,
    BigIntDiv,
    BigIntMod,
    BigIntPow,
    BigIntNeg,
    BigIntEq,
    BigIntCmp,
    // ── Symbol operations ──────────────────────────────────────────────
    SymbolCreate,
    SymbolFor,
    SymbolKeyFor,
    // ── Well-known symbols ─────────────────────────────────────────────
    SymbolWellKnown,
    // ── RegExp operations ──────────────────────────────────────────────
    RegExpCreate,
    RegExpTest,
    RegExpExec,
    // ── String prototype methods ───────────────────────────────────────
    StringMatch,
    StringReplace,
    StringSearch,
    StringSplit,
    PromiseCreate,
    PromiseInstanceResolve,
    PromiseInstanceReject,
    PromiseCreateResolveFunction,
    PromiseCreateRejectFunction,
    PromiseThen,
    PromiseCatch,
    PromiseFinally,
    PromiseAll,
    PromiseRace,
    PromiseAllSettled,
    PromiseAny,
    PromiseResolveStatic,
    PromiseRejectStatic,
    IsPromise,
    QueueMicrotask,
    DrainMicrotasks,
    AsyncFunctionStart,
    AsyncFunctionResume,
    AsyncFunctionSuspend,
    ContinuationCreate,
    ContinuationSaveVar,
    ContinuationLoadVar,
    AsyncGeneratorStart,
    AsyncGeneratorNext,
    AsyncGeneratorReturn,
    AsyncGeneratorThrow,
    PromiseWithResolvers,
    IsCallable,
    // ── 动态 import ──────────────────────────────────────────────────
    DynamicImport,
    RegisterModuleNamespace,
    // ── JSX ────────────────────────────────────────────────────────────
    JsxCreateElement,
    // ── Proxy / Reflect ────────────────────────────────────────────────
    ProxyCreate,
    ProxyRevocable,
    ReflectGet,
    ReflectSet,
    ReflectHas,
    ReflectDeleteProperty,
    ReflectApply,
    ReflectConstruct,
    ReflectGetPrototypeOf,
    ReflectSetPrototypeOf,
    ReflectIsExtensible,
    ReflectPreventExtensions,
    ReflectGetOwnPropertyDescriptor,
    ReflectDefineProperty,
    ReflectOwnKeys,
}

impl fmt::Display for Builtin {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ConsoleLog => "console.log",
            Self::ConsoleError => "console.error",
            Self::ConsoleWarn => "console.warn",
            Self::ConsoleInfo => "console.info",
            Self::ConsoleDebug => "console.debug",
            Self::ConsoleTrace => "console.trace",
            Self::Debugger => "debugger",
            Self::Throw => "throw",
            Self::AbortShadowStackOverflow => "abort_shadow_stack_overflow",
            Self::F64Mod => "f64.mod",
            Self::F64Exp => "f64.exp",
            Self::IteratorFrom => "iterator.from",
            Self::IteratorNext => "iterator.next",
            Self::IteratorClose => "iterator.close",
            Self::IteratorValue => "iterator.value",
            Self::IteratorDone => "iterator.done",
            Self::EnumeratorFrom => "enumerator.from",
            Self::EnumeratorNext => "enumerator.next",
            Self::EnumeratorKey => "enumerator.key",
            Self::EnumeratorDone => "enumerator.done",
            Self::TypeOf => "typeof",
            Self::In => "op_in",
            Self::InstanceOf => "op_instanceof",
            Self::AbstractEq => "abstract_eq",
            Self::AbstractCompare => "abstract_compare",
            Self::DefineProperty => "define_property",
            Self::GetOwnPropDesc => "get_own_prop_desc",
            Self::SetTimeout => "setTimeout",
            Self::ClearTimeout => "clearTimeout",
            Self::SetInterval => "setInterval",
            Self::ClearInterval => "clearInterval",
            Self::Fetch => "fetch",
            Self::Eval => "eval",
            Self::EvalIndirect => "eval.indirect",
            Self::EvalResult => "eval.result",
            Self::JsonStringify => "JSON.stringify",
            Self::JsonParse => "JSON.parse",
            Self::CreateClosure => "create_closure",
            Self::ArrayPush => "array.push",
            Self::ArrayPop => "array.pop",
            Self::ArrayIncludes => "array.includes",
            Self::ArrayIndexOf => "array.index_of",
            Self::ArrayJoin => "array.join",
            Self::ArrayConcat => "array.concat",
            Self::ArraySlice => "array.slice",
            Self::ArrayFill => "array.fill",
            Self::ArrayReverse => "array.reverse",
            Self::ArrayFlat => "array.flat",
            Self::ArrayInitLength => "array.init_length",
            Self::ArrayGetLength => "array.get_length",
            Self::ArrayShift => "array.shift",
            Self::ArrayUnshiftVa => "array.unshift",
            Self::ArraySort => "array.sort",
            Self::ArrayAt => "array.at",
            Self::ArrayCopyWithin => "array.copy_within",
            Self::ArrayForEach => "array.for_each",
            Self::ArrayMap => "array.map",
            Self::ArrayFilter => "array.filter",
            Self::ArrayReduce => "array.reduce",
            Self::ArrayReduceRight => "array.reduce_right",
            Self::ArrayFind => "array.find",
            Self::ArrayFindIndex => "array.find_index",
            Self::ArraySome => "array.some",
            Self::ArrayEvery => "array.every",
            Self::ArrayFlatMap => "array.flat_map",
            Self::ArrayIsArray => "array.is_array",
            Self::ArraySpliceVa => "array.splice_va",
            Self::ArrayConcatVa => "array.concat_va",
            Self::FuncCall => "func_call",
            Self::FuncApply => "func_apply",
            Self::FuncBind => "func_bind",
            Self::ObjectRest => "object_rest",
            Self::GetPrototypeFromConstructor => "get_prototype_from_constructor",
            Self::HasOwnProperty => "has_own_property",
            Self::ObjectProtoToString => "object_proto_to_string",
            Self::ObjectProtoValueOf => "object_proto_value_of",
            Self::ObjectKeys => "object.keys",
            Self::ObjectValues => "object.values",
            Self::ObjectEntries => "object.entries",
            Self::ObjectAssign => "object.assign",
            Self::ObjectCreate => "object.create",
            Self::ObjectGetPrototypeOf => "object.get_prototype_of",
            Self::ObjectSetPrototypeOf => "object.set_prototype_of",
            Self::ObjectGetOwnPropertyNames => "object.get_own_property_names",
            Self::ObjectIs => "object.is",
            Self::BigIntFromLiteral => "bigint.from_literal",
            Self::BigIntAdd => "bigint.add",
            Self::BigIntSub => "bigint.sub",
            Self::BigIntMul => "bigint.mul",
            Self::BigIntDiv => "bigint.div",
            Self::BigIntMod => "bigint.mod",
            Self::BigIntPow => "bigint.pow",
            Self::BigIntNeg => "bigint.neg",
            Self::BigIntEq => "bigint.eq",
            Self::BigIntCmp => "bigint.cmp",
            Self::SymbolCreate => "symbol.create",
            Self::SymbolFor => "symbol.for",
            Self::SymbolKeyFor => "symbol.key_for",
            Self::SymbolWellKnown => "symbol.well_known",
            Self::RegExpCreate => "regexp.create",
            Self::RegExpTest => "regexp.test",
            Self::RegExpExec => "regexp.exec",
            Self::StringMatch => "string.match",
            Self::StringReplace => "string.replace",
            Self::StringSearch => "string.search",
            Self::StringSplit => "string.split",
            Self::PromiseCreate => "promise.create",
            Self::PromiseInstanceResolve => "promise.instance_resolve",
            Self::PromiseInstanceReject => "promise.instance_reject",
            Self::PromiseCreateResolveFunction => "promise.create_resolve_function",
            Self::PromiseCreateRejectFunction => "promise.create_reject_function",
            Self::PromiseThen => "promise.then",
            Self::PromiseCatch => "promise.catch",
            Self::PromiseFinally => "promise.finally",
            Self::PromiseAll => "promise.all",
            Self::PromiseRace => "promise.race",
            Self::PromiseAllSettled => "promise.all_settled",
            Self::PromiseAny => "promise.any",
            Self::PromiseResolveStatic => "promise.resolve_static",
            Self::PromiseRejectStatic => "promise.reject_static",
            Self::IsPromise => "is_promise",
            Self::QueueMicrotask => "queue_microtask",
            Self::DrainMicrotasks => "drain_microtasks",
            Self::AsyncFunctionStart => "async_function.start",
            Self::AsyncFunctionResume => "async_function.resume",
            Self::AsyncFunctionSuspend => "async_function.suspend",
            Self::ContinuationCreate => "continuation.create",
            Self::ContinuationSaveVar => "continuation.save_var",
            Self::ContinuationLoadVar => "continuation.load_var",
            Self::AsyncGeneratorStart => "async_generator.start",
            Self::AsyncGeneratorNext => "async_generator.next",
            Self::AsyncGeneratorReturn => "async_generator.return",
            Self::PromiseWithResolvers => "promise.with_resolvers",
            Self::IsCallable => "is_callable",
            Self::AsyncGeneratorThrow => "async_generator.throw",
            Self::DynamicImport => "dynamic_import",
            Self::RegisterModuleNamespace => "register_module_namespace",
            Self::JsxCreateElement => "jsx.create_element",
            Self::ProxyCreate => "proxy.create",
            Self::ProxyRevocable => "proxy.revocable",
            Self::ReflectGet => "reflect.get",
            Self::ReflectSet => "reflect.set",
            Self::ReflectHas => "reflect.has",
            Self::ReflectDeleteProperty => "reflect.delete_property",
            Self::ReflectApply => "reflect.apply",
            Self::ReflectConstruct => "reflect.construct",
            Self::ReflectGetPrototypeOf => "reflect.get_prototype_of",
            Self::ReflectSetPrototypeOf => "reflect.set_prototype_of",
            Self::ReflectIsExtensible => "reflect.is_extensible",
            Self::ReflectPreventExtensions => "reflect.prevent_extensions",
            Self::ReflectGetOwnPropertyDescriptor => "reflect.get_own_property_descriptor",
            Self::ReflectDefineProperty => "reflect.define_property",
            Self::ReflectOwnKeys => "reflect.own_keys",
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ModuleId(pub u32);

impl fmt::Display for ModuleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "mod{}", self.0)
    }
}

/// Import 绑定信息（用于模块系统）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportBinding {
    /// 源模块 ID
    pub source_module: ModuleId,
    /// 导入的名称列表：(local_name, imported_name)
    /// - `import { x } from './foo'` → ("x", "x")
    /// - `import { y as z } from './foo'` → ("z", "y")
    /// - `import * as ns from './foo'` → ("ns", "*")
    /// - `import defaultExport from './foo'` → ("defaultExport", "default")
    pub names: Vec<(String, String)>,
    /// 模块说明符（如 './foo'），用于动态 import 的 specifier 查找
    pub specifier: String,
}

// ── Heap type tags ──────────────────────────────────────────────────────
/// 0x00 = object (HEAP_TYPE_OBJECT)
pub const HEAP_TYPE_OBJECT: u8 = 0x00;
/// 0x01 = array (HEAP_TYPE_ARRAY)
pub const HEAP_TYPE_ARRAY: u8 = 0x01;
pub const HEAP_TYPE_PROMISE: u8 = 0x02;
pub const HEAP_TYPE_CONTINUATION: u8 = 0x03;
pub const HEAP_TYPE_ASYNC_GENERATOR: u8 = 0x04;
