use swc_core::common::DUMMY_SP;
use swc_core::common::Span;
use swc_core::common::Spanned;
use swc_core::ecma::ast as swc_ast;
use wjsm_ir::{
    BasicBlock, BasicBlockId, BinaryOp, Builtin, CompareOp, Constant, ConstantId, Function,
    FunctionId, Instruction, Module, PhiSource, Program, SwitchCaseTarget, Terminator, UnaryOp,
    ValueId,
};
use crate::scope_tree::{ScopeKind, VarKind, LexicalMode, ScopeTree};
use crate::cfg_builder::{FunctionBuilder, LabelContext, LabelKind, FinallyContext, TryContext, StmtFlow};
use crate::builtins::*;
use crate::eval_helpers::*;
use crate::kind_strings::*;
use crate::{LoweringError, Diagnostic};
use super::lowerer::{Lowerer, ActiveUsingVar, AsyncContextState, HoistedVar, CapturedBinding, EVAL_SCOPE_ENV_PARAM, WK_SYMBOL_ITERATOR, WK_SYMBOL_SPECIES, WK_SYMBOL_TO_STRING_TAG, WK_SYMBOL_ASYNC_ITERATOR, WK_SYMBOL_HAS_INSTANCE, WK_SYMBOL_TO_PRIMITIVE, WK_SYMBOL_DISPOSE, WK_SYMBOL_MATCH, WK_SYMBOL_ASYNC_DISPOSE};

impl Lowerer {
    pub(crate) fn lower_ts_enum(
        &mut self,
        ts_enum: &swc_ast::TsEnumDecl,
        flow: StmtFlow,
    ) -> Result<StmtFlow, LoweringError> {
        let block = self.ensure_open(flow)?;
        let enum_name = ts_enum.id.sym.to_string();

        // 创建枚举对象
        let capacity = std::cmp::max(4, ts_enum.members.len() as u32);
        let obj_dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::NewObject {
                dest: obj_dest,
                capacity,
            },
        );

        // 遍历成员，生成正向和反向映射
        let mut implicit_value: f64 = 0.0;
        for member in &ts_enum.members {
            // 获取成员名（字符串）
            let member_name = match &member.id {
                swc_ast::TsEnumMemberId::Ident(ident) => ident.sym.to_string(),
                swc_ast::TsEnumMemberId::Str(s) => s.value.to_string_lossy().into_owned(),
            };

            // 计算成员值
            let member_value = if let Some(init_expr) = &member.init {
                // 有显式初始化表达式
                let val = self.lower_expr(init_expr, block)?;
                // 尝试从数值常量读取隐式递增值起点
                if let swc_ast::Expr::Lit(swc_ast::Lit::Num(num)) = init_expr.as_ref() {
                    implicit_value = num.value + 1.0;
                }
                val
            } else {
                // 无初始化表达式，使用隐式递增值
                let const_id = self.module.add_constant(Constant::Number(implicit_value));
                let val = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::Const {
                        dest: val,
                        constant: const_id,
                    },
                );
                implicit_value += 1.0;
                val
            };

