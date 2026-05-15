use swc_core::ecma::ast as swc_ast;
use wjsm_ir::{
    BasicBlockId, CompareOp, Constant, Instruction, Module, ValueId,
};
use crate::scope_tree::{ScopeKind, VarKind, ScopeTree};
use crate::cfg_builder::{FunctionBuilder, LabelContext, FinallyContext, TryContext};
use crate::{LoweringError, Diagnostic};

pub(crate) const EVAL_SCOPE_ENV_PARAM: &str = "$eval_env";

pub(crate) const WK_SYMBOL_ITERATOR: u32 = 0;
pub(crate) const WK_SYMBOL_SPECIES: u32 = 1;
pub(crate) const WK_SYMBOL_TO_STRING_TAG: u32 = 2;
pub(crate) const WK_SYMBOL_ASYNC_ITERATOR: u32 = 3;
pub(crate) const WK_SYMBOL_HAS_INSTANCE: u32 = 4;
pub(crate) const WK_SYMBOL_TO_PRIMITIVE: u32 = 5;
pub(crate) const WK_SYMBOL_DISPOSE: u32 = 6;
pub(crate) const WK_SYMBOL_MATCH: u32 = 7;
pub(crate) const WK_SYMBOL_ASYNC_DISPOSE: u32 = 8;

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
    pub(crate) active_finalizers: Vec<swc_ast::BlockStmt>,
    /// 匿名类 / 匿名函数计数器
    pub(crate) anon_counter: u32,
    // ── Function context stack ────────────────────────────────────────────
    pub(crate) function_stack: Vec<FunctionBuilder>,
    pub(crate) function_hoisted_stack: Vec<(Vec<HoistedVar>, std::collections::HashSet<(usize, String)>)>,
    pub(crate) function_next_value_stack: Vec<u32>,
    pub(crate) function_next_temp_stack: Vec<u32>,
    pub(crate) async_context_stack: Vec<AsyncContextState>,
    pub(crate) function_try_contexts_stack: Vec<Vec<TryContext>>,
    pub(crate) function_finally_stack_stack: Vec<Vec<FinallyContext>>,
    pub(crate) function_label_stack_stack: Vec<Vec<LabelContext>>,
    pub(crate) function_active_finalizers_stack: Vec<Vec<swc_ast::BlockStmt>>,
    pub(crate) function_pending_loop_label_stack: Vec<Option<String>>,
    // ── 闭包捕获相关 ──────────────────────────────────────────────────
    /// 每层函数的捕获绑定列表，push_function_context 时压入空 Vec。
    pub(crate) captured_names_stack: Vec<Vec<CapturedBinding>>,
    /// 每层函数的 function scope id，用于判断变量是否逃逸。
    pub(crate) function_scope_id_stack: Vec<usize>,
    /// 追踪当前是否在箭头函数中（箭头函数的 this 需要词法捕获）
    pub(crate) is_arrow_fn_stack: Vec<bool>,
    /// 每层函数的共享 env 对象 (ValueId) + 已注册的捕获绑定集合。
    /// 同一外层函数中的多个闭包共享同一个 env 对象，确保可变捕获变量的修改对所有闭包可见。
    pub(crate) shared_env_stack: Vec<Option<(ValueId, std::collections::HashSet<CapturedBinding>)>>,
    // ── 模块系统相关 ────────────────────────────────────────────────────────
    /// 当前正在编译的模块 ID（用于多模块编译）
    pub(crate) current_module_id: Option<wjsm_ir::ModuleId>,
    /// 导入映射：module_id → ImportBinding 列表
    pub(crate) import_bindings: std::collections::HashMap<wjsm_ir::ModuleId, Vec<wjsm_ir::ImportBinding>>,
    /// 导出映射：.0 = 模块 ID, .1 = 导出名 → 变量名
    pub(crate) export_map: std::collections::HashMap<(wjsm_ir::ModuleId, String), String>,
    /// 导入别名映射：local_name → source_ir_name
    /// 用于 `import { x as y }` 和 `import x from './dep'` 等场景
    pub(crate) import_aliases: std::collections::HashMap<String, String>,
    /// 动态 import() 目标映射：module_id → 被动态 import 的目标模块 ID 列表
    pub(crate) dynamic_import_targets: std::collections::HashMap<wjsm_ir::ModuleId, Vec<wjsm_ir::ModuleId>>,
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
    pub(crate) strict_mode: bool,
    pub(crate) eval_mode: bool,
    pub(crate) eval_has_scope_bridge: bool,
    pub(crate) eval_var_writes_to_scope: bool,
    pub(crate) eval_completion: Option<ValueId>,
    /// 当前作用域中活跃的 using 变量（用于作用域退出时自动 dispose）
    pub(crate) active_using_vars: Vec<ActiveUsingVar>,
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
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HoistedVar {
    pub(crate) scope_id: usize,
    pub(crate) name: String,
}

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

