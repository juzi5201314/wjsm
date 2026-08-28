use super::*;

impl Lowerer {
    /// 共享类体降级：构造器函数、原型对象、超类链、方法/访问器/静态块/字段、装饰器。
    ///
    /// 调用方负责类名词法作用域的 push/pop 与最终名字绑定。
    /// 返回（最终 block、构造器值、未被 class decorator 替换的构造器 FunctionId）。
    pub(super) fn lower_class_body(
        &mut self,
        class_name: &str,
        class: &swc_ast::Class,
        class_span: Span,
        decorator_name: Option<&str>,
        block: BasicBlockId,
    ) -> Result<(BasicBlockId, ValueId, Option<FunctionId>), LoweringError> {
        let constructor = class.body.iter().find_map(|member| match member {
            swc_ast::ClassMember::Constructor(c) => Some(c),
            _ => None,
        });

        // 本类 lowering 新建函数的起始下标：PENDING ctor 占位回填只作用于该区间，
        // 避免嵌套类误改外层类已入模的 PENDING 引用。
        let class_function_start = self.module.functions().len();

        // brand 显示名用源码类名（decorator_name 即声明/表达式的 ident），
        // 匿名类表达式对齐 V8 显示 'anonymous'，不泄漏内部 anon_class_N。
        self.push_class_private_name_scope(decorator_name.unwrap_or("anonymous"), &class.body);
        let mut private_members = self.collect_class_private_members(class_name, &class.body)?;

        // 计算键实例字段：键在类定义期求值一次（ClassFieldDefinitionEvaluation），
        // 构造期复用。键值经构造器闭包的 key env（每次类求值新建，见
        // materialize_ctor_function_value）传递，构造器沿 $env 原型链按名读取。
        let computed_instance_keys =
            Self::collect_computed_instance_key_names(&class.body, self.next_private_name_id);

        // ── 构造器 IR 函数 ──
        // 构造器体延迟到类求值完成后才执行，期间类名已初始化（构造器体/实例字段初始化器可引用类名）；
        // 函数体 lowering 期间临时退出 TDZ，结束后恢复（类求值期间仍为 TDZ）。
        let ctor_name = format!("{}.constructor", class_name);
        let class_scope_id = self.scopes.resolve_scope_id(class_name).ok();
        if let Some(sid) = class_scope_id {
            self.scopes
                .set_initialised(sid, class_name, true)
                .map_err(|msg| self.error(class_span, msg))?;
        }
        self.push_function_context(&ctor_name, BasicBlockId(0));
        // 类体代码恒为严格模式（ClassDefinitionEvaluation）。
        self.strict_mode = true;
        self.is_method = true;
        self.super_allowed = true;
        self.set_lexical_home_object_for_enclosing_method(Self::PENDING_CTOR_FUNCTION_ID, false);
        self.super_call_allowed = class.super_class.is_some();

        let env_scope_id = self
            .scopes
            .declare("$env", VarKind::Let, true)
            .map_err(|msg| self.error(class_span, msg))?;
        let this_scope_id = self
            .scopes
            .declare("$this", VarKind::Let, true)
            .map_err(|msg| self.error(class_span, msg))?;

        // TypeScript 参数属性 `constructor(private x: number)` 同时是构造器**形参**
        // 与**实例字段声明**。此前整个 TsParamProp 被丢弃，导致形参不存在、字段也
        // 不存在（`this.x` 读到 undefined）。这里把它归一成普通形参 Pat 参与形参
        // 处理，并记录字段名，稍后发射 `this.<name> = <name>`。
        let mut ctor_param_pats_owned: Vec<swc_ast::Pat> = Vec::new();
        // （形参在 param_ir_names 中的下标, 字段名）
        let mut param_prop_slots: Vec<(usize, String)> = Vec::new();
        // param_ir_names 前两项固定为 $env / $this；Rest 形参不占 IR 形参名。
        let mut ir_slot = 2usize;
        for param in constructor.into_iter().flat_map(|ctor| &ctor.params) {
            let pat = match param {
                swc_ast::ParamOrTsParamProp::Param(param) => param.pat.clone(),
                swc_ast::ParamOrTsParamProp::TsParamProp(prop) => {
                    let (pat, binding) = match &prop.param {
                        swc_ast::TsParamPropParam::Ident(binding) => {
                            (swc_ast::Pat::Ident(binding.clone()), binding)
                        }
                        swc_ast::TsParamPropParam::Assign(assign) => match &*assign.left {
                            swc_ast::Pat::Ident(binding) => {
                                (swc_ast::Pat::Assign(assign.clone()), binding)
                            }
                            // TS 早期错误：参数属性的绑定必须是标识符，不允许解构。
                            other => {
                                return Err(self.error(
                                    other.span(),
                                    "a parameter property may not be declared using a binding pattern",
                                ));
                            }
                        },
                    };
                    param_prop_slots.push((ir_slot, binding.id.sym.to_string()));
                    pat
                }
            };
            if !matches!(pat, swc_ast::Pat::Rest(_)) {
                ir_slot += 1;
            }
            ctor_param_pats_owned.push(pat);
        }
        let ctor_param_pats: Vec<&swc_ast::Pat> = ctor_param_pats_owned.iter().collect();
        let param_ir_names =
            self.build_param_ir_names_impl(&ctor_param_pats, env_scope_id, this_scope_id, false)?;
        // 形参 IR 名此时才确定，回填成 (形参绑定, 字段名)；绑定携带作用域
        // id，箭头 super() 站点可经捕获链读取外层构造器帧的形参。
        let param_prop_fields: Vec<(CapturedBinding, String)> = param_prop_slots
            .into_iter()
            .map(|(slot, field)| {
                (
                    crate::lowerer_modules::parse_ir_name_to_binding(&param_ir_names[slot]),
                    field,
                )
            })
            .collect();
        if let Some(ctor) = constructor
            && let Some(body) = &ctor.body
        {
            self.predeclare_block_stmts(&body.stmts)?;
        }

        let entry = BasicBlockId(0);
        self.emit_hoisted_var_initializers(entry);

        let is_derived = class.super_class.is_some();
        if let Some(ctor) = constructor
            && is_derived
        {
            // this TDZ（ES §9.1.1.3.4）：派生显式构造器的 this 绑定在 super()
            // 前未初始化。规范中 thisArgument 由父 [[Construct]] 按
            // OrdinaryCreateFromConstructor(newTarget) 每次调用新建；wjsm 的
            // new 站点预创建实例已持有 newTarget.prototype，入口取其原型转存
            // `$super_proto#ctor`（名字带 `#`，与任何用户标识符不冲突），
            // super() 站点据此新建每次 Construct 的 thisArgument。随后将 this
            // 槽写为未初始化哨兵，this 读取的运行时检查据此抛 ReferenceError。
            let proto_scope_id = self
                .scopes
                .declare(Self::SUPER_PROTO_BINDING, VarKind::Let, true)
                .map_err(|msg| self.error(class_span, msg))?;
            let incoming_this = self.alloc_value();
            self.current_function.append_instruction(
                entry,
                Instruction::LoadVar {
                    dest: incoming_this,
                    name: format!("${this_scope_id}.$this"),
                },
            );
            let instance_proto = self.alloc_value();
            self.current_function.append_instruction(
                entry,
                Instruction::CallBuiltin {
                    dest: Some(instance_proto),
                    builtin: Builtin::ObjectGetPrototypeOf,
                    args: vec![incoming_this],
                },
            );
            self.current_function.append_instruction(
                entry,
                Instruction::StoreVar {
                    name: format!("${proto_scope_id}.{}", Self::SUPER_PROTO_BINDING),
                    value: instance_proto,
                },
            );
            let sentinel_const = self.module.add_constant(Constant::Uninitialized);
            let sentinel_val = self.alloc_value();
            self.current_function.append_instruction(
                entry,
                Instruction::Const {
                    dest: sentinel_val,
                    constant: sentinel_const,
                },
            );
            self.current_function.append_instruction(
                entry,
                Instruction::StoreVar {
                    name: format!("${this_scope_id}.$this"),
                    value: sentinel_val,
                },
            );
            self.ctor_super_proto = Some(CapturedBinding::new(
                Self::SUPER_PROTO_BINDING.to_string(),
                proto_scope_id,
            ));
            // 构造器内箭头观察 this / super 时（如 `(() => super())()` 或
            // super() 前创建、之后调用的 `() => this`）：TDZ 哨兵与
            // BindThisValue 重绑都必须对箭头帧可见——入口即把 this 注册进
            // 共享 env（上方哨兵已写入本地槽，复制进 env 的即哨兵），
            // 本帧读写与箭头帧统一走 env。
            if ctor_arrow_observes_this(ctor) {
                self.ctor_this_via_env = true;
                self.ensure_shared_env(entry, &[CapturedBinding::lexical_this()], class_span)?;
            }
            // 字段初始化器的 `arguments` 早错误与发射解耦：即使构造器没有
            // 可达 super() 站点，类定义仍必须报告该错误。
            for member in &class.body {
                match member {
                    swc_ast::ClassMember::PrivateProp(prop) if !prop.is_static => {
                        self.check_field_initializer_arguments(prop.value.as_deref())?;
                    }
                    swc_ast::ClassMember::ClassProp(prop) if !prop.is_static => {
                        self.check_field_initializer_arguments(prop.value.as_deref())?;
                    }
                    _ => {}
                }
            }
            let has_init_work = !param_prop_fields.is_empty()
                || private_members.iter().any(|member| !member.is_static)
                || class.body.iter().any(|member| match member {
                    swc_ast::ClassMember::PrivateProp(prop) => !prop.is_static,
                    swc_ast::ClassMember::ClassProp(prop) => !prop.is_static,
                    _ => false,
                });
            // 实例初始化推迟到 super() 站点发射（ES SuperCall 步骤 8–11：
            // BindThisValue 之后立即 InitializeInstanceElements），对语句、
            // 表达式、形参默认值中的任意 super() 位置一致成立。
            self.derived_ctor_init_ctx = Some(Box::new(DerivedCtorInitCtx {
                param_prop_fields: param_prop_fields.clone(),
                members: class.body.to_vec(),
                private_members: private_members.clone(),
                computed_instance_keys: computed_instance_keys.clone(),
                has_init_work,
            }));
        }

        let parameter_block = self.emit_pat_inits_impl(&ctor_param_pats, &param_ir_names, entry)?;

        let mut field_block = parameter_block;
        if constructor.is_none() && is_derived {
            let callee = self.alloc_value();
            self.current_function.append_instruction(
                field_block,
                Instruction::GetSuperConstructor { dest: callee },
            );
            let this_val = self.alloc_value();
            self.current_function.append_instruction(
                field_block,
                Instruction::LoadVar {
                    dest: this_val,
                    name: format!("${this_scope_id}.$this"),
                },
            );
            let super_result = self.alloc_value();
            self.current_function.append_instruction(
                field_block,
                Instruction::SuperCall {
                    dest: Some(super_result),
                    callee,
                    this_val,
                    args: Vec::new(),
                    forward_args: true,
                },
            );
            // 派生类缺省构造器等价 `constructor(...args) { super(...args); }`：
            // 父构造器返回对象时按 BindThisValue 重绑 this（字段随后落在该
            // 对象上）；`? Construct(func, args, NewTarget)` 抛出的异常必须
            // 终止本构造器并向 `new` 调用点传播，不得触达字段初始化器。
            let (selected, merge) =
                self.select_super_call_result(field_block, super_result, this_val);
            field_block = self.lower_value_exception_branch(merge, selected)?;
        }
        if !(constructor.is_some() && is_derived) {
            // 基类构造器 this 从入口即存在；派生缺省构造器已在上方完成
            // super() 与重绑。参数属性字段先于字段初始化器生效（TS 语义）。
            field_block = self.emit_param_prop_fields(field_block, &param_prop_fields)?;
            field_block = self.emit_instance_initializers(
                field_block,
                &class.body,
                &private_members,
                &computed_instance_keys,
            )?;
        }

        let mut inner_flow = if field_block == entry {
            StmtFlow::Open(entry)
        } else {
            StmtFlow::Open(field_block)
        };
        if constructor.is_some() {
            self.arguments_param_count = u32::try_from(
                ctor_param_pats
                    .iter()
                    .take_while(|pat| !matches!(pat, swc_ast::Pat::Rest(_)))
                    .count(),
            )
            .map_err(|_| self.error(class_span, "too many constructor parameters"))?;
            let ctor_refs_args = constructor.is_some_and(Self::ctor_references_arguments);
            let args_block = self.emit_arguments_init(
                match inner_flow {
                    StmtFlow::Open(b) => b,
                    _ => entry,
                },
                ctor_refs_args,
            )?;
            self.eval_caller_has_arguments = if let Some(c) = constructor {
                c.params
                    .iter()
                    .filter_map(|p| match p {
                        swc_ast::ParamOrTsParamProp::Param(param) => Some(&param.pat),
                        _ => None,
                    })
                    .any(|pat| {
                        let mut names = Vec::new();
                        Self::extract_pat_bindings(std::slice::from_ref(pat), &mut names);
                        names.iter().any(|n| n == "arguments")
                    })
                    || self.scopes.lookup("arguments").is_ok()
            } else {
                self.scopes.lookup("arguments").is_ok()
            };
            inner_flow = StmtFlow::Open(args_block);
        }
        if let Some(ctor) = constructor
            && let Some(body) = &ctor.body
        {
            for stmt in &body.stmts {
                // unreachable code 合法，跳过不报错
                if matches!(inner_flow, StmtFlow::Terminated) {
                    continue;
                }
                inner_flow = self.lower_stmt(stmt, inner_flow)?;
            }
        }

        if let StmtFlow::Open(b) = inner_flow {
            if is_derived {
                // [[Construct]] 步骤 13.c / 15：派生构造器体正常完结返回当前
                // this 绑定——super() 重绑后即父构造器返回的对象。显式构造器
                // 完结时 this 可能仍未初始化（super() 未执行），GetThisBinding
                // 抛 ReferenceError；该异常属于 [[Construct]]，不可被体内
                // try/catch 捕获（emit_ctor_this_construct_check 用 Throw
                // 终结子直接向 new 站点传播）。
                let this_val = self.emit_read_ctor_this(b);
                if self.ctor_super_proto.is_some() {
                    let (checked, ok_block) = self.emit_ctor_this_construct_check(b, this_val);
                    self.current_function.set_terminator(
                        ok_block,
                        Terminator::Return {
                            value: Some(checked),
                        },
                    );
                } else {
                    self.current_function.set_terminator(
                        b,
                        Terminator::Return {
                            value: Some(this_val),
                        },
                    );
                }
            } else {
                self.current_function
                    .set_terminator(b, Terminator::Return { value: None });
            }
        }

        let old_fn = std::mem::replace(
            &mut self.current_function,
            FunctionBuilder::new("", BasicBlockId(0)),
        );
        let has_eval = old_fn.has_eval();
        let blocks = old_fn.into_blocks();
        let mut ir_function = Function::new(&ctor_name, BasicBlockId(0));
        ir_function.set_has_eval(has_eval);
        if let Some(span) =
            self.span_to_source_span(constructor.map(|c| c.span()).unwrap_or_else(|| class_span))
        {
            ir_function.set_source_span(span);
        }
        ir_function.set_params(param_ir_names);
        let ctor_captured = self.captured_names_stack.last().unwrap().clone();
        ir_function.set_captured_names(Self::captured_display_names(&ctor_captured));
        for blk in blocks {
            ir_function.push_block(blk);
        }
        ir_function.set_needs_prototype(true);
        let ctor_function_id = self.module.push_function(ir_function);
        if let Some(function) = self.module.function_mut(ctor_function_id) {
            function.home_object = Some(HomeObject::Prototype(ctor_function_id));
        }
        self.patch_pending_ctor_home_object_references(ctor_function_id, class_function_start);
        self.patch_private_member_home_objects(ctor_function_id, &private_members);
        self.pop_function_context();
        if let Some(sid) = class_scope_id {
            let _ = self.scopes.set_initialised(sid, class_name, false);
        }

        // ── 物化构造器 + 创建原型 ──
        let block = self.materialize_private_member_values(block, &mut private_members)?;

        let constructor_function = LoweredClassFunction {
            function_id: ctor_function_id,
            captured: ctor_captured,
        };
        let (block, ctor_dest, ctor_key_env) = self.materialize_ctor_function_value(
            block,
            &constructor_function,
            class_span,
            computed_instance_keys.len() as u32,
        )?;

        let proto_dest = self.alloc_value();
        let method_count = class
            .body
            .iter()
            .filter(|m| {
                matches!(
                    m,
                    swc_ast::ClassMember::Method(m) if matches!(m.kind, swc_ast::MethodKind::Method)
                )
            })
            .count() as u32;
        let proto_capacity = std::cmp::max(4, method_count);
        self.current_function.append_instruction(
            block,
            Instruction::NewObject {
                dest: proto_dest,
                capacity: proto_capacity,
            },
        );

        let mut block = block;
        if let Some(super_class) = &class.super_class {
            // 超类表达式可能引入控制流（如 eval 模式下共享 env 绑定读取的
            // 分叉合流）：必须推进到延续块，否则后续指令落回分叉前的块，
            // 使用合流 phi 值将违反支配关系。
            let super_ctor = self.lower_expr_then_continue(super_class, &mut block)?;
            let proto_key_dest = self.emit_string_const(block, "prototype");
            let super_proto = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::CallBuiltin {
                    dest: Some(super_proto),
                    builtin: Builtin::ReflectGet,
                    args: vec![super_ctor, proto_key_dest, super_ctor],
                },
            );
            self.current_function.append_instruction(
                block,
                Instruction::SetProto {
                    object: proto_dest,
                    value: super_proto,
                },
            );
            self.current_function.append_instruction(
                block,
                Instruction::SetProto {
                    object: ctor_dest,
                    value: super_ctor,
                },
            );
            let super_key_dest = self.emit_string_const(block, "__super_constructor__");
            self.emit_set_prop(block, ctor_dest, super_key_dest, super_ctor);
        }

