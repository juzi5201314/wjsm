//! 后端无关执行上下文：覆盖 host builtins 所需的全部操作。
//!
//! 各后端用自身运行时上下文实现本 trait。wasmtime 后端用 `WasmExecContext`，
//! native 后端用 `NativeExecContext`。builtins 代码以 `<E: ExecContext>` 泛型实例化，
//! 编译期单态化，零 vtable 开销。
//!
//! `ExecContext` 是 [`HeapContext`](crate::HeapContext) 的超集：堆读写等基础操作
//! 继承自 HeapContext；本 trait 补齐再入回调、属性键、Promise、枚举器等 builtins 能力。

use std::future::Future;
use std::pin::Pin;

use crate::{
    Handle, HeapContext, JsonValue, ReadableStreamByobRequestMethodKind,
    ReadableStreamDefaultControllerMethodKind, ReadableStreamDefaultReaderMethodKind,
    ReadableStreamMethodKind, TransformStreamMethodKind, Value,
    WritableStreamDefaultControllerMethodKind, WritableStreamDefaultWriterMethodKind,
    WritableStreamMethodKind,
};
use wjsm_ir::value;

/// 异步回调返回类型（BoxFuture）。默认产出 `anyhow::Result<Value>`。
pub type ExecFuture<'a, T = anyhow::Result<Value>> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// 预解析的回调调用目标：跨多次调用复用，跳过每次调用时的目标解析
/// （closure/function 表查找、Proxy apply 链遍历、Mutex 加锁）。
///
/// 快路径仅覆盖直接 wasm 函数（closure / 普通函数）；其余类型（Proxy trap、
/// bound、native callable）回退到 `call_js_async` 原路径，语义不变。
#[derive(Clone, Copy)]
pub struct PreparedCallback {
    /// 原始函数值（回退路径使用）。
    func: Value,
    /// 快路径：直接函数表索引。
    func_idx: u32,
    /// 快路径：闭包 env（普通函数为 undefined）。
    env_obj: Value,
    /// 是否为可直接表调用的 wasm 函数。
    direct: bool,
}

impl PreparedCallback {
    /// 快路径：直接经函数表索引调用 wasm 函数。
    pub fn direct(func: Value, func_idx: u32, env_obj: Value) -> Self {
        Self {
            func,
            func_idx,
            env_obj,
            direct: true,
        }
    }

    /// 回退路径：按原始函数值走通用调用。
    pub fn generic(func: Value) -> Self {
        Self {
            func,
            func_idx: 0,
            env_obj: value::encode_undefined(),
            direct: false,
        }
    }

    /// 原始函数值。
    pub fn func(&self) -> Value {
        self.func
    }

    /// 是否走快路径（直接函数表调用）。
    pub fn is_direct(&self) -> bool {
        self.direct
    }

    /// 快路径：函数表索引。
    pub fn func_idx(&self) -> u32 {
        self.func_idx
    }

    /// 快路径：闭包 env。
    pub fn env_obj(&self) -> Value {
        self.env_obj
    }
}

// ── 共享类型（后端无关）──

/// Proxy 条目（builtins 可见投影；后端内部可含 revoked 等附加字段）。
#[derive(Clone, Debug)]
pub struct ProxyEntry {
    pub target: Value,
    pub handler: Value,
}

/// Closure 条目。
#[derive(Clone, Debug)]
pub struct ClosureEntry {
    pub func_idx: u32,
    pub env_obj: Value,
}

/// Bound function 条目。
#[derive(Clone, Debug)]
pub struct BoundEntry {
    pub target_func: Value,
    pub bound_this: Value,
    pub bound_args: Vec<Value>,
}

/// Promise 结算状态。
#[derive(Clone, Copy, Debug)]
pub enum PromiseSettlement {
    Fulfill(Value),
    Reject(Value),
}

/// Promise 状态。
#[derive(Clone, Debug)]
pub enum PromiseState {
    Pending,
    Fulfilled(Value),
    Rejected(Value),
}

/// Promise 条目（builtins 可见投影）。
#[derive(Clone, Debug)]
pub struct PromiseEntry {
    pub state: PromiseState,
    pub fulfill_reactions: Vec<PromiseReaction>,
    pub reject_reactions: Vec<PromiseReaction>,
    pub handled: bool,
    /// `Some(Arc)` = 构造器 resolving function 的 already_resolved 记录。
    pub constructor_resolver: Option<std::sync::Arc<std::sync::Mutex<bool>>>,
    /// 构造器引用（species-aware；`None` = 内建 Promise）。
    pub constructor_handle: Option<Value>,
    pub is_promise: bool,
    /// 创建时捕获的 ALS/hooks scope（then 反应继承）。
    pub capture_scope: Option<CapturedScope>,
}

impl PromiseEntry {
    pub fn pending() -> Self {
        Self {
            state: PromiseState::Pending,
            fulfill_reactions: Vec::new(),
            reject_reactions: Vec::new(),
            handled: false,
            constructor_resolver: None,
            constructor_handle: None,
            is_promise: true,
            capture_scope: None,
        }
    }

    pub fn rejected(reason: Value) -> Self {
        Self {
            state: PromiseState::Rejected(reason),
            fulfill_reactions: Vec::new(),
            reject_reactions: Vec::new(),
            handled: false,
            constructor_resolver: None,
            constructor_handle: None,
            is_promise: true,
            capture_scope: None,
        }
    }
}

/// Promise reaction 类型。
#[derive(Clone, Copy, Debug)]
pub enum ReactionType {
    Fulfill,
    Reject,
    FinallyFulfill,
    FinallyReject,
}

/// Promise reaction（fulfill / reject 回调记录）。
#[derive(Clone, Debug)]
pub struct PromiseReaction {
    pub handler: Value,
    pub target_promise: Value,
    pub reaction_type: ReactionType,
}

impl PromiseReaction {
    pub fn new(handler: Value, target_promise: Value, reaction_type: ReactionType) -> Self {
        Self {
            handler,
            target_promise,
            reaction_type,
        }
    }
}

/// Promise resolving function 类型。
#[derive(Clone, Copy, Debug)]
pub enum PromiseResolvingKind {
    Fulfill,
    Reject,
}

/// Promise combinator reaction 类型（all / race / allSettled / any）。
#[derive(Clone, Copy, Debug)]
pub enum PromiseCombinatorReactionKind {
    AllFulfill,
    AllReject,
    AllSettledFulfill,
    AllSettledReject,
    AnyFulfill,
    AnyReject,
    RaceFulfill,
    RaceReject,
}

/// async_hooks 捕获的 scope（资源构造/调度时捕获，fire 时恢复）。
#[derive(Debug, Clone, Copy)]
pub struct CapturedScope {
    pub async_id: u64,
    pub trigger_async_id: u64,
    pub resource: Value,
    pub frame_id: Option<u64>,
}

/// NativeCallable 引用（后端自有枚举的不透明引用）。
///
/// 后端通过 `create_native_callable` 将此引用实例化为 NaN-boxed 值。
/// `NativeCallableRef` 在 `wjsm-host` 中只定义后端无关的子集（QueuingStrategySize 等
/// builtins 直接创建的变体）；完整枚举留在 `wjsm-host-wasm` types.rs。
#[derive(Clone, Debug)]
pub enum NativeCallableRef {
    QueuingStrategySize { kind: QueuingStrategySizeKind },
    HeadersMethod {
        handle: u32,
        kind: crate::HeadersMethodKind,
    },
    ResponseMethod {
        handle: u32,
        kind: crate::ResponseMethodKind,
    },
    RequestMethod {
        handle: u32,
        kind: crate::RequestMethodKind,
    },
    AbortControllerAbort { signal_handle: u32 },
    CjsRequireResolve {
        referrer: crate::RuntimeModuleReferrer,
    },
    CjsRequireResolvePaths {
        referrer: crate::RuntimeModuleReferrer,
    },
    ImportMetaResolve {
        referrer: crate::RuntimeModuleReferrer,
    },
    ReadableStreamConstructor,
    ReadableStreamMethod {
        handle: u32,
        kind: ReadableStreamMethodKind,
    },
    ReadableStreamDefaultReaderMethod {
        handle: u32,
        kind: ReadableStreamDefaultReaderMethodKind,
    },
    ReadableStreamDefaultControllerMethod {
        handle: u32,
        kind: ReadableStreamDefaultControllerMethodKind,
    },
    ReadableStreamByobRequestMethod {
        handle: u32,
        kind: ReadableStreamByobRequestMethodKind,
    },
    ReadableStreamAsyncIteratorNext { reader_handle: u32 },
    ReadableStreamAsyncIteratorReturn { reader_handle: u32 },
    ReadableStreamPipeToWriteFulfilled { readable_handle: u32 },
    ReadableStreamPipeToWriteRejected { readable_handle: u32 },
    WritableStreamConstructor,
    WritableStreamMethod {
        handle: u32,
        kind: WritableStreamMethodKind,
    },
    WritableStreamDefaultWriterMethod {
        handle: u32,
        kind: WritableStreamDefaultWriterMethodKind,
    },
    WritableStreamDefaultControllerMethod {
        handle: u32,
        kind: WritableStreamDefaultControllerMethodKind,
    },
    TransformStreamConstructor,
    TransformStreamMethod {
        handle: u32,
        kind: TransformStreamMethodKind,
    },
}

