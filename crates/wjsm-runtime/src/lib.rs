use anyhow::Result;
use chrono::{DateTime, Datelike, Local, TimeZone, Timelike, Utc};
use num_traits::cast::ToPrimitive;
use rand::Rng;
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::io::{self, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use swc_core::ecma::ast as swc_ast;
use wasmtime::*;
/// 影子栈大小（必须与后端保持一致）
const SHADOW_STACK_SIZE: u32 = 65536;

use wjsm_ir::{constants, value};
mod runtime_builtins;
mod runtime_eval;
mod runtime_heap;
mod runtime_host_helpers;
mod runtime_promises;
mod runtime_render;
mod runtime_values;

use runtime_builtins::*;
use runtime_eval::*;
use runtime_heap::*;
use runtime_host_helpers::*;
use runtime_promises::*;
use runtime_render::*;
use runtime_values::*;

pub fn execute(wasm_bytes: &[u8]) -> Result<()> {
    let stdout = io::stdout();
    let _ = execute_with_writer(wasm_bytes, stdout.lock())?;
    Ok(())
}

pub fn execute_with_writer<W: Write>(wasm_bytes: &[u8], writer: W) -> Result<W> {
    let engine = Engine::default();
    let module = match Module::new(&engine, wasm_bytes) {
        Ok(m) => m,
        Err(e) => {
            return Err(anyhow::anyhow!("WASM validation failed: {:?}", e));
        }
    };
    let output = Arc::new(Mutex::new(Vec::new()));

    // Iterator/enumerator side tables
    let iterators: Arc<Mutex<Vec<IteratorState>>> = Arc::new(Mutex::new(Vec::new()));
    let enumerators: Arc<Mutex<Vec<EnumeratorState>>> = Arc::new(Mutex::new(Vec::new()));
    let runtime_strings: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let runtime_error: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let timers: Arc<Mutex<Vec<TimerEntry>>> = Arc::new(Mutex::new(Vec::new()));
    let cancelled_timers: Arc<Mutex<HashSet<u32>>> = Arc::new(Mutex::new(HashSet::new()));
    let next_timer_id: Arc<Mutex<u32>> = Arc::new(Mutex::new(1));
    let closures: Arc<Mutex<Vec<ClosureEntry>>> = Arc::new(Mutex::new(Vec::new()));
    let bound_objects: Arc<Mutex<Vec<BoundRecord>>> = Arc::new(Mutex::new(Vec::new()));
    let native_callables: Arc<Mutex<Vec<NativeCallable>>> =
        Arc::new(Mutex::new(vec![NativeCallable::EvalIndirect]));
    let eval_cache: Arc<Mutex<HashMap<u64, Vec<u8>>>> = Arc::new(Mutex::new(HashMap::new()));

    let bigint_table: Arc<Mutex<Vec<num_bigint::BigInt>>> = Arc::new(Mutex::new(Vec::new()));
    let symbol_table: Arc<Mutex<Vec<SymbolEntry>>> = Arc::new(Mutex::new(vec![
        // 预分配 well-known symbols（id=0..7，对应 ECMAScript § 6.1.5.1）
        // 这些 symbol 不属于全局注册表（global_key = None），仅通过 Symbol.wellKnown 访问
        SymbolEntry {
            description: Some("Symbol(Symbol.iterator)".into()),
            global_key: None,
        }, // 0 = @@iterator
        SymbolEntry {
            description: Some("Symbol(Symbol.species)".into()),
            global_key: None,
        }, // 1 = @@species
        SymbolEntry {
            description: Some("Symbol(Symbol.toStringTag)".into()),
            global_key: None,
        }, // 2 = @@toStringTag
        SymbolEntry {
            description: Some("Symbol(Symbol.asyncIterator)".into()),
            global_key: None,
        }, // 3 = @@asyncIterator
        SymbolEntry {
            description: Some("Symbol(Symbol.hasInstance)".into()),
            global_key: None,
        }, // 4 = @@hasInstance
        SymbolEntry {
            description: Some("Symbol(Symbol.toPrimitive)".into()),
            global_key: None,
        }, // 5 = @@toPrimitive
        SymbolEntry {
            description: Some("Symbol(Symbol.dispose)".into()),
            global_key: None,
        }, // 6 = @@dispose
        SymbolEntry {
            description: Some("Symbol(Symbol.match)".into()),
            global_key: None,
        }, // 7 = @@match
        SymbolEntry {
            description: Some("Symbol(Symbol.asyncDispose)".into()),
            global_key: None,
        }, // 8 = @@asyncDispose
    ]));
    let regex_table: Arc<Mutex<Vec<RegexEntry>>> = Arc::new(Mutex::new(Vec::new()));

    let promise_table: Arc<Mutex<Vec<PromiseEntry>>> = Arc::new(Mutex::new(Vec::new()));
    let microtask_queue: Arc<Mutex<VecDeque<Microtask>>> = Arc::new(Mutex::new(VecDeque::new()));
    let continuation_table: Arc<Mutex<Vec<ContinuationEntry>>> = Arc::new(Mutex::new(Vec::new()));
    let async_generator_table: Arc<Mutex<Vec<AsyncGeneratorEntry>>> =
        Arc::new(Mutex::new(Vec::new()));
    let combinator_contexts: Arc<Mutex<Vec<CombinatorContext>>> = Arc::new(Mutex::new(Vec::new()));
    let module_namespace_cache: Arc<Mutex<HashMap<u32, i64>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let error_table: Arc<Mutex<Vec<ErrorEntry>>> = Arc::new(Mutex::new(Vec::new()));
    let map_table: Arc<Mutex<Vec<MapEntry>>> = Arc::new(Mutex::new(Vec::new()));
    let set_table: Arc<Mutex<Vec<SetEntry>>> = Arc::new(Mutex::new(Vec::new()));
    let weakmap_table: Arc<Mutex<Vec<WeakMapEntry>>> = Arc::new(Mutex::new(Vec::new()));
    let weakset_table: Arc<Mutex<Vec<WeakSetEntry>>> = Arc::new(Mutex::new(Vec::new()));
    let proxy_table: Arc<Mutex<Vec<ProxyEntry>>> = Arc::new(Mutex::new(Vec::new()));
    let arraybuffer_table: Arc<Mutex<Vec<ArrayBufferEntry>>> = Arc::new(Mutex::new(Vec::new()));
    let dataview_table: Arc<Mutex<Vec<DataViewEntry>>> = Arc::new(Mutex::new(Vec::new()));
    let typedarray_table: Arc<Mutex<Vec<TypedArrayEntry>>> = Arc::new(Mutex::new(Vec::new()));

    // GC 相关状态
    let gc_mark_bits: Arc<Mutex<Vec<u64>>> = Arc::new(Mutex::new(Vec::new()));
    let alloc_counter: Arc<Mutex<u64>> = Arc::new(Mutex::new(0));
    const GC_THRESHOLD: u64 = 1000; // 每 1000 次分配触发一次 GC 检查
    let mut store = Store::new(
        &engine,
        RuntimeState {
            output: Arc::clone(&output),
            iterators: Arc::clone(&iterators),
            enumerators: Arc::clone(&enumerators),
            runtime_strings: Arc::clone(&runtime_strings),
            runtime_error: Arc::clone(&runtime_error),
            timers: Arc::clone(&timers),
            cancelled_timers: Arc::clone(&cancelled_timers),
            next_timer_id: Arc::clone(&next_timer_id),
            gc_mark_bits: Arc::clone(&gc_mark_bits),
            alloc_counter: Arc::clone(&alloc_counter),
            gc_threshold: GC_THRESHOLD,
            closures: Arc::clone(&closures),
            bound_objects: Arc::clone(&bound_objects),
            native_callables: Arc::clone(&native_callables),
            eval_cache: Arc::clone(&eval_cache),
            bigint_table: Arc::clone(&bigint_table),
            symbol_table: Arc::clone(&symbol_table),
            regex_table: Arc::clone(&regex_table),
            promise_table: Arc::clone(&promise_table),
            microtask_queue: Arc::clone(&microtask_queue),
            continuation_table: Arc::clone(&continuation_table),
            async_generator_table: Arc::clone(&async_generator_table),
            combinator_contexts: Arc::clone(&combinator_contexts),
            module_namespace_cache: Arc::clone(&module_namespace_cache),
            error_table: Arc::clone(&error_table),
            map_table: Arc::clone(&map_table),
            set_table: Arc::clone(&set_table),
            weakmap_table: Arc::clone(&weakmap_table),
            weakset_table: Arc::clone(&weakset_table),
            proxy_table: Arc::clone(&proxy_table),
            arraybuffer_table: Arc::clone(&arraybuffer_table),
            dataview_table: Arc::clone(&dataview_table),
            typedarray_table: Arc::clone(&typedarray_table),
        },
    );

    // ── Import 0: console_log(i64) → () ─────────────────────────────────
    let mut imports: Vec<Extern> = Vec::with_capacity(322);
    imports.extend(include!("host_imports/core.rs"));
    imports.extend(include!("host_imports/timers_arrays.rs"));
    imports.extend(include!("host_imports/array_object.rs"));
    imports.extend(include!("host_imports/primitive_core.rs"));
    imports.extend(include!("host_imports/promise_async.rs"));
    imports.extend(include!("host_imports/string_methods.rs"));
    imports.extend(include!("host_imports/math_number_error.rs"));
    imports.extend(include!("host_imports/collections_buffers.rs"));
    imports.extend(include!("host_imports/proxy_traps.rs"));
    imports.push(include!("host_imports/get_builtin_global_entry.rs"));
    let instance = Instance::new(&mut store, &module, &imports)?;

    // ── Run main ──
    let main = instance.get_typed_func::<(), i64>(&mut store, "main")?;
    let main_result = main.call(&mut store, ());
    let main_ok = match main_result {
        Ok(return_val) => {
            if value::is_exception(return_val) {
                // 未捕获异常被抛出顶层，跳过后续 microtasks/timers
                false
            } else {
                true
            }
        }
        Err(trap) => {
            return Err(anyhow::anyhow!("WASM trap: {:?}", trap));
        }
    };
    
    // ── Drain microtasks after main() ────────────────────────────────────
    if main_ok {
        if let Some(Extern::Table(func_table)) = instance.get_export(&mut store, "__table") {
            if let (
                Some(Extern::Memory(memory)),
                Some(Extern::Global(shadow_sp_global)),
                Some(Extern::Global(heap_ptr_global)),
                Some(Extern::Global(obj_table_ptr_global)),
                Some(Extern::Global(obj_table_count_global)),
            ) = (
                instance.get_export(&mut store, "memory"),
                instance.get_export(&mut store, "__shadow_sp"),
                instance.get_export(&mut store, "__heap_ptr"),
                instance.get_export(&mut store, "__obj_table_ptr"),
                instance.get_export(&mut store, "__obj_table_count"),
            ) {
                drain_microtasks_from_store(
                    &mut store,
                    &func_table,
                    &memory,
                    &shadow_sp_global,
                    &heap_ptr_global,
                    &obj_table_ptr_global,
                    &obj_table_count_global,
                );
            }
        }
    }
    
    // ── Timer event loop (only if main succeeded) ─────────────────────────
    // Poll timers; fire expired callbacks via the WASM function table.  
    if main_ok {
        loop {
            let now = Instant::now();
            let mut _entry_to_fire: Option<TimerEntry> = None;

            {
                let mut timers = store.data().timers.lock().expect("timers mutex");
                let mut cancelled = store
                    .data()
                    .cancelled_timers
                    .lock()
                    .expect("cancelled_timers mutex");

                // Remove cancelled timers
                timers.retain(|t| !cancelled.contains(&t.id));
                cancelled.clear();

                if timers.is_empty() {
                    break;
                }

                // Find earliest expired timer
                if let Some(idx) = timers.iter().position(|t| t.deadline <= now) {
                    _entry_to_fire = Some(timers.remove(idx));
                } else {
                    // Sleep until next timer
                    let next = timers.iter().min_by_key(|t| t.deadline).unwrap().deadline;
                    let dur = next.saturating_duration_since(Instant::now());
                    if !dur.is_zero() {
                        std::thread::sleep(dur);
                    }
                    continue;
                }
            }

            if let Some(entry) = _entry_to_fire {
                let callback = entry.callback;
                let repeating = entry.repeating;
                let interval = entry.interval;
                let entry_id = entry.id;

                // Call the callback via WASM function table call_indirect
                let raw_idx = value::decode_function_idx(callback) as u64;
                if let Some(Extern::Table(tbl)) = instance.get_export(&mut store, "__table") {
                    if let Some(Ref::Func(Some(func))) = tbl.get(&mut store, raw_idx) {
                        if let Ok(typed) = func.typed::<(i64, i32, i32), i64>(&store) {
                            match typed.call(&mut store, (value::encode_undefined(), 0i32, 0i32)) {
                                Ok(_) => {}
                                Err(e) => {
                                    let msg = format!("timer callback error: {}", e);
                                    let mut error_lock = store
                                        .data()
                                        .runtime_error
                                        .lock()
                                        .expect("runtime_error mutex");
                                    if error_lock.is_none() {
                                        *error_lock = Some(msg);
                                    }
                                    break;
                                }
                            }
                        }
                    }
                    // Drain microtasks after timer callback
                    if let (
                        Some(Extern::Memory(mem)),
                        Some(Extern::Global(sp_global)),
                        Some(Extern::Global(heap_ptr_global)),
                        Some(Extern::Global(obj_table_ptr_global)),
                        Some(Extern::Global(obj_table_count_global)),
                    ) = (
                        instance.get_export(&mut store, "memory"),
                        instance.get_export(&mut store, "__shadow_sp"),
                        instance.get_export(&mut store, "__heap_ptr"),
                        instance.get_export(&mut store, "__obj_table_ptr"),
                        instance.get_export(&mut store, "__obj_table_count"),
                    ) {
                        drain_microtasks_from_store(
                            &mut store,
                            &tbl,
                            &mem,
                            &sp_global,
                            &heap_ptr_global,
                            &obj_table_ptr_global,
                            &obj_table_count_global,
                        );
                    }
                }

                // Re-schedule if repeating
                if repeating {
                    store
                        .data()
                        .timers
                        .lock()
                        .expect("timers mutex")
                        .push(TimerEntry {
                            id: entry_id,
                            deadline: Instant::now() + interval,
                            callback,
                            repeating: true,
                            interval,
                        });
                }
            }
        }
    }
    // ── Collect output ────────────────────────────────────────────────────
    let bytes = output
        .lock()
        .expect("runtime output buffer mutex should not be poisoned")
        .clone();
    drop(store);

    let mut writer = writer;
    writer.write_all(&bytes)?;

    // ── Check errors ─────────────────────────────────────────────────────
    if let Some(message) = runtime_error.lock().expect("runtime error mutex").clone() {
        anyhow::bail!(message);
    }

    // Propagate any wasm trap from main() call (must be after output collection)
    main_result?;

    Ok(writer)
}

struct RuntimeState {
    output: Arc<Mutex<Vec<u8>>>,
    iterators: Arc<Mutex<Vec<IteratorState>>>,
    enumerators: Arc<Mutex<Vec<EnumeratorState>>>,
    runtime_strings: Arc<Mutex<Vec<String>>>,
    runtime_error: Arc<Mutex<Option<String>>>,
    /// GC 标记位图：每个 handle 对应 1 bit，用于标记-清除 GC。
    gc_mark_bits: Arc<Mutex<Vec<u64>>>,
    /// 分配计数器：每次对象分配后递增，用于触发周期性 GC。
    alloc_counter: Arc<Mutex<u64>>,
    /// GC 触发阈值：当 alloc_counter 达到此值时触发 GC。
    #[allow(dead_code)]
    gc_threshold: u64,
    /// 定时器列表
    timers: Arc<Mutex<Vec<TimerEntry>>>,
    /// 已取消的定时器 ID 集合
    cancelled_timers: Arc<Mutex<HashSet<u32>>>,
    /// 下一个定时器 ID
    next_timer_id: Arc<Mutex<u32>>,
    /// 闭包表：每个闭包条目存储函数表索引和环境对象
    closures: Arc<Mutex<Vec<ClosureEntry>>>,
    /// 绑定函数表：存储 func.bind(this, args) 创建的绑定函数
    bound_objects: Arc<Mutex<Vec<BoundRecord>>>,
    /// 运行时原生可调用对象表：Promise resolving functions 等宿主创建函数。
    native_callables: Arc<Mutex<Vec<NativeCallable>>>,
    /// eval 编译缓存：code string hash → eval 模式 WASM bytes。
    eval_cache: Arc<Mutex<HashMap<u64, Vec<u8>>>>,
    /// BigInt 侧表：存储任意精度 BigInt 值
    bigint_table: Arc<Mutex<Vec<num_bigint::BigInt>>>,
    /// Symbol 侧表：存储 symbol 条目（description + global_key）
    symbol_table: Arc<Mutex<Vec<SymbolEntry>>>,
    /// RegExp 侧表：存储编译后的正则表达式和元数据
    regex_table: Arc<Mutex<Vec<RegexEntry>>>,
    /// Promise 侧表：object handle → Promise 内部槽；非 Promise object handle 使用空占位。
    promise_table: Arc<Mutex<Vec<PromiseEntry>>>,
    /// 微任务队列
    microtask_queue: Arc<Mutex<VecDeque<Microtask>>>,
    /// Continuation 侧表：存储异步函数续延
    continuation_table: Arc<Mutex<Vec<ContinuationEntry>>>,
    /// AsyncGenerator 侧表：存储异步生成器状态
    async_generator_table: Arc<Mutex<Vec<AsyncGeneratorEntry>>>,
    /// Promise combinator 侧表：pending 元素的 reaction 通过索引回写共享结果。
    combinator_contexts: Arc<Mutex<Vec<CombinatorContext>>>,
    /// 模块命名空间对象缓存：module_id → namespace object (i64 NaN-boxed)
    module_namespace_cache: Arc<Mutex<HashMap<u32, i64>>>,
    /// Error 侧表：存储 error 对象的 name 和 message
    error_table: Arc<Mutex<Vec<ErrorEntry>>>,
    /// Map 侧表：存储 Map 对象的键值对
    map_table: Arc<Mutex<Vec<MapEntry>>>,
    /// Set 侧表：存储 Set 对象的值
    set_table: Arc<Mutex<Vec<SetEntry>>>,
    /// WeakMap 侧表：存储 WeakMap 对象的键值对
    weakmap_table: Arc<Mutex<Vec<WeakMapEntry>>>,
    /// WeakSet 侧表：存储 WeakSet 对象的值
    weakset_table: Arc<Mutex<Vec<WeakSetEntry>>>,
    /// Proxy 侧表：存储 Proxy 对象的 target、handler 和 revoked 状态
    proxy_table: Arc<Mutex<Vec<ProxyEntry>>>,
    /// ArrayBuffer 侧表：存储 ArrayBuffer 的底层数据
    arraybuffer_table: Arc<Mutex<Vec<ArrayBufferEntry>>>,
    /// DataView 侧表：存储 DataView 的 buffer 引用和偏移量
    dataview_table: Arc<Mutex<Vec<DataViewEntry>>>,
    /// TypedArray 侧表：存储 TypedArray 的 buffer 引用、偏移量和长度
    typedarray_table: Arc<Mutex<Vec<TypedArrayEntry>>>,
}

/// 绑定函数记录
struct BoundRecord {
    target_func: i64,     // TAG_FUNCTION / TAG_CLOSURE / TAG_BOUND
    bound_this: i64,      // NaN-boxed
    bound_args: Vec<i64>, // NaN-boxed values
}

/// Symbol 条目
struct SymbolEntry {
    description: Option<String>,
    global_key: Option<String>,
}

/// Error 条目：存储 error 对象的 name 和 message
#[allow(dead_code)]
struct ErrorEntry {
    name: String,
    message: String,
    value: i64,
}

struct MapEntry {
    keys: Vec<i64>,
    values: Vec<i64>,
}

struct SetEntry {
    values: Vec<i64>,
}

#[derive(Clone, Debug)]
struct WeakMapEntry {
    map: HashMap<u32, i64>,
}

#[derive(Clone, Debug)]
struct WeakSetEntry {
    set: HashSet<u32>,
}

#[derive(Clone, Debug)]
struct ArrayBufferEntry {
    data: Vec<u8>,
}

#[derive(Clone, Debug)]
struct DataViewEntry {
    buffer_handle: u32,
    byte_offset: u32,
    byte_length: u32,
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
struct TypedArrayEntry {
    buffer_handle: u32,
    byte_offset: u32,
    length: u32,
    element_size: u8,
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
struct ProxyEntry {
    target: i64,
    handler: i64,
    revoked: bool,
}

/// RegExp 条目
#[derive(Clone)]
struct RegexEntry {
    pattern: String,
    flags: String,
    compiled: regress::Regex,
    last_index: i64,
}

/// 闭包条目
struct ClosureEntry {
    func_idx: u32,
    env_obj: i64,
}

#[derive(Clone)]
enum NativeCallable {
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
    TypedArrayConstructor(()),
    ProxyConstructor,
    ProxyRevoker { proxy_handle: u32 },
    StubGlobal(()),
}

#[derive(Clone, Copy)]
enum MapSetMethodKind {
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
enum WeakMapMethodKind {
    Set,
    Get,
    Has,
    Delete,
}

#[derive(Clone, Copy)]
enum WeakSetMethodKind {
    Add,
    Has,
    Delete,
}

#[derive(Clone, Copy)]
enum DateMethodKind {
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
enum PromiseCombinatorReactionKind {
    AllFulfill,
    AllSettledFulfill,
    AllSettledReject,
    AnyReject,
}

struct CombinatorContext {
    result_promise: i64,
    result_array: i64,
    remaining: usize,
    settled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EvalVarMapEntry {
    function_name: String,
    var_name: String,
    offset: u32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EvalLocalKind {
    Var,
    Let,
    Const,
}

struct EvalLocalBinding {
    kind: EvalLocalKind,
    value: i64,
}

#[derive(Clone)]
struct EvalFunction {
    params: Vec<String>,
    body: Vec<swc_ast::Stmt>,
    scope_env: Option<i64>,
}

#[derive(Clone, Copy)]
enum PromiseResolvingKind {
    Fulfill,
    Reject,
}

struct TimerEntry {
    id: u32,
    deadline: Instant,
    callback: i64, // NaN-boxed function handle
    repeating: bool,
    interval: Duration,
}

enum IteratorState {
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

enum EnumeratorState {
    StringEnum {
        length: usize,
        index: usize,
    },
    /// 对象属性枚举：keys 存储属性名列表
    ObjectEnum {
        keys: Vec<String>,
        index: usize,
    },
    Error,
}

#[derive(Clone)]
enum PromiseState {
    Pending,
    Fulfilled(i64),
    Rejected(i64),
}

struct PromiseEntry {
    state: PromiseState,
    fulfill_reactions: Vec<PromiseReaction>,
    reject_reactions: Vec<PromiseReaction>,
    handled: bool,
    constructor_resolver: Option<Arc<Mutex<bool>>>,
    /// 构造器引用（用于 species-aware 操作；None 表示内建 Promise）
    constructor_handle: Option<i64>,
    is_promise: bool,
}

impl PromiseEntry {
    fn pending() -> Self {
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

    fn rejected(reason: i64) -> Self {
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

    fn empty() -> Self {
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
struct PromiseReaction {
    handler: i64,
    target_promise: i64,
    reaction_type: ReactionType,
    async_resume_state: Option<i64>,
}

impl PromiseReaction {
    fn new(handler: i64, target_promise: i64, reaction_type: ReactionType) -> Self {
        Self {
            handler,
            target_promise,
            reaction_type,
            async_resume_state: None,
        }
    }
    fn new_async(
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
enum ReactionType {
    Fulfill,
    Reject,
    FinallyFulfill,
    FinallyReject,
}

#[allow(dead_code)]
enum Microtask {
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
struct ContinuationEntry {
    fn_table_idx: u32,
    outer_promise: i64,
    captured_vars: Vec<i64>,
}

#[allow(dead_code)]
struct AsyncGeneratorEntry {
    state: AsyncGeneratorState,
    continuation: i64,
    active_request: Option<AsyncGeneratorRequest>,
    waiting_resume_promise: Option<i64>,
    queue: Vec<AsyncGeneratorRequest>,
}

#[derive(Clone)]
#[allow(dead_code)]
enum AsyncGeneratorState {
    SuspendedStart,
    SuspendedYield,
    Executing,
    Completed,
}
#[derive(Clone, Copy)]
#[allow(dead_code)]
struct AsyncGeneratorRequest {
    completion_type: AsyncGeneratorCompletionType,
    value: i64,
    promise: i64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AsyncGeneratorCompletionType {
    Next,
    Return,
    Throw,
}

#[cfg(test)]
mod tests {
    use super::execute_with_writer;
    use anyhow::Result;

    fn compile_source(source: &str) -> Result<Vec<u8>> {
        let module = wjsm_parser::parse_module(source)?;
        let program = wjsm_semantic::lower_module(module)?;
        wjsm_backend_wasm::compile(&program)
    }

    #[test]
    fn execute_with_writer_prints_string_fixture() -> Result<()> {
        let wasm_bytes = compile_source(r#"console.log("Hello, Runtime!");"#)?;
        let output = execute_with_writer(&wasm_bytes, Vec::new())?;

        assert_eq!(String::from_utf8(output)?, "Hello, Runtime!\n");
        Ok(())
    }

    #[test]
    fn execute_with_writer_prints_arithmetic_fixture() -> Result<()> {
        let wasm_bytes = compile_source("console.log(1 + 2 * 3);")?;
        let output = execute_with_writer(&wasm_bytes, Vec::new())?;

        assert_eq!(String::from_utf8(output)?, "7\n");
        Ok(())
    }

    #[test]
    fn execute_direct_eval_updates_shadow_backed_var() -> Result<()> {
        let wasm_bytes = compile_source(
            r#"
            var x = 1;
            console.log(eval("x = 4"));
            console.log(x);
            "#,
        )?;
        let output = execute_with_writer(&wasm_bytes, Vec::new())?;

        assert_eq!(String::from_utf8(output)?, "4\n4\n");
        Ok(())
    }
}