        // 设置 prototype.constructor = ctor（ECMAScript: ClassDefinitionEvaluation）
        let ctor_key_dest = self.emit_string_const(block, "constructor");
        self.emit_set_prop(block, proto_dest, ctor_key_dest, ctor_dest);

        // ── 成员处理：按 ClassDefinitionEvaluation 分两遍 ──
        // 第一遍（源顺序）：ClassElementEvaluation —— 方法/访问器安装、全部字段
        // 计算键求值（含 ToPropertyKey，异常在类定义期传播）。计算键只在此求值
        // 一次：实例键写入合成词法绑定供构造器复用，静态键值暂存供第二遍使用。
        let mut block = block;
        let mut static_computed_keys: std::collections::HashMap<usize, ValueId> =
            std::collections::HashMap::new();
        for (member_index, member) in class.body.iter().enumerate() {
            match member {
                swc_ast::ClassMember::Method(method) => {
                    block = self.lower_class_method_member(
                        block,
                        method,
                        class_name,
                        ctor_function_id,
                        ctor_dest,
                        proto_dest,
                    )?;
                }
                swc_ast::ClassMember::ClassProp(prop)
                    if matches!(prop.key, swc_ast::PropName::Computed(_)) =>
                {
                    let key_dest = self.lower_prop_name_checked(&prop.key, &mut block)?;
                    if prop.is_static {
                        self.emit_static_prototype_key_guard(&mut block, key_dest)?;
                        static_computed_keys.insert(member_index, key_dest);
                    } else {
                        let key_name = computed_instance_keys
                            .get(&member_index)
                            .expect("computed instance field key name must be pre-collected");
                        let key_env = ctor_key_env
                            .expect("ctor key env must exist for computed instance field keys");
                        let name_const = self.emit_string_const(block, key_name);
                        self.emit_set_prop(block, key_env, name_const, key_dest);
                    }
                }
                swc_ast::ClassMember::Constructor(_)
                | swc_ast::ClassMember::PrivateMethod(_)
                | swc_ast::ClassMember::StaticBlock(_)
                | swc_ast::ClassMember::PrivateProp(_)
                | swc_ast::ClassMember::ClassProp(_) => {}
                other => {
                    return Err(self.error(
                        class_member_span(other),
                        format!(
                            "unsupported class member `{}` during class lowering",
                            class_member_kind(other),
                        ),
                    ));
                }
            }
        }

