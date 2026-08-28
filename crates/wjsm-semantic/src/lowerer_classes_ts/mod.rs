use self::function_values::LoweredClassFunction;
use super::*;
use swc_core::common::Span;
use swc_core::ecma::visit::{Visit, VisitWith};

struct ValueDecoratorContext<'a> {
    decorators: &'a [swc_ast::Decorator],
    kind: &'a str,
    name: &'a str,
    is_static: bool,
    is_private: bool,
}

/// 扫描派生构造器（形参默认值 + 函数体）内是否存在**观察 this 绑定的箭头
/// 函数**（体内出现 this / super 属性 / super() 之一）。命中时构造器 this
/// 的规范存储改为共享 env（见 `ctor_this_via_env`）：super() 前的 TDZ 哨兵、
/// BindThisValue 重绑（无论发生在构造器帧还是箭头帧）对全部帧经同一 env
/// 保持可见——本地槽快照无法覆盖「super() 前创建、之后调用」的箭头。
/// 普通函数与嵌套类是词法 this 边界，不参与扫描。
#[derive(Default)]
struct ArrowThisObservationScan {
    arrow_depth: u32,
    found: bool,
}

impl Visit for ArrowThisObservationScan {
    fn visit_call_expr(&mut self, call: &swc_ast::CallExpr) {
        if self.found {
            return;
        }
        if self.arrow_depth > 0 && matches!(call.callee, swc_ast::Callee::Super(_)) {
            self.found = true;
            return;
        }
        call.visit_children_with(self);
    }

    fn visit_this_expr(&mut self, _: &swc_ast::ThisExpr) {
        if self.arrow_depth > 0 {
            self.found = true;
        }
    }

    fn visit_super_prop_expr(&mut self, super_prop: &swc_ast::SuperPropExpr) {
        if self.found {
            return;
        }
        if self.arrow_depth > 0 {
            self.found = true;
            return;
        }
        super_prop.visit_children_with(self);
    }

    fn visit_arrow_expr(&mut self, arrow: &swc_ast::ArrowExpr) {
        if self.found {
            return;
        }
        self.arrow_depth += 1;
        arrow.visit_children_with(self);
        self.arrow_depth -= 1;
    }

    fn visit_function(&mut self, _: &swc_ast::Function) {}

    fn visit_class(&mut self, _: &swc_ast::Class) {}
}

pub(super) fn ctor_arrow_observes_this(ctor: &swc_ast::Constructor) -> bool {
    let mut scan = ArrowThisObservationScan::default();
    for param in &ctor.params {
        param.visit_with(&mut scan);
    }
    if let Some(body) = &ctor.body {
        body.visit_with(&mut scan);
    }
    scan.found
}

/// 派生类显式构造器的实例初始化上下文。
///
/// ECMAScript SuperCall（§13.3.7.1）步骤 8–11：BindThisValue 之后立即
/// InitializeInstanceElements——字段初始化属于 super() **求值本身**，与
/// super() 出现在哪个语句/表达式位置无关。构造器 lowering 前把所需数据
/// 存入本上下文，`Callee::Super` 站点消费。箭头帧继承外层 super 能力，
/// 上下文随之克隆进入（箭头体内的 super() 同样要发射初始化）；普通嵌套
/// 函数帧在 push_function_context 时清空，不会误发射。
#[derive(Clone)]
pub(crate) struct DerivedCtorInitCtx {
    /// TS 参数属性（构造器形参绑定, 字段名），先于字段初始化器生效。
    /// 存作用域绑定而非裸 IR 名：箭头帧发射时形参属于外层构造器帧，
    /// 须经捕获链读取。
    pub(crate) param_prop_fields: Vec<(CapturedBinding, String)>,
    /// 类体成员克隆（保留原始下标，供计算键实例字段映射使用）。
    pub(crate) members: Vec<swc_ast::ClassMember>,
    pub(crate) private_members: Vec<PrivateMemberMeta>,
    pub(crate) computed_instance_keys: std::collections::HashMap<usize, String>,
    /// 是否存在需要发射的初始化工作；为 false 时 super() 站点不加异常
    /// 分叉，保持与语句级异常检查一致的最小 IR。
    pub(crate) has_init_work: bool,
}

/// 私有名静态校验（早错误）：
/// 1. AllPrivateIdentifiersValid（ES §13.3.1.1）：任何 `obj.#x` / `#x in obj` 引用都必须
///    出现在声明 `#x` 的某个词法封闭类内，否则为 SyntaxError。
/// 2. ClassBody 私有名重复：同一类体内私有名不得重复声明（同名 getter+setter 各一次的
///    配对除外），否则为 SyntaxError。
///
/// 作为降级前的一次性 AST 遍历执行（模式同 `DerivedCtorPreSuperUse`）。
struct PrivateNameValidator {
    /// 词法作用域栈：进入每个类体时压入其声明的全部私有名集合；引用有效当且仅当其名
    /// 存在于栈中任一层（最近的或更外层的封闭类）。
    scopes: Vec<std::collections::HashSet<String>>,
    error: Option<(Span, String)>,
}

