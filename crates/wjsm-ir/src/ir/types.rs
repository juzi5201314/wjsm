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
}

#[derive(Debug, Clone, PartialEq)]
pub enum Constant {
    Number(f64),
    String(String),
    Bool(bool),
    Null,
    Undefined,
    FunctionRef(FunctionId),
    NativeCallableEval,
    BigInt(String),
    RegExp {
        pattern: String,
        flags: String,
    },
    ModuleId(ModuleId),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Function {
    name: String,
    params: Vec<String>,
    entry: BasicBlockId,
    blocks: Vec<BasicBlock>,
    has_eval: bool,
    captured_names: Vec<String>,
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

    pub fn block_by_id(&self, id: BasicBlockId) -> Option<&BasicBlock> {
        self.blocks.get(id.0 as usize)
    }

    pub fn block_by_id_mut(&mut self, id: BasicBlockId) -> Option<&mut BasicBlock> {
        self.blocks.get_mut(id.0 as usize)
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
    DeleteProp {
        dest: ValueId,
        object: ValueId,
        key: ValueId,
    },
    SetProto {
        object: ValueId,
        value: ValueId,
    },
    NewArray {
        dest: ValueId,
        capacity: u32,
    },
    GetElem {
        dest: ValueId,
        object: ValueId,
        index: ValueId,
    },
    SetElem {
        object: ValueId,
        index: ValueId,
        value: ValueId,
    },
    OptionalGetProp {
        dest: ValueId,
        object: ValueId,
        key: ValueId,
    },
    OptionalGetElem {
        dest: ValueId,
        object: ValueId,
        key: ValueId,
    },
    OptionalCall {
        dest: ValueId,
        callee: ValueId,
        this_val: ValueId,
        args: Vec<ValueId>,
    },
    ObjectSpread {
        dest: ValueId,
        source: ValueId,
    },
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
    CollectRestArgs {
        dest: ValueId,
        skip: u32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Exp,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    UShr,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompareOp {
    StrictEq,
    StrictNotEq,
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
    TypeOf,
    In,
    InstanceOf,
    AbstractEq,
    AbstractCompare,
    DefineProperty,
    GetOwnPropDesc,
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
    FuncCall,
    FuncApply,
    FuncBind,
    ObjectRest,
    GetPrototypeFromConstructor,
    HasOwnProperty,
    PrivateGet,
    PrivateSet,
    PrivateHas,
    ObjectProtoToString,
    ObjectProtoValueOf,
    ObjectKeys,
    ObjectValues,
    ObjectEntries,
    ObjectAssign,
    ObjectCreate,
    ObjectGetPrototypeOf,
    ObjectSetPrototypeOf,
    ObjectGetOwnPropertyNames,
    ObjectIs,
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
    SymbolCreate,
    SymbolFor,
    SymbolKeyFor,
    SymbolWellKnown,
    RegExpCreate,
    RegExpTest,
    RegExpExec,
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
    DynamicImport,
    RegisterModuleNamespace,
    JsxCreateElement,
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
    StringAt,
    StringCharAt,
    StringCharCodeAt,
    StringCodePointAt,
    StringConcatVa,
    StringEndsWith,
    StringIncludes,
    StringIndexOf,
    StringLastIndexOf,
    StringMatchAll,
    StringPadEnd,
    StringPadStart,
    StringRepeat,
    StringReplaceAll,
    StringSlice,
    StringStartsWith,
    StringSubstring,
    StringToLowerCase,
    StringToUpperCase,
    StringTrim,
    StringTrimEnd,
    StringTrimStart,
    StringToString,
    StringValueOf,
    StringIterator,
    StringFromCharCode,
    StringFromCodePoint,
    MathAbs, MathAcos, MathAcosh, MathAsin, MathAsinh, MathAtan, MathAtanh,
    MathAtan2, MathCbrt, MathCeil, MathClz32, MathCos, MathCosh, MathExp,
    MathExpm1, MathFloor, MathFround, MathHypot, MathImul, MathLog, MathLog1p,
    MathLog10, MathLog2, MathMax, MathMin, MathPow, MathRandom, MathRound,
    MathSign, MathSin, MathSinh, MathSqrt, MathTan, MathTanh, MathTrunc,
    NumberConstructor, NumberIsNaN, NumberIsFinite, NumberIsInteger, NumberIsSafeInteger,
    NumberParseInt, NumberParseFloat,
    NumberProtoToString, NumberProtoValueOf, NumberProtoToFixed,
    NumberProtoToExponential, NumberProtoToPrecision,
    BooleanConstructor, BooleanProtoToString, BooleanProtoValueOf,
    ErrorConstructor, TypeErrorConstructor, RangeErrorConstructor, SyntaxErrorConstructor,
    ReferenceErrorConstructor, URIErrorConstructor, EvalErrorConstructor,
    ErrorProtoToString,
    MapConstructor, MapProtoSet, MapProtoGet,
    SetConstructor, SetProtoAdd,
    MapSetHas, MapSetDelete, MapSetClear, MapSetGetSize, MapSetForEach,
    MapSetKeys, MapSetValues, MapSetEntries,
    DateConstructor, DateNow, DateParse, DateUTC,
    WeakMapConstructor, WeakMapProtoSet, WeakMapProtoGet, WeakMapProtoHas, WeakMapProtoDelete,
    WeakSetConstructor, WeakSetProtoAdd, WeakSetProtoHas, WeakSetProtoDelete,
    ArrayBufferConstructor, ArrayBufferProtoByteLength, ArrayBufferProtoSlice,
    DataViewConstructor,
    DataViewProtoGetFloat64, DataViewProtoGetFloat32,
    DataViewProtoGetInt32, DataViewProtoGetUint32,
    DataViewProtoGetInt16, DataViewProtoGetUint16,
    DataViewProtoGetInt8, DataViewProtoGetUint8,
    DataViewProtoSetFloat64, DataViewProtoSetFloat32,
    DataViewProtoSetInt32, DataViewProtoSetUint32,
    DataViewProtoSetInt16, DataViewProtoSetUint16,
    DataViewProtoSetInt8, DataViewProtoSetUint8,
    Int8ArrayConstructor, Uint8ArrayConstructor, Uint8ClampedArrayConstructor,
    Int16ArrayConstructor, Uint16ArrayConstructor,
    Int32ArrayConstructor, Uint32ArrayConstructor,
    Float32ArrayConstructor, Float64ArrayConstructor,
    TypedArrayProtoLength, TypedArrayProtoByteLength, TypedArrayProtoByteOffset,
    TypedArrayProtoSet, TypedArrayProtoSlice, TypedArrayProtoSubarray,
    GetBuiltinGlobal,
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

#[derive(Debug, Clone, PartialEq)]
pub struct SwitchCaseTarget {
    pub constant: ConstantId,
    pub target: BasicBlockId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PhiSource {
    pub predecessor: BasicBlockId,
    pub value: ValueId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ConstantId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FunctionId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BasicBlockId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ValueId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ModuleId(pub u32);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportBinding {
    pub source_module: ModuleId,
    pub names: Vec<(String, String)>,
    pub specifier: String,
}

pub const HEAP_TYPE_OBJECT: u8 = 0x00;
pub const HEAP_TYPE_ARRAY: u8 = 0x01;
pub const HEAP_TYPE_PROMISE: u8 = 0x02;
pub const HEAP_TYPE_CONTINUATION: u8 = 0x03;
pub const HEAP_TYPE_ASYNC_GENERATOR: u8 = 0x04;
