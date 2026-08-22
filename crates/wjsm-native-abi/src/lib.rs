//! Direct native compiler 与 runtime 共享的稳定 ABI 描述。
//!
//! 本 crate 只定义 generated-code-visible layout、symbol ID 与 signature；具体 runtime
//! 状态和 thunk 实现归 `wjsm-host-native`。

use std::collections::{BTreeSet, HashMap};
use std::ffi::c_void;
use std::mem::{align_of, offset_of, size_of};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicPtr, AtomicU64};

use sha2::{Digest, Sha256};
pub use wjsm_host::CallArgs;
use wjsm_ir::{Builtin, Instruction, Program};

pub const NATIVE_ABI_VERSION: u32 = 14;
pub const CALL_GATE_VERSION: u32 = 1;
pub const ROOT_FRAME_VERSION: u32 = 2;
pub const SOURCE_FRAME_VERSION: u32 = 1;
pub const BARRIER_VERSION: u32 = 2;
pub const NATIVE_BARRIER_YOUNG_MARKING_BIT: u64 = 1 << 6;
pub const NATIVE_BARRIER_OLD_MARKING_BIT: u64 = 1 << 7;
pub const NATIVE_BARRIER_MARKING_MASK: u64 =
    NATIVE_BARRIER_YOUNG_MARKING_BIT | NATIVE_BARRIER_OLD_MARKING_BIT;

/// Generated code 与 ZGC collector 共享的屏障热状态。
///
/// `phase` 与 `access_epoch` 由 collector 在 safepoint 发布；两个事件计数只由 owner
/// mutator 写入并在 safepoint 读取，避免属性快链上的原子 RMW。
#[derive(Default)]
#[repr(C)]
pub struct NativeBarrierState {
    pub phase: AtomicU64,
    pub access_epoch: AtomicU64,
    pub load_fast_events: u64,
    pub store_fast_events: u64,
}

/// `NativeVmContext::flags` 的位定义：bit0 置位时生成代码的守卫快路径才内联
/// 更新反馈槽（宿主在 runtime 构造时按 `specialization_enabled` 写入）。
pub const NATIVE_FLAGS_FEEDBACK_ENABLED: u32 = 1;

/// 每次循环回边生成的代码从 `NativeVmContext::stack_budget_bytes` 扣除的字节数。
///
/// 回边内联为「load + 饱和减 + 判零」；预算耗尽才真正调用
/// `NativeRuntimeOp::CooperativePoll`（宿主在其中重置预算并执行 inspector / GC /
/// 外部事件轮询）。步长越小轮询越频繁；取 64KiB 使 8MiB 初始预算在无外部事件的
/// 紧循环中约每 128 次回边轮询一次。
pub const COOPERATIVE_POLL_STEP_BYTES: usize = 64 * 1024;
/// 宿主 `CooperativePoll` 处理结束后重置到的预算值（见
/// [`crate::NativeVmContext::stack_budget_bytes`]）。
pub const COOPERATIVE_POLL_BUDGET: usize = 8 * 1024 * 1024;

/// Gate 无法分配时写入 vmctx 的预分配异常种类。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(u32)]
pub enum PendingExceptionKind {
    #[default]
    None = 0,
    StackOverflow = 1,
    CallArenaOverflow = 2,
    Terminated = 3,
    InternalInvariant = 4,
}

/// Compiler/runtime 共享的热上下文。所有 pointer 都指向 process-lifetime owner 所持状态。
#[derive(Debug)]
#[repr(C)]
pub struct NativeVmContext {
    pub abi_version: u32,
    pub flags: u32,
    pub agent_id: u64,
    pub control_flags: u32,
    pub js_call_depth: u32,
    pub pending_exception_kind: PendingExceptionKind,
    pub pending_source_len: u32,
    pub pending_source_capacity: u32,
    pub reserved0: u32,
    pub pending_source_slots: *mut NativeSourceSlot,
    pub heap_state: *mut c_void,
    pub allocation_state: *mut c_void,
    pub gc_state: *mut c_void,
    pub barrier_state: *mut c_void,
    pub call_arena_slots: *mut i64,
    pub call_arena_capacity: u32,
    pub call_arena_active_len: u32,
    pub function_table: *const NativeFunctionEntry,
    pub function_table_len: u32,
    pub current_table_base: u32,
    pub current_image_id: u64,
    pub call_frame_head: *mut NativeCallFrame,
    pub root_frame_head: AtomicPtr<NativeRootFrame>,
    pub source_frame_head: *mut NativeSourceFrame,
    pub stack_low: usize,
    pub stack_high: usize,
    pub stack_budget_bytes: usize,
    pub raw_access_depth: u32,
    pub suspend_status: u32,
    /// 当前 `ShapeTable` 的 proto 世代。宿主在每次 dispatcher 调用返回后同步；
    /// 生成代码用它校验 `kind=ProtoData/Accessor` 的 IC 槽是否仍有效。
    pub proto_generation: u32,
    /// handle region 基址（8 字节对齐）；generated code 用它把句柄下标换算成
    /// 8 字节 entry 地址。由宿主在 image 激活时与 `ic_slots_base` 同步设置。
    pub handle_table_base: *mut u8,
    /// Latin-1 单码元字符串的规范句柄表，固定 256 项，由宿主持有并作为 GC 根。
    /// generated code 仅在码元已证明位于 `0..=255` 后按 `unit * 8` 读取。
    pub latin1_char_strings: *const i64,
    /// 当前 image 的 IC 区基址（16 字节对齐）；无 IC 槽的 image 为 null。
    /// 槽大小 32 字节，槽内 `+0/+8/+16` 的 i64 load 仍满足 8 字节对齐。
    pub ic_slots_base: *mut u8,
    /// 当前 image 的类型反馈区基址（16 字节对齐）；无反馈槽的 image 为 null。
    /// 槽大小 [`wjsm_ir::constants::FEEDBACK_SLOT_SIZE`]；特化 overlay 永不激活为
    /// current image，其生成代码经由本基址继续写 base image 的反馈槽。
    pub feedback_slots_base: *mut u8,
    /// 对象地址的「逻辑 → 虚拟」偏移：handle entry 里的对象地址是 memory64
    /// 逻辑偏移，属性快链须加此值才能直接 load 真实映射。
    pub heap_object_delta: i64,
    /// 当前 image 的字符串常量盒装值数组基址（8 字节对齐，元素为 NaN-boxed
    /// 运行时字符串句柄，install 期发布后不再变化）；无字符串常量时为 null。
    /// 生成代码在函数入口按 `常量下标 * 8` 直读，替代旧的 MaterializeString
    /// 宿主往返。由宿主在 image 激活时与 `ic_slots_base` 同步设置。
    pub string_constants_base: *const i64,
}