/// QueuingStrategy size 计算类型。
#[derive(Clone, Copy, Debug)]
pub enum QueuingStrategySizeKind {
    Count,
    ByteLength,
}
/// ECMAScript §7.1.1 ToPrimitive hint（后端无关三态）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToPrimitiveHintKind {
    Default,
    Number,
    String,
}

#[derive(Clone, Copy, Debug)]
pub enum PropertyLookup {
    Missing,
    Slot {
        value: Value,
        is_accessor: bool,
        getter: Value,
    },
    Proxy(Value),
}

// ── ExecContext trait ──

/// 后端无关执行上下文：覆盖 host builtins 所需的全部操作。
///
/// 对象分配（`alloc_object` / `alloc_array`）继承自 [`HeapContext`]，不在此重复声明。
pub trait ExecContext: HeapContext {
    // ═══ 字符串存储与读取 ═══

    /// 存储字符串，返回 NaN-boxed 值。
    fn store_string(&mut self, s: &str) -> Value;
    /// 存储拥有所有权的字符串。
    fn store_string_owned(&mut self, s: String) -> Value;
    /// 读取字符串原始字节。
    fn read_string_bytes(&mut self, val: Value) -> Option<Vec<u8>>;
    /// 读取字符串为 lossy UTF-8。
    fn read_string_utf8_lossy(&mut self, val: Value) -> String;

    // ═══ 属性键系统 ═══

    /// 将 name_id 规范化为 V2 property key（可能需读线性内存）。
    fn canonicalize_name_id(&mut self, name_id: u32) -> Option<u32>;
    /// Intern 字符串为 property key，返回 name_id。
    fn intern_property_key(&mut self, s: &str) -> u32;
    /// 从 name_id 反查字符串（仅 RuntimeString 路径）。
    fn property_key_string(&mut self, name_id: u32) -> Option<String>;
    /// 判断 name_id 是否匹配期望字符串。
    fn name_id_matches(&mut self, name_id: u32, expected: &str) -> bool;
    /// 将属性值转为 name_id（可能需读内存/查表）。
    fn property_value_to_name_id(&mut self, prop: Value, allow_symbol: bool) -> Option<u32>;

    // ═══ 再入回调 ═══