impl Lowerer {
    pub(crate) fn new() -> Self {
        let mut scopes = ScopeTree::new();
        // 预注册 ECMAScript 全局内置标识符
        let _ = scopes.declare("undefined", VarKind::Var, true);
        let _ = scopes.declare("NaN", VarKind::Var, true);
        let _ = scopes.declare("Infinity", VarKind::Var, true);
        let _ = scopes.declare("Symbol", VarKind::Var, true);

        Self {
            module: Module::new(),
            next_value: 0,
            scopes,
            hoisted_vars: Vec::new(),
            hoisted_vars_set: std::collections::HashSet::new(),
            current_function: FunctionBuilder::new("main", BasicBlockId(0)),
            label_stack: Vec::new(),
            finally_stack: Vec::new(),
            try_contexts: Vec::new(),
            next_temp: 0,
            pending_loop_label: None,
            active_finalizers: Vec::new(),
            anon_counter: 0,
            function_stack: Vec::new(),
            function_hoisted_stack: Vec::new(),
            function_next_value_stack: Vec::new(),
            function_next_temp_stack: Vec::new(),
            async_context_stack: Vec::new(),
            function_try_contexts_stack: Vec::new(),
            function_finally_stack_stack: Vec::new(),
            function_label_stack_stack: Vec::new(),
            function_active_finalizers_stack: Vec::new(),
            function_pending_loop_label_stack: Vec::new(),
            captured_names_stack: Vec::new(),
            function_scope_id_stack: Vec::new(),
            is_arrow_fn_stack: Vec::new(),
            shared_env_stack: Vec::new(),
            current_module_id: None,
            import_bindings: std::collections::HashMap::new(),
            export_map: std::collections::HashMap::new(),
            import_aliases: std::collections::HashMap::new(),
            dynamic_import_targets: std::collections::HashMap::new(),
            dynamic_import_namespace_modules: std::collections::HashSet::new(),
            dynamic_import_namespace_objects: std::collections::HashMap::new(),
            dynamic_import_specifier_map: std::collections::HashMap::new(),
            module_export_names: std::collections::HashMap::new(),
            is_async_fn: false,
            is_async_generator_fn: false,
            async_state_counter: 0,
            captured_var_slots: std::collections::HashMap::new(),
            async_next_continuation_slot: 0,
            async_resume_blocks: Vec::new(),
            async_promise_scope_id: 0,
            async_dispatch_block: None,
            async_main_body_entry: None,
            async_main_param_ir_names: Vec::new(),
            async_env_scope_id: 0,
            async_state_scope_id: 0,
            async_resume_val_scope_id: 0,
            async_is_rejected_scope_id: 0,
            async_generator_scope_id: 0,
            async_closure_env_ir_name: None,
            strict_mode: false,
            eval_mode: false,
            eval_has_scope_bridge: false,
            eval_var_writes_to_scope: false,
            active_using_vars: Vec::new(),
            eval_completion: None,
        }
    }

