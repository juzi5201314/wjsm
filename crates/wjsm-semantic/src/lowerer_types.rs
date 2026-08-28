use super::*;

pub(crate) type ExpressionContinuationFrame = (
    Option<BasicBlockId>,
    Option<BasicBlockId>,
    Option<BasicBlockId>,
    Option<BasicBlockId>,
);

#[derive(Clone)]
pub(crate) struct IterationEnvFrame {
    pub(crate) function_scope_id: usize,
    pub(crate) bindings: Vec<CapturedBinding>,
    pub(crate) ir_name: String,
    pub(crate) parent_ir_name: String,
}

/// async / async generator 方法族的 super 绑定来源。
///
/// async 函数族的 body 是独立 IR 函数，经续体 resume 调用，activation env 是
/// 续体对象而非方法闭包 env，故 body 内 `super` 需要显式接线：
/// - 类方法的 home object 静态可知，直接烙进 body/wrapper 函数元数据
///   （运行时 `prepare_call` 按元数据填充 activation home，与箭头函数同径）；
/// - 对象字面量方法的 home 是运行时值，存于方法闭包 env 的 `home` 属性，
///   wrapper 把它转存为续体对象的自有属性，body 的 GetSuperBase 经
///   activation-env 回退路径解析。
#[derive(Clone, Copy)]
pub(crate) enum MethodSuperBinding {
    /// 非方法：body 内 super 非法。
    None,
    /// 类方法：home object 静态可知。
    Static(HomeObject),
    /// 对象字面量方法：home 经方法闭包 env → 续体自有属性传递。
    ClosureEnv,
}

/// 脚本模式顶层声明在全局环境记录中的绑定类别（ES §9.1.1.4）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ScriptGlobalKind {
    /// 对象记录 var 绑定（CreateGlobalVarBinding）。
    Var,
    /// 对象记录函数绑定（CreateGlobalFunctionBinding，顶层函数声明）。
    Func,
    /// 声明式记录词法绑定（let/const/class；`true` = 不可变）。
    Lexical { is_const: bool },
}

/// 类私有名词法条目。
#[derive(Clone)]
pub(crate) struct PrivateNameEntry {
    /// 该声明类的不可公开运行时槽名（`#名@类id`）。
    pub(crate) storage_name: String,
    /// `#x in 非对象` TypeError 的显示名：实例私有方法/访问器为类 brand（类名），
    /// 其余为 `#名`（与 V8/Node 文案一致）。
    pub(crate) in_display_name: String,
}

