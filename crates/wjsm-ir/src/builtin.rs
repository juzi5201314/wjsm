use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u16)]
pub enum Builtin {
    ConsoleLog,
    ConsoleError,
    ConsoleWarn,
    ConsoleInfo,
    ConsoleDebug,
    ConsoleTrace,
    Debugger,
    Throw,
    IteratorFrom,
    IteratorNext,
    IteratorClose,
    AsyncIteratorFrom,
    IteratorValue,
    IteratorStepValue,
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
    StrictEq,
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
    HeadersConstructor,
    RequestConstructor,
    ResponseConstructor,
    AbortControllerConstructor,
    Eval,
    EvalIndirect,
    EvalResult,
    JsonStringify,
    JsonParse,
    CreateClosure,
    // 数组方法
    ArrayPush,
    /// 数组字面量 elision：在末尾追加一个 hole。
    ArrayPushHole,
    ArrayPushSpread,
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
    ArrayFrom,
    ArraySpliceVa,
    ArrayConcatVa,
    // ── ES2023/ES2024 新增数组方法 ──
    ArrayOf,
    ArrayFindLast,
    ArrayFindLastIndex,
    ArrayLastIndexOf,
    ArrayToSorted,
    ArrayToReversed,
    ArrayToSplicedVa,
    ArrayWith,
    // ── 函数原型方法 ──
    FuncCall,
    FuncApply,
    /// super(...args) 的数组实参调用，保留当前构造上下文。
    SuperApply,
    FuncBind,
    // ── 对象解构 rest ──
    ObjectRest,
    // ── new.prototype 修复 ──
    GetPrototypeFromConstructor,
    // ── Object 原型方法 ──
    HasOwnProperty,
    PrivateGet,
    PrivateSet,
    PrivateHas,
    /// 在对象上安装私有访问器槽（getter/setter 各可为 undefined）。
    PrivateAccessorBind,
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
    ObjectGetOwnPropertySymbols,
    ObjectIs,
    ObjectGroupBy,
    ObjectHasOwn,
    ObjectFreeze,
    ObjectSeal,
    ObjectIsFrozen,
    ObjectIsSealed,
    ObjectIsExtensible,
    ObjectPreventExtensions,
    ObjectFromEntries,
    ObjectGetOwnPropertyDescriptors,
    ObjectDefineProperties,
    // ── Map.groupBy ──
    MapGroupBy,
    // ── BigInt operations ──────────────────────────────────────────────
    BigIntFromLiteral,
    BigIntAdd,
    BigIntSub,
    BigIntMul,
    BigIntDiv,
    BigIntMod,
    BigIntPow,
    BigIntNeg,
    BigIntBitAnd,
    BigIntBitOr,
    BigIntBitXor,
    BigIntShl,
    BigIntShr,
    BigIntBitNot,
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
    GeneratorStart,
    GeneratorNext,
    GeneratorReturn,
    GeneratorThrow,
    PromiseWithResolvers,
    IsCallable,
    /// ECMAScript Type(argument) is Object（含 array/function 等可构造返回值类型）
    IsJsObject,
    // ── 动态 import ──────────────────────────────────────────────────
    DynamicImport,
    DynamicImportRuntime,
    ImportMetaResolve,
    RegisterModuleNamespace,
    CjsCreateRequire,
    CjsRegisterModule,
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
    // ── String 完整方法 ──
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
    StringNormalize,
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
    // ── Math static methods ────────────────────────────────────────────
    MathAbs,
    MathAcos,
    MathAcosh,
    MathAsin,
    MathAsinh,
    MathAtan,
    MathAtanh,
    MathAtan2,
    MathCbrt,
    MathCeil,
    MathClz32,
    MathCos,
    MathCosh,
    MathExp,
    MathExpm1,
    MathFloor,
    MathFround,
    MathHypot,
    MathImul,
    MathLog,
    MathLog1p,
    MathLog10,
    MathLog2,
    MathMax,
    /// Math.max(...args) 的数组实参入口。
    MathMaxArray,
    MathMin,
    MathPow,
    MathRandom,
    MathRound,
    MathSign,
    MathSin,
    MathSinh,
    MathSqrt,
    MathTan,
    MathTanh,
    MathTrunc,
    // ── Number built-in ────────────────────────────────────────────────
    NumberConstructor,
    NumberIsNaN,
    NumberIsFinite,
    NumberIsInteger,
    NumberIsSafeInteger,
    NumberParseInt,
    NumberParseFloat,
    NumberProtoToString,
    NumberProtoValueOf,
    NumberProtoToFixed,
    NumberProtoToExponential,
    NumberProtoToPrecision,
    // ── Boolean built-in ───────────────────────────────────────────────
    BooleanConstructor,
    BooleanProtoToString,
    BooleanProtoValueOf,
    // ── Error built-in ─────────────────────────────────────────────────
    ErrorConstructor,
    TypeErrorConstructor,
    RangeErrorConstructor,
    SyntaxErrorConstructor,
    ReferenceErrorConstructor,
    URIErrorConstructor,
    EvalErrorConstructor,
    ErrorProtoToString,
    // ── Map built-in ──────────────────────────────────────────────────
    MapConstructor,
    MapProtoSet,
    MapProtoGet,
    // ── Set built-in ──────────────────────────────────────────────────
    SetConstructor,
    SetProtoAdd,
    SetProtoHas,
    SetProtoDelete,
    // ── Map/Set shared methods (dispatch at runtime) ──────────────────
    MapSetHas,
    MapSetDelete,
    MapSetClear,
    MapSetGetSize,
    MapSetForEach,
    MapSetKeys,
    MapSetValues,
    MapSetEntries,
    /// `map.keys().next().value` 直连（免迭代器对象与 next 调用链）。
    MapSetFirstKey,
    // ── Date built-in ─────────────────────────────────────────────────
    DateConstructor,
    /// `new Date(...)` lowering：编译时设置 new.target，与 DateConstructor 共用宿主实现。
    DateConstructorNew,
    DateNow,
    /// `performance.now()`：宿主毫秒时间戳（f64），确定性纯（不抛、不分配）。
    PerformanceNow,
    DateParse,
    DateUTC,
    // ── WeakMap built-in ──────────────────────────────────────────────
    WeakMapConstructor,
    WeakMapProtoSet,
    WeakMapProtoGet,
    WeakMapProtoHas,
    WeakMapProtoDelete,
    // ── WeakSet built-in ──────────────────────────────────────────────
    WeakSetConstructor,
    WeakSetProtoAdd,
    WeakSetProtoHas,
    WeakSetProtoDelete,
    // ── SharedArrayBuffer built-in ──
    SharedArrayBufferConstructor,
    SharedArrayBufferProtoByteLength,
    SharedArrayBufferProtoGrow,
    SharedArrayBufferProtoGrowable,
    SharedArrayBufferProtoMaxByteLength,
    SharedArrayBufferProtoSlice,
    SharedArrayBufferSpecies,
    // ── Atomics built-in ──
    AtomicsLoad,
    AtomicsStore,
    AtomicsAdd,
    AtomicsSub,
    AtomicsAnd,
    AtomicsOr,
    AtomicsXor,
    AtomicsExchange,
    AtomicsCompareExchange,
    AtomicsIsLockFree,
    AtomicsPause,
    AtomicsWait,
    AtomicsNotify,
    AtomicsWaitAsync,
    // ── WeakRef built-in ──────────────────────────────────────────────
    WeakRefConstructor,
    WeakRefProtoDeref,
    // ── FinalizationRegistry built-in ─────────────────────────────────
    FinalizationRegistryConstructor,
    FinalizationRegistryProtoRegister,
    FinalizationRegistryProtoUnregister,
    // ── ArrayBuffer built-in ──────────────────────────────────────────
    ArrayBufferConstructor,
    ArrayBufferProtoByteLength,
    ArrayBufferProtoSlice,
    ArrayBufferProtoResize,
    ArrayBufferProtoTransfer,
    ArrayBufferProtoTransferToFixedLength,
    ArrayBufferProtoResizable,
    ArrayBufferProtoMaxByteLength,
    ArrayBufferProtoDetached,
    // ── DataView built-in ──────────────────────────────────────────────
    DataViewConstructor,
    DataViewProtoGetFloat64,
    DataViewProtoGetFloat32,
    DataViewProtoGetInt32,
    DataViewProtoGetUint32,
    DataViewProtoGetInt16,
    DataViewProtoGetUint16,
    DataViewProtoGetInt8,
    DataViewProtoGetUint8,
    DataViewProtoSetFloat64,
    DataViewProtoSetFloat32,
    DataViewProtoSetInt32,
    DataViewProtoSetUint32,
    DataViewProtoSetInt16,
    DataViewProtoSetUint16,
    DataViewProtoSetInt8,
    DataViewProtoSetUint8,
    // ── TypedArray constructors ────────────────────────────────────────
    Int8ArrayConstructor,
    Uint8ArrayConstructor,
    Uint8ClampedArrayConstructor,
    Int16ArrayConstructor,
    Uint16ArrayConstructor,
    Int32ArrayConstructor,
    Uint32ArrayConstructor,
    Float32ArrayConstructor,
    Float64ArrayConstructor,
    // ── TypedArray prototype methods ───────────────────────────────────
    TypedArrayProtoLength,
    TypedArrayProtoByteLength,
    TypedArrayProtoByteOffset,
    TypedArrayProtoSet,
    TypedArrayProtoSlice,
    TypedArrayProtoSubarray,
    // ── TypedArray 新增构造器 ──
    BigInt64ArrayConstructor,
    BigUint64ArrayConstructor,
    // ── TypedArray 新增原型方法 — 简单方法 ──
    TypedArrayProtoFill,
    TypedArrayProtoReverse,
    TypedArrayProtoIndexOf,
    TypedArrayProtoLastIndexOf,
    TypedArrayProtoIncludes,
    TypedArrayProtoJoin,
    TypedArrayProtoToString,
    TypedArrayProtoCopyWithin,
    TypedArrayProtoAt,
    // ── TypedArray 新增原型方法 — 回调方法 (Type 12) ──
    TypedArrayProtoForEach,
    TypedArrayProtoMap,
    TypedArrayProtoFilter,
    TypedArrayProtoReduce,
    TypedArrayProtoReduceRight,
    TypedArrayProtoFind,
    TypedArrayProtoFindIndex,
    TypedArrayProtoSome,
    TypedArrayProtoEvery,
    TypedArrayProtoSort,
    // ── TypedArray 迭代器方法 ──
    TypedArrayProtoEntries,
    TypedArrayProtoKeys,
    TypedArrayProtoValues,
    GetBuiltinGlobal,
    CreateGlobalObject,
    CreateException,
    ExceptionValue,
    // ── Eval exception check ──
    IsException,
    // ── new.target meta property ──
    NewTarget,
    // ── Arguments Exotic Object ──
    CreateUnmappedArgumentsObject,
    CreateMappedArgumentsObject,
    // ── ScopeRecord eval bridge ───────────────────────────────────────
    /// dest: i64 — scope record handle
    ScopeRecordCreate,
    /// args[0]: record, args[1]: name (string), args[2]: value (i64), args[3]: is_tdz (bool), args[4]: is_const (bool)
    ScopeRecordAddBinding,
    /// dest: i64 — value (or TAG_EXCEPTION if TDZ)
    EvalGetBinding,
    /// dest: i64 — written value
    EvalSetBinding,
    /// dest: i64 — bool (0 or 1)
    EvalHasBinding,
    /// dest: i64 — bool | TAG_EXCEPTION（DeleteBinding §9.1.1.1.8：调用方声明式
    /// 绑定 false；with 层 / 全局对象属性按 [[Delete]]；不可解析名 true）
    EvalDeleteBinding,
    /// dest: i64 — prototype | undefined | TAG_EXCEPTION
    EvalSuperBase,
    /// args[0]: record, args[1]: key (i64 integer tag), args[2]: value (i64)
    ScopeRecordSetMeta,
    /// args[0]: record — removes the scope record from the runtime map
    ScopeRecordDestroy,
    // ── WHATWG Streams ──
    ReadableStreamConstructor,
    WritableStreamConstructor,
    TransformStreamConstructor,
    CountQueuingStrategyConstructor,
    ByteLengthQueuingStrategyConstructor,
    // ── structuredClone ──
    StructuredClone,
    // ── 全局 Number 强制转换函数 ──
    GlobalIsNaN,
    GlobalIsFinite,
    // ── Symbol.prototype ──
    SymbolProtoToString,
    SymbolProtoValueOf,
    // ── RegExp well-known symbol methods ──
    RegExpProtoMatch,
    RegExpProtoMatchAll,
    RegExpProtoReplace,
    RegExpProtoSearch,
    RegExpProtoSplit,
    // ── BigInt.prototype ──
    BigIntProtoToString,
    BigIntProtoValueOf,
    // ── 数组内联优化辅助 builtin（array_inline pass）──
    /// 按长度创建全 hole 数组（length=len），供 map 结果容器。
    ArrayAllocate,
    /// 判断数组索引处是否存在非 hole 元素。
    ArrayHasElement,
    /// 内联快路径守卫：值是否为裸真数组（不穿透 Proxy，区别于用户可见的
    /// `Array.isArray`）。Proxy 包装数组的 length/元素读取须走 trap 完整
    /// 协议，退回慢路径 builtin。
    ArrayIsPlain,
    /// 内联快路径守卫（map/filter）：裸真数组且 ArraySpeciesCreate
    /// （§23.1.3.2）保证走缺省 ArrayCreate 且全程不可观察——实例无自有
    /// constructor、[[Prototype]] 为当前 realm %Array.prototype% 且其
    /// constructor 仍为固有 %Array% 数据属性、%Array% 的 @@species 仍为
    /// 固有 getter。任一不满足退回慢路径 builtin 执行完整 species 协议。
    ArraySpeciesDefault,
    /// JS truthiness → bool。
    ToBoolean,
    /// `Object.prototype.propertyIsEnumerable`
    PropertyIsEnumerable,
    // ── 字符串累加器优化辅助 builtin（string_concat pass）──
    /// 向编译器证明不逃逸的局部字符串累加器追加片段。
    StringBuilderAppend,
    /// 在累加器首次可观察前冻结其可变缓冲区。
    StringBuilderFinish,
    /// NaN-box runtime value 的字符串类型守卫。
    IsString,
    /// TDZ 运行时检查：值为未初始化哨兵时抛 ReferenceError，否则原样返回。
    /// args: [value, name(字符串常量，用于错误消息)]。
    TdzCheck,
    /// ECMAScript ToPropertyKey（§7.1.19）：对象键经 ToPrimitive(string) 再入
    /// 用户 `toString` / `valueOf` / `Symbol.toPrimitive`，转换抛出的异常原样传播；
    /// 非对象输入原样返回。args: [key]。
    ToPropertyKey,
    /// with 语句对象环境记录的 HasBinding（§9.1.1.2.1）：
    /// `? HasProperty(bindings, N)` 后按 `@@unscopables` 过滤；Proxy trap /
    /// unscopables getter 抛出的异常原样传播。args: [object, name]。
    WithHasBinding,
    /// with 语句头部的 ToObject（§7.1.18）：null/undefined 抛 TypeError，
    /// 对象/可调用体原样返回，原语装箱为包装对象。args: [value]。
    WithToObject,
    /// 向 direct eval 的 ScopeRecord 追加一个 with 对象环境层（由内到外依次
    /// 追加）：`inner_names` 为声明于该层内侧、解析时先于该层命中的静态绑定名
    /// 集合（NUL 分隔字符串）。运行时 EvalGet/Set/HasBinding 按层序在静态绑定
    /// 与 with 对象之间正确插入对象环境记录。args: [record, object, inner_names]。
    ScopeRecordAddWithLayer,
    /// 读取 ScopeRecord 自有绑定的当前值（不经过 with 层与 outer 对象）。
    /// direct eval 结束后的绑定回写必须用平面读取：经 EvalGetBinding 会被
    /// with 层拦截，把 with 对象属性错误回写进调用方静态绑定。绑定处于 TDZ
    /// 时返回未初始化哨兵（写回哨兵即保持原槽 TDZ 状态——派生构造器 this
    /// 等动态 TDZ 绑定在 eval 后可能仍未初始化，经 EvalGetBinding 会抛
    /// ReferenceError 且异常编码会被写入槽位）；绑定缺失返回 undefined。
    /// args: [record, name]。
    ScopeRecordGetBinding,
    /// 解析名字在 ScopeRecord with 层链中的 this 基座（§9.1.1.2.10
    /// WithBaseObject）：命中某层对象环境时返回该 with 对象，被内侧静态绑定
    /// 遮蔽或全链未命中时返回 undefined。Proxy has trap / @@unscopables
    /// getter 异常原样传播。args: [record, name]。
    EvalWithBase,
    /// 派生构造器 this 的 GetThisBinding 检查（ES §9.1.1.3.4）：this 绑定
    /// 仍为未初始化哨兵（super() 尚未执行）时抛 ReferenceError，否则原样
    /// 返回。args: [value(当前 this 绑定)]。
    ThisTdzCheck,
    /// SuperCall 的 BindThisValue 步骤 2（ES §9.1.1.3.1）：this 绑定已初始化
    /// 说明 super() 已成功执行过一次，再次调用抛 ReferenceError；仍为未初始化
    /// 哨兵时原样返回。args: [value(当前 this 绑定)]。
    SuperCallOnceCheck,
    /// SetFunctionName（ES §10.2.9）的运行时形态：计算属性键的方法/访问器与
    /// 匿名函数定义在键求值后设置 `name`。args: [function, key(ToPropertyKey
    /// 后的字符串或 symbol), prefix(0=无 1="get " 2="set ")]。无返回值。
    FunctionSetName,
    /// `Function.prototype.toString`（ES §20.2.3.5）：有 [[SourceText]] 的
    /// 用户函数返回原始源码片段，内建/bound/proxy 返回 NativeFunction 形态
    /// `function <name>() { [native code] }`，非 callable this 抛 TypeError。
    /// args: [this]。
    FunctionToString,
    // ── 全局环境记录（ES §9.1.1.4，脚本模式 GlobalDeclarationInstantiation）──
    /// GlobalDeclarationInstantiation（§16.1.7）步骤 1–6 的声明冲突预检：
    /// kind=0（词法名）检查 HasVarDeclaration / HasLexicalDeclaration /
    /// HasRestrictedGlobalProperty，kind=1（var/函数名）检查
    /// HasLexicalDeclaration。冲突抛 SyntaxError。args: [global, name, kind]。
    GlobalEnvCheck,
    /// CreateGlobalVarBinding（§9.1.1.4.17）：全局对象无同名自有属性且可扩展时
    /// 定义 {undefined, writable, enumerable, configurable=args[2]} 数据属性，
    /// 并把名字计入 [[VarNames]]。args: [global, name, configurable]。
    GlobalEnvDeclareVar,
    /// CreateGlobalFunctionBinding（§9.1.1.4.18）：按既有属性可配置性定义/更新
    /// 全局函数属性（脚本级恒 configurable=false），并计入 [[VarNames]]。
    /// args: [global, name, value]。
    GlobalEnvDeclareFunc,
    /// 全局声明式记录 CreateMutableBinding / CreateImmutableBinding：创建
    /// 未初始化（TDZ）词法绑定。args: [global, name, is_const]。
    GlobalEnvDeclareLex,
    /// 全局声明式记录 InitializeBinding：写入初值并解除 TDZ。
    /// args: [global, name, value]。
    GlobalEnvInitLex,
    /// 全局环境 ResolveBinding + GetValue：先查声明式记录（TDZ 抛
    /// ReferenceError），再落全局对象属性；均未命中时 flags bit0（typeof 容忍）
    /// 置位返回 undefined，否则抛 "x is not defined"。args: [global, name, flags]。
    GlobalEnvGet,
    /// 全局环境 SetMutableBinding / PutValue：声明式记录命中时检查 TDZ 与
    /// const（TypeError "Assignment to constant variable."）；否则按对象记录
    /// 语义写属性——strict（args[3]）且属性不存在抛 ReferenceError，sloppy
    /// 创建 configurable 隐式全局。args: [global, name, value, strict]。
    GlobalEnvSet,
    /// 全局环境 DeleteBinding：声明式记录绑定不可删除（false）；否则按全局
    /// 对象 [[Delete]] 返回结果，属性缺失返回 true。args: [global, name]。
    GlobalEnvDelete,
    /// `DataView.prototype.getBigInt64`（ES §25.3.4）：按字节序读取 8 字节
    /// 有符号 64 位整数，返回 BigInt。args: [view, byteOffset, littleEndian?]。
    DataViewProtoGetBigInt64,
    /// `DataView.prototype.getBigUint64`（ES §25.3.4）：按字节序读取 8 字节
    /// 无符号 64 位整数，返回 BigInt。args: [view, byteOffset, littleEndian?]。
    DataViewProtoGetBigUint64,
    /// `DataView.prototype.setBigInt64`（ES §25.3.4）：把 BigInt 按 2^64 取模
    /// 写入 8 字节。args: [view, byteOffset, value, littleEndian?]。
    DataViewProtoSetBigInt64,
    /// `DataView.prototype.setBigUint64`（ES §25.3.4）：与 setBigInt64 写入
    /// 相同位型（ToBigUint64 与 ToBigInt64 的字节表示一致）。
    /// args: [view, byteOffset, value, littleEndian?]。
    DataViewProtoSetBigUint64,
    /// mapped arguments 形参绑定读取（ES §10.4.4 [[ParameterMap]] 的实现侧）：
    /// 索引仍在 map 中时读 arguments 对象自有索引属性（映射期间该属性即绑定
    /// 真值），已解除映射（defineProperty 降级 / delete / freeze）后读侧表中
    /// 快照出的独立绑定槽。args: [arguments, index]。
    MappedArgumentsBindingRead,
    /// mapped arguments 形参绑定写入：索引在 map 中时写 arguments 对象自有
    /// 索引属性（对 `arguments[i]` 立即可见），解除映射后写侧表绑定槽。
    /// args: [arguments, index, value]，dest 回传写入值。
    MappedArgumentsBindingWrite,
    /// `Object.prototype.isPrototypeOf`（ES §20.1.3.3）：V 非对象直接 false，
    /// 否则 ToObject(this) 后沿 V 的 [[GetPrototypeOf]] 链查 SameValue。
    /// args: [this, value]。
    ObjectProtoIsPrototypeOf,
    /// `Object.prototype.toLocaleString`（ES §20.1.3.5）：等价于
    /// `Invoke(this, "toString")`，转发到 this 的 toString。args: [this]。
    ObjectProtoToLocaleString,
    /// `get Object.prototype.__proto__`（ES §B.2.2.1.1）：ToObject(this) 后
    /// 返回 [[GetPrototypeOf]]()。args: [this]。
    ObjectProtoGetProto,
    /// `set Object.prototype.__proto__`（ES §B.2.2.1.2）：RequireObjectCoercible
    /// 后仅对象接收者且 proto 为对象/null 时执行 [[SetPrototypeOf]]，失败抛
    /// TypeError，恒返回 undefined。args: [this, proto]。
    ObjectProtoSetProto,
    /// `Object.prototype.__defineGetter__`（ES §B.2.2.2）：getter 非 callable
    /// 抛 TypeError，否则定义 {get, enumerable: true, configurable: true}。
    /// args: [this, key, getter]。
    ObjectProtoDefineGetter,
    /// `Object.prototype.__defineSetter__`（ES §B.2.2.3）：setter 非 callable
    /// 抛 TypeError，否则定义 {set, enumerable: true, configurable: true}。
    /// args: [this, key, setter]。
    ObjectProtoDefineSetter,
    /// `Object.prototype.__lookupGetter__`（ES §B.2.2.4）：沿原型链查首个自有
    /// 属性，访问器返回其 [[Get]]，数据属性返回 undefined。args: [this, key]。
    ObjectProtoLookupGetter,
    /// `Object.prototype.__lookupSetter__`（ES §B.2.2.5）：沿原型链查首个自有
    /// 属性，访问器返回其 [[Set]]，数据属性返回 undefined。args: [this, key]。
    ObjectProtoLookupSetter,
    /// `String.raw`（ES §22.1.2.4）：按 template.raw 的 length 交替拼接原始
    /// 文本段与替换值；template 或其 raw 属性为 undefined/null 时抛
    /// TypeError（ToObject 失败）。args: [template, ...substitutions]。
    StringRaw,
    /// intrinsic 调用快路径守卫：判定站点对应的 intrinsic 属性仍处于
    /// 原始（pristine）状态——未被赋值覆盖、未被 delete、未被换成访问器、
    /// 容器全局名未被运行时遮蔽。守卫为纯查询，无任何可观察副作用，也不
    /// 新增字符串驻留（属性名由宿主按 wire_id 经 `intrinsic_sites` 反查）；
    /// 返回 bool，false 时语义层落入通用属性查找 + 动态调用路径。
    /// args: [family(常量，见 `constants::INTRINSIC_FAMILY_*`), wire_id,
    /// receiver?(仅 STRING_PROTO / ARRAY_PROTO)]。
    IntrinsicPristine,
    /// intrinsic 慢路径的 callee/容器解析：按站点家族以完整属性语义解析
    /// （全局名经 GlobalEnvGet 语义，缺失抛 ReferenceError；成员经通用
    /// [[Get]]，getter 生效），返回解析出的值或异常。属性名同样由宿主经
    /// `intrinsic_sites` 反查，不进制品常量池。
    /// args: [family, wire_id] 解析全局名（GLOBAL_IDENT 为站点全局名、
    /// STATIC_MEMBER 为容器全局名）；[family, wire_id, receiver] 解析
    /// receiver 上的站点属性成员。
    IntrinsicResolve,
    /// 全局 `EventTarget` 构造器（WHATWG DOM §2.7）：创建空监听器列表的
    /// 事件目标对象。args: []（实参忽略）。
    EventTargetConstructor,
    /// 全局 `AbortSignal` 接口对象（WHATWG DOM §3.2）：不可直接构造，
    /// [[Construct]] 恒抛 TypeError "Illegal constructor"。
    AbortSignalConstructor,
    /// 全局 `Event` 构造器（WHATWG DOM §2.5）：type 必选（ToString），
    /// options 字典读 bubbles / cancelable / composed。args: [type, options?]。
    EventConstructor,
    /// throw completion 语义的 IteratorClose（ES §7.4.6 步骤 5）：completion
    /// 为 throw 时原始异常胜出——return 方法查找抛出、非 callable、调用抛出、
    /// 返回非对象全部吞咽，恒返回 completion；宿主内部 invariant 失败仍以
    /// 异常哨兵上浮。`IteratorClose` 保留给非 throw 完成（break/return/正常
    /// 关闭），其 return() 错误按步骤 6/7 传播。args: [iterator, completion]。
    IteratorCloseThrowCompletion,
    /// 把命名空间对象收口为 Module Namespace Exotic Object（§10.4.6）：
    /// [[Prototype]] 置 null、标记不可扩展、登记宿主侧命名空间身份
    /// （[[Set]] 恒 false、导出经 [[GetOwnProperty]] 呈现为
    /// writable=true 数据描述符等）。args: [namespace]。
    FinalizeModuleNamespace,
    /// `String.prototype.isWellFormed`（ES §22.1.3.10）：ToString(this) 后按
    /// IsStringWellFormedUnicode 判定 UTF-16 码元序列是否无孤立代理项。
    StringIsWellFormed,
    /// `String.prototype.toWellFormed`（ES §22.1.3.33）：ToString(this) 后把
    /// 每个孤立代理项替换为 U+FFFD，返回良构副本。
    StringToWellFormed,
    /// `Array.fromAsync`（ES2024 §23.1.2.1）：返回 promise 的异步收集，宿主
    /// 状态机经微任务驱动 async/sync 迭代器或 array-like 的逐元素 Await。
    ArrayFromAsync,
    /// `Set.prototype.union`（ES §24.2.4.16）。
    SetProtoUnion,
    /// `Set.prototype.intersection`（ES §24.2.4.9）。
    SetProtoIntersection,
    /// `Set.prototype.difference`（ES §24.2.4.5）。
    SetProtoDifference,
    /// `Set.prototype.symmetricDifference`（ES §24.2.4.15）。
    SetProtoSymmetricDifference,
    /// `Set.prototype.isSubsetOf`（ES §24.2.4.10）。
    SetProtoIsSubsetOf,
    /// `Set.prototype.isSupersetOf`（ES §24.2.4.11）。
    SetProtoIsSupersetOf,
    /// `Set.prototype.isDisjointFrom`（ES §24.2.4.12）。
    SetProtoIsDisjointFrom,
    /// 取回已注册的 canonical 模块命名空间对象（§10.4.6.12 GetModuleNamespace
    /// 缓存）：按 (当前 image, ModuleId) 解析运行时模块键并读命名空间缓存。
    /// builtin 段与用户段拆分 image 时，段内 `import * as` 的命名空间由
    /// `$builtin_main` 创建注册，用户段经本 builtin 取回同一对象而非重建。
    /// args: [module_id]。
    GetModuleNamespace,
    /// `get DataView.prototype.buffer`（ES §25.3.4.1）：规范 accessor getter。
    DataViewProtoBuffer,
    /// `get DataView.prototype.byteLength`（ES §25.3.4.2）。
    DataViewProtoByteLength,
    /// `get DataView.prototype.byteOffset`（ES §25.3.4.3）。
    DataViewProtoByteOffset,
    /// sync `yield*` 收到 throw completion 的向内转发（§27.5.3.7 步骤 7.b）：
    /// GetMethod(iterator, "throw") 后调用并把结果对象缓存进委托迭代器条目
    /// （done/current），供 header 的 IteratorDone/IteratorValue 续走；方法
    /// 缺失时按步骤 7.b.iii 先 IteratorClose（normal completion，close 错误
    /// 传播）再返回 TypeError 异常。args: [iterator, thrown]。
    IteratorDelegateThrow,
    /// sync `yield*` 收到 return completion 的向内转发（§27.5.3.7 步骤 7.c）：
    /// GetMethod(iterator, "return") 缺失时返回 undefined 哨兵（语义层按
    /// 步骤 7.c.iii 直接 ReturnCompletion(received)），否则调用并缓存结果
    /// 对象后原样返回（结果恒为对象，与哨兵不混淆）。args: [iterator, value]。
    IteratorDelegateReturn,
    /// async `yield*` 收到 throw completion 的向内转发同步段（§27.5.3.7
    /// 步骤 7.b 的 async 形态）：返回 {k, v} 标记对象——k=0 为 throw 方法
    /// 调用结果（语义层 Await 后按 done 分支）、k=1 为方法缺失时 return()
    /// 的 close 结果（Await 后校验对象再抛缺方法 TypeError）；throw 与
    /// return 皆缺时直接返回 TypeError 异常。args: [iterator, thrown]。
    AsyncIteratorDelegateThrow,
    /// async `yield*` 收到 return completion 的向内转发同步段（§27.5.3.7
    /// 步骤 7.c 的 async 形态）：返回 {k, v} 标记对象——k=0 为 return 方法
    /// 调用结果（语义层 Await 后按 done 分支）、k=2 为方法缺失（语义层
    /// Await(received) 后 ReturnCompletion，步骤 7.c.iii）。args: [iterator, value]。
    AsyncIteratorDelegateReturn,
    /// IteratorComplete 前置的结果对象校验（§7.4.4 步骤 1 的 Object 断言）：
    /// 非对象返回 TypeError（V8 kIteratorResultNotAnObject 文案），否则原样
    /// 返回。async `yield*` 转发 Await 后由语义层调用。args: [result]。
    IteratorResultRequireObject,
    /// `yield*` throw 方法缺失的 TypeError 错误对象（§27.5.3.7 步骤 7.b.iii.6，
    /// V8 kThrowMethodMissing 文案）：async 形态的 close Await 完成后由语义层
    /// 取错误对象并抛出。args: []。
    IteratorThrowMethodMissingError,
}