        // 静态私有方法/访问器在静态初始化器运行前绑定到构造器：
        // 规范上方法属于 ClassElementEvaluation（第一遍），静态字段初始化器与
        // static block（第二遍）可经 `this.#m()` 调用它们。
        self.emit_static_private_member_binds(block, ctor_dest, &private_members);

        // 第二遍（源顺序）：静态元素执行期 —— 静态字段初始化器与 static block。
        // 键已全部求值完毕，此处只求初始化器/执行块体（ES ClassDefinitionEvaluation
        // 对 staticElements 的 DefineField / Call 步骤）；静态字段初始化器以
        // 合成函数求值，`this` 为构造器本身（见 lower_static_field_member）。
        let mut static_init_idx = 0u32;
        let mut static_field_init_idx = 0u32;
        for (member_index, member) in class.body.iter().enumerate() {
            match member {
                swc_ast::ClassMember::StaticBlock(static_block) => {
                    block = self.lower_class_static_block(
                        block,
                        static_block,
                        class_name,
                        ctor_function_id,
                        ctor_dest,
                        static_init_idx,
                    )?;
                    static_init_idx += 1;
                }
                swc_ast::ClassMember::PrivateProp(prop) if prop.is_static => {
                    let field_name =
                        self.resolve_private_storage_name(prop.key.name.as_ref(), prop.key.span)?;
                    let key_dest = self.emit_string_const(block, &field_name);
                    block = self.lower_static_field_member(
                        block,
                        &static_field::StaticFieldInit {
                            class_name,
                            ctor_function_id,
                            ctor_dest,
                            key_dest,
                            init_value: prop.value.as_deref(),
                            is_private: true,
                            span: prop.span,
                            init_index: static_field_init_idx,
                        },
                    )?;
                    static_field_init_idx += 1;
                }
                swc_ast::ClassMember::ClassProp(prop) if prop.is_static => {
                    let key_dest = match static_computed_keys.get(&member_index) {
                        Some(key) => *key,
                        // 静态属性名只发射 Const，不产生控制流。
                        None => self.lower_prop_name(&prop.key, block)?,
                    };
                    block = self.lower_static_field_member(
                        block,
                        &static_field::StaticFieldInit {
                            class_name,
                            ctor_function_id,
                            ctor_dest,
                            key_dest,
                            init_value: prop.value.as_deref(),
                            is_private: false,
                            span: prop.span,
                            init_index: static_field_init_idx,
                        },
                    )?;
                    static_field_init_idx += 1;
                }
                _ => {}
            }
        }