impl Default for NativeVmContext {
    fn default() -> Self {
        Self {
            abi_version: NATIVE_ABI_VERSION,
            flags: 0,
            agent_id: 0,
            control_flags: 0,
            js_call_depth: 0,
            pending_exception_kind: PendingExceptionKind::None,
            pending_source_len: 0,
            pending_source_capacity: 0,
            reserved0: 0,
            pending_source_slots: std::ptr::null_mut(),
            heap_state: std::ptr::null_mut(),
            allocation_state: std::ptr::null_mut(),
            gc_state: std::ptr::null_mut(),
            barrier_state: std::ptr::null_mut(),
            call_arena_slots: std::ptr::null_mut(),
            call_arena_capacity: 0,
            call_arena_active_len: 0,
            function_table: std::ptr::null(),
            function_table_len: 0,
            current_table_base: 0,
            current_image_id: 0,
            call_frame_head: std::ptr::null_mut(),
            root_frame_head: AtomicPtr::new(std::ptr::null_mut()),
            source_frame_head: std::ptr::null_mut(),
            stack_low: 0,
            stack_high: 0,
            stack_budget_bytes: 0,
            raw_access_depth: 0,
            suspend_status: 0,
            proto_generation: 0,
            handle_table_base: std::ptr::null_mut(),
            latin1_char_strings: std::ptr::null(),
            ic_slots_base: std::ptr::null_mut(),
            feedback_slots_base: std::ptr::null_mut(),
            heap_object_delta: 0,
            string_constants_base: std::ptr::null(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct NativeSourceSlot {
    pub image_id: u64,
    pub function_index: u32,
    pub line: u32,
    pub column: u32,
    pub reserved: u32,
}

/// 反馈槽记录的值类别。判别值是稳定 5-bit 编码：NaN-box tag 直接沿用其
/// 数值（生成代码用 `(v >> 32) & 0x1f` 一步算出），原始 f64（未被 NaN-box 的
/// number）是独立 [`Self::Number`] 类别占用 `0x1f`，二者保证不碰撞。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum NativeFeedbackTag {
    NativeCallable = 0x00,
    String = 0x01,
    Undefined = 0x02,
    Null = 0x03,
    Boolean = 0x04,
    Exception = 0x05,
    Iterator = 0x06,
    Enumerator = 0x07,
    Object = 0x08,
    Function = 0x09,
    Closure = 0x0a,
    Array = 0x0b,
    Bound = 0x0c,
    BigInt = 0x0d,
    Symbol = 0x0e,
    RegExp = 0x0f,
    Proxy = 0x10,
    ScopeRecord = 0x11,
    ArrayHole = 0x12,
    /// 未知/未来新增的内部值类别。
    Other = 0x13,
    /// 原始 f64 number；生成代码 `select(is_number, 0x1f, tag)` 直接得出。
    Number = 0x1f,
}

impl NativeFeedbackTag {
    /// 按运行时编码判别一个 boxed 值的反馈类别。
    pub fn of(encoded: i64) -> Self {
        use wjsm_ir::value;
        if value::is_f64(encoded) {
            return Self::Number;
        }
        let tag = ((encoded as u64) >> 32) & value::TAG_MASK;
        match tag {
            value::TAG_UNDEFINED => Self::Undefined,
            value::TAG_NULL => Self::Null,
            value::TAG_BOOL => Self::Boolean,
            value::TAG_STRING => Self::String,
            value::TAG_BIGINT => Self::BigInt,
            value::TAG_SYMBOL => Self::Symbol,
            value::TAG_FUNCTION => Self::Function,
            value::TAG_CLOSURE => Self::Closure,
            value::TAG_BOUND => Self::Bound,
            value::TAG_NATIVE_CALLABLE => Self::NativeCallable,
            value::TAG_OBJECT => Self::Object,
            value::TAG_ARRAY => Self::Array,
            value::TAG_REGEXP => Self::RegExp,
            value::TAG_PROXY => Self::Proxy,
            value::TAG_EXCEPTION => Self::Exception,
            value::TAG_ITERATOR => Self::Iterator,
            value::TAG_ENUMERATOR => Self::Enumerator,
            value::TAG_SCOPE_RECORD => Self::ScopeRecord,
            value::TAG_ARRAY_HOLE => Self::ArrayHole,
            _ => Self::Other,
        }
    }

    pub const fn code(self) -> u8 {
        self as u8
    }
}

/// 把至多 [`wjsm_ir::constants::FEEDBACK_MAX_TAGS`] 个 tag 打包成 64 位签名：
/// 低 4 位是 tag 数，随后每个 tag 占 6 位（判别值 ≤ 31，6 位留出余量）。
pub fn encode_feedback_tag_signature(tags: &[NativeFeedbackTag]) -> u64 {
    let capped = tags
        .len()
        .min(wjsm_ir::constants::FEEDBACK_MAX_TAGS as usize);
    let mut signature = capped as u64;
    for (index, tag) in tags.iter().take(capped).enumerate() {
        signature |= u64::from(tag.code()) << (4 + 6 * index);
    }
    signature
}

/// 反馈槽的 `#[repr(C)]` 字段访问视图；布局常量见
/// `wjsm_ir::constants::FEEDBACK_SLOT_*`，二者由编译期断言绑定。
#[derive(Clone, Copy, Debug, Default)]
#[repr(C, align(16))]
pub struct NativeFeedbackSlot {
    pub last_target_image_id: u64,
    pub last_tag_signature: u64,
    pub consecutive_count: u32,
    pub total_count: u32,
    pub caller_function: u32,
    pub site_index: u32,
    pub last_target_function: u32,
    pub operation: u32,
    pub state: u32,
    pub reserved: u32,
}

#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct NativeFunctionEntry {
    pub slow_entry: NativeSlowEntry,
    pub local_function_id: u32,
    pub frame_bytes: u32,
    pub image_id: u64,
}

pub type NativeSlowEntry = unsafe extern "C" fn(
    ctx: *mut NativeVmContext,
    env: i64,
    this_value: i64,
    args_base: u32,
    args_count: u32,
) -> i64;

/// Native root frame 可表达的最大 slot 数；与 Cranelift `Offset32` 的 i64 slot 范围一致。
pub const MAX_NATIVE_ROOT_SLOTS: usize = i32::MAX as usize / size_of::<i64>();
/// Native root bitmap 可表达的最大 word 数。
pub const MAX_NATIVE_ROOT_BITMAP_WORDS: usize = MAX_NATIVE_ROOT_SLOTS.div_ceil(u64::BITS as usize);

#[derive(Debug)]
#[repr(C)]
pub struct NativeCallFrame {
    pub previous: *mut NativeCallFrame,
    pub image_id: u64,
    pub function_index: u32,
    pub table_base: u32,
}

/// Generated code 每个 safepoint 发布的 root 视图。
///
/// GC 扫描（`native_root_values`）只读 `previous / slots / bitmap_words /
/// bitmap_word_count`：`bitmap_words[..bitmap_word_count]` 中恰好置位 `0..root_count`，
/// 因此 `slots` 中索引 ≥ root_count 的槽永远不会被读取，generated code 无需清理它们。
#[derive(Debug)]
#[repr(C)]
pub struct NativeRootFrame {
    pub previous: *mut NativeRootFrame,
    pub slots: *mut i64,
    pub bitmap_words: *const u64,
    pub bitmap_word_count: u32,
}

#[derive(Debug)]
#[repr(C)]
pub struct NativeSourceFrame {
    pub previous: *mut NativeSourceFrame,
    pub image_id: u64,
    pub function_index: u32,
    pub source_position: u32,
}

/// Artifact 中的 semantic host operation ID。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(transparent)]
pub struct NativeHostOp(u16);

impl NativeHostOp {
    pub const fn from_builtin(builtin: Builtin) -> Self {
        Self(builtin.wire_id())
    }

    pub fn from_id(id: u16) -> Option<Self> {
        Builtin::from_wire_id(id).map(|_| Self(id))
    }

    pub const fn id(self) -> u16 {
        self.0
    }

    pub fn name(self) -> &'static str {
        Builtin::from_wire_id(self.0)
            .expect("NativeHostOp is constructed from a validated builtin ID")
            .as_str()
    }
}

/// 非 builtin IR 操作经同步 dispatcher 使用的稳定 operation ID。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum NativeRuntimeOp {
    BinaryAdd = 0x1_0000,
    BinarySub = 0x1_0001,
    BinaryMul = 0x1_0002,
    BinaryDiv = 0x1_0003,
    BinaryMod = 0x1_0004,
    BinaryExp = 0x1_0005,
    BinaryBitAnd = 0x1_0006,
    BinaryBitOr = 0x1_0007,
    BinaryBitXor = 0x1_0008,
    BinaryShl = 0x1_0009,
    BinaryShr = 0x1_000a,
    BinaryUShr = 0x1_000b,
    UnaryNot = 0x1_0100,
    UnaryNeg = 0x1_0101,
    UnaryPos = 0x1_0102,
    UnaryBitNot = 0x1_0103,
    UnaryVoid = 0x1_0104,
    UnaryIsNullish = 0x1_0105,
    IsTruthy = 0x1_0107,
    UnaryDelete = 0x1_0106,
    CompareStrictEq = 0x1_0200,
    CompareStrictNotEq = 0x1_0201,
    StoreVar = 0x1_0300,
    LoadVar = 0x1_0301,
    // 0x1_0400（原 MaterializeString）已随 install 期字符串常量发布退役。
    MaterializeBigInt = 0x1_0401,
    MaterializeRegExp = 0x1_0402,
    MaterializeFunction = 0x1_0403,
    CloneArrayTemplate = 0x1_0404,
    StringConcat = 0x1_0500,
    NewObject = 0x1_0501,
    GetProp = 0x1_0502,
    SetProp = 0x1_0503,
    /// 以 own data property 语义定义属性，绕过原型链 setter。
    CreateDataProperty = 0x1_0511,
    DeleteProp = 0x1_0504,
    SetProto = 0x1_0505,
    NewArray = 0x1_0506,
    GetElem = 0x1_0507,
    SetElem = 0x1_0508,
    ObjectSpread = 0x1_0509,
    OptionalGetProp = 0x1_050a,
    OptionalGetElem = 0x1_050b,
    GetSuperBase = 0x1_050c,
    GetSuperConstructor = 0x1_050d,
    /// 带 IC 槽回填的 [[Get]]：`[object, key, ic_slot_ptr]`，miss 时回填槽。
    GetPropIc = 0x1_050e,
    /// 带 IC 槽回填的 [[Set]]：`[object, key, value, ic_slot_ptr]`，miss 时回填槽。
    /// 成功写入自有数据属性后回填 `(shape_id, value_index)`，其余一律退化 MEGAMORPHIC。
    SetPropIc = 0x1_050f,
    /// accessor IC 命中后的直接 getter 调用：`[getter, receiver]`。
    /// 仅由 CLIF 快路径在 shape + 世代命中后使用，宿主不再查属性表。
    GetPropAccessor = 0x1_0510,
    PrepareCall = 0x1_0600,
    PrepareConstruct = 0x1_0606,
    FinishCall = 0x1_0601,
    LoadArgument = 0x1_0602,
    LoadCallEnv = 0x1_0603,
    PrepareSuperCall = 0x1_0607,
    PrepareSuperCallForward = 0x1_0608,
    CollectRestArguments = 0x1_0604,
    GuardSameFunction = 0x1_0605,
    CreateException = 0x1_0700,
    ExceptionValue = 0x1_0701,
    CooperativePoll = 0x1_0702,
    DebugCheck = 0x1_0703,
}