/// 把 `Builtin` 变体直接映射到宿主 handler 的跳表宏。
///
/// 展开为对 `builtin` 的穷尽 match：每个变体调用对应的 handler（通常是一个
/// `dispatch_*` 模块入口，返回 `Option<i64>`），handler 未认领（`None`）时落到
/// `$fallback` 表达式。`wire_id()` 是枚举判别值（`repr(u16)`、从 0 连续），
/// 编译器会把该 match 编译成跳表 / 二分，替代逐模块线性探测链。
///
/// 穷尽性由编译器保证：新增 `Builtin` 变体而未在表中登记会直接编译失败。
#[macro_export]
macro_rules! dispatch_jumptable {
    ($builtin:ident, ($ctx:ident, $state:ident, $args:ident) $fallback:block => {
        $($module:path => $($variant:path)|+ $(,)?)+
    }) => {
        match $builtin {
            $($(
                $variant => {
                    let result = $module($ctx, $state, $builtin, $args);
                    if let Some(value) = result { value } else { $fallback }
                }
            )+)+
            _ => $fallback,
        }
    };
}

impl Builtin {
    /// 返回 portable artifact 使用的稳定宽度 builtin ID。
    pub const fn wire_id(self) -> u16 {
        self as u16
    }

    /// 返回当前 portable artifact 可识别的最后一个 builtin ID。
    pub const fn last_wire_id() -> u16 {
        Self::IteratorThrowMethodMissingError as u16
    }