        // ── 后处理 ──

        let proto_key_dest = self.emit_string_const(block, "prototype");
        self.emit_set_prop(block, ctor_dest, proto_key_dest, proto_dest);

        let direct_constructor = class.decorators.is_empty().then_some(ctor_function_id);
        let (block, ctor_dest) =
            self.emit_apply_class_decorators(block, ctor_dest, &class.decorators, decorator_name)?;

        self.pop_class_private_name_scope();
        Ok((block, ctor_dest, direct_constructor))
    }

    /// 处理单个类方法成员（Method / Getter / Setter）。
    fn lower_class_method_member(
        &mut self,
        mut block: BasicBlockId,
        method: &swc_ast::ClassMethod,
        class_name: &str,
        ctor_function_id: FunctionId,
        ctor_dest: ValueId,
        proto_dest: ValueId,
    ) -> Result<BasicBlockId, LoweringError> {
        let is_static = method.is_static;
        let target = if is_static { ctor_dest } else { proto_dest };
        let (method_name, m_key_dest) = self.lower_class_member_key(&method.key, &mut block)?;
        if is_static && matches!(method.key, swc_ast::PropName::Computed(_)) {
            self.emit_static_prototype_key_guard(&mut block, m_key_dest)?;
        }

        match method.kind {
            swc_ast::MethodKind::Method => {
                if method.function.is_generator || method.function.is_async {
                    // generator / async 方法体延迟到类求值完成后才执行，期间类名已初始化；
                    // 二者都需要 body + wrapper 双函数结构，路由在 lower_method_prop_to_fn。
                    let class_scope_id = self.scopes.resolve_scope_id(class_name).ok();
                    if let Some(sid) = class_scope_id {
                        self.scopes
                            .set_initialised(sid, class_name, true)
                            .map_err(|msg| self.error(method.span, msg))?;
                    }
                    // 类方法的 [[HomeObject]] 静态可知，async 函数族 body 的 super
                    // 经静态元数据接线，不依赖 env 上的 `home`。generator（含 async
                    // generator）wrapper 因此无需包 home env——包装反而使 body 深度 0
                    // 的捕获写落在包装对象上遮蔽共享 env；async 非 generator 方法
                    // 保留 home wrapper（eval meta 等仍消费 env home）。
                    let method_home = if method.function.is_generator {
                        None
                    } else {
                        Some(target)
                    };
                    let static_home = if is_static {
                        HomeObject::Constructor(ctor_function_id)
                    } else {
                        HomeObject::Prototype(ctor_function_id)
                    };
                    let function = self.lower_method_prop_to_fn(
                        &method.key,
                        &method.function,
                        method_home,
                        Some(static_home),
                    )?;
                    if let Some(sid) = class_scope_id {
                        let _ = self.scopes.set_initialised(sid, class_name, false);
                    }
                    let (continuation, mut method_value) = self.materialize_method_function_value(
                        block,
                        &function,
                        method_home,
                        method.function.span,
                    )?;
                    block = continuation;
                    if !method.function.decorators.is_empty() {
                        (block, method_value) = self.emit_apply_value_decorators(
                            block,
                            method_value,
                            &ValueDecoratorContext {
                                decorators: &method.function.decorators,
                                kind: "method",
                                name: &method_name,
                                is_static,
                                is_private: false,
                            },
                        )?;
                    }
                    self.emit_set_prop(block, target, m_key_dest, method_value);
                    return Ok(block);
                }

                let fn_name = format!("{class_name}.{method_name}");
                let function = self.lower_class_method_fn(
                    class_name,
                    &fn_name,
                    &method.function,
                    method.span,
                    ctor_function_id,
                    is_static,
                )?;
                let (continuation, mut m_dest) =
                    self.materialize_class_function_value(block, &function, method.span)?;
                block = continuation;
                if !method.function.decorators.is_empty() {
                    (block, m_dest) = self.emit_apply_value_decorators(
                        block,
                        m_dest,
                        &ValueDecoratorContext {
                            decorators: &method.function.decorators,
                            kind: "method",
                            name: &method_name,
                            is_static,
                            is_private: false,
                        },
                    )?;
                }
                self.emit_set_prop(block, target, m_key_dest, m_dest);
                Ok(block)
            }
            swc_ast::MethodKind::Getter | swc_ast::MethodKind::Setter => {
                let accessor = if matches!(method.kind, swc_ast::MethodKind::Getter) {
                    "get"
                } else {
                    "set"
                };
                let fn_name = format!("{class_name}.{accessor}_{method_name}");
                let function = self.lower_class_method_fn(
                    class_name,
                    &fn_name,
                    &method.function,
                    method.span,
                    ctor_function_id,
                    is_static,
                )?;
                let (continuation, mut fn_dest) =
                    self.materialize_class_function_value(block, &function, method.span)?;
                block = continuation;
                if !method.function.decorators.is_empty() {
                    let kind = if matches!(method.kind, swc_ast::MethodKind::Getter) {
                        "getter"
                    } else {
                        "setter"
                    };
                    (block, fn_dest) = self.emit_apply_value_decorators(
                        block,
                        fn_dest,
                        &ValueDecoratorContext {
                            decorators: &method.function.decorators,
                            kind,
                            name: &method_name,
                            is_static,
                            is_private: false,
                        },
                    )?;
                }
                let desc = self.build_descriptor(accessor, fn_dest, false, true, block)?;
                self.current_function.append_instruction(
                    block,
                    Instruction::CallBuiltin {
                        dest: None,
                        builtin: Builtin::DefineProperty,
                        args: vec![target, m_key_dest, desc],
                    },
                );
                Ok(block)
            }
        }
    }

    /// 处理类静态块成员：创建 IR 函数并在当前 block 发起调用。
    ///
    /// ClassDefinitionEvaluation 对 ClassStaticBlockDefinition 的
    /// `? Call(bodyFunction, F)`：块体抛出的异常必须在类定义期传播
    /// （与静态字段初始化器同一路径），后续静态元素与类名绑定不得执行。
    fn lower_class_static_block(
        &mut self,
        block: BasicBlockId,
        static_block: &swc_ast::StaticBlock,
        class_name: &str,
        ctor_function_id: FunctionId,
        ctor_dest: ValueId,
        idx: u32,
    ) -> Result<BasicBlockId, LoweringError> {
        let fn_name = format!("{}.static_init_{}", class_name, idx);
        let function = self.lower_class_static_block_fn(
            &fn_name,
            &static_block.body,
            static_block.span,
            ctor_function_id,
        )?;
        let (continuation, function_value) =
            self.materialize_class_function_value(block, &function, static_block.span)?;
        let result = self.alloc_value();
        self.current_function.append_instruction(
            continuation,
            Instruction::Call {
                dest: Some(result),
                callee: function_value,
                this_val: ctor_dest,
                args: vec![],
            },
        );
        self.lower_value_exception_branch(continuation, result)
    }

    /// 为类方法/访问器创建 IR 函数并返回其函数标识及捕获集合。
    fn lower_class_method_fn(
        &mut self,
        class_name: &str,
        fn_name: &str,
        function: &swc_ast::Function,
        method_span: Span,
        ctor_function_id: FunctionId,
        is_static: bool,
    ) -> Result<LoweredClassFunction, LoweringError> {
        // 方法体延迟到类求值完成后才执行，期间类名已初始化（方法体可引用类名）；
        // 函数体 lowering 期间临时退出 TDZ，结束后恢复。
        let class_scope_id = self.scopes.resolve_scope_id(class_name).ok();
        if let Some(sid) = class_scope_id {
            self.scopes
                .set_initialised(sid, class_name, true)
                .map_err(|msg| self.error(method_span, msg))?;
        }
        self.push_function_context(fn_name, BasicBlockId(0));
        // 类体代码恒为严格模式（ClassDefinitionEvaluation）。
        self.strict_mode = true;
        self.is_method = true;
        self.super_allowed = true;
        self.set_lexical_home_object_for_enclosing_method(ctor_function_id, is_static);

        let env_scope_id = self
            .scopes
            .declare("$env", VarKind::Let, true)
            .map_err(|msg| self.error(method_span, msg))?;
        let this_scope_id = self
            .scopes
            .declare("$this", VarKind::Let, true)
            .map_err(|msg| self.error(method_span, msg))?;

        let mut param_ir_names = vec![
            format!("${env_scope_id}.$env"),
            format!("${this_scope_id}.$this"),
        ];
        for param in &function.params {
            if let swc_ast::Pat::Ident(binding_ident) = &param.pat {
                let name = binding_ident.id.sym.to_string();
                let scope_id = self
                    .scopes
                    .declare(&name, VarKind::Let, true)
                    .map_err(|msg| self.error(method_span, msg))?;
                param_ir_names.push(format!("${scope_id}.{name}"));
            }
        }

        if let Some(body) = &function.body {
            self.predeclare_block_stmts(&body.stmts)?;
        }

        let m_entry = BasicBlockId(0);
        self.emit_hoisted_var_initializers(m_entry);
        self.arguments_param_count = Self::count_regular_params(&function.params);
        let m_entry =
            self.emit_arguments_init(m_entry, Self::function_needs_arguments_object(function))?;
        self.eval_caller_has_arguments = Self::detect_param_arguments(&function.params)
            || self.scopes.lookup("arguments").is_ok();

        let mut m_flow = StmtFlow::Open(m_entry);
        if let Some(body) = &function.body {
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

        let home_object = if is_static {
            HomeObject::Constructor(ctor_function_id)
        } else {
            HomeObject::Prototype(ctor_function_id)
        };
        let function =
            self.finalize_class_method_function(fn_name, method_span, param_ir_names, home_object);
        self.pop_function_context();
        if let Some(sid) = class_scope_id {
            let _ = self.scopes.set_initialised(sid, class_name, false);
        }
        Ok(function)
    }

    /// 为类静态块创建 IR 函数并返回其函数标识及捕获集合。
    fn lower_class_static_block_fn(
        &mut self,
        fn_name: &str,
        body: &swc_ast::BlockStmt,
        span: Span,
        ctor_function_id: FunctionId,
    ) -> Result<LoweredClassFunction, LoweringError> {
        self.push_function_context(fn_name, BasicBlockId(0));
        // 类体代码恒为严格模式（ClassDefinitionEvaluation）。
        self.strict_mode = true;
        self.is_method = true;
        self.super_allowed = true;
        self.set_lexical_home_object_for_enclosing_method(ctor_function_id, true);

        let env_scope_id = self
            .scopes
            .declare("$env", VarKind::Let, true)
            .map_err(|msg| self.error(span, msg))?;
        let this_scope_id = self
            .scopes
            .declare("$this", VarKind::Let, true)
            .map_err(|msg| self.error(span, msg))?;
        let param_ir_names = vec![
            format!("${env_scope_id}.$env"),
            format!("${this_scope_id}.$this"),
        ];

        self.predeclare_block_stmts(&body.stmts)?;

        let m_entry = BasicBlockId(0);
        self.emit_hoisted_var_initializers(m_entry);
        self.arguments_param_count = 0;
        let m_entry = self.emit_arguments_init(m_entry, Self::body_references_arguments(body))?;
        self.eval_caller_has_arguments = self.scopes.lookup("arguments").is_ok();

        let mut m_flow = StmtFlow::Open(m_entry);
        for stmt in &body.stmts {
            if matches!(m_flow, StmtFlow::Terminated) {
                continue;
            }
            m_flow = self.lower_stmt(stmt, m_flow)?;
        }

        if let StmtFlow::Open(b) = m_flow {
            self.current_function
                .set_terminator(b, Terminator::Return { value: None });
        }

        let function = self.finalize_class_method_function(
            fn_name,
            span,
            param_ir_names,
            HomeObject::Constructor(ctor_function_id),
        );
        self.pop_function_context();
        Ok(function)
    }

    /// 收尾方法 IR 函数：提取 blocks、设置元数据，并返回统一的 class function metadata。
    pub(super) fn finalize_class_method_function(
        &mut self,
        fn_name: &str,
        span: Span,
        param_ir_names: Vec<String>,
        home_object: HomeObject,
    ) -> LoweredClassFunction {
        let old_function = std::mem::replace(
            &mut self.current_function,
            FunctionBuilder::new("", BasicBlockId(0)),
        );
        let has_eval = old_function.has_eval();
        let blocks = old_function.into_blocks();
        let mut ir_function = Function::new(fn_name, BasicBlockId(0));
        ir_function.set_has_eval(has_eval);
        if let Some(source_span) = self.span_to_source_span(span) {
            ir_function.set_source_span(source_span);
        }
        ir_function.set_params(param_ir_names);
        let captured = self.captured_names_stack.last().unwrap().clone();
        ir_function.set_captured_names(Self::captured_display_names(&captured));
        ir_function.home_object = Some(home_object);
        for block in blocks {
            ir_function.push_block(block);
        }
        let function_id = self.module.push_function(ir_function);
        LoweredClassFunction {
            function_id,
            captured,
        }
    }

    /// 提取类成员键（方法/访问器）：返回 (名称字符串, 运行时 key value)。
    /// 计算键按 MethodDefinitionEvaluation 求值 + ToPropertyKey 并推进 block，
    /// 键异常在方法闭包创建/安装之前传播。
    fn lower_class_member_key(
        &mut self,
        key: &swc_ast::PropName,
        block: &mut BasicBlockId,
    ) -> Result<(String, ValueId), LoweringError> {
        match key {
            swc_ast::PropName::Ident(ident) => {
                let name = ident.sym.to_string();
                Ok((name, self.emit_string_const(*block, ident.sym.as_ref())))
            }
            swc_ast::PropName::Str(s) => {
                let name = s.value.to_string_lossy().into_owned();
                let key_dest = self.emit_string_const(*block, &name);
                Ok((name, key_dest))
            }
            swc_ast::PropName::Computed(_)
            | swc_ast::PropName::Num(_)
            | swc_ast::PropName::BigInt(_) => {
                let key_dest = self.lower_prop_name_checked(key, block)?;
                Ok(("<computed>".to_string(), key_dest))
            }
        }
    }

    /// 静态成员计算键的 `"prototype"` 守卫：MakeConstructor 使构造器的
    /// `prototype` 不可写不可配置，静态成员定义必然失败；各引擎（V8 /
    /// SpiderMonkey / JSC）一致在键求值（ToPropertyKey 后）立即抛 TypeError，
    /// 初始化器与后续键不再求值。Symbol 键与 `"prototype"` 严格不等，自然放行。
    fn emit_static_prototype_key_guard(
        &mut self,
        block: &mut BasicBlockId,
        key_dest: ValueId,
    ) -> Result<(), LoweringError> {
        let proto_const = self.emit_string_const(*block, "prototype");
        let is_proto = self.alloc_value();
        self.current_function.append_instruction(
            *block,
            Instruction::Compare {
                dest: is_proto,
                op: CompareOp::StrictEq,
                lhs: key_dest,
                rhs: proto_const,
            },
        );
        let throw_block = self.current_function.new_block();
        let cont_block = self.current_function.new_block();
        self.current_function.set_terminator(
            *block,
            Terminator::Branch {
                condition: is_proto,
                true_block: throw_block,
                false_block: cont_block,
            },
        );
        let msg_val = self.emit_string_const(
            throw_block,
            "Classes may not have a static property named 'prototype'",
        );
        let error_val = self.alloc_value();
        self.current_function.append_instruction(
            throw_block,
            Instruction::CallBuiltin {
                dest: Some(error_val),
                builtin: Builtin::TypeErrorConstructor,
                args: vec![msg_val],
            },
        );
        self.emit_throw_value(throw_block, error_val)?;
        *block = cont_block;
        Ok(())
    }

    /// 收集计算键实例字段的 key env 属性名（`$class_key#id_idx`）。
    ///
    /// 名字带 `#` 且无 `${scope}.` 前缀，与源绑定的 env 键（`$N.name`）及方法
    /// home env 的 `home` 键都不冲突。`class_private_id` 取
    /// `push_class_private_name_scope` 之后的 `next_private_name_id`，与
    /// `$private_function#` 命名同一约定，保证跨类唯一。
    fn collect_computed_instance_key_names(
        body: &[swc_ast::ClassMember],
        class_private_id: u32,
    ) -> std::collections::HashMap<usize, String> {
        let mut names = std::collections::HashMap::new();
        let mut next_key_index = 0usize;
        for (member_index, member) in body.iter().enumerate() {
            let swc_ast::ClassMember::ClassProp(prop) = member else {
                continue;
            };
            if prop.is_static || !matches!(prop.key, swc_ast::PropName::Computed(_)) {
                continue;
            }
            names.insert(
                member_index,
                format!("$class_key#{class_private_id}_{next_key_index}"),
            );
            next_key_index += 1;
        }
        names
    }

    /// 物化构造器函数值。
    ///
    /// 含计算键实例字段（`computed_key_count > 0`）时，在捕获 env 外包一层
    /// key env：每次类求值新建（循环/多次求值的各个类彼此隔离），定义期把
    /// 求得的键写为其自有属性，构造器沿 `$env` 原型链按名读取。构造器统一
    /// `super_allowed`，其对外层绑定的写入总是沿链定位 owner env，不会误写
    /// 到 key env（与方法 home env 包层的既有约定一致）。
    fn materialize_ctor_function_value(
        &mut self,
        block: BasicBlockId,
        function: &LoweredClassFunction,
        span: Span,
        computed_key_count: u32,
    ) -> Result<(BasicBlockId, ValueId, Option<ValueId>), LoweringError> {
        if computed_key_count == 0 {
            let (continuation, value) =
                self.materialize_class_function_value(block, function, span)?;
            return Ok((continuation, value, None));
        }

        let function_ref = self
            .module
            .add_constant(Constant::FunctionRef(function.function_id));
        let function_value = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: function_value,
                constant: function_ref,
            },
        );
        let (block, base_env) = if function.captured.is_empty() {
            (block, self.load_env_object(block))
        } else {
            let env = self.ensure_shared_env(block, &function.captured, span)?;
            (self.resolve_store_block(block), env)
        };
        let key_env = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::NewObject {
                dest: key_env,
                capacity: computed_key_count,
            },
        );
        self.current_function.append_instruction(
            block,
            Instruction::SetProto {
                object: key_env,
                value: base_env,
            },
        );
        let closure = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::CallBuiltin {
                dest: Some(closure),
                builtin: Builtin::CreateClosure,
                args: vec![function_value, key_env],
            },
        );
        Ok((block, closure, Some(key_env)))
    }
}
