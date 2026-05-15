use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use swc_core::ecma::ast as swc_ast;

pub(crate) struct RuntimeState {
    pub(crate) output: Arc<Mutex<Vec<u8>>>,
    pub(crate) iterators: Arc<Mutex<Vec<IteratorState>>>,
    pub(crate) enumerators: Arc<Mutex<Vec<EnumeratorState>>>,
    pub(crate) runtime_strings: Arc<Mutex<Vec<String>>>,
    pub(crate) runtime_error: Arc<Mutex<Option<String>>>,
    pub(crate) gc_mark_bits: Arc<Mutex<Vec<u64>>>,
    pub(crate) alloc_counter: Arc<Mutex<u64>>,
    #[allow(dead_code)]
    pub(crate) gc_threshold: u64,
    pub(crate) timers: Arc<Mutex<Vec<TimerEntry>>>,
    pub(crate) cancelled_timers: Arc<Mutex<HashSet<u32>>>,
    pub(crate) next_timer_id: Arc<Mutex<u32>>,
    pub(crate) closures: Arc<Mutex<Vec<ClosureEntry>>>,
    pub(crate) bound_objects: Arc<Mutex<Vec<BoundRecord>>>,
    pub(crate) native_callables: Arc<Mutex<Vec<NativeCallable>>>,
    pub(crate) eval_cache: Arc<Mutex<HashMap<u64, Vec<u8>>>>,
    pub(crate) bigint_table: Arc<Mutex<Vec<num_bigint::BigInt>>>,
    pub(crate) symbol_table: Arc<Mutex<Vec<SymbolEntry>>>,
    pub(crate) regex_table: Arc<Mutex<Vec<RegexEntry>>>,
    pub(crate) promise_table: Arc<Mutex<Vec<PromiseEntry>>>,
    pub(crate) microtask_queue: Arc<Mutex<VecDeque<Microtask>>>,
    pub(crate) continuation_table: Arc<Mutex<Vec<ContinuationEntry>>>,
    pub(crate) async_generator_table: Arc<Mutex<Vec<AsyncGeneratorEntry>>>,
    pub(crate) combinator_contexts: Arc<Mutex<Vec<CombinatorContext>>>,
    pub(crate) module_namespace_cache: Arc<Mutex<HashMap<u32, i64>>>,
    pub(crate) error_table: Arc<Mutex<Vec<ErrorEntry>>>,
    pub(crate) map_table: Arc<Mutex<Vec<MapEntry>>>,
    pub(crate) set_table: Arc<Mutex<Vec<SetEntry>>>,
    pub(crate) weakmap_table: Arc<Mutex<Vec<WeakMapEntry>>>,
    pub(crate) weakset_table: Arc<Mutex<Vec<WeakSetEntry>>>,
    pub(crate) proxy_table: Arc<Mutex<Vec<ProxyEntry>>>,
    pub(crate) arraybuffer_table: Arc<Mutex<Vec<ArrayBufferEntry>>>,
    pub(crate) dataview_table: Arc<Mutex<Vec<DataViewEntry>>>,
    pub(crate) typedarray_table: Arc<Mutex<Vec<TypedArrayEntry>>>,
}

pub(crate) struct BoundRecord {
    pub(crate) target_func: i64,
    pub(crate) bound_this: i64,
    pub(crate) bound_args: Vec<i64>,
}

pub(crate) struct SymbolEntry {
    pub(crate) description: Option<String>,
    pub(crate) global_key: Option<String>,
}

#[allow(dead_code)]
pub(crate) struct ErrorEntry {
    pub(crate) name: String,
    pub(crate) message: String,
}

pub(crate) struct MapEntry {
    pub(crate) keys: Vec<i64>,
    pub(crate) values: Vec<i64>,
}

pub(crate) struct SetEntry {
    pub(crate) values: Vec<i64>,
}

#[derive(Clone, Debug)]
pub(crate) struct WeakMapEntry {
    pub(crate) map: HashMap<u32, i64>,
}

#[derive(Clone, Debug)]
pub(crate) struct WeakSetEntry {
    pub(crate) set: HashSet<u32>,
}

#[derive(Clone, Debug)]
pub(crate) struct ArrayBufferEntry {
    pub(crate) data: Vec<u8>,
}