    /// 从 portable artifact 的 builtin ID 恢复枚举。
    pub fn from_wire_id(id: u16) -> Option<Self> {
        if id > Self::last_wire_id() {
            return None;
        }

        // SAFETY: `Builtin` 使用 `repr(u16)`，全部 variant 连续且上界已检查。
        Some(unsafe { std::mem::transmute::<u16, Self>(id) })
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            // Console / debugger / 迭代器 / 枚举器 / 运算符 / 属性描述符
            Self::ConsoleLog => "console.log",
            Self::ConsoleError => "console.error",
            Self::ConsoleWarn => "console.warn",
            Self::ConsoleInfo => "console.info",
            Self::ConsoleDebug => "console.debug",
            Self::ConsoleTrace => "console.trace",
            Self::Debugger => "debugger",
            Self::Throw => "throw",
            Self::IteratorFrom => "iterator.from",
            Self::IteratorNext => "iterator.next",
            Self::IteratorClose => "iterator.close",
            Self::AsyncIteratorFrom => "async_iterator.from",
            Self::IteratorValue => "iterator.value",
            Self::IteratorStepValue => "iterator.step_value",
            Self::IteratorDone => "iterator.done",
            Self::EnumeratorFrom => "enumerator.from",
            Self::EnumeratorNext => "enumerator.next",
            Self::EnumeratorKey => "enumerator.key",
            Self::EnumeratorDone => "enumerator.done",
            Self::TypeOf => "typeof",
            Self::In => "op_in",
            Self::InstanceOf => "op_instanceof",
            Self::StrictEq => "strict_eq",
            Self::AbstractEq => "abstract_eq",
            Self::AbstractCompare => "abstract_compare",
            Self::DefineProperty => "define_property",
            Self::GetOwnPropDesc => "get_own_prop_desc",

            // 宿主 API（setTimeout / fetch / eval / JSON / closure）
            Self::SetTimeout => "setTimeout",
            Self::ClearTimeout => "clearTimeout",
            Self::SetInterval => "setInterval",
            Self::ClearInterval => "clearInterval",
            Self::Fetch => "fetch",
            Self::HeadersConstructor => "Headers",
            Self::RequestConstructor => "Request",
            Self::ResponseConstructor => "Response",
            Self::AbortControllerConstructor => "AbortController",
            Self::Eval => "eval",
            Self::EvalIndirect => "eval.indirect",
            Self::EvalResult => "eval.result",
            Self::JsonStringify => "JSON.stringify",
            Self::JsonParse => "JSON.parse",
            Self::CreateClosure => "create_closure",

            // 数组方法
            Self::ArrayPush => "array.push",
            Self::ArrayPushHole => "array.push_hole",
            Self::ArrayPushSpread => "array.push_spread",
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
            Self::ArrayFrom => "array.from",
            Self::ArraySpliceVa => "array.splice_va",
            Self::ArrayConcatVa => "array.concat_va",
            Self::ArrayOf => "array.of",
            Self::ArrayFindLast => "array.find_last",
            Self::ArrayFindLastIndex => "array.find_last_index",
            Self::ArrayLastIndexOf => "array.last_index_of",
            Self::ArrayToSorted => "array.to_sorted",
            Self::ArrayToReversed => "array.to_reversed",
            Self::ArrayToSplicedVa => "array.to_spliced_va",
            Self::ArrayWith => "array.with",

            // 函数原型方法 / super 数组调用 / 对象方法 / Map.groupBy
            Self::FuncCall => "func_call",
            Self::FuncApply => "func_apply",
            Self::SuperApply => "super_apply",
            Self::FuncBind => "func_bind",
            Self::ObjectRest => "object_rest",
            Self::GetPrototypeFromConstructor => "get_prototype_from_constructor",
            Self::HasOwnProperty => "has_own_property",
            Self::PrivateGet => "private_get",
            Self::PrivateSet => "private_set",
            Self::PrivateHas => "private_has",
            Self::PrivateAccessorBind => "private_accessor_bind",
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
            Self::ObjectGetOwnPropertySymbols => "object.get_own_property_symbols",
            Self::ObjectIs => "object.is",
            Self::ObjectGroupBy => "object.group_by",
            Self::ObjectHasOwn => "object.has_own",
            Self::ObjectFreeze => "object.freeze",
            Self::ObjectSeal => "object.seal",
            Self::ObjectIsFrozen => "object.is_frozen",
            Self::ObjectIsSealed => "object.is_sealed",
            Self::ObjectIsExtensible => "object.is_extensible",
            Self::ObjectPreventExtensions => "object.prevent_extensions",
            Self::ObjectFromEntries => "object.from_entries",
            Self::ObjectGetOwnPropertyDescriptors => "object.get_own_property_descriptors",
            Self::ObjectDefineProperties => "object.define_properties",
            Self::MapGroupBy => "map.group_by",

            // BigInt / Symbol / RegExp / 基础字符串方法
            Self::BigIntFromLiteral => "bigint.from_literal",
            Self::BigIntAdd => "bigint.add",
            Self::BigIntSub => "bigint.sub",
            Self::BigIntMul => "bigint.mul",
            Self::BigIntDiv => "bigint.div",
            Self::BigIntMod => "bigint.mod",
            Self::BigIntPow => "bigint.pow",
            Self::BigIntNeg => "bigint.neg",
            Self::BigIntBitAnd => "bigint.bit_and",
            Self::BigIntBitOr => "bigint.bit_or",
            Self::BigIntBitXor => "bigint.bit_xor",
            Self::BigIntShl => "bigint.shl",
            Self::BigIntShr => "bigint.shr",
            Self::BigIntBitNot => "bigint.bit_not",
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

            // Promise / async / continuation / generator
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
            Self::AsyncGeneratorThrow => "async_generator.throw",
            Self::GeneratorStart => "generator.start",
            Self::GeneratorNext => "generator.next",
            Self::GeneratorReturn => "generator.return",
            Self::GeneratorThrow => "generator.throw",
            Self::PromiseWithResolvers => "promise.with_resolvers",
            Self::IsCallable => "is_callable",
            Self::IsJsObject => "is_js_object",

            // 动态 import / import.meta / JSX / Proxy / Reflect / 完整字符串方法
            Self::DynamicImport => "dynamic_import",
            Self::DynamicImportRuntime => "dynamic_import_runtime",
            Self::ImportMetaResolve => "import_meta.resolve",
            Self::RegisterModuleNamespace => "register_module_namespace",
            Self::FinalizeModuleNamespace => "finalize_module_namespace",
            Self::GetModuleNamespace => "get_module_namespace",
            Self::CjsCreateRequire => "cjs.create_require",
            Self::CjsRegisterModule => "cjs.register_module",
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
            Self::StringAt => "string.at",
            Self::StringCharAt => "string.char_at",
            Self::StringCharCodeAt => "string.char_code_at",
            Self::StringCodePointAt => "string.code_point_at",
            Self::StringConcatVa => "string.concat_va",
            Self::StringEndsWith => "string.ends_with",
            Self::StringIncludes => "string.includes",
            Self::StringIndexOf => "string.index_of",
            Self::StringLastIndexOf => "string.last_index_of",
            Self::StringMatchAll => "string.match_all",
            Self::StringPadEnd => "string.pad_end",
            Self::StringPadStart => "string.pad_start",
            Self::StringRepeat => "string.repeat",
            Self::StringReplaceAll => "string.replace_all",
            Self::StringSlice => "string.slice",
            Self::StringStartsWith => "string.starts_with",
            Self::StringNormalize => "string.normalize",
            Self::StringSubstring => "string.substring",
            Self::StringToLowerCase => "string.to_lower_case",
            Self::StringToUpperCase => "string.to_upper_case",
            Self::StringTrim => "string.trim",
            Self::StringTrimEnd => "string.trim_end",
            Self::StringTrimStart => "string.trim_start",
            Self::StringToString => "string.to_string",
            Self::StringValueOf => "string.value_of",
            Self::StringIterator => "string.iterator",
            Self::StringFromCharCode => "string.from_char_code",
            Self::StringFromCodePoint => "string.from_code_point",

            // Math 静态方法
            Self::MathAbs => "Math.abs",
            Self::MathAcos => "Math.acos",
            Self::MathAcosh => "Math.acosh",
            Self::MathAsin => "Math.asin",
            Self::MathAsinh => "Math.asinh",
            Self::MathAtan => "Math.atan",
            Self::MathAtanh => "Math.atanh",
            Self::MathAtan2 => "Math.atan2",
            Self::MathCbrt => "Math.cbrt",
            Self::MathCeil => "Math.ceil",
            Self::MathClz32 => "Math.clz32",
            Self::MathCos => "Math.cos",
            Self::MathCosh => "Math.cosh",
            Self::MathExp => "Math.exp",
            Self::MathExpm1 => "Math.expm1",
            Self::MathFloor => "Math.floor",
            Self::MathFround => "Math.fround",
            Self::MathHypot => "Math.hypot",
            Self::MathImul => "Math.imul",
            Self::MathLog => "Math.log",
            Self::MathLog1p => "Math.log1p",
            Self::MathLog10 => "Math.log10",
            Self::MathLog2 => "Math.log2",
            Self::MathMax => "Math.max",
            Self::MathMaxArray => "Math.max_array",
            Self::MathMin => "Math.min",
            Self::MathPow => "Math.pow",
            Self::MathRandom => "Math.random",
            Self::MathRound => "Math.round",
            Self::MathSign => "Math.sign",
            Self::MathSin => "Math.sin",
            Self::MathSinh => "Math.sinh",
            Self::MathSqrt => "Math.sqrt",
            Self::MathTan => "Math.tan",
            Self::MathTanh => "Math.tanh",
            Self::MathTrunc => "Math.trunc",

            // Number / Boolean / Error 内建对象
            Self::NumberConstructor => "Number",
            Self::NumberIsNaN => "Number.isNaN",
            Self::NumberIsFinite => "Number.isFinite",
            Self::NumberIsInteger => "Number.isInteger",
            Self::NumberIsSafeInteger => "Number.isSafeInteger",
            Self::NumberParseInt => "Number.parseInt",
            Self::NumberParseFloat => "Number.parseFloat",
            Self::NumberProtoToString => "Number.prototype.toString",
            Self::NumberProtoValueOf => "Number.prototype.valueOf",
            Self::NumberProtoToFixed => "Number.prototype.toFixed",
            Self::NumberProtoToExponential => "Number.prototype.toExponential",
            Self::NumberProtoToPrecision => "Number.prototype.toPrecision",
            Self::BooleanConstructor => "Boolean",
            Self::BooleanProtoToString => "Boolean.prototype.toString",
            Self::BooleanProtoValueOf => "Boolean.prototype.valueOf",
            Self::ErrorConstructor => "Error",
            Self::TypeErrorConstructor => "TypeError",
            Self::RangeErrorConstructor => "RangeError",
            Self::SyntaxErrorConstructor => "SyntaxError",
            Self::ReferenceErrorConstructor => "ReferenceError",
            Self::URIErrorConstructor => "URIError",
            Self::EvalErrorConstructor => "EvalError",
            Self::ErrorProtoToString => "Error.prototype.toString",

            // Map / Set / Date 内建对象
            Self::MapConstructor => "Map",
            Self::MapProtoSet => "Map.prototype.set",
            Self::MapProtoGet => "Map.prototype.get",
            Self::SetConstructor => "Set",
            Self::SetProtoAdd => "Set.prototype.add",
            Self::SetProtoHas => "Set.prototype.has",
            Self::SetProtoDelete => "Set.prototype.delete",
            Self::MapSetHas => "MapSet.has",
            Self::MapSetDelete => "MapSet.delete",
            Self::MapSetClear => "MapSet.clear",
            Self::MapSetGetSize => "MapSet.size",
            Self::MapSetForEach => "MapSet.forEach",
            Self::MapSetKeys => "MapSet.keys",
            Self::MapSetValues => "MapSet.values",
            Self::MapSetEntries => "MapSet.entries",
            Self::MapSetFirstKey => "MapSet.first_key",
            Self::DateConstructor => "Date",
            Self::DateConstructorNew => "new Date",
            Self::DateNow => "Date.now",
            Self::PerformanceNow => "performance.now",
            Self::DateParse => "Date.parse",
            Self::DateUTC => "Date.UTC",

            // WeakMap / WeakSet / SharedArrayBuffer / Atomics / WeakRef / FinalizationRegistry
            Self::WeakMapConstructor => "WeakMap",
            Self::WeakMapProtoSet => "WeakMap.prototype.set",
            Self::WeakMapProtoGet => "WeakMap.prototype.get",
            Self::WeakMapProtoHas => "WeakMap.prototype.has",
            Self::WeakMapProtoDelete => "WeakMap.prototype.delete",
            Self::WeakSetConstructor => "WeakSet",
            Self::WeakSetProtoAdd => "WeakSet.prototype.add",
            Self::WeakSetProtoHas => "WeakSet.prototype.has",
            Self::WeakSetProtoDelete => "WeakSet.prototype.delete",
            Self::SharedArrayBufferConstructor => "SharedArrayBuffer",
            Self::SharedArrayBufferProtoByteLength => "SharedArrayBuffer.prototype.byteLength",
            Self::SharedArrayBufferProtoGrow => "SharedArrayBuffer.prototype.grow",
            Self::SharedArrayBufferProtoGrowable => "SharedArrayBuffer.prototype.growable",
            Self::SharedArrayBufferProtoMaxByteLength => {
                "SharedArrayBuffer.prototype.maxByteLength"
            }
            Self::SharedArrayBufferProtoSlice => "SharedArrayBuffer.prototype.slice",
            Self::SharedArrayBufferSpecies => "SharedArrayBuffer[Symbol.species]",
            Self::AtomicsLoad => "Atomics.load",
            Self::AtomicsStore => "Atomics.store",
            Self::AtomicsAdd => "Atomics.add",
            Self::AtomicsSub => "Atomics.sub",
            Self::AtomicsAnd => "Atomics.and",
            Self::AtomicsOr => "Atomics.or",
            Self::AtomicsXor => "Atomics.xor",
            Self::AtomicsExchange => "Atomics.exchange",
            Self::AtomicsCompareExchange => "Atomics.compareExchange",
            Self::AtomicsIsLockFree => "Atomics.isLockFree",
            Self::AtomicsPause => "Atomics.pause",
            Self::AtomicsWait => "Atomics.wait",
            Self::AtomicsNotify => "Atomics.notify",
            Self::AtomicsWaitAsync => "Atomics.waitAsync",
            Self::WeakRefConstructor => "WeakRef",
            Self::WeakRefProtoDeref => "WeakRef.prototype.deref",
            Self::FinalizationRegistryConstructor => "FinalizationRegistry",
            Self::FinalizationRegistryProtoRegister => "FinalizationRegistry.prototype.register",
            Self::FinalizationRegistryProtoUnregister => {
                "FinalizationRegistry.prototype.unregister"
            }

            // ArrayBuffer / DataView / TypedArray
            Self::ArrayBufferConstructor => "ArrayBuffer",
            Self::ArrayBufferProtoByteLength => "ArrayBuffer.prototype.byteLength",
            Self::ArrayBufferProtoSlice => "ArrayBuffer.prototype.slice",
            Self::ArrayBufferProtoResize => "ArrayBuffer.prototype.resize",
            Self::ArrayBufferProtoTransfer => "ArrayBuffer.prototype.transfer",
            Self::ArrayBufferProtoTransferToFixedLength => {
                "ArrayBuffer.prototype.transferToFixedLength"
            }
            Self::ArrayBufferProtoResizable => "ArrayBuffer.prototype.resizable",
            Self::ArrayBufferProtoMaxByteLength => "ArrayBuffer.prototype.maxByteLength",
            Self::ArrayBufferProtoDetached => "ArrayBuffer.prototype.detached",
            Self::DataViewConstructor => "DataView",
            Self::DataViewProtoGetFloat64 => "DataView.prototype.getFloat64",
            Self::DataViewProtoGetFloat32 => "DataView.prototype.getFloat32",
            Self::DataViewProtoGetInt32 => "DataView.prototype.getInt32",
            Self::DataViewProtoGetUint32 => "DataView.prototype.getUint32",
            Self::DataViewProtoGetInt16 => "DataView.prototype.getInt16",
            Self::DataViewProtoGetUint16 => "DataView.prototype.getUint16",
            Self::DataViewProtoGetInt8 => "DataView.prototype.getInt8",
            Self::DataViewProtoGetUint8 => "DataView.prototype.getUint8",
            Self::DataViewProtoSetFloat64 => "DataView.prototype.setFloat64",
            Self::DataViewProtoSetFloat32 => "DataView.prototype.setFloat32",
            Self::DataViewProtoSetInt32 => "DataView.prototype.setInt32",
            Self::DataViewProtoSetUint32 => "DataView.prototype.setUint32",
            Self::DataViewProtoSetInt16 => "DataView.prototype.setInt16",
            Self::DataViewProtoSetUint16 => "DataView.prototype.setUint16",
            Self::DataViewProtoSetInt8 => "DataView.prototype.setInt8",
            Self::DataViewProtoSetUint8 => "DataView.prototype.setUint8",
            Self::DataViewProtoBuffer => "DataView.prototype.buffer",
            Self::DataViewProtoByteLength => "DataView.prototype.byteLength",
            Self::DataViewProtoByteOffset => "DataView.prototype.byteOffset",
            Self::Int8ArrayConstructor => "Int8Array",
            Self::Uint8ArrayConstructor => "Uint8Array",
            Self::Uint8ClampedArrayConstructor => "Uint8ClampedArray",
            Self::Int16ArrayConstructor => "Int16Array",
            Self::Uint16ArrayConstructor => "Uint16Array",
            Self::Int32ArrayConstructor => "Int32Array",
            Self::Uint32ArrayConstructor => "Uint32Array",
            Self::Float32ArrayConstructor => "Float32Array",
            Self::Float64ArrayConstructor => "Float64Array",
            Self::TypedArrayProtoLength => "TypedArray.prototype.length",
            Self::TypedArrayProtoByteLength => "TypedArray.prototype.byteLength",
            Self::TypedArrayProtoByteOffset => "TypedArray.prototype.byteOffset",
            Self::TypedArrayProtoSet => "TypedArray.prototype.set",
            Self::TypedArrayProtoSlice => "TypedArray.prototype.slice",
            Self::TypedArrayProtoSubarray => "TypedArray.prototype.subarray",
            Self::BigInt64ArrayConstructor => "BigInt64Array",
            Self::BigUint64ArrayConstructor => "BigUint64Array",
            Self::TypedArrayProtoFill => "TypedArray.prototype.fill",
            Self::TypedArrayProtoReverse => "TypedArray.prototype.reverse",
            Self::TypedArrayProtoIndexOf => "TypedArray.prototype.indexOf",
            Self::TypedArrayProtoLastIndexOf => "TypedArray.prototype.lastIndexOf",
            Self::TypedArrayProtoIncludes => "TypedArray.prototype.includes",
            Self::TypedArrayProtoJoin => "TypedArray.prototype.join",
            Self::TypedArrayProtoToString => "TypedArray.prototype.toString",
            Self::TypedArrayProtoCopyWithin => "TypedArray.prototype.copyWithin",
            Self::TypedArrayProtoAt => "TypedArray.prototype.at",
            Self::TypedArrayProtoForEach => "TypedArray.prototype.forEach",
            Self::TypedArrayProtoMap => "TypedArray.prototype.map",
            Self::TypedArrayProtoFilter => "TypedArray.prototype.filter",
            Self::TypedArrayProtoReduce => "TypedArray.prototype.reduce",
            Self::TypedArrayProtoReduceRight => "TypedArray.prototype.reduceRight",
            Self::TypedArrayProtoFind => "TypedArray.prototype.find",
            Self::TypedArrayProtoFindIndex => "TypedArray.prototype.findIndex",
            Self::TypedArrayProtoSome => "TypedArray.prototype.some",
            Self::TypedArrayProtoEvery => "TypedArray.prototype.every",
            Self::TypedArrayProtoSort => "TypedArray.prototype.sort",
            Self::TypedArrayProtoEntries => "TypedArray.prototype.entries",
            Self::TypedArrayProtoKeys => "TypedArray.prototype.keys",
            Self::TypedArrayProtoValues => "TypedArray.prototype.values",

            // 杂项：全局辅助、异常、eval 桥接、参数对象、作用域记录、WHATWG 流
            Self::GetBuiltinGlobal => "get_builtin_global",
            Self::CreateGlobalObject => "create_global_object",
            Self::CreateException => "create_exception",
            Self::ExceptionValue => "exception_value",
            Self::IsException => "is_exception",
            Self::NewTarget => "new.target",
            Self::CreateUnmappedArgumentsObject => "create_unmapped_arguments_object",
            Self::CreateMappedArgumentsObject => "create_mapped_arguments_object",
            Self::ScopeRecordCreate => "scope_record_create",
            Self::ScopeRecordAddBinding => "scope_record_add_binding",
            Self::EvalGetBinding => "eval_get_binding",
            Self::EvalSetBinding => "eval_set_binding",
            Self::EvalHasBinding => "eval_has_binding",
            Self::EvalDeleteBinding => "eval_delete_binding",
            Self::EvalSuperBase => "eval_super_base",
            Self::ScopeRecordSetMeta => "scope_record_set_meta",
            Self::ScopeRecordDestroy => "scope_record_destroy",
            Self::ReadableStreamConstructor => "ReadableStream",
            Self::WritableStreamConstructor => "WritableStream",
            Self::TransformStreamConstructor => "TransformStream",
            Self::CountQueuingStrategyConstructor => "CountQueuingStrategy",
            Self::ByteLengthQueuingStrategyConstructor => "ByteLengthQueuingStrategy",
            Self::StructuredClone => "structuredClone",
            Self::GlobalIsNaN => "isNaN",
            Self::GlobalIsFinite => "isFinite",
            Self::SymbolProtoToString => "Symbol.prototype.toString",
            Self::SymbolProtoValueOf => "Symbol.prototype.valueOf",
            Self::RegExpProtoMatch => "RegExp.prototype[@@match]",
            Self::RegExpProtoMatchAll => "RegExp.prototype[@@matchAll]",
            Self::RegExpProtoReplace => "RegExp.prototype[@@replace]",
            Self::RegExpProtoSearch => "RegExp.prototype[@@search]",
            Self::RegExpProtoSplit => "RegExp.prototype[@@split]",
            Self::BigIntProtoToString => "BigInt.prototype.toString",
            Self::BigIntProtoValueOf => "BigInt.prototype.valueOf",
            Self::ArrayAllocate => "array.allocate",
            Self::ArrayHasElement => "array.has_element",
            Self::ArrayIsPlain => "array.is_plain",
            Self::ArraySpeciesDefault => "array.species_default",
            Self::ToBoolean => "to_boolean",
            Self::PropertyIsEnumerable => "property_is_enumerable",
            Self::StringBuilderAppend => "string.builder_append",
            Self::StringBuilderFinish => "string.builder_finish",
            Self::IsString => "is_string",
            Self::TdzCheck => "tdz_check",
            Self::ToPropertyKey => "to_property_key",
            Self::WithHasBinding => "with.has_binding",
            Self::WithToObject => "with.to_object",
            Self::ScopeRecordAddWithLayer => "scope_record.add_with_layer",
            Self::ScopeRecordGetBinding => "scope_record.get_binding",
            Self::EvalWithBase => "eval.with_base",
            Self::ThisTdzCheck => "this_tdz_check",
            Self::SuperCallOnceCheck => "super_call_once_check",
            Self::FunctionSetName => "function.set_name",
            Self::FunctionToString => "function.to_string",
            Self::GlobalEnvCheck => "global_env.check",
            Self::GlobalEnvDeclareVar => "global_env.declare_var",
            Self::GlobalEnvDeclareFunc => "global_env.declare_func",
            Self::GlobalEnvDeclareLex => "global_env.declare_lex",
            Self::GlobalEnvInitLex => "global_env.init_lex",
            Self::GlobalEnvGet => "global_env.get",
            Self::GlobalEnvSet => "global_env.set",
            Self::GlobalEnvDelete => "global_env.delete",
            Self::DataViewProtoGetBigInt64 => "DataView.prototype.getBigInt64",
            Self::DataViewProtoGetBigUint64 => "DataView.prototype.getBigUint64",
            Self::DataViewProtoSetBigInt64 => "DataView.prototype.setBigInt64",
            Self::DataViewProtoSetBigUint64 => "DataView.prototype.setBigUint64",
            Self::MappedArgumentsBindingRead => "mapped_arguments_binding_read",
            Self::MappedArgumentsBindingWrite => "mapped_arguments_binding_write",
            Self::ObjectProtoIsPrototypeOf => "Object.prototype.isPrototypeOf",
            Self::ObjectProtoToLocaleString => "Object.prototype.toLocaleString",
            Self::ObjectProtoGetProto => "get Object.prototype.__proto__",
            Self::ObjectProtoSetProto => "set Object.prototype.__proto__",
            Self::ObjectProtoDefineGetter => "Object.prototype.__defineGetter__",
            Self::ObjectProtoDefineSetter => "Object.prototype.__defineSetter__",
            Self::ObjectProtoLookupGetter => "Object.prototype.__lookupGetter__",
            Self::ObjectProtoLookupSetter => "Object.prototype.__lookupSetter__",
            Self::StringRaw => "string.raw",
            Self::IntrinsicPristine => "intrinsic_pristine",
            Self::IntrinsicResolve => "intrinsic_resolve",
            Self::EventTargetConstructor => "EventTarget",
            Self::AbortSignalConstructor => "AbortSignal",
            Self::EventConstructor => "Event",
            Self::IteratorCloseThrowCompletion => "iterator.close_throw",
            Self::StringIsWellFormed => "string.is_well_formed",
            Self::StringToWellFormed => "string.to_well_formed",
            Self::ArrayFromAsync => "array.from_async",
            Self::SetProtoUnion => "Set.prototype.union",
            Self::SetProtoIntersection => "Set.prototype.intersection",
            Self::SetProtoDifference => "Set.prototype.difference",
            Self::SetProtoSymmetricDifference => "Set.prototype.symmetricDifference",
            Self::SetProtoIsSubsetOf => "Set.prototype.isSubsetOf",
            Self::SetProtoIsSupersetOf => "Set.prototype.isSupersetOf",
            Self::SetProtoIsDisjointFrom => "Set.prototype.isDisjointFrom",
            Self::IteratorDelegateThrow => "iterator.delegate_throw",
            Self::IteratorDelegateReturn => "iterator.delegate_return",
            Self::AsyncIteratorDelegateThrow => "async_iterator.delegate_throw",
            Self::AsyncIteratorDelegateReturn => "async_iterator.delegate_return",
            Self::IteratorResultRequireObject => "iterator.result_require_object",
            Self::IteratorThrowMethodMissingError => "iterator.throw_method_missing_error",
        }
    }
}

impl fmt::Display for Builtin {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}
