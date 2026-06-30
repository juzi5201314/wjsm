use super::*;

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
    /// 每层函数的共享 env 对象 (ValueId) + 已注册的捕获绑定集合。
    /// 同一外层函数中的多个闭包共享同一个 env 对象，确保可变捕获变量的修改对所有闭包可见。
    pub(crate) shared_env_stack: Vec<Option<(ValueId, std::collections::HashSet<CapturedBinding>)>>,
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
    pub(crate) import_aliases:
        std::collections::HashMap<(wjsm_ir::ModuleId, String), String>,
    /// 每个模块的顶层块作用域 ID（predeclare 阶段分配，lower 阶段重新进入）。
    /// 使各模块顶层 let/const 处于独立作用域，避免跨模块同名冲突（#43）。
    pub(crate) module_scopes: std::collections::HashMap<wjsm_ir::ModuleId, usize>,
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
    pub(crate) static_namespace_import_sources:
        Vec<(wjsm_ir::ModuleId, String, wjsm_ir::ModuleId)>,

    pub(crate) is_async_fn: bool,
    pub(crate) is_async_generator_fn: bool,

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
    pub(crate) is_arrow: bool,
    pub(crate) is_method: bool,
    /// 当前函数形参个数，供 emit_arguments_init 使用。
    pub(crate) arguments_param_count: u32,
    pub(crate) script_mode: bool,
    pub(crate) diagnostic_source: Option<std::sync::Arc<str>>,
    pub(crate) diagnostic_filename: String,
    pub(crate) eval_mode: bool,
    pub(crate) eval_has_scope_bridge: bool,
    pub(crate) eval_var_writes_to_scope: bool,
    pub(crate) eval_scope_record: bool,
    pub(crate) eval_caller_has_arguments: bool,
    pub(crate) eval_completion: Option<ValueId>,
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
    /// 当前作用域中活跃的 using 变量（用于作用域退出时自动 dispose）
    pub(crate) active_using_vars: Vec<ActiveUsingVar>,
    /// 追踪当前作用域中已推断为 Array 的绑定（scope_id, name）。
    /// Array.prototype 静态优化只在已知数组 receiver 上启用，避免劫持 Map/Set 等同名方法。
    pub(crate) array_bindings: std::collections::HashSet<(usize, String)>,
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
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HoistedVar {
    pub(crate) scope_id: usize,
    pub(crate) name: String,
}

pub(crate) type HoistedBindingSet = std::collections::HashSet<(usize, String)>;
pub(crate) type FunctionHoistedState = (Vec<HoistedVar>, HoistedBindingSet);

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
