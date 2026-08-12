//! Direct native compiler 与 runtime 共享的稳定 ABI 描述。
//!
//! 本 crate 只定义 generated-code-visible layout、symbol ID 与 signature；具体 runtime
//! 状态和 thunk 实现归 `wjsm-host-native`。

use std::collections::BTreeSet;
use std::ffi::c_void;
use std::mem::{align_of, offset_of, size_of};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicPtr, AtomicU32};

use sha2::{Digest, Sha256};
pub use wjsm_host::CallArgs;
use wjsm_ir::{Builtin, Instruction, Program};

pub const NATIVE_ABI_VERSION: u32 = 5;
pub const CALL_GATE_VERSION: u32 = 1;
pub const ROOT_FRAME_VERSION: u32 = 1;
pub const SOURCE_FRAME_VERSION: u32 = 1;
pub const BARRIER_VERSION: u32 = 1;

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

#[derive(Debug)]
#[repr(C)]
pub struct NativeRootFrame {
    pub previous: *mut NativeRootFrame,
    pub slots: *mut i64,
    pub bitmap_words: *const u64,
    pub bitmap_word_count: u32,
    pub safepoint_id: AtomicU32,
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
    MaterializeString = 0x1_0400,
    MaterializeBigInt = 0x1_0401,
    MaterializeRegExp = 0x1_0402,
    MaterializeFunction = 0x1_0403,
    StringConcat = 0x1_0500,
    NewObject = 0x1_0501,
    GetProp = 0x1_0502,
    SetProp = 0x1_0503,
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
            0x1_0400 => Some(Self::MaterializeString),
            0x1_0401 => Some(Self::MaterializeBigInt),
            0x1_0402 => Some(Self::MaterializeRegExp),
            0x1_0403 => Some(Self::MaterializeFunction),
            0x1_0500 => Some(Self::StringConcat),
            0x1_0501 => Some(Self::NewObject),
            0x1_0502 => Some(Self::GetProp),
            0x1_0503 => Some(Self::SetProp),
            0x1_0504 => Some(Self::DeleteProp),
            0x1_0509 => Some(Self::ObjectSpread),
            0x1_050a => Some(Self::OptionalGetProp),
            0x1_050b => Some(Self::OptionalGetElem),
            0x1_050c => Some(Self::GetSuperBase),
            0x1_050d => Some(Self::GetSuperConstructor),
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

/// Compiler 自有、不进入 portable artifact 的 libcall。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u16)]
pub enum NativeLibCall {
    F64Modulo = 0,
    F64Power = 1,
    MemoryCopy = 2,

    MemoryFill = 3,
    IntegerDivide = 4,
    IntegerRemainder = 5,
}

impl NativeLibCall {
    pub const fn id(self) -> u16 {
        self as u16
    }

    pub const fn symbol_name(self) -> &'static str {
        match self {
            Self::F64Modulo => "wjsm_native_f64_modulo",
            Self::F64Power => "wjsm_native_f64_power",
            Self::MemoryCopy => "wjsm_native_memory_copy",
            Self::MemoryFill => "wjsm_native_memory_fill",
            Self::IntegerDivide => "wjsm_native_integer_divide",
            Self::IntegerRemainder => "wjsm_native_integer_remainder",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u16)]
pub enum NativeSignature {
    SlowJsEntry = 0,
    HostOperation = 1,
    BinaryF64 = 2,
    MemoryCopy = 3,
    MemoryFill = 4,
    BinaryI64 = 5,
}
/// Compiler 可引用的 process-lifetime runtime thunk。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u16)]
pub enum NativeHostSymbol {
    HostOperationDispatcher = 0,
}

impl NativeHostSymbol {
    pub const fn id(self) -> u16 {
        self as u16
    }

    pub const fn symbol_name(self) -> &'static str {
        match self {
            Self::HostOperationDispatcher => "wjsm_native_host_operation",
        }
    }