pub(crate) struct Lowerer {
    pub(crate) module: Module,
    pub(crate) next_value: u32,
    pub(crate) scopes: ScopeTree,
    pub(crate) hoisted_vars: Vec<HoistedVar>,
    /// 用于 O(1) 重复检测的 HashSet。
    pub(crate) hoisted_vars_set: std::collections::HashSet<(usize, String)>,
    pub(crate) current_function: FunctionBuilder,
    pub(crate) label_stack: Vec<LabelContext>,
    pub(crate) finally_stack: Vec<FinallyContext>,
    pub(crate) try_contexts: Vec<TryContext>,
    pub(crate) next_temp: u32,
    pub(crate) pending_loop_label: Option<String>,
    pub(crate) active_finalizers: Vec<PendingFinalizer>,
    /// 匿名类 / 匿名函数计数器
    pub(crate) anon_counter: u32,
    /// NamedEvaluation（ES §8.4.5）的名字提示：变量声明 / 赋值 / 属性定义 /
    /// 默认值 / export default 在降级匿名函数定义（无名函数表达式、箭头、
    /// 无名类表达式）之前设置；`lower_expr` 入口取走，只对匿名函数定义形态
    /// （含括号 / TS 断言透传）回填，其余表达式形态自然丢弃。
    pub(crate) named_eval_hint: Option<String>,
    /// 类私有名词法栈：源名 → 该声明类的槽名与 `in` 错误显示名。
    pub(crate) private_name_stack: Vec<std::collections::HashMap<String, PrivateNameEntry>>,
    pub(crate) next_private_name_id: u32,
    // ── Function context stack ────────────────────────────────────────────
    pub(crate) function_stack: Vec<FunctionBuilder>,
    pub(crate) function_hoisted_stack: Vec<FunctionHoistedState>,
    pub(crate) function_next_value_stack: Vec<u32>,
    pub(crate) function_next_temp_stack: Vec<u32>,
    pub(crate) async_context_stack: Vec<AsyncContextState>,
    pub(crate) function_try_contexts_stack: Vec<Vec<TryContext>>,
    pub(crate) function_finally_stack_stack: Vec<Vec<FinallyContext>>,
    pub(crate) function_label_stack_stack: Vec<Vec<LabelContext>>,
    pub(crate) function_active_finalizers_stack: Vec<Vec<PendingFinalizer>>,
    pub(crate) function_pending_loop_label_stack: Vec<Option<String>>,
    // ── 闭包捕获相关 ──────────────────────────────────────────────────
    /// 每层函数的捕获绑定列表，push_function_context 时压入空 Vec。
    pub(crate) captured_names_stack: Vec<Vec<CapturedBinding>>,
    /// 当前函数中被任意后代函数捕获的本地 binding；不随 CFG shared-env 状态回退。
    pub(crate) shared_binding_names_stack: Vec<std::collections::HashSet<CapturedBinding>>,
    /// 每层函数的 function scope id，用于判断变量是否逃逸。
    pub(crate) function_scope_id_stack: Vec<usize>,
    /// 追踪当前是否在箭头函数中（箭头函数的 this 需要词法捕获）
    pub(crate) is_arrow_fn_stack: Vec<bool>,
    /// 当前函数是否拥有 [[HomeObject]] / 可合法解析 super。
    pub(crate) super_allowed: bool,
    /// 当前函数是否可合法执行 super() 构造调用。
    pub(crate) super_call_allowed: bool,
    pub(crate) function_super_allowed_stack: Vec<bool>,
    pub(crate) function_super_call_allowed_stack: Vec<bool>,
    pub(crate) function_is_arrow_stack: Vec<bool>,
    pub(crate) function_is_method_stack: Vec<bool>,
    /// 词法上可继承的 [[HomeObject]]（类方法体内嵌套箭头函数使用）。
    pub(crate) lexical_home_object: Option<HomeObject>,
    pub(crate) function_lexical_home_object_stack: Vec<Option<HomeObject>>,
    /// 派生类显式构造器的实例初始化上下文：super() 站点按 ECMAScript
    /// SuperCall 语义（BindThisValue 后 InitializeInstanceElements）发射
    /// 参数属性字段与实例字段初始化器。构造器帧持有；箭头帧随词法 super
    /// 能力克隆继承（箭头体内 super() 同样发射）；普通嵌套函数帧为 None。
    pub(crate) derived_ctor_init_ctx: Option<Box<crate::lowerer_classes_ts::DerivedCtorInitCtx>>,
    pub(crate) function_derived_ctor_init_ctx_stack:
        Vec<Option<Box<crate::lowerer_classes_ts::DerivedCtorInitCtx>>>,
    /// 派生构造器体内存在箭头函数时为 true：this 的规范存储改为共享 env
    /// （构造器入口注册），构造器帧的 this 读写、箭头帧的词法捕获与
    /// BindThisValue 重绑经同一 env 保持一致——super() 前后的 TDZ 哨兵与
    /// 重绑结果对全部帧同步可见。
    pub(crate) ctor_this_via_env: bool,
    pub(crate) function_ctor_this_via_env_stack: Vec<bool>,
    /// 派生类显式构造器的实例原型绑定（`$super_proto#ctor`）：入口取 `new`
    /// 传入预创建实例的原型（即 newTarget.prototype）存入该绑定，并将 this
    /// 绑定置为未初始化哨兵（this TDZ）。super() 站点据此为**每次** Construct
    /// 新建 thisArgument（规范 OrdinaryCreateFromConstructor 语义——二次
    /// super() 的父构造器副作用落在随后被丢弃的新对象上）。
    /// Some 同时标记「this 可能处于 TDZ」——this 读取须发射运行时检查。
    /// 箭头帧随词法 super 能力克隆继承；普通嵌套函数帧为 None。
    pub(crate) ctor_super_proto: Option<CapturedBinding>,
    pub(crate) function_ctor_super_proto_stack: Vec<Option<CapturedBinding>>,
    /// 模块/外层作用域 `const X = <字面量>` 绑定 → 字面量常量（IR 变量名
    /// `$<scope_id>.<name>` 为键）。闭包捕获读取时若命中则直接折叠为常量，
    /// 避免每次读取都经 env `obj_get` host 调用（基准：算术循环读模块 const
    /// 上限每迭代 2 次 host 调用，折叠后 19x）。仅在折叠命中时
    /// `add_constant`，未使用的记录不污染常量池。
    ///
    /// 语义安全：const 不可重赋值，值恒定；且仅当捕获函数体在声明语句之后
    /// 降级（此时记录已存在）才折叠——函数值只能在其 create_closure（声明语句
    /// 处）之后被调用，故读取必在初始化之后，无 TDZ 风险。
    pub(crate) module_const_literals: std::collections::HashMap<String, wjsm_ir::Constant>,
    /// 每层函数的共享 env 对象（ValueId + 已注册捕获变量集合 + 最后写入的 block + 是否 dominate 全部后续 block）。
    pub(crate) shared_env_stack: Vec<Option<SharedEnvFrame>>,
    /// 当前正在 lowering 的按迭代词法环境；嵌套循环按原型链连接。
    pub(crate) iteration_env_stack: Vec<IterationEnvFrame>,
    // ── 模块系统相关 ────────────────────────────────────────────────────────
    /// 当前正在编译的模块 ID（用于多模块编译）
    pub(crate) current_module_id: Option<wjsm_ir::ModuleId>,
    /// 导入映射：module_id → ImportBinding 列表
    pub(crate) import_bindings:
        std::collections::HashMap<wjsm_ir::ModuleId, Vec<wjsm_ir::ImportBinding>>,
    /// 导出映射：.0 = 模块 ID, .1 = 导出名 → 变量名
    pub(crate) export_map: std::collections::HashMap<(wjsm_ir::ModuleId, String), String>,
    /// 导入别名映射：(导入方模块 ID, local_name) → source_ir_name
    /// 用于 `import { x as y }` 和 `import x from './dep'` 等场景。
    /// 按导入方模块 ID 隔离，避免不同模块的同名 local 互相覆盖（#44）。
    pub(crate) import_aliases: std::collections::HashMap<(wjsm_ir::ModuleId, String), String>,
    /// 每个模块的顶层作用域 ID（predeclare 阶段分配，lower 阶段重新进入）。
    /// 同时隔离模块顶层词法声明、var 声明与函数声明。
    pub(crate) module_scopes: std::collections::HashMap<wjsm_ir::ModuleId, usize>,
    /// 每个模块的编译期路径元数据，供 CJS 路径绑定和 import.meta 使用。
    pub(crate) module_metadata: std::collections::HashMap<wjsm_ir::ModuleId, ModuleMetadata>,
    /// 动态 import() 目标映射：module_id → 被动态 import 的目标模块 ID 列表
    pub(crate) dynamic_import_targets:
        std::collections::HashMap<wjsm_ir::ModuleId, Vec<wjsm_ir::ModuleId>>,
    /// 动态 import specifier → ModuleId 映射：(当前模块 ID, specifier) → 目标 ModuleId
    pub(crate) dynamic_import_specifier_map:
        std::collections::HashMap<(wjsm_ir::ModuleId, String), wjsm_ir::ModuleId>,
    /// 需要构建命名空间对象的模块集合
    pub(crate) dynamic_import_namespace_modules: std::collections::HashSet<wjsm_ir::ModuleId>,
    /// 命名空间对象的 ValueId：ModuleId → ValueId（在模块体执行前创建，模块体执行后填充属性）
    pub(crate) dynamic_import_namespace_objects:
        std::collections::HashMap<wjsm_ir::ModuleId, wjsm_ir::ValueId>,
    pub(crate) module_export_names:
        std::collections::HashMap<wjsm_ir::ModuleId, std::collections::BTreeSet<String>>,
    /// 重导出声明（来自 analyze_module_links）
    pub(crate) re_export_map:
        std::collections::HashMap<wjsm_ir::ModuleId, Vec<wjsm_ir::ReExportBinding>>,
    /// 静态 `import * as ns` 的命名空间对象 ValueId：(导入方模块 ID, local_name) → ValueId
    pub(crate) static_namespace_import_objects:
        std::collections::HashMap<(wjsm_ir::ModuleId, String), wjsm_ir::ValueId>,
    /// 静态命名空间导入来源：(导入方模块 ID, local_name, 来源模块 ID)
    pub(crate) static_namespace_import_sources: Vec<(wjsm_ir::ModuleId, String, wjsm_ir::ModuleId)>,