impl NativeRuntimeOp {
    pub const fn id(self) -> u32 {
        self as u32
    }
    pub const fn from_id(id: u32) -> Option<Self> {
        match id {
            0x1_0000 => Some(Self::BinaryAdd),
            0x1_0001 => Some(Self::BinarySub),
            0x1_0002 => Some(Self::BinaryMul),
            0x1_0003 => Some(Self::BinaryDiv),
            0x1_0004 => Some(Self::BinaryMod),
            0x1_0005 => Some(Self::BinaryExp),
            0x1_0006 => Some(Self::BinaryBitAnd),
            0x1_0007 => Some(Self::BinaryBitOr),
            0x1_0008 => Some(Self::BinaryBitXor),
            0x1_0009 => Some(Self::BinaryShl),
            0x1_000a => Some(Self::BinaryShr),
            0x1_000b => Some(Self::BinaryUShr),
            0x1_0100 => Some(Self::UnaryNot),
            0x1_0101 => Some(Self::UnaryNeg),
            0x1_0102 => Some(Self::UnaryPos),
            0x1_0103 => Some(Self::UnaryBitNot),
            0x1_0104 => Some(Self::UnaryVoid),
            0x1_0105 => Some(Self::UnaryIsNullish),
            0x1_0107 => Some(Self::IsTruthy),
            0x1_0106 => Some(Self::UnaryDelete),
            0x1_0200 => Some(Self::CompareStrictEq),
            0x1_0201 => Some(Self::CompareStrictNotEq),
            0x1_0300 => Some(Self::StoreVar),
            0x1_0301 => Some(Self::LoadVar),
            0x1_0401 => Some(Self::MaterializeBigInt),
            0x1_0402 => Some(Self::MaterializeRegExp),
            0x1_0403 => Some(Self::MaterializeFunction),
            0x1_0404 => Some(Self::CloneArrayTemplate),
            0x1_0500 => Some(Self::StringConcat),
            0x1_0501 => Some(Self::NewObject),
            0x1_0502 => Some(Self::GetProp),
            0x1_0503 => Some(Self::SetProp),
            0x1_0511 => Some(Self::CreateDataProperty),
            0x1_0504 => Some(Self::DeleteProp),
            0x1_0509 => Some(Self::ObjectSpread),
            0x1_050a => Some(Self::OptionalGetProp),
            0x1_050b => Some(Self::OptionalGetElem),
            0x1_050c => Some(Self::GetSuperBase),
            0x1_050d => Some(Self::GetSuperConstructor),
            0x1_050e => Some(Self::GetPropIc),
            0x1_050f => Some(Self::SetPropIc),
            0x1_0510 => Some(Self::GetPropAccessor),
            0x1_0505 => Some(Self::SetProto),
            0x1_0506 => Some(Self::NewArray),
            0x1_0507 => Some(Self::GetElem),
            0x1_0508 => Some(Self::SetElem),
            0x1_0600 => Some(Self::PrepareCall),
            0x1_0606 => Some(Self::PrepareConstruct),
            0x1_0601 => Some(Self::FinishCall),
            0x1_0602 => Some(Self::LoadArgument),
            0x1_0603 => Some(Self::LoadCallEnv),
            0x1_0604 => Some(Self::CollectRestArguments),
            0x1_0605 => Some(Self::GuardSameFunction),
            0x1_0607 => Some(Self::PrepareSuperCall),
            0x1_0608 => Some(Self::PrepareSuperCallForward),
            0x1_0700 => Some(Self::CreateException),
            0x1_0701 => Some(Self::ExceptionValue),
            0x1_0702 => Some(Self::CooperativePoll),
            0x1_0703 => Some(Self::DebugCheck),
            _ => None,
        }
    }
}
pub fn native_variable_names(program: &Program) -> Vec<String> {
    let mut names = BTreeSet::new();
    for function in program.functions() {
        for block in function.blocks() {
            for instruction in block.instructions() {
                match instruction {
                    Instruction::LoadVar { name, .. } | Instruction::StoreVar { name, .. } => {
                        names.insert(name.clone());
                    }
                    _ => {}
                }
            }
        }
    }
    names.into_iter().collect()
}