    /// 同步调用 JS 函数（后端自行决定桥接策略；语义上等同 `call_js_async` 的阻塞等价物）。
    fn call_js(&mut self, func: Value, this: Value, args: &[Value]) -> anyhow::Result<Value>;
    /// 异步调用 JS 函数。
    fn call_js_async<'a>(
        &'a mut self,
        func: Value,
        this: Value,
        args: &'a [Value],
    ) -> ExecFuture<'a>;
    /// 判断值是否可调用（含 Proxy apply trap 链）。
    fn is_callable(&mut self, val: Value) -> bool;

    /// 预解析可调用目标（closure/普通函数走直接函数表快路径；
    /// Proxy trap/bound/native 回退到通用路径）。不可调用返回 None。
    ///
    /// 结果可跨多次 `call_prepared_async` 复用，避免每次调用重复解析
    /// （closure 表 Mutex 加锁 + 类型分派 + Proxy apply 链遍历）。
    fn prepare_callback(&mut self, func: Value) -> Option<PreparedCallback> {
        self.is_callable(func).then(|| PreparedCallback::generic(func))
    }
    /// 用预解析目标调用 JS 函数；语义等同 `call_js_async`。
    /// `prepared` 必须来自 `prepare_callback`（或 None 对应值本身可调用）。
    fn call_prepared_async<'a>(
        &'a mut self,
        prepared: &'a PreparedCallback,
        this: Value,
        args: &'a [Value],
    ) -> ExecFuture<'a> {
        self.call_js_async(prepared.func(), this, args)
    }

    // ═══ 状态表访问 ═══

    /// 查询 Proxy 条目（已撤销或缺失返回 None）。
    fn proxy_entry(&mut self, proxy: Handle) -> Option<ProxyEntry>;
    /// 查询 Proxy 条目，不因 revoked 过滤；仅用于 `typeof` 的 [[Call]] 能力判定。
    fn proxy_entry_any(&mut self, proxy: Handle) -> Option<ProxyEntry>;
    /// 分配 Proxy 记录并返回 NaN-boxed proxy 值。
    fn alloc_proxy(&mut self, target: Value, handler: Value) -> Value;
    /// 创建绑定到指定 Proxy 的撤销函数。
    fn create_proxy_revoker(&mut self, proxy: Value) -> Value;
    /// 查询 Closure 条目。
    fn closure_entry(&mut self, handle: Handle) -> Option<ClosureEntry>;
    /// 查询 Bound function 条目。
    fn bound_entry(&mut self, handle: Handle) -> Option<BoundEntry>;
    /// 分派 NativeCallable（Promise resolving / stream 等）。
    fn dispatch_native_callable(&mut self, idx: u32, this: Value, args: &[Value]) -> Option<Value>;

    // ═══ 枚举器 ═══

    /// 从值创建枚举器，返回 NaN-boxed 枚举器值。
    fn create_enumerator(&mut self, val: Value) -> Value;
    /// 推进枚举器到下一个。
    fn enumerator_advance(&mut self, handle: Handle);
    /// 获取当前键。
    fn enumerator_key(&mut self, handle: Handle) -> Value;
    /// 是否已完成。
    fn enumerator_done(&mut self, handle: Handle) -> bool;

    // ═══ 异常与错误 ═══

    /// 抛出异常值（存入 error_table + 设置 runtime_error）。
    fn throw_exception(&mut self, val: Value);
    /// 设置最近错误消息。
    fn set_last_error(&mut self, msg: String);
    /// 取出最近错误消息。
    fn take_last_error(&mut self) -> Option<String>;
    /// 创建 TypeError 异常值。
    fn make_type_error(&mut self, msg: &str) -> Value;
    /// 将任意 JS value 包装为 exception。
    fn make_exception(&mut self, value: Value) -> Value;
    /// 创建 SyntaxError 异常值。
    fn make_syntax_error(&mut self, msg: &str) -> Value;

    // ═══ 属性助手（高层）═══

    /// 定义数据属性（按字符串键）。
    fn define_data_property(&mut self, obj: Value, key: &str, value: Value);
    /// 按 name_id 读取属性值。
    fn get_property_by_name_id(&mut self, obj: Value, name_id: u32) -> Value;
    /// 按 name_id 获取方法（含 callable 检查）。
    fn get_method_by_name_id(&mut self, obj: Value, name_id: u32) -> anyhow::Result<Option<Value>>;

    // ═══ 数组助手 ═══

    /// 写数组元素（含 handle 解析）。
    fn array_write_elem(&mut self, arr: Value, index: u32, value: Value);
    /// 读数组长度（值级，含 handle 解析）。
    fn array_read_length(&mut self, arr: Value) -> Option<u32>;
    fn array_read_elem(&mut self, arr: Value, index: u32) -> Option<Value>;

    /// 写数组长度。
    fn array_write_length(&mut self, arr: Value, len: u32);

    // ═══ 值渲染 ═══

    /// 将值渲染为显示字符串。
    fn render_value(&mut self, val: Value) -> String;
    /// 渲染路径的属性读取：own + 原型链数据槽（legacy 与 V2 堆统一），
    /// 不触发 getter/Proxy trap；不存在返回 None。
    fn read_property_for_render(&mut self, obj: Value, key: &str) -> Option<Value>;
    /// RegExp 条目的 (pattern, flags)；非 regexp 或缺失返回 None。
    fn regexp_pattern_flags(&mut self, val: Value) -> Option<(String, String)>;
    /// 对象 own 可枚举数据槽快照：(name_id, value)；跳过 undefined 与私有槽。
    /// 对象 handle 无效时返回 None（JSON.stringify 序列化为 "null"）。
    fn own_enumerable_data_slots(&mut self, obj: Value) -> Option<Vec<(u32, Value)>>;
    /// 将 JSON 中间值物化为 JS 值（后端负责堆分配与属性定义）。
    fn json_materialize(&mut self, json_value: &JsonValue) -> Value;

    // ═══ Promise ═══

    /// 分配新 Promise。
    fn alloc_promise(&mut self) -> Value;
    /// 结算 Promise。
    fn settle_promise(&mut self, promise: Value, settlement: PromiseSettlement);
    /// 解析 Promise（可能触发 thenable 链）。
    fn resolve_promise(&mut self, promise: Value, value: Value);

    /// 分配动态 import promise，并安装当前后端的可观察方法属性。
    fn alloc_dynamic_import_promise(&mut self) -> Value;
    // ── Promise 高层算法原语（P5 迁移所需） ──

    /// 分配带初始 entry 的 Promise（含 prototype + promise_table 插入 + async_hooks scope）。
    fn alloc_promise_with_entry(&mut self, entry: PromiseEntry) -> Value;
    /// 从 promise 值取回原始 handle（obj_table 下标）。
    fn raw_promise_handle(&self, promise: Value) -> usize;
    /// `.then()` / `.catch()` / `.finally()` 结果 promise 的 species constructor handle。
    /// `None` = 内建 Promise；`Some(ctor)` = 子类 constructor。
    fn promise_result_species_constructor_handle(&mut self, exemplar: Value) -> Option<Value>;
    /// 按 constructor 设置 promise 原型（species-aware）。
    fn set_promise_proto_from_constructor(&mut self, promise: Value, constructor: Value);
    /// 创建 promise resolving function（含 already_resolved 记录）。
    fn create_promise_resolving_function(
        &mut self,
        promise: Value,
        kind: PromiseResolvingKind,
    ) -> Value;
    /// `NewPromiseCapability(C)`：返回 (promise, resolve, reject)。
    fn new_promise_capability(&mut self, constructor: Value) -> (Value, Value, Value);
    /// 捕获 child promise scope（async_hooks ALS 继承）。
    fn capture_child_promise_scope(
        &mut self,
        promise: Value,
        parent: Option<CapturedScope>,
    ) -> Option<CapturedScope>;
    /// 清除 pending unhandled rejection 记录。
    fn clear_pending_unhandled_rejection(&self, handle: usize);
    /// 注册 pending unhandled rejection（rejected promise 未被 .then/.catch 处理时）。
    fn push_pending_unhandled_rejection(&self, handle: usize);
    /// 查询 promise entry 的 constructor_handle。
    fn promise_constructor_handle(&self, promise: Value) -> Option<Value>;
    /// 是否为 thenable（有 `.then` 方法的对象/函数）。
    fn is_thenable(&mut self, val: Value) -> bool;
    /// 创建 combinator reaction handler（Promise.all/race/allSettled/any 用）。
    fn create_combinator_reaction_handler(
        &self,
        context: u32,
        index: usize,
        kind: PromiseCombinatorReactionKind,
    ) -> Value;
    /// 创建 combinator context（allSettled/any 等）。
    fn create_combinator_context(&self, result_promise: Value, result_array: Value) -> u32;
    /// 设置 combinator context 的 remaining 计数。
    fn set_combinator_remaining(&self, context: u32, remaining: usize);
    /// 递增 combinator context 的 outstanding settlement 计数（pending 元素挂接 reaction 时）。
    fn increment_combinator_outstanding_settlements(&self, context: u32);
    /// 标记 combinator context 已结算。
    fn mark_combinator_settled(&self, context: u32);
    /// 尝试回收 combinator context（已结算且无 outstanding settlement 时归还 free list）。
    fn try_recycle_combinator_context(&self, context: u32);
    /// 分配 AggregateError 对象（Promise.any 全拒绝路径；含 name/message/errors/stack）。
    fn alloc_aggregate_error(&mut self, errors: Value) -> Value;
    /// 分配 allSettled result record（`{ status, value }` / `{ status, reason }`）。
    fn alloc_all_settled_result(&mut self, status: &str, value_name: &str, value: Value) -> Value;
    /// 查询 promise entry 的 constructor_resolver（already_resolved 记录）。
    fn promise_constructor_resolver(
        &self,
        promise: Value,
    ) -> Option<std::sync::Arc<std::sync::Mutex<bool>>>;
    /// 推入 promise reaction 到 entry。
    fn push_promise_reaction(
        &mut self,
        promise: Value,
        reaction: PromiseReaction,
        is_fulfill: bool,
    );
    /// 推入 promise reaction microtask（promise 已 settled 时）。
    fn queue_promise_reaction_microtask(
        &self,
        promise: Value,
        reaction_type: ReactionType,
        handler: Value,
        argument: Value,
        scope: Option<CapturedScope>,
    );
    /// 创建 NativeCallable 值。
    fn create_native_callable(&self, callable: NativeCallableRef) -> Value;

    // ── Promise entry 操作（P5 迁移所需） ──

    /// 插入 promise entry 到 promise_table。
    fn insert_promise_entry(&mut self, handle: usize, entry: PromiseEntry);
    /// 标记 promise 为已处理（.then/.catch/.finally 调用时）。
    fn mark_promise_handled(&mut self, promise: Value);
    /// 查询 promise 状态。
    fn promise_state(&self, promise: Value) -> PromiseState;
    /// 查询 promise 的 capture_scope（async_hooks ALS）。
    fn promise_capture_scope(&self, promise: Value) -> Option<CapturedScope>;

    // ═══ GC ═══

    /// GC safepoint poll（分配计数器检查 + 可能触发回收）。
    fn gc_safepoint_poll(&mut self);
    /// GC 屏障缓冲区 flush。
    fn gc_barrier_flush(&mut self);

    // ═══ 原型链 ═══

    /// 获取对象原型 handle。
    fn prototype_of(&mut self, handle: Handle) -> Option<Handle>;
    /// 获取 RegExp 原型值（惰性初始化）。
    fn regexp_prototype(&mut self) -> Value;

    // ═══ Handle 解析 ═══

    /// 解析值为 handle 索引（含 function_props_base 重定位）。
    fn resolve_handle_idx(&mut self, val: Value) -> Option<usize>;

    // ═══ 进程 ═══

    /// 检查是否有待处理的进程退出信号。
    fn pending_exit_signal(&mut self) -> Option<i32>;

    // ═══ 值转换（ToNumber / ToPrimitive / ToBoolean）═══

    /// ToNumber；失败时返回 exception 值（非 panic）。
    fn value_to_number(&mut self, val: Value) -> Value;
    /// ToPrimitive；`hint_number == true` 时 hint 为 Number。
    fn to_primitive(&mut self, val: Value, hint_number: bool) -> Value;
    /// ToNumber（已是 primitive 或再走 ToPrimitive 后的路径）。
    fn to_number(&mut self, val: Value) -> Value;
    /// ToBoolean。
    fn to_boolean(&mut self, val: Value) -> bool;
    /// ToPrimitive（三态 hint 完整版；`to_primitive` 的超集，String hint 路径用）。
    fn to_primitive_hinted(&mut self, val: Value, hint: ToPrimitiveHintKind) -> Value;
    /// 相同 RuntimeString 语义的字符串相等（strict_eq 字符串分支用）。
    fn string_values_equal(&mut self, a: Value, b: Value) -> bool;
    /// 字符串 UTF-16 字典序比较（abstract_compare 字符串分支用）；a < b 返回 true。
    fn string_lt(&mut self, a: Value, b: Value) -> bool;
    /// TAG_FUNCTION 与 TAG_CLOSURE 交叉相等：closure.func_idx == function idx。
    fn function_closure_identity_eq(&mut self, func: Value, closure: Value) -> bool;

    // ═══ 错误构造 ═══

    /// 创建 RangeError 异常值。
    fn make_range_error(&mut self, msg: &str) -> Value;
    /// 创建命名 Error 对象（构造器路径：message + options/cause）。
    fn create_error_object(&mut self, name: &str, message_arg: Value, options: Value) -> Value;
    /// Error.prototype.toString。
    fn error_proto_to_string(&mut self, this_val: Value) -> Value;
    /// 将错误对象压入 error_table 并返回 TAG_EXCEPTION 值。
    fn push_exception(&mut self, name: &str, message: &str, error_obj: Value) -> Value;

    // ═══ BigInt 侧表 ═══

    /// 存储 BigInt，返回 NaN-boxed 值。
    fn store_bigint(&mut self, n: num_bigint::BigInt) -> Value;
    /// 读取 BigInt。
    fn read_bigint(&mut self, val: Value) -> Option<num_bigint::BigInt>;

    // ═══ Symbol 侧表 ═══

    /// 创建 Symbol（可选 description / global_key）。
    fn create_symbol(&mut self, description: Option<String>, global_key: Option<String>) -> Value;
    /// 读取 Symbol 条目 (description, global_key)。
    fn symbol_entry(&mut self, val: Value) -> Option<(Option<String>, Option<String>)>;
    /// 按 global_key 查找 Symbol.for 注册表。
    fn find_global_symbol(&mut self, key: &str) -> Option<Value>;
    /// well-known symbol id → 值（id 与启动时 symbol_table 索引一致）。
    fn symbol_well_known(&mut self, id: i32) -> Value;
    /// 在 Symbol 构造器上安装 well-known symbols。
    fn install_well_known_symbols_on_symbol_constructor(&mut self, ctor: Value);

    // ═══ 线性内存字符串 ═══

    /// 从主 memory 读取 C 风格 / 定长字符串。
    fn read_memory_string(&mut self, ptr: u32, len: Option<u32>) -> String;
    /// 从主 memory 读取 name 指针处的字节（primordial 属性名等）。
    fn read_memory_string_bytes(&mut self, ptr: u32) -> Vec<u8>;

    // ═══ NativeCallable 领域方法（枚举留在 host-wasm）═══

    /// Number 原始值原型方法（0=toString … 4=toPrecision）。
    fn create_number_primitive_method(&mut self, method: u8) -> Value;
    /// BigInt 原始值原型方法（0=toString, 1=valueOf）。
    fn create_bigint_primitive_method(&mut self, method: u8) -> Value;
    /// 按全局构造器名创建 NativeCallable（"Array"/"Object"/…）。
    fn create_global_builtin(&mut self, name: &str) -> Option<Value>;
    /// NativeCallable::EvalFunction 的参数个数。
    fn native_eval_function_param_count(&mut self, val: Value) -> Option<usize>;
    /// 是否为 process.hrtime NativeCallable。
    fn is_process_hrtime_callable(&mut self, val: Value) -> bool;
    /// 创建 process.hrtime.bigint。
    fn create_process_hrtime_bigint(&mut self) -> Value;
    /// 调用 NativeCallable（无 args）。
    fn call_native_callable(&mut self, func: Value, this: Value, args: &[Value]) -> Value;

    // ═══ 属性 / 原型 ═══

    /// 设置对象 [[Prototype]]。
    fn set_object_proto(&mut self, obj: Value, proto: Value);
    /// 按 name_id + flags 定义数据属性。
    fn define_data_property_by_name_id(
        &mut self,
        obj: Value,
        name_id: u32,
        value: Value,
        flags: i32,
    );
    /// 将 name_id 转为属性键值（symbol 路径）；失败返回 None。
    fn name_id_to_property_key_value(&mut self, name_id: u32) -> Option<Value>;
    /// Reflect.get 同步路径（含 Proxy trap；后端自行 block_on）。
    fn reflect_get_sync(&mut self, target: Value, prop: Value, receiver: Value) -> Value;
    /// 判定函数值是否为当前 realm 的 `%Array.prototype.join%`。
    fn is_array_prototype_join(&mut self, candidate: Value) -> bool;
    /// 同步调用 getter。
    fn invoke_getter_sync(&mut self, getter: Value, receiver: Value) -> Value;
    /// 数组命名属性（side table）读取。
    fn array_named_prop_get(&mut self, arr: Value, name_id: u32) -> Option<Value>;
    /// V2 原型链属性槽：返回 (value, is_accessor, getter)。
    fn get_property_slot_on_proto(
        &mut self,
        handle: Handle,
        name_id: u32,
    ) -> Option<(Value, bool, Value)>;
    /// V2 原型链查找，保留遇到 Proxy prototype 的分派信息。
    fn lookup_property_on_proto(&mut self, handle: Handle, name_id: u32) -> PropertyLookup;

    /// 字符串 UTF-16 长度。
    fn string_utf16_len(&mut self, val: Value) -> Option<u32>;
    /// 解析对象到线性内存指针（legacy path）。
    fn resolve_object_ptr(&mut self, val: Value) -> Option<usize>;
    /// 沿 legacy 原型链读 name_id 属性（不调用 getter）。
    fn read_property_by_name_id_proto_walk(
        &mut self,
        obj_ptr: usize,
        name_id: u32,
    ) -> Option<Value>;
    /// 沿 legacy 原型链读 name_id（含 getter 调用）。
    fn get_by_name_id_on_proto_chain(
        &mut self,
        receiver: Value,
        obj_ptr: usize,
        name_id: u32,
    ) -> Option<Value>;
    /// function/closure/bound 的 props handle。
    fn handle_index_of(&mut self, val: Value) -> Option<Handle>;
    /// 确保对象（含延迟分配 props 的函数值）具备可写属性存储。
    fn ensure_property_storage(&mut self, value: Value) -> bool;
    /// function/closure/bound 的内建与 props 属性读取 adapter。
    fn callable_get_property(&mut self, value: Value, name_id: u32) -> Value;
    /// NativeCallable 的内建静态/原型属性读取 adapter。
    fn native_callable_get_property(&mut self, value: Value, name_id: u32) -> Value;
    /// 值转 JSON/显示字符串（Symbol.for key 等）；失败返回 exception。
    fn value_to_key_string(&mut self, val: Value) -> Result<String, Value>;

    // ═══ 原始类型属性分派 ═══

    fn primitive_symbol_get_property(&mut self, boxed: Value, name_id: u32) -> Value;
    fn primitive_regexp_get_property(&mut self, boxed: Value, name_id: u32) -> Value;
    fn primitive_regexp_set_property(&mut self, boxed: Value, name_id: u32, val: Value);
    fn regexp_create(&mut self, pattern: String, flags: String) -> Value;
    fn regexp_test(&mut self, regex: Value, str_val: Value) -> Value;
    fn regexp_exec(&mut self, regex: Value, str_val: Value) -> Value;

    // ═══ Generator ═══

    fn generator_prototype(&mut self) -> Value;
    fn create_generator_method(&mut self, generator: Value, kind: u8) -> Value;
    fn create_generator_identity(&mut self, generator: Value) -> Value;
    fn init_generator_entry(&mut self, generator: Value, continuation: Value) -> Value;
    fn generator_next(&mut self, generator: Value, value: Value) -> Value;
    fn generator_return(&mut self, generator: Value, value: Value) -> Value;
    fn generator_throw(&mut self, generator: Value, value: Value) -> Value;

    // ═══ AsyncGenerator ═══

    fn async_generator_prototype(&mut self) -> Value;
    fn create_async_generator_method(&mut self, generator: Value, kind: u8) -> Value;
    fn create_async_generator_identity(&mut self, generator: Value) -> Value;
    fn init_async_generator_entry(&mut self, generator: Value, continuation: Value);
    fn async_generator_next(&mut self, generator: Value, value: Value) -> Value;
    fn async_generator_return(&mut self, generator: Value, value: Value) -> Value;
    fn async_generator_throw(&mut self, generator: Value, value: Value) -> Value;

    // ═══ AsyncFunction / Continuation ═══

    /// 解析函数/闭包/原始索引到 func_table 下标。
    fn resolve_func_table_idx(&mut self, fn_val: Value) -> u32;
    /// 分配 continuation handle（返回 raw handle，非 boxed）。
    fn alloc_continuation(
        &mut self,
        fn_table_idx: u32,
        outer_promise: Value,
        captured_var_count: usize,
    ) -> u32;
    /// 写 continuation.captured_vars[slot]。
    fn continuation_set_var(&mut self, cont_handle: u32, slot: usize, val: Value);
    /// 读 continuation.captured_vars[slot]。
    fn continuation_get_var(&mut self, cont_handle: u32, slot: usize) -> Value;
    /// 压入 AsyncResume 微任务。
    fn enqueue_async_resume(
        &mut self,
        fn_table_idx: u32,
        continuation: Value,
        state: u32,
        resume_val: Value,
        completion: u8,
    );
    /// state==0 时执行 async 函数体直至首个 await；返回 true 表示已直接跑完（无需再入队）。
    fn async_function_initial_call<'a>(
        &'a mut self,
        fn_table_idx: u32,
        continuation: Value,
        resume_val: Value,
    ) -> ExecFuture<'a, bool>;
    /// await suspend：挂起 continuation 到 promise reactions。
    fn async_function_suspend(&mut self, continuation: Value, awaited_promise: Value, state: Value);
    /// 分配 iterator result 对象 `{value, done}`。
    fn alloc_iterator_result(&mut self, value: Value, done: bool) -> Value;

    // ═══ Object async（Proxy 感知；实现可委托未迁移的 proxy_reflect）═══

    fn object_get_prototype_of_async<'a>(&'a mut self, obj: Value) -> ExecFuture<'a, Value>;
    fn object_is_extensible_async<'a>(&'a mut self, obj: Value) -> ExecFuture<'a, bool>;
    fn object_prevent_extensions_async<'a>(&'a mut self, obj: Value) -> ExecFuture<'a, bool>;
    fn object_keys_async<'a>(&'a mut self, obj: Value) -> ExecFuture<'a, Value>;
    fn object_entries_async<'a>(&'a mut self, obj: Value) -> ExecFuture<'a, Value>;
    fn object_values_async<'a>(&'a mut self, obj: Value) -> ExecFuture<'a, Value>;
    fn object_get_own_property_names_async<'a>(&'a mut self, obj: Value) -> ExecFuture<'a, Value>;
    fn object_get_own_property_symbols_async<'a>(&'a mut self, obj: Value)
    -> ExecFuture<'a, Value>;
    fn object_assign_async<'a>(
        &'a mut self,
        target: Value,
        args_base: i32,
        args_count: i32,
    ) -> ExecFuture<'a, Value>;

    // ═══ Inspector ═══

    /// CDP debug_break 暂停循环（无 inspector 时立即返回）。
    fn debug_break<'a>(&'a mut self, line: i32, col: i32, flags: i32) -> ExecFuture<'a, ()>;

    // ═══ RuntimeString I/O（UTF-16 完整保留）═══

    /// 从值读取 RuntimeString（含 unpaired surrogate）。
    fn get_runtime_string(&mut self, val: Value) -> crate::RuntimeString;
    /// 存储 RuntimeString，返回 NaN-boxed 值。
    fn store_runtime_string(&mut self, s: crate::RuntimeString) -> Value;

    // ═══ 私有字段 / 属性槽 ═══

    /// Own 属性槽：(value, flags, getter, setter)；不存在返回 None。
    fn get_own_property_slot(
        &mut self,
        handle: Handle,
        name_id: u32,
    ) -> Option<(Value, u32, Value, Value)>;
    /// 按 name_id 写属性值。
    fn set_property_by_name_id(&mut self, handle: Handle, name_id: u32, val: Value) -> bool;
    /// 按 name_id 删除 own 属性（V2 堆路径；数组元素请用 array_write_hole）。
    fn delete_property_by_name_id(&mut self, handle: Handle, name_id: u32) -> bool;
    /// 带 flags 定义数据属性。
    fn define_data_property_with_flags(
        &mut self,
        handle: Handle,
        name_id: u32,
        val: Value,
        flags: u32,
    ) -> bool;
    /// 带 flags 定义 accessor 属性。
    fn define_accessor_property_with_flags(
        &mut self,
        handle: Handle,
        name_id: u32,
        getter: Value,
        setter: Value,
        flags: u32,
    ) -> bool;

    // ═══ Closure 侧表 ═══

    /// 创建闭包，返回 NaN-boxed closure 值。
    fn create_closure(&mut self, func_idx: u32, env_obj: Value) -> Value;
    /// 读闭包 func_idx；缺失返回 None。
    fn closure_func_idx(&mut self, idx: u32) -> Option<u32>;
    /// 读闭包 env_obj；缺失返回 None。
    fn closure_env(&mut self, idx: u32) -> Option<Value>;

    // ═══ 数组 exotic 原语（算法在 builtins；此处仅堆布局操作）═══

    /// 数组 push 元素（含 length 更新）。
    fn array_push(&mut self, arr: Value, val: Value) -> Value;
    /// 数组 push hole。
    fn array_push_hole(&mut self, arr: Value) -> Value;
    /// 解析数组 handle 是否有效。
    fn resolve_array(&mut self, arr: Value) -> bool;
    /// 读数组元素；hole 返回 None。
    fn array_elem_at(&mut self, arr: Value, index: u32) -> Option<Value>;
    /// 写数组 hole（删除下标槽）。
    fn array_write_hole(&mut self, arr: Value, index: u32);
    /// 确保数组容量 ≥ needed（动态扩容）。
    fn array_ensure_capacity(&mut self, arr: Value, needed: u32) -> bool;
    /// ArraySpeciesCreate 同步路径（原生 Array 快速路径）。
    fn array_species_create(&mut self, exemplar: Value, length: u32) -> Value;
    /// ArraySpeciesCreate 异步路径（可再入自定义构造器）。
    fn array_species_create_async<'a>(
        &'a mut self,
        exemplar: Value,
        length: u32,
    ) -> ExecFuture<'a, Value>;

    // ═══ TypedArray 低层原语 ═══

    /// 解析 TypedArray 视图（buffer/offset/length/kind）。
    fn typedarray_resolve(&mut self, this: Value) -> Option<TypedArrayView>;
    /// 读 TypedArray 元素。
    fn typedarray_read_elem(&mut self, view: &TypedArrayView, index: u32) -> Option<Value>;
    /// 写 TypedArray 元素。
    fn typedarray_write_elem(&mut self, view: &TypedArrayView, index: u32, val: Value);

    // ═══ Bound function ═══

    /// 创建 bound function（Function.prototype.bind）。
    fn create_bound_function(
        &mut self,
        target: Value,
        this_arg: Value,
        bound_args: Vec<Value>,
    ) -> Value;

    // ═══ 集合 / Buffer 表原语 ═══

    /// Map 表：新建空 Map，返回 table index。
    fn map_table_create(&mut self) -> u32;
    /// Set 表：新建空 Set，返回 table index。
    fn set_table_create(&mut self) -> u32;
    /// Map.set。
    fn map_set(&mut self, handle: u32, key: Value, val: Value);
    /// Map.get；缺失返回 None。
    fn map_get(&mut self, handle: u32, key: Value) -> Option<Value>;
    /// Map/Set has。
    fn map_set_has(&mut self, handle: u32, key: Value, is_set: bool) -> bool;
    /// Map/Set delete。
    fn map_set_delete(&mut self, handle: u32, key: Value, is_set: bool) -> bool;
    /// Map/Set clear。
    fn map_set_clear(&mut self, handle: u32, is_set: bool);
    /// Map/Set size。
    fn map_set_size(&mut self, handle: u32, is_set: bool) -> u32;
    /// Set.add。
    fn set_add(&mut self, handle: u32, key: Value);
    /// Map/Set 条目快照：Map → (key,value)；Set → (value,value)。
    fn map_set_entries_snapshot(&mut self, handle: u32, is_set: bool) -> Vec<(Value, Value)>;
    /// 创建 Map/Set 迭代器（kind: 0=keys, 1=values, 2=entries）。
    fn create_map_set_iterator(&mut self, handle: u32, is_set: bool, kind: u8) -> Value;

    /// WeakMap 表新建。
    fn weakmap_table_create(&mut self) -> u32;
    fn weakmap_set(&mut self, handle: u32, key_handle: Handle, val: Value);
    fn weakmap_get(&mut self, handle: u32, key_handle: Handle) -> Option<Value>;
    fn weakmap_has(&mut self, handle: u32, key_handle: Handle) -> bool;
    fn weakmap_delete(&mut self, handle: u32, key_handle: Handle) -> bool;

    /// WeakSet 表新建。
    fn weakset_table_create(&mut self) -> u32;
    fn weakset_add(&mut self, handle: u32, key_handle: Handle);
    fn weakset_has(&mut self, handle: u32, key_handle: Handle) -> bool;
    fn weakset_delete(&mut self, handle: u32, key_handle: Handle) -> bool;

    /// ArrayBuffer 表：分配 length 字节，返回 table index。
    fn arraybuffer_create(&mut self, byte_length: u32) -> Option<u32>;
    /// ArrayBuffer 字节长度。
    fn arraybuffer_byte_length(&mut self, handle: u32) -> Option<u32>;
    /// ArrayBuffer 切片复制到新 buffer，返回新 handle。
    fn arraybuffer_slice(&mut self, handle: u32, start: u32, end: u32) -> Option<u32>;
    /// ArrayBuffer 原始字节读。
    fn arraybuffer_read_bytes(&mut self, handle: u32, offset: usize, len: usize)
    -> Option<Vec<u8>>;
    /// ArrayBuffer 原始字节写。
    fn arraybuffer_write_bytes(&mut self, handle: u32, offset: usize, bytes: &[u8]) -> bool;

    /// SharedArrayBuffer 表。
    fn shared_arraybuffer_create(&mut self, byte_length: u32) -> Option<u32>;
    fn shared_arraybuffer_byte_length(&mut self, handle: u32) -> Option<u32>;
    /// 创建 SAB backing 并将元数据挂到目标对象；`max_byte_length=None` 表示定长。
    fn shared_arraybuffer_create_object(
        &mut self,
        target: Value,
        byte_length: u64,
        max_byte_length: Option<u64>,
    ) -> Value;
    /// 读取 `(handle, byte_length, max_byte_length)`。
    fn shared_arraybuffer_info(&mut self, this: Value) -> Option<(u32, u64, Option<u64>)>;
    /// 扩容 backing 并同步可观察元数据。
    fn shared_arraybuffer_grow(&mut self, this: Value, new_length: u64) -> bool;
    /// 复制 `[start, end)` 为新的定长 SAB 对象。
    fn shared_arraybuffer_slice(&mut self, this: Value, start: u64, end: u64) -> Option<Value>;

    /// 解析 ArrayBuffer / SharedArrayBuffer 底层 backing。
    /// 返回 `(table_handle, byte_length, is_shared)`。
    fn resolve_buffer_backing(&mut self, buffer: Value) -> Option<(u32, u32, bool)>;

    /// 读 buffer 原始字节（ArrayBuffer 或 SharedArrayBuffer）。
    fn buffer_read_bytes(
        &mut self,
        handle: u32,
        is_shared: bool,
        offset: usize,
        len: usize,
    ) -> Option<Vec<u8>>;
    /// 写 buffer 原始字节。
    fn buffer_write_bytes(
        &mut self,
        handle: u32,
        is_shared: bool,
        offset: usize,
        bytes: &[u8],
    ) -> bool;

    // ═══ Atomics 后端原语 ═══

    fn buffer_atomic_load(&mut self, view: &TypedArrayView, byte_offset: u64) -> Option<i64>;
    fn buffer_atomic_store(
        &mut self,
        view: &TypedArrayView,
        byte_offset: u64,
        value: i64,
    ) -> Option<()>;
    fn buffer_atomic_rmw(
        &mut self,
        view: &TypedArrayView,
        byte_offset: u64,
        op: AtomicsRmwOp,
        operand: i64,
    ) -> Option<i64>;
    fn buffer_atomic_compare_exchange(
        &mut self,
        view: &TypedArrayView,
        byte_offset: u64,
        expected: i64,
        replacement: i64,
    ) -> Option<i64>;
    fn atomics_notify(
        &mut self,
        view: &TypedArrayView,
        byte_offset: u64,
        count: Option<u32>,
    ) -> u32;
    fn atomics_wait_async_op<'a>(
        &'a mut self,
        view: TypedArrayView,
        byte_offset: u64,
        expected: i64,
        timeout_ms: f64,
    ) -> ExecFuture<'a>;
    fn atomics_wait_sync<'a>(
        &'a mut self,
        view: TypedArrayView,
        byte_offset: u64,
        expected: i64,
        timeout_ms: f64,
    ) -> ExecFuture<'a>;

    /// DataView 表：挂到 buffer 上；`buffer_object` 为可选 identity 引用。
    fn dataview_create(
        &mut self,
        buffer_handle: u32,
        buffer_object: Option<Value>,
        byte_offset: u32,
        byte_length: u32,
        is_shared: bool,
    ) -> Option<u32>;
    fn dataview_resolve(&mut self, handle: u32) -> Option<(u32, u32, u32, bool)>;

    /// TypedArray 表登记。
    #[allow(clippy::too_many_arguments)]
    fn typedarray_table_create(
        &mut self,
        buffer_handle: u32,
        buffer_object: Option<Value>,
        byte_offset: u32,
        length: u32,
        element_size: u8,
        element_kind: u8,
        is_shared: bool,
    ) -> u32;

    /// Live TypedArray 迭代器：`kind` 0=entries / 1=keys / 2=values。
    fn create_typedarray_iterator(&mut self, this: Value, kind: u8) -> Value;

    /// Proxy-aware Reflect.ownKeys → 键数组。
    fn reflect_own_keys(&mut self, target: Value) -> Value;

    /// 创建集合/缓冲原型方法 NativeCallable。
    /// kind 字符串约定见 WasmExecContext 实现。
    fn create_collection_method(&mut self, kind: &str) -> Value;

    /// Date 原型方法 NativeCallable（kind 字符串与 DateMethodKind 同名 snake）。
    fn create_date_method(&mut self, kind: &str) -> Value;
    /// 读 Date 实例 `__date_ms__`。
    fn date_read_ms(&mut self, this: Value) -> f64;
    /// Date 多参数 → 毫秒（含本地/UTC 时区）。
    fn date_args_to_ms(&mut self, args: &[Value], is_utc: bool) -> f64;
    /// 当前 UTC 毫秒。
    fn date_now_ms(&mut self) -> f64;
    /// 构造时 `new.target`。
    fn new_target(&mut self) -> Value;
    /// 将 Date.prototype 挂到实例。
    fn set_date_prototype(&mut self, obj: Value);

    /// JS 全局对象单例。
    fn js_global_get(&mut self) -> Value;
    fn js_global_set(&mut self, obj: Value);
    fn install_process_global(&mut self, global: Value);
    fn install_node_web_globals(&mut self, global: Value);
    fn push_host_temp_roots(&mut self, vals: &[Value]) -> usize;
    fn truncate_host_temp_roots(&mut self, len: usize);

    /// 释放未绑定 owner 的 Map 条目。
    fn release_unowned_map_entry(&mut self, handle: u32);
    /// 释放未绑定 owner 的 Set 条目。
    fn release_unowned_set_entry(&mut self, handle: u32);
    /// 绑定 Map/Set owner handle（对象 identity）。
    fn bind_map_owner(&mut self, handle: u32, owner: Handle);
    fn bind_set_owner(&mut self, handle: u32, owner: Handle);

    // ═══ Misc / 模块 / 微任务 ═══

    fn queue_microtask(&mut self, callback: Value);
    fn register_module_namespace(&mut self, module_id: u32, namespace: Value);
    fn dynamic_import(&mut self, module_id: u32) -> Value;

    fn module_resolve(
        &mut self,
        referrer: crate::RuntimeModuleReferrer,
        specifier: &str,
        kind: crate::RuntimeModuleResolutionKind,
    ) -> Result<crate::RuntimeResolvedModule, crate::RuntimeModuleLoadError>;
    fn module_resolve_paths(
        &mut self,
        referrer: crate::RuntimeModuleReferrer,
        specifier: &str,
    ) -> Result<Option<Vec<std::path::PathBuf>>, crate::RuntimeModuleLoadError>;
    fn module_cached_require(
        &mut self,
        key: &crate::RuntimeModuleKey,
    ) -> crate::RuntimeModuleRequireResult;
    fn module_cached_import(
        &mut self,
        key: &crate::RuntimeModuleKey,
    ) -> crate::RuntimeModuleImportResult;
    fn module_instantiate_sync(
        &mut self,
        resolved: &crate::RuntimeResolvedModule,
        env: crate::RuntimeInstantiationEnv,
    ) -> Result<crate::RuntimeInstantiatedModule, crate::RuntimeModuleLoadError>;
    fn module_instantiate_async<'a>(
        &'a mut self,
        resolved: crate::RuntimeResolvedModule,
        env: crate::RuntimeInstantiationEnv,
    ) -> ExecFuture<
        'a,
        Result<crate::RuntimeInstantiatedModule, crate::RuntimeModuleLoadError>,
    >;
    fn module_finish_loaded(
        &mut self,
        key: crate::RuntimeModuleKey,
        instantiated: crate::RuntimeInstantiatedModule,
    );
    fn module_finish_errored(
        &mut self,
        key: crate::RuntimeModuleKey,
        module_id: Option<u32>,
        reason: Value,
    );
    fn module_require_cache_entry(
        &mut self,
        cache_key: &str,
    ) -> Option<crate::RuntimeRequireCacheEntry>;
    fn module_require_cache_entries(&mut self) -> Vec<crate::RuntimeRequireCacheEntry>;
    fn module_delete_require_cache_entry(&mut self, cache_key: &str) -> bool;
    fn create_require_cache_proxy(&mut self) -> Value;
    /// 分配 null 原型对象。
    fn alloc_null_proto_object(&mut self, capacity: u32) -> Value;

    // ═══ Object 低层原语（算法在 builtins）═══

    /// ToObject。
    fn to_object(&mut self, val: Value) -> Value;
    /// [[Extensible]]。
    fn is_extensible(&mut self, obj: Value) -> bool;
    /// [[PreventExtensions]]。
    fn prevent_extensions(&mut self, obj: Value) -> bool;
    /// Own 属性 (name_id, flags) 列表。
    fn own_property_entries(&mut self, handle: Handle) -> Vec<(u32, u32)>;
    /// 更新 own 属性 flags。
    fn update_property_flags(&mut self, handle: Handle, name_id: u32, flags: u32) -> bool;
    /// Own 可枚举/全部字符串属性名。
    fn collect_own_property_names(&mut self, obj: Value, enumerable_only: bool) -> Vec<String>;
    /// Own symbol 键列表。
    fn collect_own_property_symbols(&mut self, obj: Value) -> Vec<Value>;
    /// 按字符串键读属性（含 accessor getter）。
    fn read_property_by_string_key(&mut self, obj: Value, key: &str) -> Value;
    /// Own 属性是否存在。
    fn has_own_property_by_name_id(&mut self, handle: Handle, name_id: u32) -> bool;
    /// OrdinaryGetOwnProperty → 描述符对象（或 undefined）。
    fn get_own_property_descriptor_value(&mut self, target: Value, prop: Value) -> Value;
    /// DefinePropertyOrThrow。
    fn define_property_or_throw(&mut self, target: Value, key: Value, desc: Value) -> bool;
    /// 读对象当前 [[Prototype]] 的 raw handle（null = 0xFFFF_FFFF）。
    fn object_proto_handle(&mut self, obj: Value) -> Option<u32>;
    /// 将 proto 值转为可写入 header 的 raw handle。
    fn value_to_proto_handle(&mut self, proto: Value) -> u32;
    /// 写入 [[Prototype]]（raw handle）。
    fn set_prototype_handle(&mut self, obj: Value, proto_handle: u32) -> bool;
    /// handle 是否仍存活。
    fn handle_is_live(&mut self, handle: Handle) -> bool;
    /// 将 heap handle 编码为 JS 对象/数组值。
    fn encode_handle_as_value(&mut self, handle: Handle) -> Value;

    // ═══ WeakRef / FinalizationRegistry 表原语 ═══

    /// 解析弱引用目标 handle。
    fn weak_target_handle(&mut self, target: Value) -> Option<Handle>;
    /// 推入 WeakRef 表，返回索引。
    fn weakref_table_push(&mut self, target_handle: Handle) -> u32;
    /// 读 WeakRef 表目标；已清则为 None。
    fn weakref_table_get_target(&mut self, index: u32) -> Option<Handle>;
    /// 推入 FinalizationRegistry 表，返回索引。
    fn finalization_registry_table_push(&mut self, object_handle: Handle, callback: Value) -> u32;
    /// 追加 registration。
    fn finalization_registry_add(
        &mut self,
        registry_idx: u32,
        target_handle: Handle,
        held_value: Value,
        unregister_token: Option<Value>,
    );
    /// 按 token 注销；返回是否移除了至少一条。
    fn finalization_registry_unregister_token(&mut self, registry_idx: u32, token: Value) -> bool;
    /// 创建 WeakRef/FR 原型方法 NativeCallable。
    /// kind: "weakref_deref" | "fr_register" | "fr_unregister"
    fn create_weakref_method(&mut self, kind: &str) -> Value;

    // ═══ String / RegExp 低层原语 ═══

    fn create_string_primitive_method(&mut self, method: u8) -> Value;
    fn create_string_iterator(&mut self, s: crate::RuntimeString) -> Value;
    fn obj_proto_to_string(&mut self, receiver: Value) -> Value;
    /// RegExp 是否含 `g` flag。
    fn regexp_is_global(&mut self, regex: Value) -> bool;
    /// 值转显示字符串（含 ToString 路径）。
    fn value_to_display_string(&mut self, val: Value) -> String;
    /// 调用 well-known symbol 方法；无方法返回 None，有方法返回 Some(结果)。
    fn call_symbol_method_async<'a>(
        &'a mut self,
        target: Value,
        symbol_idx: u32,
        this_arg: Value,
        args: &'a [Value],
    ) -> ExecFuture<'a, Option<Value>>;
    /// RegExp 匹配信息（Send-safe，无 Match 引用）。
    fn regexp_collect_matches(
        &mut self,
        regex: Value,
        subject: &str,
        global: bool,
    ) -> Vec<RegExpMatchInfo>;
    /// RegExp 默认 match/search/split/matchAll（无 @@ 方法时的回退）。
    fn regexp_string_match_default(&mut self, receiver: Value, regexp: Value) -> Value;
    fn regexp_string_search_default(&mut self, receiver: Value, regexp: Value) -> Value;
    fn regexp_string_split_default(&mut self, receiver: Value, sep: Value, limit: Value) -> Value;
    fn regexp_match_all_default(&mut self, this_val: Value, regexp: Value) -> Value;

    // ═══ op_in / 迭代器低层原语 ═══

    /// Proxy 是否已撤销（存在且 revoked）。
    fn proxy_is_revoked(&mut self, proxy: Handle) -> bool;
    /// 从属性对象读 `"has"` 等 trap（own + proto）。
    fn read_data_property(&mut self, obj: Value, key: &str) -> Value;

    /// 创建数组迭代器。
    fn create_array_iterator(&mut self, arr: Value) -> Value;
    /// 创建 Set 值迭代器（若 val 为 set 包装对象）。
    fn try_create_set_iterator(&mut self, val: Value) -> Option<Value>;
    /// 包装普通对象迭代器（读 next/return/throw）。
    // ═══ Fetch 状态表与 I/O 边界 ═══
    fn alloc_headers(&mut self, entry: crate::HeadersEntry) -> u32;
    fn with_headers<R>(
        &mut self,
        handle: u32,
        f: impl FnOnce(&mut crate::HeadersEntry) -> R,
    ) -> Option<R>;
    fn alloc_fetch_response(&mut self, entry: crate::FetchResponseEntry) -> u32;
    fn with_fetch_response<R>(
        &mut self,
        handle: u32,
        f: impl FnOnce(&mut crate::FetchResponseEntry) -> R,
    ) -> Option<R>;
    fn alloc_fetch_request(&mut self, entry: crate::FetchRequestEntry) -> u32;
    fn with_fetch_request<R>(
        &mut self,
        handle: u32,
        f: impl FnOnce(&mut crate::FetchRequestEntry) -> R,
    ) -> Option<R>;
    fn alloc_abort_signal(&mut self, entry: crate::AbortSignalEntry) -> u32;
    fn with_abort_signal<R>(
        &mut self,
        handle: u32,
        f: impl FnOnce(&mut crate::AbortSignalEntry) -> R,
    ) -> Option<R>;
    fn http_fetch_begin<'a>(
        &'a mut self,
        request: crate::HttpRequestSpec,
    ) -> ExecFuture<'a>;
    fn create_arraybuffer_from_bytes(&mut self, bytes: &[u8]) -> Value;
    fn consume_fetch_body_to_bytes(
        &mut self,
        http_handle: u32,
        promise: Value,
        kind: crate::ResponseMethodKind,
    ) -> bool;

    fn fetch_resource_timing_enabled(&mut self) -> bool;
    fn performance_now(&mut self) -> f64;
    fn commit_fetch_resource_timing(&mut self, timing: &crate::FetchResourceTimingState);

    // ═══ WHATWG Streams 状态表 ═══

    fn stream_create_uint8array(&mut self, bytes: &[u8]) -> Value;
    fn stream_typedarray_u8_bytes(&mut self, typedarray: Value) -> Option<Vec<u8>>;
    fn stream_write_u8_bytes(&mut self, view: Value, bytes: &[u8]) -> Option<usize>;
    fn stream_transfer_byob_view(&mut self, view: Value, bytes_written: usize) -> Option<Value>;
    fn mark_response_body_used(&mut self, response_handle: Option<u32>, response_obj: Option<Value>);
    fn schedule_readable_pull(&mut self, callback: Value, this_value: Value, controller: Value);
    fn schedule_readable_pipe_pump(&mut self, readable_handle: u32);
    fn fetch_body_reader_read(
        &mut self,
        reader_handle: u32,
        http_handle: u32,
        byob_view: Option<Value>,
    ) -> Option<Value>;
    fn create_writable_abort_signal(&mut self) -> Value;
    fn mark_writable_stream_signal_aborted(&mut self, stream_handle: u32, reason: Value);
    fn schedule_writable_sink_write(
        &mut self,
        callback: Value,
        this_value: Value,
        chunk: Value,
        controller: Value,
        write_promise: Value,
    );
    fn schedule_writable_sink_close(
        &mut self,
        callback: Option<Value>,
        this_value: Value,
        controller: Value,
        writable_stream_handle: u32,
        close_promise: Value,
    );
    fn schedule_transform_stream_transform(
        &mut self,
        callback: Value,
        this_value: Value,
        chunk: Value,
        controller: Value,
        write_promise: Value,
    );
    fn schedule_transform_stream_flush(&mut self, params: TransformStreamFlushParams);

    fn cancel_http_response(&mut self, http_handle: u32);


    fn alloc_readable_stream(&mut self, entry: crate::ReadableStreamEntry) -> u32;
    fn with_readable_stream<R>(
        &mut self,
        handle: u32,
        f: impl FnOnce(&mut crate::ReadableStreamEntry) -> R,
    ) -> Option<R>;
    fn bind_readable_stream_object(&mut self, object: Handle, handle: u32);

    fn alloc_reader(&mut self, entry: crate::ReaderEntry) -> u32;
    fn with_reader<R>(
        &mut self,
        handle: u32,
        f: impl FnOnce(&mut crate::ReaderEntry) -> R,
    ) -> Option<R>;
    fn with_readers<R>(
        &mut self,
        f: impl FnOnce(&mut [Option<crate::ReaderEntry>]) -> R,
    ) -> R;
    fn bind_reader_object(&mut self, object: Handle, handle: u32);

    fn alloc_stream_controller(&mut self, entry: crate::StreamControllerEntry) -> u32;
    fn with_stream_controller<R>(
        &mut self,
        handle: u32,
        f: impl FnOnce(&mut crate::StreamControllerEntry) -> R,
    ) -> Option<R>;
    fn bind_stream_controller_object(&mut self, object: Handle, handle: u32);

    fn alloc_byob_request(&mut self, entry: crate::ByobRequestEntry) -> u32;
    fn with_byob_request<R>(
        &mut self,
        handle: u32,
        f: impl FnOnce(&mut crate::ByobRequestEntry) -> R,
    ) -> Option<R>;
    fn bind_byob_request_object(&mut self, object: Handle, handle: u32);

    fn alloc_writable_stream(&mut self, entry: crate::WritableStreamEntry) -> u32;
    fn with_writable_stream<R>(
        &mut self,
        handle: u32,
        f: impl FnOnce(&mut crate::WritableStreamEntry) -> R,
    ) -> Option<R>;
    fn bind_writable_stream_object(&mut self, object: Handle, handle: u32);

    fn alloc_writer(&mut self, entry: crate::WriterEntry) -> u32;
    fn with_writer<R>(
        &mut self,
        handle: u32,
        f: impl FnOnce(&mut crate::WriterEntry) -> R,
    ) -> Option<R>;
    fn with_writers<R>(
        &mut self,
        f: impl FnOnce(&mut [Option<crate::WriterEntry>]) -> R,
    ) -> R;
    fn bind_writer_object(&mut self, object: Handle, handle: u32);

    fn alloc_transform_stream(&mut self, entry: crate::TransformStreamEntry) -> u32;
    fn with_transform_stream<R>(
        &mut self,
        handle: u32,
        f: impl FnOnce(&mut crate::TransformStreamEntry) -> R,
    ) -> Option<R>;
    fn with_transform_streams<R>(
        &mut self,
        f: impl FnOnce(&mut [Option<crate::TransformStreamEntry>]) -> R,
    ) -> R;
    fn bind_transform_stream_object(&mut self, object: Handle, handle: u32);
    fn create_object_iterator(&mut self, iterator: Value) -> Value;
    /// 创建错误迭代器状态。
    fn create_error_iterator(&mut self) -> Value;

    /// 同步推进非 Object 迭代器；若需调用 .next() 返回 NeedObjectNext。
    fn iterator_next_sync_step(&mut self, handle: Value) -> IteratorNextStep;
    /// 写入 ObjectIter 当前项。
    fn iterator_store_object_current(
        &mut self,
        handle: Value,
        current: Value,
        done: bool,
        has_current: bool,
    );
    /// 同步可决的 IteratorDone；需回调时返回 None。
    fn iterator_done_sync(&mut self, handle: Value) -> Option<bool>;
    /// ObjectIter 的 (iterator, next) 以便异步推进 done。
    fn iterator_object_next_pair(&mut self, handle: Value) -> Option<(Value, Value)>;
    /// ObjectIter 的 (iterator, return_method)；已 done 或不存在返回 None。
    fn iterator_object_return_pair(&mut self, handle: Value) -> Option<(Value, Option<Value>)>;
    /// 标记 ObjectIter done。
    fn iterator_mark_done(&mut self, handle: Value);
    /// 查 async-from-sync 表。
    fn iterator_lookup_afs(&mut self, handle: Value) -> Option<u32>;
    /// 读取 async-from-sync 条目关联的外层迭代器 handle。
    fn async_from_sync_outer_iterator(&mut self, afs: u32) -> Option<Value>;
    /// 从 AsyncFromSyncNext native callable 读取条目索引。
    fn async_from_sync_native_handle(&mut self, next: Value) -> Option<u32>;
    /// 推进 async-from-sync 状态机，返回其 promise/IteratorResult。
    fn advance_async_from_sync<'a>(&'a mut self, afs: u32) -> ExecFuture<'a, Value>;
    /// IteratorValue（当前项）。
    fn iterator_current_value(&mut self, handle: Value) -> Value;
    /// 异常 → rejected promise。
    fn promise_reject_exception(&mut self, exc: Value) -> Value;
    /// 解析 IteratorResult `{value, done}`。
    fn parse_iterator_result(&mut self, result: Value) -> Option<(Value, bool)>;
    /// 值是否为 promise。
    fn is_promise_value(&mut self, val: Value) -> bool;
    /// 已结算 promise 的结果；pending 返回 None。
    fn promise_settled(&mut self, promise: Value) -> Option<Result<Value, Value>>;
    /// 异常 reason。
    fn exception_reason(&mut self, exc: Value) -> Value;
    /// 分配 rejected promise。
    fn alloc_rejected_promise(&mut self, reason: Value) -> Value;
}