    pub const fn signature(self) -> NativeSignature {
        match self {
            Self::HostOperationDispatcher => NativeSignature::HostOperation,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeSymbolDescriptor {
    pub id: u16,
    pub name: &'static str,
    pub signature: NativeSignature,
    pub may_gc: bool,
    pub may_reenter: bool,
}

pub fn native_libcall_descriptor(libcall: NativeLibCall) -> NativeSymbolDescriptor {
    let signature = match libcall {
        NativeLibCall::F64Modulo | NativeLibCall::F64Power => NativeSignature::BinaryF64,
        NativeLibCall::MemoryCopy => NativeSignature::MemoryCopy,
        NativeLibCall::MemoryFill => NativeSignature::MemoryFill,
        NativeLibCall::IntegerDivide | NativeLibCall::IntegerRemainder => {
            NativeSignature::BinaryI64
        }
    };
    NativeSymbolDescriptor {
        id: libcall.id(),
        name: libcall.symbol_name(),
        signature,
        may_gc: false,
        may_reenter: false,
    }
}

pub fn native_abi_hash() -> [u8; 32] {
    static HASH: OnceLock<[u8; 32]> = OnceLock::new();
    *HASH.get_or_init(|| {
        let mut hasher = Sha256::new();
        hasher.update(b"wjsm-native-abi-v5\0");
        hasher.update(wjsm_artifact_format::semantic_abi_hash());
        hash_layout::<NativeVmContext>(&mut hasher, b"NativeVmContext");
        hash_layout::<NativeFunctionEntry>(&mut hasher, b"NativeFunctionEntry");
        hash_layout::<NativeCallFrame>(&mut hasher, b"NativeCallFrame");
        hash_layout::<NativeRootFrame>(&mut hasher, b"NativeRootFrame");
        hash_layout::<NativeSourceFrame>(&mut hasher, b"NativeSourceFrame");
        hash_layout::<NativeSourceSlot>(&mut hasher, b"NativeSourceSlot");
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
        ] {
            hasher.update(
                u64::try_from(offset)
                    .expect("ABI offset fits u64")
                    .to_le_bytes(),
            );
        }
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
            NativeRuntimeOp::IsTruthy,
            NativeRuntimeOp::UnaryIsNullish,
            NativeRuntimeOp::UnaryDelete,
            NativeRuntimeOp::CompareStrictEq,
            NativeRuntimeOp::CompareStrictNotEq,
            NativeRuntimeOp::StoreVar,
            NativeRuntimeOp::LoadVar,
            NativeRuntimeOp::MaterializeString,
            NativeRuntimeOp::MaterializeBigInt,
            NativeRuntimeOp::MaterializeRegExp,
            NativeRuntimeOp::MaterializeFunction,
            NativeRuntimeOp::StringConcat,
            NativeRuntimeOp::NewObject,
            NativeRuntimeOp::GetProp,
            NativeRuntimeOp::SetProp,
            NativeRuntimeOp::ObjectSpread,
            NativeRuntimeOp::DeleteProp,
            NativeRuntimeOp::SetProto,
            NativeRuntimeOp::OptionalGetProp,
            NativeRuntimeOp::OptionalGetElem,
            NativeRuntimeOp::GetSuperBase,
            NativeRuntimeOp::GetSuperConstructor,
            NativeRuntimeOp::NewArray,
            NativeRuntimeOp::GetElem,
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
        for libcall in [
            NativeLibCall::F64Modulo,
            NativeLibCall::F64Power,
            NativeLibCall::MemoryCopy,
            NativeLibCall::MemoryFill,
            NativeLibCall::IntegerDivide,
            NativeLibCall::IntegerRemainder,
        ] {
            let descriptor = native_libcall_descriptor(libcall);
            hasher.update(descriptor.id.to_le_bytes());
            hasher.update(descriptor.name.as_bytes());
            hasher.update((descriptor.signature as u16).to_le_bytes());
            hasher.update([
                u8::from(descriptor.may_gc),
                u8::from(descriptor.may_reenter),
            ]);
        }
        let symbol = NativeHostSymbol::HostOperationDispatcher;
        hasher.update(symbol.id().to_le_bytes());
        hasher.update(symbol.symbol_name().as_bytes());
        hasher.update((symbol.signature() as u16).to_le_bytes());
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
    fn host_op_round_trips_builtin_id() {
        let op = NativeHostOp::from_builtin(Builtin::ConsoleLog);
        assert_eq!(NativeHostOp::from_id(op.id()), Some(op));
        assert_eq!(op.name(), "console.log");
    }
}