impl PrivateNameValidator {
    fn new() -> Self {
        Self {
            scopes: Vec::new(),
            error: None,
        }
    }

    /// 收集类体声明的全部私有名，并检测重复声明。返回该类的私有名集合。
    fn collect_class_private_names(
        &mut self,
        class: &swc_ast::Class,
    ) -> std::collections::HashSet<String> {
        use std::collections::HashMap;
        // 每个私有名累计 (值/普通方法 计数, getter 计数, setter 计数)。
        let mut tally: HashMap<String, (u32, u32, u32)> = HashMap::new();
        let mut order: Vec<(String, Span)> = Vec::new();
        for member in &class.body {
            let (name, span, slot) = match member {
                swc_ast::ClassMember::PrivateMethod(m) => {
                    let slot = match m.kind {
                        swc_ast::MethodKind::Getter => 1usize,
                        swc_ast::MethodKind::Setter => 2usize,
                        swc_ast::MethodKind::Method => 0usize,
                    };
                    (m.key.name.to_string(), m.key.span, slot)
                }
                swc_ast::ClassMember::PrivateProp(p) => {
                    (p.key.name.to_string(), p.key.span, 0usize)
                }
                _ => continue,
            };
            let entry = tally.entry(name.clone()).or_insert((0, 0, 0));
            match slot {
                0 => entry.0 += 1,
                1 => entry.1 += 1,
                _ => entry.2 += 1,
            }
            order.push((name, span));
        }
        // 重复规则：非访问器名只能出现一次且不可与访问器同名；getter / setter 各至多一次。
        if self.error.is_none() {
            for (name, span) in &order {
                let (values, getters, setters) = tally[name];
                let duplicate = values > 1
                    || (values >= 1 && getters + setters > 0)
                    || getters > 1
                    || setters > 1;
                if duplicate {
                    self.error = Some((
                        *span,
                        format!("Identifier '#{name}' has already been declared"),
                    ));
                    break;
                }
            }
        }
        tally.into_keys().collect()
    }
}

impl Visit for PrivateNameValidator {
    fn visit_class(&mut self, class: &swc_ast::Class) {
        let names = self.collect_class_private_names(class);
        self.scopes.push(names);
        class.visit_children_with(self);
        self.scopes.pop();
    }

    fn visit_private_name(&mut self, name: &swc_ast::PrivateName) {
        // 引用（含类体内的声明键）：声明键此时已在作用域内，故仅词法外的引用会报错。
        if self.error.is_none()
            && !self
                .scopes
                .iter()
                .any(|scope| scope.contains(name.name.as_ref()))
        {
            self.error = Some((
                name.span,
                format!(
                    "Private field '#{}' must be declared in an enclosing class",
                    name.name
                ),
            ));
        }
    }
}

/// 对整棵模块 AST 运行私有名静态校验，返回首个早错误（若有）。
pub(crate) fn validate_private_names(module: &swc_ast::Module) -> Result<(), LoweringError> {
    let mut validator = PrivateNameValidator::new();
    module.visit_with(&mut validator);
    if let Some((span, message)) = validator.error {
        return Err(LoweringError::Diagnostic(Diagnostic::new(
            span.lo.0, span.hi.0, message,
        )));
    }
    Ok(())
}

/// 用于诊断的 ClassMember 源码区间。
pub(super) fn class_member_span(member: &swc_ast::ClassMember) -> Span {
    match member {
        swc_ast::ClassMember::Constructor(c) => c.span,
        swc_ast::ClassMember::Method(m) => m.span,
        swc_ast::ClassMember::PrivateMethod(m) => m.span,
        swc_ast::ClassMember::ClassProp(p) => p.span,
        swc_ast::ClassMember::PrivateProp(p) => p.span,
        swc_ast::ClassMember::StaticBlock(b) => b.span,
        swc_ast::ClassMember::TsIndexSignature(t) => t.span,
        swc_ast::ClassMember::Empty(e) => e.span,
        swc_ast::ClassMember::AutoAccessor(a) => a.span,
    }
}
/// 用于错误消息的 ClassMember 变体名称。
pub(super) fn class_member_kind(member: &swc_ast::ClassMember) -> &'static str {
    match member {
        swc_ast::ClassMember::Constructor(_) => "constructor",
        swc_ast::ClassMember::Method(_) => "method",
        swc_ast::ClassMember::PrivateMethod(_) => "private method",
        swc_ast::ClassMember::ClassProp(_) => "class property",
        swc_ast::ClassMember::PrivateProp(_) => "private property",
        swc_ast::ClassMember::StaticBlock(_) => "static block",
        swc_ast::ClassMember::TsIndexSignature(_) => "index signature",
        swc_ast::ClassMember::Empty(_) => "empty",
        swc_ast::ClassMember::AutoAccessor(_) => "auto accessor",
    }
}

/// 类私有方法 / 访问器在 lowering 阶段的绑定描述。
#[derive(Clone)]
pub(crate) struct PrivateMemberMeta {
    field_name: String,
    is_static: bool,
    kind: PrivateMemberKind,
}

#[derive(Clone)]
enum PrivateMemberKind {
    Method(PrivateFunctionMeta),
    Accessor {
        getter: Option<PrivateFunctionMeta>,
        setter: Option<PrivateFunctionMeta>,
    },
}