/// 按段分区编号：builtin 名字占据 `0..B`，用户独有名字从 `B` 起。
///
/// 用户独有名字不能按全局字典序插到 builtin 名字中间，否则旧 builtin image 的槽号会后移。
pub fn native_variable_slots_for_segments(
    builtin: &Program,
    user: &Program,
) -> (HashMap<String, u32>, HashMap<String, u32>) {
    let builtin_names = native_variable_names(builtin);
    let mut user_names = native_variable_names(user);
    user_names.retain(|name| !builtin_names.iter().any(|existing| existing == name));
    let mut builtin_slots = HashMap::new();
    for (index, name) in builtin_names.iter().enumerate() {
        let slot = u32::try_from(index).expect("builtin 变量槽数在 u32 内");
        builtin_slots.insert(name.clone(), slot);
    }
    let base = u32::try_from(builtin_names.len()).expect("builtin 变量槽数在 u32 内");
    let mut user_slots = builtin_slots.clone();
    for (offset, name) in user_names.into_iter().enumerate() {
        let slot = base + u32::try_from(offset).expect("用户独有变量槽数在 u32 内");
        user_slots.insert(name, slot);
    }
    (builtin_slots, user_slots)
}

/// Generated code 可引用的 native thunk ABI 签名。
///
/// `may_gc` / `may_reenter` 为 false 的签名是「叶子」调用：generated code 可以
/// 在不发布额外 GC root、不预留 call arena 的情况下直接调用。数学 thunk 与 ZGC
/// 屏障 thunk 属于这类；`HostOperation` 是统一 dispatcher，可能触发 GC / 重入，
/// 必须走完整 arena + safepoint 路径。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u16)]
pub enum NativeSignature {
    HostOperation = 0,
    /// `(F64) -> F64`
    F64Unary = 1,
    /// `(F64, F64) -> F64`
    F64Binary = 2,
    /// `(vmctx, handle) -> address`
    ZgcLoadBarrier = 3,
    /// `(vmctx, owner, slot, value) -> status`
    ZgcStoreBarrier = 4,
    /// `(vmctx, left, right) -> value`
    ValueBinary = 5,
    /// `(vmctx, value) -> value`
    ValueUnary = 6,
    /// `(vmctx, first, second, third) -> value`
    ValueTernary = 7,
    /// `(vmctx, first, second, F64) -> value`
    ValueBinaryF64 = 8,
}

impl NativeSignature {
    pub const fn may_gc(self) -> bool {
        !matches!(
            self,
            Self::F64Unary | Self::F64Binary | Self::ZgcLoadBarrier | Self::ZgcStoreBarrier
        )
    }

    pub const fn may_reenter(self) -> bool {
        !matches!(
            self,
            Self::F64Unary | Self::F64Binary | Self::ZgcLoadBarrier | Self::ZgcStoreBarrier
        )
    }

    pub const fn argument_count(self) -> u8 {
        match self {
            Self::HostOperation => 0,
            Self::F64Unary => 1,
            Self::F64Binary => 2,
            Self::ZgcLoadBarrier => 2,
            Self::ZgcStoreBarrier => 4,
            Self::ValueBinary => 3,
            Self::ValueUnary => 2,
            Self::ValueTernary => 4,
            Self::ValueBinaryF64 => 4,
        }
    }
}