    pub(crate) is_async_fn: bool,
    pub(crate) is_async_generator_fn: bool,
    pub(crate) is_generator_fn: bool,

    pub(crate) async_state_counter: u32,
    pub(crate) captured_var_slots: std::collections::HashMap<String, u32>,
    pub(crate) async_next_continuation_slot: u32,
    pub(crate) async_resume_blocks: Vec<(u32, BasicBlockId)>,
    pub(crate) async_promise_scope_id: usize,
    pub(crate) async_dispatch_block: Option<BasicBlockId>,
    pub(crate) async_main_body_entry: Option<BasicBlockId>,
    pub(crate) async_main_param_ir_names: Vec<String>,
    pub(crate) async_env_scope_id: usize,
    pub(crate) async_state_scope_id: usize,
    pub(crate) async_resume_val_scope_id: usize,
    pub(crate) async_is_rejected_scope_id: usize,
    pub(crate) async_generator_scope_id: usize,
    pub(crate) async_closure_env_ir_name: Option<String>,
    pub(crate) pending_suspends: Vec<lowerer_async_eval::PendingSuspend>,
    pub(crate) strict_mode: bool,
    /// push_function_context 保存外层严格模式，pop 恢复。
    pub(crate) function_strict_stack: Vec<bool>,
    pub(crate) is_arrow: bool,
    pub(crate) is_method: bool,
    /// 当前函数形参个数，供 emit_arguments_init 使用。
    pub(crate) arguments_param_count: u32,
    /// `arguments` 对象的预物化来源：generator/async 函数 body 从续体槽位加载 wrapper
    /// 侧物化好的 arguments 对象时设置；`emit_arguments_init` 在入口 take 消费，
    /// 命中时直接绑定该对象而不再发射 `CollectRestArgs`（body 的原生调用帧没有用户实参）。
    pub(crate) arguments_source_override: Option<ValueId>,
    /// rest 形参的预收集来源：generator/async 函数 body 从续体槽位加载 wrapper 侧
    /// 收集好的 rest 实参数组时设置；`emit_pat_inits_impl` 在入口 take 消费，
    /// 命中时直接解构该数组而不再发射 `CollectRestArgs`。
    pub(crate) rest_args_source_override: Option<ValueId>,
    /// mapped arguments 的形参别名元数据（ES §10.4.4.7 [[ParameterMap]]）：
    /// 全部形参为简单标识符时为各形参 IR 名（`$scope.name`，含重复形参改名
    /// 后的临时槽）的有序列表；非简单列表 / 不适用的降级点置 None。
    /// 各函数降级点在 `emit_arguments_init` 前设置，其入口 take 消费。
    pub(crate) arguments_simple_param_ir_names: Option<Vec<String>>,
    /// 函数（含嵌套闭包子树）可能包含 direct eval：eval 经激活记录读写局部
    /// 槽位，与形参别名重定向不相容；命中时禁用 [[ParameterMap]]（mapped
    /// 对象仍创建，但保持普通属性行为）。与上一字段成对设置/消费。
    pub(crate) arguments_alias_blocked: bool,
    /// 形参列表是否为 simple parameter list（无默认值 / rest / 解构）。
    /// §10.2.11 步骤 22.a：严格模式**或**非简单形参列表都建 unmapped 对象，
    /// 所以这条不能和上面的别名列表合并——零形参函数是简单列表（callee 为
    /// 数据属性）但没有可别名的形参。与上两字段成对设置/消费。
    pub(crate) arguments_simple_param_list: bool,
    /// mapped arguments 形参别名表：(形参声明作用域, 形参名) → 别名信息。
    /// 命中的绑定读写全部改经 MappedArgumentsBindingRead/Write，形参绑定
    /// 真值由 arguments 对象（映射期间的自有索引属性 / 解除后的宿主侧
    /// 绑定槽）持有，原生局部槽成为死存储。按 scope_id 全局唯一，无需出栈。
    pub(crate) mapped_arg_aliases:
        std::collections::HashMap<(usize, String), lowerer_mapped_args::MappedArgAlias>,
    pub(crate) script_mode: bool,
    /// 脚本模式主程序的顶层声明名 → 全局环境绑定类别（ES §16.1.7 GDI）。
    /// 命中的名字在读/写/typeof/delete 全部路由到 GlobalEnv 系列 builtin，
    /// 真值以宿主全局环境记录（对象记录 + 声明式记录）为唯一权威。
    pub(crate) script_global_names: std::collections::HashMap<String, ScriptGlobalKind>,
    /// GDI 词法声明的收集序（let/const/class，按源码声明顺序）。
    pub(crate) script_global_lexicals: Vec<(String, bool)>,
    /// GDI var 声明名的收集序（含块内 var 提升与 Annex B 函数名）。
    pub(crate) script_global_vars: Vec<String>,
    /// 仅由直接 eval 字面量静态提升引入的 var 名（EvalDeclarationInstantiation
    /// 的 CreateGlobalVarBinding(name, true)）：全局属性按 configurable=true
    /// 创建；显式 var/函数声明命中同名时移出本集合（非可配置优先）。
    pub(crate) script_global_eval_vars: std::collections::HashSet<String>,
    /// 当前正在降级脚本顶层词法声明（let/const）的绑定初始化：
    /// 命中的脚本全局词法名走 InitializeBinding（GlobalEnvInitLex，解除 TDZ）
    /// 而非 SetMutableBinding。仅声明语句的 pattern 目标受此标志影响。
    pub(crate) script_global_decl_init: bool,
    /// 是否在语句入口发射 `Instruction::DebugCheck`（默认关闭，不影响现有 IR 快照）。
    pub(crate) emit_debug_checks: bool,
    pub(crate) diagnostic_source: Option<std::sync::Arc<str>>,
    pub(crate) diagnostic_filename: String,
    pub(crate) eval_mode: bool,
    pub(crate) eval_has_scope_bridge: bool,
    pub(crate) eval_var_writes_to_scope: bool,
    pub(crate) eval_scope_record: bool,
    pub(crate) eval_caller_has_arguments: bool,
    /// eval 完成值槽的变量名（`$tmp.N`）。eval 模式在模块入口分配并初始化为
    /// undefined；完成值经内存槽而非 SSA 值线程化，跨 try/catch、循环等任意
    /// 控制流不产生支配性问题。详见 `lowerer_stmt/eval_completion.rs`。
    pub(crate) eval_completion_var: Option<String>,
    /// eval 调用在表达式上下文时的异常检查分叉后的 continue block。
    /// 由 lower_direct_eval_call 设置，由 resolve_store_block 消费。
    pub(crate) eval_continue_block: Option<BasicBlockId>,
    /// 由 lower_new_expr 在构建了异常检查分叉后设置，由 resolve_store_block 消费。
    pub(crate) new_expr_continue_block: Option<BasicBlockId>,
    /// 由 await 表达式设置，由 resolve_store_block 消费。
    pub(crate) await_continue_block: Option<BasicBlockId>,
    /// 由 lower_logical / lower_cond 在创建控制流表达式后设置其 merge block，
    /// 由 resolve_store_block 消费，确保后续指令插入到正确的继续块中。
    pub(crate) expr_merge_block: Option<BasicBlockId>,
    pub(crate) function_expr_continuation_stack: Vec<ExpressionContinuationFrame>,
    /// 当前作用域中活跃的 using 变量（用于作用域退出时自动 dispose）
    pub(crate) active_using_vars: Vec<ActiveUsingVar>,
    /// 追踪当前作用域中已推断为 Array 的绑定（scope_id, name）。
    /// Array.prototype 静态优化只在已知数组 receiver 上启用，避免劫持 Map/Set 等同名方法。
    pub(crate) array_bindings: std::collections::HashSet<(usize, String)>,
    /// 追踪当前作用域中已证明为 String 的 `const` 绑定（scope_id, name）。
    ///
    /// `slice` / `concat` / `includes` / `indexOf` / `lastIndexOf` 在
    /// String.prototype 与 Array.prototype 上同名，直连内建必须以「receiver 确为
    /// 字符串」的正向证明为前提。只收 `const`：它不可重新赋值，绑定一旦证明成立
    /// 就在整个作用域内成立，不受单遍 lowering 的源码顺序影响。
    pub(crate) string_bindings: std::collections::HashSet<(usize, String)>,
    /// 由 let 初始化器推断为字符串的绑定；仅用于生成运行时 IsString 守卫，
    /// 因为 let 后续可被重新赋值，不能进入无守卫的静态直连集合。
    pub(crate) maybe_string_bindings: std::collections::HashSet<(usize, String)>,
    /// 追踪当前作用域中已推断为 TypedArray 的绑定（scope_id, name）。
    /// 用于在 lower_call_expr 中让 arr.at()/arr.indexOf() 等走 TypedArray dispatch，
    /// 而不是被 String.prototype dispatch 错误拦截。
    pub(crate) typedarray_bindings: std::collections::HashSet<(usize, String)>,
    /// 追踪当前作用域中已推断为 SharedArrayBuffer 的绑定（scope_id, name）。
    /// 用于在 lower_call_expr 中让 sab.slice() / sab.grow() 等走 SAB dispatch，
    /// 而不是被 String.prototype dispatch 错误拦截（修复评审 P1 劫持问题）。
    pub(crate) sab_bindings: std::collections::HashSet<(usize, String)>,
    /// 追踪当前作用域中已推断为 DataView 的绑定（scope_id, name）。
    /// DataView 原型方法使用专用宿主导入签名，静态已知 receiver 必须直连 CallBuiltin，避免通用 call_indirect 调用约定不匹配。
    pub(crate) dataview_bindings: std::collections::HashSet<(usize, String)>,
    /// 追踪当前作用域中已推断为 Map 的绑定（scope_id, name）。
    /// Map 原型方法（set/get/has/delete 等）静态已知 receiver 直连 CallBuiltin，
    /// 免去每次调用的通用 Get + NativeCallable dispatch 往返。
    pub(crate) map_bindings: std::collections::HashSet<(usize, String)>,
    /// 追踪当前作用域中已推断为 Set 的绑定（scope_id, name）。
    pub(crate) set_bindings: std::collections::HashSet<(usize, String)>,
    /// 已降级的 with 语句计数：为 0 时标识符解析零成本跳过 with 分派
    /// （绝大多数程序不含 with，不应为其付出作用域链遍历代价）。
    pub(crate) with_scope_count: u32,
}