#[derive(Clone, Debug)]
pub(crate) struct DataViewEntry {
    pub(crate) buffer_handle: u32,
    pub(crate) byte_offset: u32,
    pub(crate) byte_length: u32,
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
pub(crate) struct TypedArrayEntry {
    pub(crate) buffer_handle: u32,
    pub(crate) byte_offset: u32,
    pub(crate) length: u32,
    pub(crate) element_size: u8,
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
pub(crate) struct ProxyEntry {
    pub(crate) target: i64,
    pub(crate) handler: i64,
    pub(crate) revoked: bool,
}

#[derive(Clone)]
pub(crate) struct RegexEntry {
    pub(crate) pattern: String,
    pub(crate) flags: String,
    pub(crate) compiled: regress::Regex,
    pub(crate) last_index: i64,
}

pub(crate) struct ClosureEntry {
    pub(crate) func_idx: u32,
    pub(crate) env_obj: i64,
}

#[derive(Clone)]
pub(crate) enum NativeCallable {
    EvalIndirect,
    EvalFunction(EvalFunction),
    PromiseResolvingFunction {
        promise: i64,
        already_resolved: Arc<Mutex<bool>>,
        kind: PromiseResolvingKind,
    },
    PromiseCombinatorReaction {
        context: usize,
        index: usize,
        kind: PromiseCombinatorReactionKind,
    },
    AsyncGeneratorMethod {
        generator: i64,
        kind: AsyncGeneratorCompletionType,
    },
    AsyncGeneratorIdentity {
        generator: i64,
    },
    MapSetMethod {
        kind: MapSetMethodKind,
    },
    DateMethod {
        kind: DateMethodKind,
    },
    WeakMapMethod {
        kind: WeakMapMethodKind,
    },
    WeakSetMethod {
        kind: WeakSetMethodKind,
    },
    ArrayConstructor,
    ObjectConstructor,
    FunctionConstructor,
    StringConstructor,
    BooleanConstructor,
    NumberConstructor,
    SymbolConstructor,
    BigIntConstructor,
    RegExpConstructor,
    ErrorConstructor,
    TypeErrorConstructor,
    RangeErrorConstructor,
    SyntaxErrorConstructor,
    ReferenceErrorConstructor,
    URIErrorConstructor,
    EvalErrorConstructor,
    AggregateErrorConstructor,
    MapConstructor,
    SetConstructor,
    WeakMapConstructor,
    WeakSetConstructor,
    DateConstructorGlobal,
    PromiseConstructor,
    ArrayBufferConstructorGlobal,
    DataViewConstructorGlobal,
    TypedArrayConstructor(String),
    ProxyConstructor,
    StubGlobal(String),
}

#[derive(Clone, Copy)]
pub(crate) enum MapSetMethodKind {
    MapSet,
    MapGet,
    SetAdd,
    Has,
    Delete,
    Clear,
    Size,
    ForEach,
    Keys,
    Values,
    Entries,
}

#[derive(Clone, Copy)]
pub(crate) enum WeakMapMethodKind {
    Set,
    Get,
    Has,
    Delete,
}

#[derive(Clone, Copy)]
pub(crate) enum WeakSetMethodKind {
    Add,
    Has,
    Delete,
}

#[derive(Clone, Copy)]
pub(crate) enum DateMethodKind {
    GetDate,
    GetDay,
    GetFullYear,
    GetHours,
    GetMilliseconds,
    GetMinutes,
    GetMonth,
    GetSeconds,
    GetTime,
    GetTimezoneOffset,
    GetUTCDate,
    GetUTCDay,
    GetUTCFullYear,
    GetUTCHours,
    GetUTCMilliseconds,
    GetUTCMinutes,
    GetUTCMonth,
    GetUTCSeconds,
    SetDate,
    SetFullYear,
    SetHours,
    SetMilliseconds,
    SetMinutes,
    SetMonth,
    SetSeconds,
    SetTime,
    SetUTCDate,
    SetUTCFullYear,
    SetUTCHours,
    SetUTCMilliseconds,
    SetUTCMinutes,
    SetUTCMonth,
    SetUTCSeconds,
    ToString,
    ToDateString,
    ToTimeString,
    ToISOString,
    ToUTCString,
    ToJSON,
    ValueOf,
}

#[derive(Clone, Copy)]
pub(crate) enum PromiseCombinatorReactionKind {
    AllFulfill,
    AllSettledFulfill,
    AllSettledReject,
    AnyReject,
}

pub(crate) struct CombinatorContext {
    pub(crate) result_promise: i64,
    pub(crate) result_array: i64,
    pub(crate) remaining: usize,
    pub(crate) settled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EvalVarMapEntry {
    pub(crate) function_name: String,
    pub(crate) var_name: String,
    pub(crate) offset: u32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum EvalLocalKind {
    Var,
    Let,
    Const,
}

pub(crate) struct EvalLocalBinding {
    pub(crate) kind: EvalLocalKind,
    pub(crate) value: i64,
}

#[derive(Clone)]
pub(crate) struct EvalFunction {
    pub(crate) params: Vec<String>,
    pub(crate) body: Vec<swc_ast::Stmt>,
    pub(crate) scope_env: Option<i64>,
}

#[derive(Clone, Copy)]
pub(crate) enum PromiseResolvingKind {
    Fulfill,
    Reject,
}

pub(crate) struct TimerEntry {
    pub(crate) id: u32,
    pub(crate) deadline: Instant,
    pub(crate) callback: i64,
    pub(crate) repeating: bool,
    pub(crate) interval: Duration,
}

pub(crate) enum IteratorState {
    StringIter {
        data: Vec<u8>,
        byte_pos: usize,
    },
    ArrayIter {
        ptr: usize,
        index: u32,
        length: u32,
    },
    ObjectIter {
        next: i64,
        return_method: Option<i64>,
        current_value: i64,
        done: bool,
        has_current: bool,
    },
    Error,
}

pub(crate) enum EnumeratorState {
    StringEnum {
        length: usize,
        index: usize,
    },
    ObjectEnum {
        keys: Vec<String>,
        index: usize,
    },
    Error,
}

#[derive(Clone)]
pub(crate) enum PromiseState {
    Pending,
    Fulfilled(i64),
    Rejected(i64),
}

pub(crate) struct PromiseEntry {
    pub(crate) state: PromiseState,
    pub(crate) fulfill_reactions: Vec<PromiseReaction>,
    pub(crate) reject_reactions: Vec<PromiseReaction>,
    pub(crate) handled: bool,
    pub(crate) constructor_resolver: Option<Arc<Mutex<bool>>>,
    pub(crate) constructor_handle: Option<i64>,
    pub(crate) is_promise: bool,
}

impl PromiseEntry {
    pub(crate) fn pending() -> Self {
        Self {
            state: PromiseState::Pending,
            fulfill_reactions: Vec::new(),
            reject_reactions: Vec::new(),
            handled: false,
            constructor_resolver: None,
            constructor_handle: None,
            is_promise: true,
        }
    }

    pub(crate) fn rejected(reason: i64) -> Self {
        Self {
            state: PromiseState::Rejected(reason),
            fulfill_reactions: Vec::new(),
            reject_reactions: Vec::new(),
            handled: false,
            constructor_resolver: None,
            constructor_handle: None,
            is_promise: true,
        }
    }

    pub(crate) fn empty() -> Self {
        Self {
            state: PromiseState::Pending,
            fulfill_reactions: Vec::new(),
            reject_reactions: Vec::new(),
            handled: false,
            constructor_resolver: None,
            constructor_handle: None,
            is_promise: false,
        }
    }
}

#[derive(Clone)]
pub(crate) struct PromiseReaction {
    pub(crate) handler: i64,
    pub(crate) target_promise: i64,
    pub(crate) reaction_type: ReactionType,
    pub(crate) async_resume_state: Option<i64>,
}

impl PromiseReaction {
    pub(crate) fn new(handler: i64, target_promise: i64, reaction_type: ReactionType) -> Self {
        Self {
            handler,
            target_promise,
            reaction_type,
            async_resume_state: None,
        }
    }
    pub(crate) fn new_async(
        handler: i64,
        target_promise: i64,
        reaction_type: ReactionType,
        state: i64,
    ) -> Self {
        Self {
            handler,
            target_promise,
            reaction_type,
            async_resume_state: Some(state),
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum ReactionType {
    Fulfill,
    Reject,
    FinallyFulfill,
    FinallyReject,
}

#[allow(dead_code)]
pub(crate) enum Microtask {
    PromiseReaction {
        promise: i64,
        reaction_type: ReactionType,
        handler: i64,
        argument: i64,
    },
    PromiseResolveThenable {
        promise: i64,
        thenable: i64,
        then: i64,
    },
    MicrotaskCallback {
        callback: i64,
    },
    AsyncResume {
        fn_table_idx: u32,
        continuation: i64,
        state: u32,
        resume_val: i64,
        is_rejected: bool,
    },
}

#[allow(dead_code)]
pub(crate) struct ContinuationEntry {
    pub(crate) fn_table_idx: u32,
    pub(crate) outer_promise: i64,
    pub(crate) captured_vars: Vec<i64>,
}

#[allow(dead_code)]
pub(crate) struct AsyncGeneratorEntry {
    pub(crate) state: AsyncGeneratorState,
    pub(crate) continuation: i64,
    pub(crate) active_request: Option<AsyncGeneratorRequest>,
    pub(crate) waiting_resume_promise: Option<i64>,
    pub(crate) queue: Vec<AsyncGeneratorRequest>,
}

#[derive(Clone)]
#[allow(dead_code)]
pub(crate) enum AsyncGeneratorState {
    SuspendedStart,
    SuspendedYield,
    Executing,
    Completed,
}

#[derive(Clone, Copy)]
#[allow(dead_code)]
pub(crate) struct AsyncGeneratorRequest {
    pub(crate) completion_type: AsyncGeneratorCompletionType,
    pub(crate) value: i64,
    pub(crate) promise: i64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum AsyncGeneratorCompletionType {
    Next,
    Return,
    Throw,
}

pub(crate) enum PromiseSettlement {
    Fulfill(i64),
    Reject(i64),
}