/// Compiler 可引用的 process-lifetime runtime thunk。
///
/// 数学 thunk 使用统一的 Rust thunk 实现（见 `wjsm-host-native`），不依赖平台
/// libc 的符号命名差异；ID 一旦发布必须保持稳定，只能追加不能重排。
///
/// `HostOperationDispatcher` 的 C ABI 自 ABI v10 起为五参数：
/// `(ctx: *mut NativeVmContext, operation: u32, args: *const i64, args_count: u32,
/// feedback_slot: *mut u8) -> i64`；非反馈调用点传 null 槽指针，反馈调用点传
/// 当前 image 反馈区内的 48 字节槽地址。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u16)]
pub enum NativeHostSymbol {
    HostOperationDispatcher = 0,
    MathAcos = 1,
    MathAcosh = 2,
    MathAsin = 3,
    MathAsinh = 4,
    MathAtan = 5,
    MathAtanh = 6,
    MathAtan2 = 7,
    MathCbrt = 8,
    MathCos = 9,
    MathCosh = 10,
    MathExp = 11,
    MathExpm1 = 12,
    MathLog = 13,
    MathLog1p = 14,
    MathLog10 = 15,
    MathLog2 = 16,
    MathSin = 17,
    MathSinh = 18,
    MathTan = 19,
    MathTanh = 20,
    MathPow = 21,
    ZgcLoadBarrierAssist = 22,
    ZgcStoreBarrier = 23,
    StringAdd = 24,
    StringBuilderAppend = 25,
    StringBuilderFinish = 26,
    StringBuilderAppendNumber = 27,
}

impl NativeHostSymbol {
    pub const ALL: &[NativeHostSymbol] = &[
        Self::HostOperationDispatcher,
        Self::MathAcos,
        Self::MathAcosh,
        Self::MathAsin,
        Self::MathAsinh,
        Self::MathAtan,
        Self::MathAtanh,
        Self::MathAtan2,
        Self::MathCbrt,
        Self::MathCos,
        Self::MathCosh,
        Self::MathExp,
        Self::MathExpm1,
        Self::MathLog,
        Self::MathLog1p,
        Self::MathLog10,
        Self::MathLog2,
        Self::MathSin,
        Self::MathSinh,
        Self::MathTan,
        Self::MathTanh,
        Self::MathPow,
        Self::ZgcLoadBarrierAssist,
        Self::ZgcStoreBarrier,
        Self::StringAdd,
        Self::StringBuilderAppend,
        Self::StringBuilderFinish,
        Self::StringBuilderAppendNumber,
    ];

    pub const fn id(self) -> u16 {
        self as u16
    }

    pub const fn symbol_name(self) -> &'static str {
        match self {
            Self::HostOperationDispatcher => "wjsm_native_host_operation",
            Self::MathAcos => "wjsm_native_math_acos",
            Self::MathAcosh => "wjsm_native_math_acosh",
            Self::MathAsin => "wjsm_native_math_asin",
            Self::MathAsinh => "wjsm_native_math_asinh",
            Self::MathAtan => "wjsm_native_math_atan",
            Self::MathAtanh => "wjsm_native_math_atanh",
            Self::MathAtan2 => "wjsm_native_math_atan2",
            Self::MathCbrt => "wjsm_native_math_cbrt",
            Self::MathCos => "wjsm_native_math_cos",
            Self::MathCosh => "wjsm_native_math_cosh",
            Self::MathExp => "wjsm_native_math_exp",
            Self::MathExpm1 => "wjsm_native_math_expm1",
            Self::MathLog => "wjsm_native_math_log",
            Self::MathLog1p => "wjsm_native_math_log1p",
            Self::MathLog10 => "wjsm_native_math_log10",
            Self::MathLog2 => "wjsm_native_math_log2",
            Self::MathSin => "wjsm_native_math_sin",
            Self::MathSinh => "wjsm_native_math_sinh",
            Self::MathTan => "wjsm_native_math_tan",
            Self::MathTanh => "wjsm_native_math_tanh",
            Self::MathPow => "wjsm_native_math_pow",
            Self::ZgcLoadBarrierAssist => "wjsm_native_zgc_load_barrier_assist",
            Self::ZgcStoreBarrier => "wjsm_native_zgc_store_barrier",
            Self::StringAdd => "wjsm_native_string_add",
            Self::StringBuilderAppend => "wjsm_native_string_builder_append",
            Self::StringBuilderFinish => "wjsm_native_string_builder_finish",
            Self::StringBuilderAppendNumber => "wjsm_native_string_builder_append_number",
        }
    }

    pub const fn signature(self) -> NativeSignature {
        match self {
            Self::HostOperationDispatcher => NativeSignature::HostOperation,
            Self::MathAtan2 | Self::MathPow => NativeSignature::F64Binary,
            Self::ZgcLoadBarrierAssist => NativeSignature::ZgcLoadBarrier,
            Self::ZgcStoreBarrier => NativeSignature::ZgcStoreBarrier,
            Self::StringAdd => NativeSignature::ValueBinary,
            Self::StringBuilderAppend => NativeSignature::ValueTernary,
            Self::StringBuilderFinish => NativeSignature::ValueUnary,
            Self::StringBuilderAppendNumber => NativeSignature::ValueBinaryF64,
            _ => NativeSignature::F64Unary,
        }
    }

    /// 需要 typed f64 直连的 Math builtin 到 thunk 的稳定映射。
    pub const fn for_builtin(builtin: Builtin) -> Option<Self> {
        Some(match builtin {
            Builtin::MathAcos => Self::MathAcos,
            Builtin::MathAcosh => Self::MathAcosh,
            Builtin::MathAsin => Self::MathAsin,
            Builtin::MathAsinh => Self::MathAsinh,
            Builtin::MathAtan => Self::MathAtan,
            Builtin::MathAtanh => Self::MathAtanh,
            Builtin::MathAtan2 => Self::MathAtan2,
            Builtin::MathCbrt => Self::MathCbrt,
            Builtin::MathCos => Self::MathCos,
            Builtin::MathCosh => Self::MathCosh,
            Builtin::MathExp => Self::MathExp,
            Builtin::MathExpm1 => Self::MathExpm1,
            Builtin::MathLog => Self::MathLog,
            Builtin::MathLog1p => Self::MathLog1p,
            Builtin::MathLog10 => Self::MathLog10,
            Builtin::MathLog2 => Self::MathLog2,
            Builtin::MathSin => Self::MathSin,
            Builtin::MathSinh => Self::MathSinh,
            Builtin::MathTan => Self::MathTan,
            Builtin::MathTanh => Self::MathTanh,
            Builtin::MathPow => Self::MathPow,
            _ => return None,
        })
    }

    pub fn from_symbol_name(name: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|symbol| symbol.symbol_name() == name)
    }
}