/// 追踪当前作用域中的 using 变量，用于在作用域退出时自动 dispose。
#[derive(Debug, Clone)]
pub(crate) struct ActiveUsingVar {
    pub(crate) ir_name: String,
    pub(crate) is_async: bool,
}

#[derive(Clone)]
pub(crate) struct AsyncContextState {
    pub(crate) is_async_fn: bool,
    pub(crate) is_async_generator_fn: bool,
    pub(crate) is_generator_fn: bool,
    pub(crate) async_state_counter: u32,
    pub(crate) captured_var_slots: std::collections::HashMap<String, u32>,
    pub(crate) async_next_continuation_slot: u32,
    pub(crate) async_resume_blocks: Vec<(u32, BasicBlockId)>,
    pub(crate) async_promise_scope_id: usize,
    pub(crate) async_dispatch_block: Option<BasicBlockId>,
    pub(crate) async_env_scope_id: usize,
    pub(crate) async_state_scope_id: usize,
    pub(crate) async_resume_val_scope_id: usize,
    pub(crate) async_is_rejected_scope_id: usize,
    pub(crate) async_generator_scope_id: usize,
    pub(crate) async_closure_env_ir_name: Option<String>,
    pub(crate) pending_suspends: Vec<lowerer_async_eval::PendingSuspend>,
    /// 悬挂中的 arguments/rest 预物化来源随函数上下文入栈清零、出栈恢复：
    /// 形参默认值里的嵌套函数在 override 设定与消费之间降级，若不清零会把
    /// 外层 body 的 ValueId 泄漏进嵌套函数（跨函数值引用 → IR 验证失败）。
    pub(crate) arguments_source_override: Option<ValueId>,
    pub(crate) rest_args_source_override: Option<ValueId>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HoistedVar {
    pub(crate) scope_id: usize,
    pub(crate) name: String,
}

pub(crate) type HoistedBindingSet = std::collections::HashSet<(usize, String)>;
pub(crate) type FunctionHoistedState = (Vec<HoistedVar>, HoistedBindingSet);
/// 单层函数的共享 env 状态：(env ValueId, 已注册捕获集合, 最后写入 block, 是否 dominate 后续 block)。
pub(crate) type SharedEnvFrame = (
    ValueId,
    std::collections::HashSet<CapturedBinding>,
    BasicBlockId,
    bool,
);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct CapturedBinding {
    pub(crate) name: String,
    pub(crate) scope_id: Option<usize>,
}

impl CapturedBinding {
    pub(crate) fn new(name: impl Into<String>, scope_id: usize) -> Self {
        Self {
            name: name.into(),
            scope_id: Some(scope_id),
        }
    }

    pub(crate) fn lexical_this() -> Self {
        Self {
            name: "$this".to_string(),
            scope_id: None,
        }
    }

    pub(crate) fn lexical_new_target() -> Self {
        Self {
            name: "__wjsm_new_target".to_string(),
            scope_id: None,
        }
    }

    pub(crate) fn is_lexical_new_target(&self) -> bool {
        self.scope_id.is_none() && self.name == "__wjsm_new_target"
    }

    pub(crate) fn is_lexical_this(&self) -> bool {
        self.scope_id.is_none() && self.name == "$this"
    }

    pub(crate) fn env_key(&self) -> String {
        match self.scope_id {
            Some(scope_id) => format!("${scope_id}.{}", self.name),
            None => self.name.clone(),
        }
    }

    pub(crate) fn display_name(&self) -> String {
        self.env_key()
    }

    pub(crate) fn var_ir_name(&self) -> String {
        match self.scope_id {
            Some(scope_id) => format!("${scope_id}.{}", self.name),
            None => self.name.clone(),
        }
    }
}