#[derive(Clone)]
struct PrivateFunctionMeta {
    lowered_function: LoweredClassFunction,
    instance_binding: Option<CapturedBinding>,
    value: Option<ValueId>,
    span: Span,
    /// generator / async generator 的 wrapper 函数：与公有 generator 方法一致，
    /// 不接线 home_object（generator 体内 super 不经 wrapper 解析）。
    is_generator: bool,
}

impl Lowerer {
    fn push_class_private_name_scope(&mut self, class_name: &str, body: &[swc_ast::ClassMember]) {
        let class_private_id = self.next_private_name_id;
        self.next_private_name_id += 1;
        let mut names = std::collections::HashMap::new();
        for member in body {
            // 实例私有方法/访问器共享类 brand，其 `in` 错误显示名为类名（对齐 V8/Node）；
            // 实例字段与全部 static 私有成员各有独立槽，显示 `#名`。
            let (source_name, brand_display) = match member {
                swc_ast::ClassMember::PrivateMethod(method) => {
                    (method.key.name.to_string(), !method.is_static)
                }
                swc_ast::ClassMember::PrivateProp(prop) => (prop.key.name.to_string(), false),
                _ => continue,
            };
            let in_display_name = if brand_display {
                class_name.to_string()
            } else {
                format!("#{source_name}")
            };
            names
                .entry(source_name.clone())
                .or_insert_with(|| PrivateNameEntry {
                    storage_name: format!("#{source_name}@{class_private_id}"),
                    in_display_name,
                });
        }
        self.private_name_stack.push(names);
    }

    fn pop_class_private_name_scope(&mut self) {
        self.private_name_stack.pop();
    }

    fn resolve_private_name_entry(
        &self,
        source_name: &str,
        span: Span,
    ) -> Result<&PrivateNameEntry, LoweringError> {
        self.private_name_stack
            .iter()
            .rev()
            .find_map(|scope| scope.get(source_name))
            .ok_or_else(|| {
                self.error(
                    span,
                    format!("Private field '#{source_name}' is not declared"),
                )
            })
    }

    pub(crate) fn resolve_private_storage_name(
        &self,
        source_name: &str,
        span: Span,
    ) -> Result<String, LoweringError> {
        Ok(self
            .resolve_private_name_entry(source_name, span)?
            .storage_name
            .clone())
    }

    /// `#x in obj` 需要的（槽名, 错误显示名）二元组。
    pub(crate) fn resolve_private_in_names(
        &self,
        source_name: &str,
        span: Span,
    ) -> Result<(String, String), LoweringError> {
        let entry = self.resolve_private_name_entry(source_name, span)?;
        Ok((entry.storage_name.clone(), entry.in_display_name.clone()))
    }

    pub(crate) fn emit_string_const(&mut self, block: BasicBlockId, value: &str) -> ValueId {
        let constant = self
            .module
            .add_constant(Constant::String(value.to_string()));
        let dest = self.alloc_value();
        self.current_function
            .append_instruction(block, Instruction::Const { dest, constant });
        dest
    }

    fn emit_undefined_const(&mut self, block: BasicBlockId) -> ValueId {
        let constant = self.module.add_constant(Constant::Undefined);
        let dest = self.alloc_value();
        self.current_function
            .append_instruction(block, Instruction::Const { dest, constant });
        dest
    }

    fn emit_bool_const(&mut self, block: BasicBlockId, value: bool) -> ValueId {
        let constant = self.module.add_constant(Constant::Bool(value));
        let dest = self.alloc_value();
        self.current_function
            .append_instruction(block, Instruction::Const { dest, constant });
        dest
    }

    fn emit_context_prop(
        &mut self,
        block: BasicBlockId,
        context: ValueId,
        key: &str,
        value: ValueId,
    ) {
        let key = self.emit_string_const(block, key);
        self.emit_set_prop(block, context, key, value);
    }

    fn emit_decorator_context(
        &mut self,
        block: BasicBlockId,
        kind: &str,
        name: Option<&str>,
        is_static: Option<bool>,
        is_private: Option<bool>,
    ) -> ValueId {
        let context = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::NewObject {
                dest: context,
                capacity: 5,
            },
        );

        let kind = self.emit_string_const(block, kind);
        self.emit_context_prop(block, context, "kind", kind);

        let name = name
            .map(|name| self.emit_string_const(block, name))
            .unwrap_or_else(|| self.emit_undefined_const(block));
        self.emit_context_prop(block, context, "name", name);

        if let Some(is_static) = is_static {
            let value = self.emit_bool_const(block, is_static);
            self.emit_context_prop(block, context, "static", value);
        }
        if let Some(is_private) = is_private {
            let value = self.emit_bool_const(block, is_private);
            self.emit_context_prop(block, context, "private", value);
        }