pub fn native_abi_hash() -> [u8; 32] {
    static HASH: OnceLock<[u8; 32]> = OnceLock::new();
    *HASH.get_or_init(|| {
        let mut hasher = Sha256::new();
        hasher.update(b"wjsm-native-abi-v13\0");
        hasher.update(wjsm_artifact_format::semantic_abi_hash());
        hash_layout::<NativeVmContext>(&mut hasher, b"NativeVmContext");
        hash_layout::<NativeFunctionEntry>(&mut hasher, b"NativeFunctionEntry");
        hash_layout::<NativeCallFrame>(&mut hasher, b"NativeCallFrame");
        hash_layout::<NativeRootFrame>(&mut hasher, b"NativeRootFrame");
        hash_layout::<NativeSourceFrame>(&mut hasher, b"NativeSourceFrame");
        hash_layout::<NativeSourceSlot>(&mut hasher, b"NativeSourceSlot");
        hash_layout::<NativeFeedbackSlot>(&mut hasher, b"NativeFeedbackSlot");
        hash_layout::<NativeBarrierState>(&mut hasher, b"NativeBarrierState");
        hash_layout::<CallArgs>(&mut hasher, b"CallArgs");
        for offset in [
            offset_of!(NativeVmContext, abi_version),
            offset_of!(NativeVmContext, pending_exception_kind),
            offset_of!(NativeVmContext, call_arena_slots),
            offset_of!(NativeVmContext, function_table),
            offset_of!(NativeVmContext, call_frame_head),
            offset_of!(NativeVmContext, root_frame_head),
            offset_of!(NativeVmContext, source_frame_head),
            offset_of!(NativeVmContext, stack_budget_bytes),
            offset_of!(NativeVmContext, raw_access_depth),
            offset_of!(NativeVmContext, proto_generation),
            offset_of!(NativeVmContext, handle_table_base),
            offset_of!(NativeVmContext, latin1_char_strings),
            offset_of!(NativeVmContext, ic_slots_base),
            offset_of!(NativeVmContext, feedback_slots_base),
            offset_of!(NativeVmContext, heap_object_delta),
            offset_of!(NativeVmContext, string_constants_base),
        ] {
            hasher.update(
                u64::try_from(offset)
                    .expect("ABI offset fits u64")
                    .to_le_bytes(),
            );
        }
        // 反馈槽字段 offset 与 tag 编码版本：dispatcher 五参数协议的可观察契约。
        for offset in [
            offset_of!(NativeFeedbackSlot, last_target_image_id),
            offset_of!(NativeFeedbackSlot, last_tag_signature),
            offset_of!(NativeFeedbackSlot, consecutive_count),
            offset_of!(NativeFeedbackSlot, total_count),
            offset_of!(NativeFeedbackSlot, caller_function),
            offset_of!(NativeFeedbackSlot, site_index),
            offset_of!(NativeFeedbackSlot, last_target_function),
            offset_of!(NativeFeedbackSlot, operation),
            offset_of!(NativeFeedbackSlot, state),
        ] {
            hasher.update(
                u64::try_from(offset)
                    .expect("feedback slot offset fits u64")
                    .to_le_bytes(),
            );
        }
        for offset in [
            offset_of!(NativeBarrierState, phase),
            offset_of!(NativeBarrierState, access_epoch),
            offset_of!(NativeBarrierState, load_fast_events),
            offset_of!(NativeBarrierState, store_fast_events),
        ] {
            hasher.update(
                u64::try_from(offset)
                    .expect("barrier state offset fits u64")
                    .to_le_bytes(),
            );
        }
        hasher.update(wjsm_ir::constants::FEEDBACK_SLOT_SIZE.to_le_bytes());
        hasher.update([
            NativeFeedbackTag::Number.code(),
            NativeFeedbackTag::Other.code(),
        ]);
        for version in [
            NATIVE_ABI_VERSION,
            CALL_GATE_VERSION,
            ROOT_FRAME_VERSION,
            SOURCE_FRAME_VERSION,
            BARRIER_VERSION,
        ] {
            hasher.update(version.to_le_bytes());
        }
        for kind in [
            PendingExceptionKind::None,
            PendingExceptionKind::StackOverflow,
            PendingExceptionKind::CallArenaOverflow,
            PendingExceptionKind::Terminated,
            PendingExceptionKind::InternalInvariant,
        ] {
            hasher.update((kind as u32).to_le_bytes());
        }
        for operation in [
            NativeRuntimeOp::BinaryAdd,
            NativeRuntimeOp::BinarySub,
            NativeRuntimeOp::BinaryMul,
            NativeRuntimeOp::BinaryDiv,
            NativeRuntimeOp::BinaryMod,
            NativeRuntimeOp::BinaryExp,
            NativeRuntimeOp::BinaryBitAnd,
            NativeRuntimeOp::BinaryBitOr,
            NativeRuntimeOp::BinaryBitXor,
            NativeRuntimeOp::BinaryShl,
            NativeRuntimeOp::BinaryShr,
            NativeRuntimeOp::BinaryUShr,
            NativeRuntimeOp::UnaryNot,
            NativeRuntimeOp::UnaryNeg,
            NativeRuntimeOp::UnaryPos,
            NativeRuntimeOp::UnaryBitNot,
            NativeRuntimeOp::UnaryVoid,
            NativeRuntimeOp::UnaryIsNullish,
            NativeRuntimeOp::IsTruthy,
            NativeRuntimeOp::UnaryDelete,
            NativeRuntimeOp::CompareStrictEq,
            NativeRuntimeOp::CompareStrictNotEq,
            NativeRuntimeOp::StoreVar,
            NativeRuntimeOp::LoadVar,
            NativeRuntimeOp::MaterializeBigInt,
            NativeRuntimeOp::MaterializeRegExp,
            NativeRuntimeOp::MaterializeFunction,
            NativeRuntimeOp::CloneArrayTemplate,
            NativeRuntimeOp::StringConcat,
            NativeRuntimeOp::NewObject,
            NativeRuntimeOp::GetProp,
            NativeRuntimeOp::SetProp,
            NativeRuntimeOp::CreateDataProperty,
            NativeRuntimeOp::DeleteProp,
            NativeRuntimeOp::SetProto,
            NativeRuntimeOp::NewArray,
            NativeRuntimeOp::GetElem,
            NativeRuntimeOp::SetElem,
            NativeRuntimeOp::ObjectSpread,
            NativeRuntimeOp::OptionalGetProp,
            NativeRuntimeOp::OptionalGetElem,
            NativeRuntimeOp::GetSuperBase,
            NativeRuntimeOp::GetSuperConstructor,
            NativeRuntimeOp::GetPropIc,
            NativeRuntimeOp::SetPropIc,
            NativeRuntimeOp::GetPropAccessor,
            NativeRuntimeOp::PrepareCall,
            NativeRuntimeOp::PrepareConstruct,
            NativeRuntimeOp::FinishCall,
            NativeRuntimeOp::LoadArgument,
            NativeRuntimeOp::LoadCallEnv,
            NativeRuntimeOp::PrepareSuperCall,
            NativeRuntimeOp::PrepareSuperCallForward,
            NativeRuntimeOp::CollectRestArguments,
            NativeRuntimeOp::GuardSameFunction,
            NativeRuntimeOp::CreateException,
            NativeRuntimeOp::ExceptionValue,
            NativeRuntimeOp::CooperativePoll,
            NativeRuntimeOp::DebugCheck,
        ] {
            hasher.update(operation.id().to_le_bytes());
        }
        for signature in [
            NativeSignature::HostOperation,
            NativeSignature::F64Unary,
            NativeSignature::F64Binary,
            NativeSignature::ZgcLoadBarrier,
            NativeSignature::ZgcStoreBarrier,
        ] {
            hasher.update((signature as u16).to_le_bytes());
            hasher.update([u8::from(signature.may_gc())]);
            hasher.update([u8::from(signature.may_reenter())]);
            hasher.update([signature.argument_count()]);
        }
        for symbol in NativeHostSymbol::ALL {
            hasher.update(symbol.id().to_le_bytes());
            hasher.update(symbol.symbol_name().as_bytes());
            hasher.update((symbol.signature() as u16).to_le_bytes());
        }
        hasher.update(include_bytes!("../../wjsm-ir/src/value.rs"));
        hasher.update(include_bytes!("../../wjsm-ir/src/constants.rs"));
        hasher.finalize().into()
    })
}