    pub(crate) fn capture_async_context(&self) -> AsyncContextState {
        AsyncContextState {
            is_async_fn: self.is_async_fn,
            is_async_generator_fn: self.is_async_generator_fn,
            async_state_counter: self.async_state_counter,
            captured_var_slots: self.captured_var_slots.clone(),
            async_next_continuation_slot: self.async_next_continuation_slot,
            async_resume_blocks: self.async_resume_blocks.clone(),
            async_promise_scope_id: self.async_promise_scope_id,
            async_dispatch_block: self.async_dispatch_block,
            async_env_scope_id: self.async_env_scope_id,
            async_state_scope_id: self.async_state_scope_id,
            async_resume_val_scope_id: self.async_resume_val_scope_id,
            async_is_rejected_scope_id: self.async_is_rejected_scope_id,
            async_generator_scope_id: self.async_generator_scope_id,
            async_closure_env_ir_name: self.async_closure_env_ir_name.clone(),
        }
    }

    pub(crate) fn restore_async_context(&mut self, context: AsyncContextState) {
        self.is_async_fn = context.is_async_fn;
        self.is_async_generator_fn = context.is_async_generator_fn;
        self.async_state_counter = context.async_state_counter;
        self.captured_var_slots = context.captured_var_slots;
        self.async_next_continuation_slot = context.async_next_continuation_slot;
        self.async_resume_blocks = context.async_resume_blocks;
        self.async_promise_scope_id = context.async_promise_scope_id;
        self.async_dispatch_block = context.async_dispatch_block;
        self.async_env_scope_id = context.async_env_scope_id;
        self.async_state_scope_id = context.async_state_scope_id;
        self.async_resume_val_scope_id = context.async_resume_val_scope_id;
        self.async_is_rejected_scope_id = context.async_is_rejected_scope_id;
        self.async_generator_scope_id = context.async_generator_scope_id;
        self.async_closure_env_ir_name = context.async_closure_env_ir_name;
    }

    pub(crate) fn reset_async_context(&mut self) {
        self.restore_async_context(AsyncContextState {
            is_async_fn: false,
            is_async_generator_fn: false,
            async_state_counter: 0,
            captured_var_slots: std::collections::HashMap::new(),
            async_next_continuation_slot: 0,
            async_resume_blocks: Vec::new(),
            async_promise_scope_id: 0,
            async_dispatch_block: None,
            async_env_scope_id: 0,
            async_state_scope_id: 0,
            async_resume_val_scope_id: 0,
            async_is_rejected_scope_id: 0,
            async_generator_scope_id: 0,
            async_closure_env_ir_name: None,
        });
    }

    pub(crate) fn push_function_context(&mut self, name: impl Into<String>, entry: BasicBlockId) {
        self.async_context_stack.push(self.capture_async_context());
        self.function_stack.push(std::mem::replace(
            &mut self.current_function,
            FunctionBuilder::new(name, entry),
        ));
        // 压入函数作用域到现有作用域树，而非创建新树
        self.scopes.push_scope(ScopeKind::Function);
        // 记录当前函数的 scope id（用于逃逸分析）
        let fn_scope_id = self.scopes.current_scope_id();
        self.function_scope_id_stack.push(fn_scope_id);
        self.captured_names_stack.push(Vec::new());
        self.is_arrow_fn_stack.push(false); // 默认非箭头函数，箭头函数会单独设置
        self.shared_env_stack.push(None); // 新函数上下文，尚无共享 env 对象
        self.function_hoisted_stack.push((
            std::mem::take(&mut self.hoisted_vars),
            std::mem::take(&mut self.hoisted_vars_set),
        ));
        self.function_next_value_stack.push(self.next_value);
        self.function_next_temp_stack.push(self.next_temp);
        self.next_value = 0;
        self.next_temp = 0;
        self.function_try_contexts_stack.push(std::mem::take(&mut self.try_contexts));
        self.function_finally_stack_stack.push(std::mem::take(&mut self.finally_stack));
        self.function_label_stack_stack.push(std::mem::take(&mut self.label_stack));
        self.function_active_finalizers_stack.push(std::mem::take(&mut self.active_finalizers));
        self.function_pending_loop_label_stack.push(self.pending_loop_label.take());
        self.reset_async_context();
    }