        context
    }

    fn emit_decorator_result_or_original(
        &mut self,
        block: BasicBlockId,
        original: ValueId,
        result: ValueId,
    ) -> (BasicBlockId, ValueId) {
        let undefined = self.emit_undefined_const(block);
        let has_replacement = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Compare {
                dest: has_replacement,
                op: CompareOp::StrictNotEq,
                lhs: result,
                rhs: undefined,
            },
        );

        let replacement_block = self.current_function.new_block();
        let original_block = self.current_function.new_block();
        let merge_block = self.current_function.new_block();
        self.current_function.set_terminator(
            block,
            Terminator::Branch {
                condition: has_replacement,
                true_block: replacement_block,
                false_block: original_block,
            },
        );
        self.current_function.set_terminator(
            replacement_block,
            Terminator::Jump {
                target: merge_block,
            },
        );
        self.current_function.set_terminator(
            original_block,
            Terminator::Jump {
                target: merge_block,
            },
        );

        let value = self.alloc_value();
        self.current_function.append_instruction(
            merge_block,
            Instruction::Phi {
                dest: value,
                sources: vec![
                    PhiSource {
                        predecessor: replacement_block,
                        value: result,
                    },
                    PhiSource {
                        predecessor: original_block,
                        value: original,
                    },
                ],
            },
        );
        (merge_block, value)
    }

    fn emit_apply_class_decorators(
        &mut self,
        mut block: BasicBlockId,
        mut class_value: ValueId,
        decorators: &[swc_ast::Decorator],
        class_name: Option<&str>,
    ) -> Result<(BasicBlockId, ValueId), LoweringError> {
        let mut decorator_values = Vec::with_capacity(decorators.len());
        for decorator in decorators {
            let value = self.lower_expr(&decorator.expr, block)?;
            block = self.resolve_store_block(block);
            decorator_values.push(value);
        }

        for decorator in decorator_values.into_iter().rev() {
            let context = self.emit_decorator_context(block, "class", class_name, None, None);
            let result = self.alloc_value();
            let this_val = self.emit_undefined_const(block);
            self.current_function.append_instruction(
                block,
                Instruction::Call {
                    dest: Some(result),
                    callee: decorator,
                    this_val,
                    args: vec![class_value, context],
                },
            );
            (block, class_value) =
                self.emit_decorator_result_or_original(block, class_value, result);
        }

        Ok((block, class_value))
    }

    fn emit_apply_value_decorators(
        &mut self,
        mut block: BasicBlockId,
        mut original: ValueId,
        ctx: &ValueDecoratorContext,
    ) -> Result<(BasicBlockId, ValueId), LoweringError> {
        let ValueDecoratorContext {
            decorators,
            kind,
            name,
            is_static,
            is_private,
        } = ctx;
        let mut decorator_values = Vec::with_capacity(decorators.len());
        for decorator in *decorators {
            let value = self.lower_expr(&decorator.expr, block)?;
            block = self.resolve_store_block(block);
            decorator_values.push(value);
        }

        for decorator in decorator_values.into_iter().rev() {
            let context = self.emit_decorator_context(
                block,
                kind,
                Some(name),
                Some(*is_static),
                Some(*is_private),
            );
            let result = self.alloc_value();
            let this_val = self.emit_undefined_const(block);
            self.current_function.append_instruction(
                block,
                Instruction::Call {
                    dest: Some(result),
                    callee: decorator,
                    this_val,
                    args: vec![original, context],
                },
            );
            (block, original) = self.emit_decorator_result_or_original(block, original, result);
        }

        Ok((block, original))
    }

    fn emit_instance_initializers(
        &mut self,
        mut block: BasicBlockId,
        members: &[swc_ast::ClassMember],
        private_members: &[PrivateMemberMeta],
        computed_instance_keys: &std::collections::HashMap<usize, String>,
    ) -> Result<BasicBlockId, LoweringError> {
        for (member_index, member) in members.iter().enumerate() {
            match member {
                swc_ast::ClassMember::PrivateProp(prop) if !prop.is_static => {
                    self.check_field_initializer_arguments(prop.value.as_deref())?;
                    let field_name =
                        self.resolve_private_storage_name(prop.key.name.as_ref(), prop.key.span)?;
                    // NamedEvaluation：私有字段的匿名函数定义按私有名
                    // description（含 `#`）命名（§10.2.9 步骤 2）。
                    if prop
                        .value
                        .as_deref()
                        .is_some_and(Self::is_anonymous_fn_definition)
                    {
                        self.named_eval_hint = Some(format!("#{}", prop.key.name));
                    }
                    block =
                        self.emit_field_init(block, &field_name, prop.value.as_deref(), true)?;
                }
                swc_ast::ClassMember::ClassProp(prop) if !prop.is_static => {
                    self.check_field_initializer_arguments(prop.value.as_deref())?;
                    block = if let Some(key_name) = computed_instance_keys.get(&member_index) {
                        // 计算键在类定义期已求值并 ToPropertyKey（求值一次），
                        // 存于构造器闭包的 key env；此处沿 $env 链按名读取，
                        // 不得重新求值键表达式。
                        let env_val = self.load_env_object(block);
                        let name_const = self.emit_string_const(block, key_name);
                        let key_dest = self.alloc_value();
                        self.current_function.append_instruction(
                            block,
                            Instruction::GetProp {
                                dest: key_dest,
                                object: env_val,
                                key: name_const,
                            },
                        );
                        self.emit_field_init_common(block, key_dest, prop.value.as_deref(), false)?
                    } else {
                        self.emit_field_init_with_key(block, &prop.key, prop.value.as_deref())?
                    };
                }
                swc_ast::ClassMember::Constructor(_)
                | swc_ast::ClassMember::Method(_)
                | swc_ast::ClassMember::PrivateMethod(_)
                | swc_ast::ClassMember::StaticBlock(_) => {}
                swc_ast::ClassMember::PrivateProp(p) if p.is_static => {}
                swc_ast::ClassMember::ClassProp(p) if p.is_static => {}
                other => {
                    return Err(self.error(
                        class_member_span(other),
                        format!(
                            "unsupported class member `{}` during instance field initialization",
                            class_member_kind(other),
                        ),
                    ));
                }
            }
        }

        for member in private_members {
            if member.is_static {
                continue;
            }
            let this_val = self.emit_read_ctor_this(block);
            match &member.kind {
                PrivateMemberKind::Method(function) => {
                    let (continuation, function_value) =
                        self.load_private_instance_function_value(block, function)?;
                    block = continuation;
                    self.emit_private_method_bind(
                        block,
                        this_val,
                        &member.field_name,
                        function_value,
                    );
                }
                PrivateMemberKind::Accessor { getter, setter } => {
                    let getter_value = if let Some(function) = getter {
                        let (continuation, value) =
                            self.load_private_instance_function_value(block, function)?;
                        block = continuation;
                        Some(value)
                    } else {
                        None
                    };
                    let setter_value = if let Some(function) = setter {
                        let (continuation, value) =
                            self.load_private_instance_function_value(block, function)?;
                        block = continuation;
                        Some(value)
                    } else {
                        None
                    };
                    self.emit_private_accessor_bind(
                        block,
                        this_val,
                        &member.field_name,
                        getter_value,
                        setter_value,
                    );
                }
            }
            block = self.resolve_store_block(block);
        }

        Ok(block)
    }

    /// 在 super() 站点发射 InitializeInstanceElements（ES SuperCall 步骤 11）：
    /// Construct 异常先分叉传播（初始化器不得执行），随后在重绑后的 this 上
    /// 依序发射参数属性字段与实例字段/私有成员初始化。非构造器帧（上下文为
    /// None）或无初始化工作时原样返回，保持既有语句级异常检查路径的最小 IR。
    pub(crate) fn emit_super_site_instance_inits(
        &mut self,
        block: BasicBlockId,
        super_result: ValueId,
    ) -> Result<BasicBlockId, LoweringError> {
        let Some(ctx) = self.derived_ctor_init_ctx.take() else {
            return Ok(block);
        };
        if !ctx.has_init_work {
            self.derived_ctor_init_ctx = Some(ctx);
            return Ok(block);
        }
        let mut block = self.lower_value_exception_branch(block, super_result)?;
        block = self.emit_param_prop_fields(block, &ctx.param_prop_fields)?;
        block = self.emit_instance_initializers(
            block,
            &ctx.members,
            &ctx.private_members,
            &ctx.computed_instance_keys,
        )?;
        self.derived_ctor_init_ctx = Some(ctx);
        Ok(block)
    }

    /// 收集类体中的私有方法/访问器并生成对应 IR 函数。
    fn collect_class_private_members(
        &mut self,
        class_name: &str,
        body: &[swc_ast::ClassMember],
    ) -> Result<Vec<PrivateMemberMeta>, LoweringError> {
        use std::collections::HashMap;
        let mut out: Vec<PrivateMemberMeta> = Vec::new();
        let mut accessor_pending: HashMap<(String, bool), usize> = HashMap::new();
        let mut next_function_index = 0;

        for member in body {
            let swc_ast::ClassMember::PrivateMethod(pm) = member else {
                continue;
            };
            let field_name =
                self.resolve_private_storage_name(pm.key.name.as_ref(), pm.key.span)?;
            let is_static = pm.is_static;
            // 私有 generator（含 async generator）方法：yield/await 需要续体调度，
            // 不能走下方直接降级方法体的同步路径，改由声明路径的 body + wrapper
            // 双函数结构生成 wrapper 函数值（与公有 generator 方法的路由一致）。
            if matches!(pm.kind, swc_ast::MethodKind::Method) && pm.function.is_generator {
                let member = self.lower_private_generator_member(
                    class_name,
                    pm,
                    field_name,
                    &mut next_function_index,
                )?;
                out.push(member);
                continue;
            }
            // 私有普通 async 方法：await 需要续体调度，复用 async 函数表达式的
            // body + wrapper 双函数结构（与公有 async 方法的路由一致）。
            if matches!(pm.kind, swc_ast::MethodKind::Method) && pm.function.is_async {
                let member = self.lower_private_async_member(
                    class_name,
                    pm,
                    field_name,
                    &mut next_function_index,
                )?;
                out.push(member);
                continue;
            }
            let accessor = matches!(
                pm.kind,
                swc_ast::MethodKind::Getter | swc_ast::MethodKind::Setter
            );
            let role = if matches!(pm.kind, swc_ast::MethodKind::Getter) {
                "get"
            } else if matches!(pm.kind, swc_ast::MethodKind::Setter) {
                "set"
            } else {
                ""
            };
            let fn_name = if accessor {
                if is_static {
                    format!("{}.static_{}_{}", class_name, role, pm.key.name)
                } else {
                    format!("{}.{}_{}", class_name, role, pm.key.name)
                }
            } else if is_static {
                format!("{}.static_{}", class_name, pm.key.name)
            } else {
                format!("{}.{}", class_name, pm.key.name)
            };

            // 私有方法体延迟到类求值完成后才执行，期间类名已初始化（方法体可引用类名）；
            // 函数体 lowering 期间临时退出 TDZ，结束后恢复。
            let class_scope_id = self.scopes.resolve_scope_id(class_name).ok();
            if let Some(sid) = class_scope_id {
                self.scopes
                    .set_initialised(sid, class_name, true)
                    .map_err(|msg| self.error(pm.span, msg))?;
            }
            self.push_function_context(&fn_name, BasicBlockId(0));
            // 类体代码恒为严格模式（ClassDefinitionEvaluation）。
            self.strict_mode = true;
            self.is_method = true;
            self.super_allowed = true;
            self.set_lexical_home_object_for_enclosing_method(
                Self::PENDING_CTOR_FUNCTION_ID,
                is_static,
            );
            let env_scope_id = self
                .scopes
                .declare("$env", VarKind::Let, true)
                .map_err(|msg| self.error(pm.span, msg))?;
            let this_scope_id = self
                .scopes
                .declare("$this", VarKind::Let, true)
                .map_err(|msg| self.error(pm.span, msg))?;
            let mut param_ir_names = vec![
                format!("${env_scope_id}.$env"),
                format!("${this_scope_id}.$this"),
            ];
            for param in &pm.function.params {
                if let swc_ast::Pat::Ident(binding_ident) = &param.pat {
                    let name = binding_ident.id.sym.to_string();
                    let scope_id = self
                        .scopes
                        .declare(&name, VarKind::Let, true)
                        .map_err(|msg| self.error(pm.span, msg))?;
                    param_ir_names.push(format!("${scope_id}.{name}"));
                }
            }
            if let Some(body) = &pm.function.body {
                self.predeclare_block_stmts(&body.stmts)?;
            }
            let m_entry = BasicBlockId(0);
            self.emit_hoisted_var_initializers(m_entry);
            self.set_arguments_params(&pm.function.params);
            let m_entry = self.emit_arguments_init(
                m_entry,
                Self::function_needs_arguments_object(&pm.function),
            )?;
            self.eval_caller_has_arguments = Self::detect_param_arguments(&pm.function.params)
                || self.scopes.lookup("arguments").is_ok();
            let mut m_flow = StmtFlow::Open(m_entry);
            if let Some(body) = &pm.function.body {
                for stmt in &body.stmts {
                    if matches!(m_flow, StmtFlow::Terminated) {
                        continue;
                    }
                    m_flow = self.lower_stmt(stmt, m_flow)?;
                }
            }
            if let StmtFlow::Open(b) = m_flow {
                self.current_function
                    .set_terminator(b, Terminator::Return { value: None });
            }
            let m_old_fn = std::mem::replace(
                &mut self.current_function,
                FunctionBuilder::new("", BasicBlockId(0)),
            );
            let m_has_eval = m_old_fn.has_eval();
            let m_blocks = m_old_fn.into_blocks();
            let mut m_ir_function = Function::new(&fn_name, BasicBlockId(0));
            m_ir_function.set_has_eval(m_has_eval);
            if let Some(span) = self.span_to_source_span(pm.span()) {
                m_ir_function.set_source_span(span);
            }
            if let Some(text) = self.method_definition_source_text(pm.span(), pm.is_static) {
                m_ir_function.set_source_text(text);
            }
            m_ir_function.set_params(param_ir_names);
            let m_captured = self.captured_names_stack.last().unwrap().clone();
            m_ir_function.set_captured_names(Self::captured_display_names(&m_captured));
            for b in m_blocks {
                m_ir_function.push_block(b);
            }
            let m_function_id = self.module.push_function(m_ir_function);
            self.pop_function_context();
            if let Some(sid) = class_scope_id {
                let _ = self.scopes.set_initialised(sid, class_name, false);
            }
            let instance_binding = self.declare_private_instance_binding(
                is_static,
                pm.span,
                &mut next_function_index,
            )?;
            let private_function = PrivateFunctionMeta {
                lowered_function: LoweredClassFunction {
                    function_id: m_function_id,
                    captured: m_captured,
                },
                instance_binding,
                value: None,
                span: pm.span,
                is_generator: false,
            };

            if accessor {
                let key = (field_name.clone(), is_static);
                if let Some(position) = accessor_pending.get(&key).copied() {
                    let PrivateMemberKind::Accessor { getter, setter } = &mut out[position].kind
                    else {
                        unreachable!("private accessor metadata kind changed")
                    };
                    if matches!(pm.kind, swc_ast::MethodKind::Getter) {
                        *getter = Some(private_function);
                    } else {
                        *setter = Some(private_function);
                    }
                } else {
                    let (getter, setter) = if matches!(pm.kind, swc_ast::MethodKind::Getter) {
                        (Some(private_function), None)
                    } else {
                        (None, Some(private_function))
                    };
                    let position = out.len();
                    out.push(PrivateMemberMeta {
                        field_name: field_name.clone(),
                        is_static,
                        kind: PrivateMemberKind::Accessor { getter, setter },
                    });
                    accessor_pending.insert(key, position);
                }
            } else {
                out.push(PrivateMemberMeta {
                    field_name,
                    is_static,
                    kind: PrivateMemberKind::Method(private_function),
                });
            }
        }
        Ok(out)
    }

    /// 私有 generator / async generator 方法：复用函数声明路径的 body + wrapper
    /// 双函数结构（yield/await 经续体槽调度），返回统一的私有成员元数据。
    fn lower_private_generator_member(
        &mut self,
        class_name: &str,
        pm: &swc_ast::PrivateMethod,
        field_name: String,
        next_function_index: &mut usize,
    ) -> Result<PrivateMemberMeta, LoweringError> {
        let is_static = pm.is_static;
        let fn_name = if is_static {
            format!("{}.static_#{}", class_name, pm.key.name)
        } else {
            format!("{}.#{}", class_name, pm.key.name)
        };
        // 方法体延迟到类求值完成后才执行，期间类名已初始化（方法体可引用类名）；
        // 函数体 lowering 期间临时退出 TDZ，结束后恢复。
        let class_scope_id = self.scopes.resolve_scope_id(class_name).ok();
        if let Some(sid) = class_scope_id {
            self.scopes
                .set_initialised(sid, class_name, true)
                .map_err(|msg| self.error(pm.span, msg))?;
        }
        let declaration = swc_ast::FnDecl {
            ident: swc_ast::Ident::new(
                fn_name.into(),
                pm.key.span,
                swc_core::common::SyntaxContext::empty(),
            ),
            declare: false,
            function: pm.function.clone(),
        };
        // 私有 generator 体内 super 尚未接线：构造器 id 在 collect 阶段未知，
        // 且 body/wrapper 双函数的 home 元数据需成对回填，留作后续任务。
        let lowered = if pm.function.is_async {
            self.lower_async_gen_function(&declaration, MethodSuperBinding::None)
        } else {
            self.lower_gen_function(&declaration)
        };
        if let Some(sid) = class_scope_id {
            let _ = self.scopes.set_initialised(sid, class_name, false);
        }
        let (function_id, captured) = lowered?;
        // [[SourceText]] 取 MethodDefinition 文本（含 `#名`，剥离 `static`）。
        let source_text = self.method_definition_source_text(pm.span(), is_static);
        self.set_function_source_text(function_id, source_text);
        let instance_binding =
            self.declare_private_instance_binding(is_static, pm.span, next_function_index)?;
        Ok(PrivateMemberMeta {
            field_name,
            is_static,
            kind: PrivateMemberKind::Method(PrivateFunctionMeta {
                lowered_function: LoweredClassFunction {
                    function_id,
                    captured,
                },
                instance_binding,
                value: None,
                span: pm.span,
                is_generator: true,
            }),
        })
    }

    /// 私有普通 async 方法：复用 async 函数表达式路径的 body + wrapper 双函数结构
    /// （await 经续体槽调度），返回统一的私有成员元数据。
    ///
    /// [[HomeObject]] 静态可知，但构造器 id 在 collect 阶段未知：以 PENDING 占位
    /// 接线 `MethodSuperBinding::Static`，类体收尾由
    /// `patch_pending_ctor_home_object_references` 统一回填真实构造器 id。
    fn lower_private_async_member(
        &mut self,
        class_name: &str,
        pm: &swc_ast::PrivateMethod,
        field_name: String,
        next_function_index: &mut usize,
    ) -> Result<PrivateMemberMeta, LoweringError> {
        let is_static = pm.is_static;
        let fn_name = if is_static {
            format!("{}.static_#{}", class_name, pm.key.name)
        } else {
            format!("{}.#{}", class_name, pm.key.name)
        };
        // 方法体延迟到类求值完成后才执行，期间类名已初始化（方法体可引用类名）；
        // 函数体 lowering 期间临时退出 TDZ，结束后恢复。
        let class_scope_id = self.scopes.resolve_scope_id(class_name).ok();
        if let Some(sid) = class_scope_id {
            self.scopes
                .set_initialised(sid, class_name, true)
                .map_err(|msg| self.error(pm.span, msg))?;
        }
        let fake_expr = swc_ast::FnExpr {
            ident: None,
            function: pm.function.clone(),
        };
        let home = if is_static {
            HomeObject::Constructor(Self::PENDING_CTOR_FUNCTION_ID)
        } else {
            HomeObject::Prototype(Self::PENDING_CTOR_FUNCTION_ID)
        };
        let lowered =
            self.lower_async_function_parts(&fn_name, &fake_expr, MethodSuperBinding::Static(home));
        if let Some(sid) = class_scope_id {
            let _ = self.scopes.set_initialised(sid, class_name, false);
        }
        let (function_id, captured) = lowered?;
        // [[SourceText]] 取 MethodDefinition 文本（含 `#名`，剥离 `static`）。
        let source_text = self.method_definition_source_text(pm.span(), is_static);
        self.set_function_source_text(function_id, source_text);
        let instance_binding =
            self.declare_private_instance_binding(is_static, pm.span, next_function_index)?;
        Ok(PrivateMemberMeta {
            field_name,
            is_static,
            kind: PrivateMemberKind::Method(PrivateFunctionMeta {
                lowered_function: LoweredClassFunction {
                    function_id,
                    captured,
                },
                instance_binding,
                value: None,
                span: pm.span,
                is_generator: false,
            }),
        })
    }

    /// 实例私有方法的函数值经词法绑定传递给构造器（静态私有方法直接物化，无需绑定）。
    fn declare_private_instance_binding(
        &mut self,
        is_static: bool,
        span: Span,
        next_function_index: &mut usize,
    ) -> Result<Option<CapturedBinding>, LoweringError> {
        if is_static {
            return Ok(None);
        }
        let binding_name = format!(
            "$private_function#{}_{}",
            self.next_private_name_id, *next_function_index
        );
        *next_function_index += 1;
        let scope_id = self
            .scopes
            .declare(&binding_name, VarKind::Let, true)
            .map_err(|message| self.error(span, message))?;
        Ok(Some(CapturedBinding::new(binding_name, scope_id)))
    }

    fn materialize_private_member_values(
        &mut self,
        mut block: BasicBlockId,
        private_members: &mut [PrivateMemberMeta],
    ) -> Result<BasicBlockId, LoweringError> {
        for member in private_members {
            match &mut member.kind {
                PrivateMemberKind::Method(function) => {
                    block = self.materialize_private_function_value(block, function)?;
                }
                PrivateMemberKind::Accessor { getter, setter } => {
                    if let Some(function) = getter {
                        block = self.materialize_private_function_value(block, function)?;
                    }
                    if let Some(function) = setter {
                        block = self.materialize_private_function_value(block, function)?;
                    }
                }
            }
        }
        Ok(block)
    }

    fn materialize_private_function_value(
        &mut self,
        block: BasicBlockId,
        function: &mut PrivateFunctionMeta,
    ) -> Result<BasicBlockId, LoweringError> {
        let (continuation, value) = self.materialize_class_function_value(
            block,
            &function.lowered_function,
            function.span,
        )?;
        if let Some(binding) = &function.instance_binding {
            self.current_function.append_instruction(
                continuation,
                Instruction::StoreVar {
                    name: binding.var_ir_name(),
                    value,
                },
            );
        }
        function.value = Some(value);
        Ok(continuation)
    }

    fn load_private_instance_function_value(
        &mut self,
        block: BasicBlockId,
        function: &PrivateFunctionMeta,
    ) -> Result<(BasicBlockId, ValueId), LoweringError> {
        let binding = function
            .instance_binding
            .as_ref()
            .expect("instance private function must have a lexical binding");
        let value = self.load_captured_binding(block, binding)?;
        Ok((self.resolve_store_block(block), value))
    }

    fn patch_private_member_home_objects(
        &mut self,
        ctor_function_id: FunctionId,
        private_members: &[PrivateMemberMeta],
    ) {
        for member in private_members {
            let home_object = if member.is_static {
                HomeObject::Constructor(ctor_function_id)
            } else {
                HomeObject::Prototype(ctor_function_id)
            };
            let mut patch = |function: &PrivateFunctionMeta| {
                // generator wrapper 与公有 generator 方法一致，不接线 home_object。
                if function.is_generator {
                    return;
                }
                if let Some(ir_function) = self
                    .module
                    .function_mut(function.lowered_function.function_id)
                {
                    ir_function.home_object = Some(home_object);
                }
            };
            match &member.kind {
                PrivateMemberKind::Method(function) => patch(function),
                PrivateMemberKind::Accessor { getter, setter } => {
                    for function in getter.iter().chain(setter) {
                        patch(function);
                    }
                }
            }
        }
    }

    fn emit_static_private_member_binds(
        &mut self,
        block: BasicBlockId,
        ctor_dest: ValueId,
        private_members: &[PrivateMemberMeta],
    ) {
        for member in private_members {
            if !member.is_static {
                continue;
            }
            match &member.kind {
                PrivateMemberKind::Method(function) => {
                    let value = function
                        .value
                        .expect("static private method must be materialized");
                    self.emit_private_method_bind(block, ctor_dest, &member.field_name, value);
                }
                PrivateMemberKind::Accessor { getter, setter } => {
                    let getter_value = getter.as_ref().map(|function| {
                        function
                            .value
                            .expect("static private getter must be materialized")
                    });
                    let setter_value = setter.as_ref().map(|function| {
                        function
                            .value
                            .expect("static private setter must be materialized")
                    });
                    self.emit_private_accessor_bind(
                        block,
                        ctor_dest,
                        &member.field_name,
                        getter_value,
                        setter_value,
                    );
                }
            }
        }
    }
}

mod class_body;
mod decl;
mod expr;
mod function_values;
mod static_field;