            // 正向映射：obj[memberName] = value
            let key_const = self.module.add_constant(Constant::String(member_name.clone()));
            let key_dest = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::Const {
                    dest: key_dest,
                    constant: key_const,
                },
            );
            self.current_function.append_instruction(
                block,
                Instruction::SetProp {
                    object: obj_dest,
                    key: key_dest,
                    value: member_value,
                },
            );

            // 反向映射：obj[value] = memberName（数字值 → 成员名）
            let reverse_key_str = if let Some(init_expr) = &member.init {
                if let swc_ast::Expr::Lit(swc_ast::Lit::Num(num)) = init_expr.as_ref() {
                    Some(if num.value == num.value.trunc() {
                        format!("{}", num.value as i64)
                    } else {
                        format!("{}", num.value)
                    })
                } else {
                    None
                }
            } else {
                Some(format!("{}", (implicit_value - 1.0) as i64))
            };
            if let Some(num_str) = reverse_key_str {
                self.emit_enum_reverse_mapping(block, obj_dest, &num_str, &member_name);
            }
        }

        // StoreVar: 将枚举对象赋给枚举名
        let scope_id = self
            .scopes
            .resolve_scope_id(&enum_name)
            .map_err(|msg| self.error(ts_enum.span(), msg))?;
        let ir_name = format!("${scope_id}.{enum_name}");
        self.current_function.append_instruction(
            block,
            Instruction::StoreVar {
                name: ir_name,
                value: obj_dest,
            },
        );
        let _ = self.scopes.mark_initialised(&enum_name);

        Ok(StmtFlow::Open(block))
    }

    pub(crate) fn emit_enum_reverse_mapping(
        &mut self,
        block: BasicBlockId,
        obj_dest: ValueId,
        num_str: &str,
        member_name: &str,
    ) {
        let rev_key_const = self.module.add_constant(Constant::String(num_str.to_string()));
        let rev_key_dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: rev_key_dest,
                constant: rev_key_const,
            },
        );
        let rev_val_const = self.module.add_constant(Constant::String(member_name.to_string()));
        let rev_val_dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: rev_val_dest,
                constant: rev_val_const,
            },
        );
        self.current_function.append_instruction(
            block,
            Instruction::SetProp {
                object: obj_dest,
                key: rev_key_dest,
                value: rev_val_dest,
            },
        );
    }

    pub(crate) fn lower_ts_module(
        &mut self,
        ts_module: &swc_ast::TsModuleDecl,
        flow: StmtFlow,
    ) -> Result<StmtFlow, LoweringError> {
        let block = self.ensure_open(flow)?;

        if ts_module.declare {
            return Ok(StmtFlow::Open(block));
        }

        let module_name = match &ts_module.id {
            swc_ast::TsModuleName::Ident(ident) => ident.sym.to_string(),
            swc_ast::TsModuleName::Str(s) => s.value.to_string_lossy().into_owned(),
        };

        let obj_dest = self.lower_ts_module_body(ts_module, block)?;

        let scope_id = self
            .scopes
            .resolve_scope_id(&module_name)
            .map_err(|msg| self.error(ts_module.span(), msg))?;
        let ir_name = format!("${scope_id}.{module_name}");
        self.current_function.append_instruction(
            block,
            Instruction::StoreVar {
                name: ir_name,
                value: obj_dest,
            },
        );
        let _ = self.scopes.mark_initialised(&module_name);

        Ok(StmtFlow::Open(block))
    }

    pub(crate) fn lower_ts_module_body(
        &mut self,
        ts_module: &swc_ast::TsModuleDecl,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let obj_dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::NewObject {
                dest: obj_dest,
                capacity: 4,
            },
        );

        if let Some(body) = &ts_module.body {
            match body {
                swc_ast::TsNamespaceBody::TsModuleBlock(module_block) => {
                    self.scopes.push_scope(ScopeKind::Block);
                    for item in &module_block.body {
                        match item {
                            swc_ast::ModuleItem::Stmt(stmt) => {
                                self.predeclare_stmt_with_mode_and_eval_strings(
                                    stmt,
                                    LexicalMode::Include,
                                    &mut std::collections::HashMap::new(),
                                )?;
                            }
                            swc_ast::ModuleItem::ModuleDecl(module_decl) => {
                                if let swc_ast::ModuleDecl::ExportDecl(export_decl) = module_decl {
                                    self.predeclare_stmt_with_mode_and_eval_strings(
                                        &swc_ast::Stmt::Decl(export_decl.decl.clone()),
                                        LexicalMode::Include,
                                        &mut std::collections::HashMap::new(),
                                    )?;
                                }
                            }
                        }
                    }
                    for item in &module_block.body {
                        match item {
                            swc_ast::ModuleItem::Stmt(stmt) => {
                                if let swc_ast::Stmt::Decl(_decl) = stmt {
                                    self.lower_stmt(stmt, StmtFlow::Open(block))?;
                                }
                            }
                            swc_ast::ModuleItem::ModuleDecl(module_decl) => {
                                self.lower_module_decl_into_object(module_decl, obj_dest, block)?;
                            }
                        }
                    }
                    self.scopes.pop_scope();
                }
                swc_ast::TsNamespaceBody::TsNamespaceDecl(nested) => {
                    let nested_module = swc_ast::TsModuleDecl {
                        span: nested.span,
                        declare: false,
                        global: false,
                        namespace: true,
                        id: swc_ast::TsModuleName::Ident(nested.id.clone()),
                        body: Some(*nested.body.clone()),
                    };
                    let nested_obj = self.lower_ts_module_body(&nested_module, block)?;
                    let nested_name = nested.id.sym.to_string();
                    let key_const = self.module.add_constant(Constant::String(nested_name));
                    let key_dest = self.alloc_value();
                    self.current_function.append_instruction(
                        block,
                        Instruction::Const {
                            dest: key_dest,
                            constant: key_const,
                        },
                    );
                    self.current_function.append_instruction(
                        block,
                        Instruction::SetProp {
                            object: obj_dest,
                            key: key_dest,
                            value: nested_obj,
                        },
                    );
                }
            }
        }

        Ok(obj_dest)
    }

    pub(crate) fn lower_module_decl_into_object(
        &mut self,
        module_decl: &swc_ast::ModuleDecl,
        obj_dest: ValueId,
        block: BasicBlockId,
    ) -> Result<(), LoweringError> {
        match module_decl {
            swc_ast::ModuleDecl::ExportDecl(export_decl) => {
                let decl_name = match &export_decl.decl {
                    swc_ast::Decl::Fn(fn_decl) => Some(fn_decl.ident.sym.to_string()),
                    swc_ast::Decl::Var(var_decl) => {
                        var_decl.decls.first().and_then(|d| {
                            match &d.name {
                                swc_ast::Pat::Ident(ident) => Some(ident.id.sym.to_string()),
                                _ => None,
                            }
                        })
                    }
                    swc_ast::Decl::TsEnum(ts_enum) => Some(ts_enum.id.sym.to_string()),
                    swc_ast::Decl::TsModule(ts_module) => {
                        match &ts_module.id {
                            swc_ast::TsModuleName::Ident(ident) => Some(ident.sym.to_string()),
                            _ => None,
                        }
                    }
                    _ => None,
                };
                self.lower_stmt(
                    &swc_ast::Stmt::Decl(export_decl.decl.clone()),
                    StmtFlow::Open(block),
                )?;
                if let Some(name) = decl_name {
                    if let Ok(scope_id) = self.scopes.resolve_scope_id(&name) {
                        let ir_name = format!("${scope_id}.{name}");
                        let val = self.alloc_value();
                        self.current_function.append_instruction(
                            block,
                            Instruction::LoadVar {
                                dest: val,
                                name: ir_name,
                            },
                        );
                        let key_const = self.module.add_constant(Constant::String(name));
                        let key_dest = self.alloc_value();
                        self.current_function.append_instruction(
                            block,
                            Instruction::Const {
                                dest: key_dest,
                                constant: key_const,
                            },
                        );
                        self.current_function.append_instruction(
                            block,
                            Instruction::SetProp {
                                object: obj_dest,
                                key: key_dest,
                                value: val,
                            },
                        );
                    }
                }
            }

            _ => {}
        }
        Ok(())
    }

    // ── using 声明 (Explicit Resource Management) ────────────────────────────

    pub(crate) fn lower_using_decl(
        &mut self,
        using_decl: &swc_ast::UsingDecl,
        flow: StmtFlow,
    ) -> Result<StmtFlow, LoweringError> {
        let block = self.ensure_open(flow)?;

        for declarator in &using_decl.decls {
            let mut names = Vec::new();
            Self::extract_pat_bindings(&[declarator.name.clone()], &mut names);
            for name in names {
                let scope_id = self
                    .scopes
                    .resolve_scope_id(&name)
                    .map_err(|msg| self.error(using_decl.span, msg))?;
                let ir_name = format!("${scope_id}.{name}");

                // 降低初始化表达式
                if let Some(init_expr) = &declarator.init {
                    let value = self.lower_expr(init_expr, block)?;
                    self.current_function.append_instruction(
                        block,
                        Instruction::StoreVar {
                            name: ir_name.clone(),
                            value,
                        },
                    );
                }

                // 标记已初始化
                let _ = self.scopes.mark_initialised(&name);

                // 记录 using 变量
                self.active_using_vars.push(ActiveUsingVar {
                    ir_name,
                    is_async: using_decl.is_await,
                });
            }
        }

        Ok(StmtFlow::Open(block))
    }

    pub(crate) fn emit_using_disposal(&mut self, block: BasicBlockId) -> BasicBlockId {
        if self.active_using_vars.is_empty() {
            return block;
        }

        // Clone vars to avoid borrow checker issues
        let vars = self.active_using_vars.clone();
        let mut current_block = block;
        for var in vars.iter().rev() {
            // 1. LoadVar
            let val = self.alloc_value();
            self.current_function.append_instruction(
                current_block,
                Instruction::LoadVar {
                    dest: val,
                    name: var.ir_name.clone(),
                },
            );

            // 2. 检查值不是 null/undefined（用条件分支跳过 dispose）
            let skip_block = self.current_function.new_block();
            let dispose_block = self.current_function.new_block();
            let merge_block = self.current_function.new_block();

            // 检查是否为 null 或 undefined
            let is_nullish = self.alloc_value();
            let undef_const = self.module.add_constant(Constant::Undefined);
            let undef_val = self.alloc_value();
            self.current_function.append_instruction(
                current_block,
                Instruction::Const {
                    dest: undef_val,
                    constant: undef_const,
                },
            );
            // Compare with undefined first
            self.current_function.append_instruction(
                current_block,
                Instruction::Compare {
                    dest: is_nullish,
                    op: CompareOp::StrictEq,
                    lhs: val,
                    rhs: undef_val,
                },
            );
            // Branch: if is_nullish → skip, else check null
            self.current_function.set_terminator(
                current_block,
                Terminator::Branch {
                    condition: is_nullish,
                    true_block: skip_block,
                    false_block: dispose_block,
                },
            );

            // In dispose_block: get @@dispose / @@asyncDispose and call it
            let symbol_idx = if var.is_async { WK_SYMBOL_ASYNC_DISPOSE } else { WK_SYMBOL_DISPOSE };
            let symbol_const = self.module.add_constant(Constant::Number(symbol_idx as f64));
            let symbol_val = self.alloc_value();
            self.current_function.append_instruction(
                dispose_block,
                Instruction::Const {
                    dest: symbol_val,
                    constant: symbol_const,
                },
            );
            let wk_sym = self.alloc_value();
            self.current_function.append_instruction(
                dispose_block,
                Instruction::CallBuiltin {
                    dest: Some(wk_sym),
                    builtin: Builtin::SymbolWellKnown,
                    args: vec![symbol_val],
                },
            );

            // obj[Symbol.dispose]
            let dispose_method = self.alloc_value();
            self.current_function.append_instruction(
                dispose_block,
                Instruction::GetProp {
                    dest: dispose_method,
                    object: val,
                    key: wk_sym,
                },
            );

            // Call dispose method with obj as this
            self.current_function.append_instruction(
                dispose_block,
                Instruction::Call {
                    dest: None,
                    callee: dispose_method,
                    this_val: val,
                    args: vec![],
                },
            );

            self.current_function.set_terminator(
                dispose_block,
                Terminator::Jump {
                    target: merge_block,
                },
            );

            // skip_block: just jump to merge
            self.current_function.set_terminator(
                skip_block,
                Terminator::Jump {
                    target: merge_block,
                },
            );

            current_block = merge_block;
        }

        current_block
    }

}