/// `schedule_transform_stream_flush` 参数包：归集 8 个独立参数为单 struct，
/// 降低 trait 方法参数数，避免 clippy `too many arguments` 警告。
#[derive(Clone, Copy, Debug)]
pub struct TransformStreamFlushParams {
    pub callback: Option<Value>,
    pub this_value: Value,
    pub controller: Value,
    pub writable_stream_handle: u32,
    pub readable_stream_handle: u32,
    pub readable_controller_handle: u32,
    pub close_promise: Value,
}

/// RegExp 单次匹配的 Send-safe 投影。
#[derive(Clone, Debug)]
pub struct RegExpMatchInfo {
    pub start: usize,
    pub end: usize,
    pub captures: Vec<Option<std::ops::Range<usize>>>,
    pub named: Vec<(String, Option<std::ops::Range<usize>>)>,
}

/// `iterator_next` 同步步骤结果。
#[derive(Clone, Debug)]
pub enum IteratorNextStep {
    /// 同步推进完成（String/Array/Map/...）。
    Advanced,
    /// 句柄无效。
    Missing,
    /// 需调用 ObjectIter.next。
    NeedObjectNext { iterator: Value, next: Value },
    /// 需推进 async-from-sync。
    NeedAsyncFromSync { afs: u32 },
    /// Error 迭代器 → 返回 done result。
    ErrorDone,
}

/// Atomics read-modify-write 操作。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AtomicsRmwOp {
    Add,
    Sub,
    And,
    Or,
    Xor,
    Exchange,
}

/// TypedArray 视图（后端无关投影）。
#[derive(Clone, Copy, Debug)]
pub struct TypedArrayView {
    pub buffer_handle: u32,
    pub byte_offset: u32,
    pub length: u32,
    pub element_size: u8,
    pub element_kind: u8,
    pub is_shared: bool,
}