fn hash_layout<T>(hasher: &mut Sha256, name: &[u8]) {
    hasher.update(name);
    hasher.update(
        u64::try_from(size_of::<T>())
            .expect("ABI size fits u64")
            .to_le_bytes(),
    );
    hasher.update(
        u64::try_from(align_of::<T>())
            .expect("ABI alignment fits u64")
            .to_le_bytes(),
    );
}

const _: () = {
    assert!(size_of::<CallArgs>() == 8);
    assert!(align_of::<CallArgs>() == 4);
    assert!(offset_of!(CallArgs, base) == 0);
    assert!(offset_of!(CallArgs, len) == 4);
    assert!(size_of::<PendingExceptionKind>() == 4);
    // 反馈槽视图必须与 wjsm-ir::constants 的布局常量逐字段一致。
    assert!(size_of::<NativeFeedbackSlot>() == 48);
    assert!(size_of::<NativeFeedbackSlot>() == wjsm_ir::constants::FEEDBACK_SLOT_SIZE as usize);
    assert!(
        offset_of!(NativeFeedbackSlot, last_target_image_id)
            == wjsm_ir::constants::FEEDBACK_SLOT_TARGET_IMAGE_OFFSET as usize
    );
    assert!(
        offset_of!(NativeFeedbackSlot, last_tag_signature)
            == wjsm_ir::constants::FEEDBACK_SLOT_TAG_SIGNATURE_OFFSET as usize
    );
    assert!(
        offset_of!(NativeFeedbackSlot, consecutive_count)
            == wjsm_ir::constants::FEEDBACK_SLOT_CONSECUTIVE_OFFSET as usize
    );
    assert!(
        offset_of!(NativeFeedbackSlot, total_count)
            == wjsm_ir::constants::FEEDBACK_SLOT_TOTAL_OFFSET as usize
    );
    assert!(
        offset_of!(NativeFeedbackSlot, caller_function)
            == wjsm_ir::constants::FEEDBACK_SLOT_CALLER_FUNCTION_OFFSET as usize
    );
    assert!(
        offset_of!(NativeFeedbackSlot, site_index)
            == wjsm_ir::constants::FEEDBACK_SLOT_SITE_INDEX_OFFSET as usize
    );
    assert!(
        offset_of!(NativeFeedbackSlot, last_target_function)
            == wjsm_ir::constants::FEEDBACK_SLOT_TARGET_FUNCTION_OFFSET as usize
    );
    assert!(
        offset_of!(NativeFeedbackSlot, operation)
            == wjsm_ir::constants::FEEDBACK_SLOT_OPERATION_OFFSET as usize
    );
    assert!(
        offset_of!(NativeFeedbackSlot, state)
            == wjsm_ir::constants::FEEDBACK_SLOT_STATE_OFFSET as usize
    );
    assert!(NativeFeedbackTag::Number.code() <= 0x1f);
    assert!(NativeFeedbackTag::Other.code() <= 0x1f);
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn call_args_layout_is_stable() {
        assert_eq!(size_of::<CallArgs>(), 8);
        assert_eq!(offset_of!(CallArgs, base), 0);
        assert_eq!(offset_of!(CallArgs, len), 4);
    }

    #[test]
    fn abi_hash_is_stable_within_process() {
        assert_eq!(native_abi_hash(), native_abi_hash());
        assert_ne!(native_abi_hash(), [0; 32]);
    }

    #[test]
    fn native_barrier_layout_and_symbols_are_stable() {
        assert_eq!(size_of::<NativeBarrierState>(), 32);
        assert_eq!(align_of::<NativeBarrierState>(), 8);
        assert_eq!(offset_of!(NativeBarrierState, phase), 0);
        assert_eq!(offset_of!(NativeBarrierState, access_epoch), 8);
        assert_eq!(offset_of!(NativeBarrierState, load_fast_events), 16);
        assert_eq!(offset_of!(NativeBarrierState, store_fast_events), 24);

        let load = NativeHostSymbol::ZgcLoadBarrierAssist;
        assert_eq!(load.id(), 22);
        assert_eq!(load.symbol_name(), "wjsm_native_zgc_load_barrier_assist");
        assert_eq!(load.signature(), NativeSignature::ZgcLoadBarrier);
        let store = NativeHostSymbol::ZgcStoreBarrier;
        assert_eq!(store.id(), 23);
        assert_eq!(store.symbol_name(), "wjsm_native_zgc_store_barrier");
        assert_eq!(store.signature(), NativeSignature::ZgcStoreBarrier);
        for signature in [load.signature(), store.signature()] {
            assert!(!signature.may_gc());
            assert!(!signature.may_reenter());
        }
    }

    #[test]
    fn host_op_round_trips_builtin_id() {
        let op = NativeHostOp::from_builtin(Builtin::ConsoleLog);
        assert_eq!(NativeHostOp::from_id(op.id()), Some(op));
        assert_eq!(op.name(), "console.log");
    }

    #[test]
    fn feedback_tags_distinguish_number_from_boxed_values() {
        use wjsm_ir::value;
        assert_eq!(
            NativeFeedbackTag::of(value::encode_f64(1.5)),
            NativeFeedbackTag::Number
        );
        assert_eq!(
            NativeFeedbackTag::of(value::encode_f64(f64::NAN)),
            NativeFeedbackTag::Number
        );
        assert_eq!(
            NativeFeedbackTag::of(value::encode_undefined()),
            NativeFeedbackTag::Undefined
        );
        assert_eq!(
            NativeFeedbackTag::of(value::encode_null()),
            NativeFeedbackTag::Null
        );
        assert_eq!(
            NativeFeedbackTag::of(value::encode_bool(true)),
            NativeFeedbackTag::Boolean
        );
        assert_eq!(
            NativeFeedbackTag::of(value::encode_handle(value::TAG_STRING, 7)),
            NativeFeedbackTag::String
        );
        assert_eq!(
            NativeFeedbackTag::of(value::encode_handle(value::TAG_BIGINT, 7)),
            NativeFeedbackTag::BigInt
        );
        assert_eq!(
            NativeFeedbackTag::of(value::encode_handle(value::TAG_OBJECT, 7)),
            NativeFeedbackTag::Object
        );
        assert_eq!(
            NativeFeedbackTag::of(value::encode_handle(value::TAG_FUNCTION, 7)),
            NativeFeedbackTag::Function
        );
    }

    #[test]
    fn feedback_tag_signature_packs_count_then_tags() {
        let tags = [
            NativeFeedbackTag::Number,
            NativeFeedbackTag::Number,
            NativeFeedbackTag::String,
        ];
        let signature = encode_feedback_tag_signature(&tags);
        assert_eq!(signature & 0xf, 3);
        assert_eq!(
            (signature >> 4) & 0x3f,
            u64::from(NativeFeedbackTag::Number.code())
        );
        assert_eq!(
            (signature >> 10) & 0x3f,
            u64::from(NativeFeedbackTag::Number.code())
        );
        assert_eq!(
            (signature >> 16) & 0x3f,
            u64::from(NativeFeedbackTag::String.code())
        );
        // 超出 FEEDBACK_MAX_TAGS 的 tag 只影响计数截断，不产生越界位。
        let many = vec![NativeFeedbackTag::Number; 32];
        let capped = encode_feedback_tag_signature(&many);
        assert_eq!(
            capped & 0xf,
            u64::from(wjsm_ir::constants::FEEDBACK_MAX_TAGS)
        );
    }

    #[test]
    fn math_thunk_symbols_cover_all_typed_builtins() {
        let typed = [
            Builtin::MathAcos,
            Builtin::MathAcosh,
            Builtin::MathAsin,
            Builtin::MathAsinh,
            Builtin::MathAtan,
            Builtin::MathAtanh,
            Builtin::MathAtan2,
            Builtin::MathCbrt,
            Builtin::MathCos,
            Builtin::MathCosh,
            Builtin::MathExp,
            Builtin::MathExpm1,
            Builtin::MathLog,
            Builtin::MathLog1p,
            Builtin::MathLog10,
            Builtin::MathLog2,
            Builtin::MathSin,
            Builtin::MathSinh,
            Builtin::MathTan,
            Builtin::MathTanh,
            Builtin::MathPow,
        ];
        for builtin in typed {
            let symbol = NativeHostSymbol::for_builtin(builtin).expect("typed builtin 有 thunk");
            assert_eq!(
                NativeHostSymbol::from_symbol_name(symbol.symbol_name()),
                Some(symbol)
            );
            assert!(symbol.id() < 32);
            assert!(!symbol.signature().may_gc());
            assert!(!symbol.signature().may_reenter());
        }
        assert_eq!(NativeHostSymbol::for_builtin(Builtin::MathAbs), None);
        assert_eq!(
            NativeHostSymbol::from_symbol_name("wjsm_native_host_operation"),
            Some(NativeHostSymbol::HostOperationDispatcher)
        );
        assert!(
            NativeHostSymbol::HostOperationDispatcher
                .signature()
                .may_gc()
        );
    }

    fn named_var_program(names: &[&str]) -> Program {
        let mut program = Program::new();
        let mut function = wjsm_ir::Function::new("vars", wjsm_ir::BasicBlockId(0));
        let mut block = wjsm_ir::BasicBlock::new(wjsm_ir::BasicBlockId(0));
        for (index, name) in names.iter().enumerate() {
            let dest = wjsm_ir::ValueId(u32::try_from(index).expect("测试变量数在 u32 内"));
            block.push_instruction(wjsm_ir::Instruction::LoadVar {
                dest,
                name: (*name).to_string(),
            });
        }
        block.set_terminator(wjsm_ir::Terminator::Return { value: None });
        function.push_block(block);
        program.push_function(function);
        program
    }

    #[test]
    fn native_variable_slots_for_segments_keeps_builtin_indices_stable() {
        let builtin = named_var_program(&["$1.x", "$3.y"]);
        let user = named_var_program(&["$1.x", "$2.foo"]);
        let (builtin_slots, user_slots) = native_variable_slots_for_segments(&builtin, &user);
        assert_eq!(builtin_slots.get("$1.x").copied(), Some(0));
        assert_eq!(builtin_slots.get("$3.y").copied(), Some(1));
        assert_eq!(user_slots.get("$1.x").copied(), Some(0));
        assert_eq!(user_slots.get("$3.y").copied(), Some(1));
        assert_eq!(user_slots.get("$2.foo").copied(), Some(2));

        let other_user = named_var_program(&["$0.bar"]);
        let (again_builtin, other_user_slots) =
            native_variable_slots_for_segments(&builtin, &other_user);
        assert_eq!(again_builtin, builtin_slots);
        assert_eq!(other_user_slots.get("$1.x").copied(), Some(0));
        assert_eq!(other_user_slots.get("$3.y").copied(), Some(1));
        assert_eq!(other_user_slots.get("$0.bar").copied(), Some(2));
    }
}