    pub(crate) fn pop_function_context(&mut self) {
        self.current_function = self.function_stack.pop().expect("function stack underflow");
        // 弹出函数作用域，回到外层作用域
        self.scopes.pop_scope();
        self.function_scope_id_stack.pop();
        self.captured_names_stack.pop();
        self.is_arrow_fn_stack.pop();
        self.shared_env_stack.pop();
        let (vars, set) = self
            .function_hoisted_stack
            .pop()
            .expect("hoisted stack underflow");
        self.hoisted_vars = vars;
        self.hoisted_vars_set = set;
        self.next_value = self
            .function_next_value_stack
            .pop()
            .expect("next value stack underflow");
        self.next_temp = self
            .function_next_temp_stack
            .pop()
            .expect("next temp stack underflow");
        self.try_contexts = self
            .function_try_contexts_stack
            .pop()
            .expect("try contexts stack underflow");
        self.finally_stack = self
            .function_finally_stack_stack
            .pop()
            .expect("finally stack stack underflow");
        self.label_stack = self
            .function_label_stack_stack
            .pop()
            .expect("label stack stack underflow");
        self.active_finalizers = self
            .function_active_finalizers_stack
            .pop()
            .expect("active finalizers stack underflow");
        self.pending_loop_label = self
            .function_pending_loop_label_stack
            .pop()
            .expect("pending loop label stack underflow");
        let async_context = self
            .async_context_stack
            .pop()
            .expect("async context stack underflow");
        self.restore_async_context(async_context);
    }

    pub(crate) fn current_function_scope_id(&self) -> usize {
        self.function_scope_id_stack.last().copied().unwrap_or(0)
    }

    pub(crate) fn binding_owner_function_scope(&self, binding: &CapturedBinding) -> usize {
        binding
            .scope_id
            .map(|scope_id| self.scopes.function_scope_for_scope(scope_id))
            .unwrap_or_else(|| self.current_function_scope_id())
    }

    pub(crate) fn binding_belongs_to_current_function(&self, binding: &CapturedBinding) -> bool {
        self.binding_owner_function_scope(binding) == self.current_function_scope_id()
    }

    pub(crate) fn record_capture(&mut self, binding: CapturedBinding) {
        if let Some(captured) = self.captured_names_stack.last_mut() {
            if !captured.contains(&binding) {
                captured.push(binding);
            }
        }
    }

    pub(crate) fn captured_display_names(captured: &[CapturedBinding]) -> Vec<String> {
        captured.iter().map(CapturedBinding::display_name).collect()
    }

    pub(crate) fn is_shared_binding(&self, binding: &CapturedBinding) -> bool {
        self.shared_env_stack
            .last()
            .and_then(|shared| shared.as_ref())
            .map_or(false, |(_, names)| names.contains(binding))
    }

    pub(crate) fn shared_env_value(&self) -> Option<ValueId> {
        self.shared_env_stack
            .last()
            .and_then(|shared| shared.as_ref().map(|(value, _)| *value))
    }

    pub(crate) fn append_env_key_const(&mut self, block: BasicBlockId, binding: &CapturedBinding) -> ValueId {
        let key_const = self
            .module
            .add_constant(Constant::String(binding.env_key()));
        let key_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: key_val,
                constant: key_const,
            },
        );
        key_val
    }

    pub(crate) fn load_captured_binding(
        &mut self,
        block: BasicBlockId,
        binding: &CapturedBinding,
    ) -> Result<ValueId, LoweringError> {
        let env_val = if self.binding_belongs_to_current_function(binding) {
            self.shared_env_value()
                .expect("shared binding must have a materialized env")
        } else {
            self.record_capture(binding.clone());
            self.load_env_object(block)
        };
        let key_val = self.append_env_key_const(block, binding);
        let dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::GetProp {
                dest,
                object: env_val,
                key: key_val,
            },
        );
        Ok(dest)
    }

}
