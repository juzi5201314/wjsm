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
    pub(crate) fn lower_var_decl(
        &mut self,
        var_decl: &swc_ast::VarDecl,
        flow: StmtFlow,
    ) -> Result<StmtFlow, LoweringError> {
        let mut block = self.ensure_open(flow)?;
        let kind = match var_decl.kind {
            swc_ast::VarDeclKind::Var => VarKind::Var,
            swc_ast::VarDeclKind::Let => VarKind::Let,
            swc_ast::VarDeclKind::Const => VarKind::Const,
        };

        for declarator in &var_decl.decls {
            if let Some(init) = &declarator.init {
                let value = self.lower_expr(init, block)?;
                self.lower_destructure_pattern(&declarator.name, value, block, kind)?;
                block = self.resolve_store_block(block);
            } else {
                if matches!(kind, VarKind::Const) {
                    return Err(self.error(var_decl.span, "const declarations must be initialised"));
                }
                if matches!(kind, VarKind::Var) {
                    // var without init: already initialised in pre-scan, skip
                    let mut names = Vec::new();
                    Self::extract_pat_bindings(&[declarator.name.clone()], &mut names);
                    for name in names {
                        self.scopes
                            .mark_initialised(&name)
                            .map_err(|msg| self.error(var_decl.span, msg))?;
                    }
                    continue;
                }

                // `let x;`（非解构）或 `let [a, b];` — 初始化为 undefined
                let undef_cid = self.module.add_constant(Constant::Undefined);
                let undef_val = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::Const {
                        dest: undef_val,
                        constant: undef_cid,
                    },
                );
                self.lower_destructure_pattern(&declarator.name, undef_val, block, kind)?;
            }
        }

        Ok(StmtFlow::Open(block))
    }

    // ── Destructuring pattern lowering ──────────────────────────────────────

    /// 构建函数参数的 param_ir_names 并声明变量。
    ///   - 简单参数 (x): 直接使用变量名
    ///   - 简单参数 + 默认值 (x = 1): 直接使用变量名
    ///   - 解构参数 ({a}) / 解构+默认值 ([a] = [1]): 使用临时变量名
    pub(crate) fn build_param_ir_names(
        &mut self,
        params: &[swc_ast::Param],
        env_scope_id: usize,
        this_scope_id: usize,
    ) -> Result<Vec<String>, LoweringError> {
        self.build_param_ir_names_impl(
            params.iter().map(|p| &p.pat).collect::<Vec<_>>().as_slice(),
            env_scope_id,
            this_scope_id,
        )
    }

    /// 为箭头函数的参数（Vec<Pat>）构建 param_ir_names.
    pub(crate) fn build_arrow_param_ir_names(
        &mut self,
        params: &[swc_ast::Pat],
        env_scope_id: usize,
        this_scope_id: usize,
    ) -> Result<Vec<String>, LoweringError> {
        self.build_param_ir_names_impl(
            params.iter().collect::<Vec<_>>().as_slice(),
            env_scope_id,
            this_scope_id,
        )
    }

    pub(crate) fn build_param_ir_names_impl(
        &mut self,
        pats: &[&swc_ast::Pat],
        env_scope_id: usize,
        this_scope_id: usize,
    ) -> Result<Vec<String>, LoweringError> {
        let mut ir_names: Vec<String> = vec![
            format!("${env_scope_id}.$env"),
            format!("${this_scope_id}.$this"),
        ];

        for pat in pats {
            match pat {
                swc_ast::Pat::Ident(binding) => {
                    let name = binding.id.sym.to_string();
                    let scope_id = self
                        .scopes
                        .declare(&name, VarKind::Let, true)
                        .map_err(|msg| self.error(binding.span(), msg))?;
                    ir_names.push(format!("${scope_id}.{name}"));
                }
                swc_ast::Pat::Assign(assign) => match &*assign.left {
                    swc_ast::Pat::Ident(binding) => {
                        let name = binding.id.sym.to_string();
                        let scope_id = self
                            .scopes
                            .declare(&name, VarKind::Let, true)
                            .map_err(|msg| self.error(binding.span(), msg))?;
                        ir_names.push(format!("${scope_id}.{name}"));
                    }
                    _ => {
                        let temp = self.alloc_temp_name();
                        let scope_id = self
                            .scopes
                            .declare(&temp, VarKind::Let, true)
                            .map_err(|msg| self.error(assign.span, msg))?;
                        ir_names.push(format!("${scope_id}.{temp}"));
                        let mut nested = Vec::new();
                        Self::extract_pat_bindings(&[*assign.left.clone()], &mut nested);
                        for n in &nested {
                            self.scopes
                                .declare(n, VarKind::Let, true)
                                .map_err(|msg| self.error(assign.span, msg))?;
                        }
                    }
                },
                swc_ast::Pat::Rest(rest) => {
                    let mut nested = Vec::new();
                    Self::extract_pat_bindings(&[*rest.arg.clone()], &mut nested);
                    for n in &nested {
                        self.scopes
                            .declare(n, VarKind::Let, true)
                            .map_err(|msg| self.error(pat.span(), msg))?;
                    }
                }
                _ => {
                    let temp = self.alloc_temp_name();
                    let scope_id = self
                        .scopes
                        .declare(&temp, VarKind::Let, true)
                        .map_err(|msg| self.error(pat.span(), msg))?;
                    ir_names.push(format!("${scope_id}.{temp}"));
                    let mut nested = Vec::new();
                    Self::extract_pat_bindings(&[(*pat).clone()], &mut nested);
                    for n in &nested {
                        self.scopes
                            .declare(n, VarKind::Let, true)
                            .map_err(|msg| self.error(pat.span(), msg))?;
                    }
                }
            }
        }

        Ok(ir_names)
    }

    /// 在函数体入口生成参数初始化代码（默认值 + 解构）。
    pub(crate) fn emit_param_inits(
        &mut self,
        params: &[swc_ast::Param],
        param_ir_names: &[String],
        block: BasicBlockId,
    ) -> Result<BasicBlockId, LoweringError> {
        self.emit_pat_inits_impl(
            params.iter().map(|p| &p.pat).collect::<Vec<_>>().as_slice(),
            param_ir_names,
            block,
        )
    }

    pub(crate) fn emit_arrow_param_inits(
        &mut self,
        pats: &[swc_ast::Pat],
        param_ir_names: &[String],
        block: BasicBlockId,
    ) -> Result<BasicBlockId, LoweringError> {
        self.emit_pat_inits_impl(
            pats.iter().collect::<Vec<_>>().as_slice(),
            param_ir_names,
            block,
        )
    }

    pub(crate) fn emit_field_init(
        &mut self,
        block: BasicBlockId,
        this_scope_id: usize,
        field_name: &str,
        init_value: Option<&swc_ast::Expr>,
        is_private: bool,
    ) -> Result<BasicBlockId, LoweringError> {
        let key_const = self.module.add_constant(Constant::String(field_name.to_string()));
        let key_dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const { dest: key_dest, constant: key_const },
        );
        let this_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::LoadVar { dest: this_val, name: format!("${this_scope_id}.$this") },
        );
        let init_val = if let Some(value) = init_value {
            self.lower_expr(value, block)?
        } else {
            let ud_const = self.module.add_constant(Constant::Undefined);
            let ud_dest = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::Const { dest: ud_dest, constant: ud_const },
            );
            ud_dest
        };
        if is_private {
            self.current_function.append_instruction(
                block,
                Instruction::CallBuiltin {
                    dest: None,
                    builtin: Builtin::PrivateSet,
                    args: vec![this_val, key_dest, init_val],
                },
            );
        } else {
            self.current_function.append_instruction(
                block,
                Instruction::SetProp { object: this_val, key: key_dest, value: init_val },
            );
        }
        Ok(self.resolve_store_block(block))
    }

    pub(crate) fn emit_static_field_init(
        &mut self,
        block: BasicBlockId,
        ctor_dest: ValueId,
        field_name: &str,
        init_value: Option<&swc_ast::Expr>,
        is_private: bool,
    ) -> Result<(), LoweringError> {
        let key_const = self.module.add_constant(Constant::String(field_name.to_string()));
        let key_dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const { dest: key_dest, constant: key_const },
        );
        let init_val = if let Some(value) = init_value {
            self.lower_expr(value, block)?
        } else {
            let ud_const = self.module.add_constant(Constant::Undefined);
            let ud_dest = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::Const { dest: ud_dest, constant: ud_const },
            );
            ud_dest
        };
        if is_private {
            self.current_function.append_instruction(
                block,
                Instruction::CallBuiltin {
                    dest: None,
                    builtin: Builtin::PrivateSet,
                    args: vec![ctor_dest, key_dest, init_val],
                },
            );
        } else {
            self.current_function.append_instruction(
                block,
                Instruction::SetProp { object: ctor_dest, key: key_dest, value: init_val },
            );
        }
        Ok(())
    }

    pub(crate) fn emit_private_method_bind(
        &mut self,
        block: BasicBlockId,
        target_val: ValueId,
        field_name: &str,
        func_id: FunctionId,
    ) {
        let key_const = self.module.add_constant(Constant::String(field_name.to_string()));
        let key_dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const { dest: key_dest, constant: key_const },
        );
        let fn_dest = self.alloc_value();
        let fn_ref_const = self.module.add_constant(Constant::FunctionRef(func_id));
        self.current_function.append_instruction(
            block,
            Instruction::Const { dest: fn_dest, constant: fn_ref_const },
        );
        self.current_function.append_instruction(
            block,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::PrivateSet,
                args: vec![target_val, key_dest, fn_dest],
            },
        );
    }

    pub(crate) fn emit_pat_inits_impl(
        &mut self,
        pats: &[&swc_ast::Pat],
        param_ir_names: &[String],
        mut block: BasicBlockId,
    ) -> Result<BasicBlockId, LoweringError> {
        // param_ir_names[0] = $env, [1] = $this, [2..] = user params (excluding rest)
        let mut ir_name_idx: usize = 2;
        let mut regular_param_count: u32 = 0;
        for pat in pats.iter() {
            if let swc_ast::Pat::Rest(_) = pat {
                break;
            }
            regular_param_count += 1;
        }

        for pat in pats.iter() {
            if let swc_ast::Pat::Rest(rest) = pat {
                let skip = regular_param_count;
                let rest_val = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::CollectRestArgs {
                        dest: rest_val,
                        skip,
                    },
                );
                self.lower_destructure_pattern(&rest.arg, rest_val, block, VarKind::Let)?;
                block = self.resolve_store_block(block);
                break;
            }

            let ir_name = &param_ir_names[ir_name_idx];
            match pat {
                swc_ast::Pat::Ident(_) => {
                    // 简单参数无默认值：值已在 local 中，无需操作
                }
                swc_ast::Pat::Assign(assign) => {
                    let raw = self.alloc_value();
                    self.current_function.append_instruction(
                        block,
                        Instruction::LoadVar {
                            dest: raw,
                            name: ir_name.clone(),
                        },
                    );
                    let resolved = self.lower_default_value_check(raw, &assign.right, block)?;
                    let store_block = self.resolve_store_block(block);
                    self.current_function.append_instruction(
                        store_block,
                        Instruction::StoreVar {
                            name: ir_name.clone(),
                            value: resolved,
                        },
                    );

                    if !matches!(&*assign.left, swc_ast::Pat::Ident(_)) {
                        let loaded = self.alloc_value();
                        self.current_function.append_instruction(
                            store_block,
                            Instruction::LoadVar {
                                dest: loaded,
                                name: ir_name.clone(),
                            },
                        );
                        self.lower_destructure_pattern(
                            &assign.left,
                            loaded,
                            store_block,
                            VarKind::Let,
                        )?;
                    }
                }
                _ => {
                    // 解构参数
                    let raw = self.alloc_value();
                    self.current_function.append_instruction(
                        block,
                        Instruction::LoadVar {
                            dest: raw,
                            name: ir_name.clone(),
                        },
                    );
                    self.lower_destructure_pattern(pat, raw, block, VarKind::Let)?;
                }
            }
            ir_name_idx += 1;
            block = self.resolve_open_after_expr(block, self.resolve_store_block(block));
        }
        Ok(block)
    }

    /// 将解构 pattern 降低为一系列 IR 指令（GetProp/GetElem + StoreVar）。
    /// 递归处理嵌套的 Array/Object/Assign pattern。
    pub(crate) fn lower_destructure_pattern(
        &mut self,
        pat: &swc_ast::Pat,
        src_val: ValueId,
        block: BasicBlockId,
        kind: VarKind,
    ) -> Result<(), LoweringError> {
        match pat {
            swc_ast::Pat::Ident(binding) => {
                let name = binding.id.sym.to_string();
                let scope_id = self
                    .scopes
                    .resolve_scope_id(&name)
                    .map_err(|msg| self.error(pat.span(), msg))?;
                let ir_name = format!("${scope_id}.{name}");

                if matches!(kind, VarKind::Var) {
                    self.scopes
                        .mark_initialised(&name)
                        .map_err(|msg| self.error(pat.span(), msg))?;
                } else {
                    self.scopes
                        .mark_initialised(&name)
                        .map_err(|msg| self.error(pat.span(), msg))?;
                }

                let store_block = self.resolve_store_block(block);
                self.current_function.append_instruction(
                    store_block,
                    Instruction::StoreVar {
                        name: ir_name,
                        value: src_val,
                    },
                );
                self.append_eval_var_leak_if_needed(&name, kind, src_val, store_block);
            }
            swc_ast::Pat::Object(object_pat) => {
                self.lower_object_destructure(object_pat, src_val, block, kind)?;
            }
            swc_ast::Pat::Array(array_pat) => {
                self.lower_array_destructure(array_pat, src_val, block, kind)?;
            }
            swc_ast::Pat::Assign(assign_pat) => {
                let resolved = self.lower_default_value_check(src_val, &assign_pat.right, block)?;
                self.lower_destructure_pattern(&assign_pat.left, resolved, block, kind)?;
            }
            swc_ast::Pat::Rest(_) => {
                return Err(self.error(
                    pat.span(),
                    "rest element must be used as a function parameter or inside array destructuring",
                ));
            }
            swc_ast::Pat::Expr(_) | swc_ast::Pat::Invalid(_) => {}
        }
        Ok(())
    }

    /// 对象解构: `{ prop1, prop2: alias, ...rest }`
    pub(crate) fn lower_object_destructure(
        &mut self,
        object_pat: &swc_ast::ObjectPat,
        src_val: ValueId,
        mut block: BasicBlockId,
        kind: VarKind,
    ) -> Result<(), LoweringError> {
        for prop in &object_pat.props {
            match prop {
                swc_ast::ObjectPatProp::KeyValue(kv) => {
                    // { key: pattern } 或 { [computed]: pattern }
                    let key_val = self.lower_prop_name(&kv.key, block)?;
                    let dest = self.alloc_value();
                    self.current_function.append_instruction(
                        block,
                        Instruction::GetProp {
                            dest,
                            object: src_val,
                            key: key_val,
                        },
                    );
                    self.lower_destructure_pattern(&kv.value, dest, block, kind)?;
                }
                swc_ast::ObjectPatProp::Assign(assign) => {
                    // { key } 等价于 { key: key }
                    let name = assign.key.id.sym.to_string();
                    let key_const = self.module.add_constant(Constant::String(name.clone()));
                    let key_val = self.alloc_value();
                    self.current_function.append_instruction(
                        block,
                        Instruction::Const {
                            dest: key_val,
                            constant: key_const,
                        },
                    );
                    let dest = self.alloc_value();
                    self.current_function.append_instruction(
                        block,
                        Instruction::GetProp {
                            dest,
                            object: src_val,
                            key: key_val,
                        },
                    );

                    // 如果有默认值 { key = default }
                    if let Some(default_expr) = &assign.value {
                        let resolved = self.lower_default_value_check(dest, default_expr, block)?;
                        let scope_id = self
                            .scopes
                            .resolve_scope_id(&name)
                            .map_err(|msg| self.error(assign.key.span(), msg))?;
                        let ir_name = format!("${scope_id}.{name}");
                        self.scopes
                            .mark_initialised(&name)
                            .map_err(|msg| self.error(assign.key.span(), msg))?;
                        let store_block = self.resolve_store_block(block);
                        self.current_function.append_instruction(
                            store_block,
                            Instruction::StoreVar {
                                name: ir_name,
                                value: resolved,
                            },
                        );
                        self.append_eval_var_leak_if_needed(&name, kind, resolved, store_block);
                    } else {
                        self.lower_destructure_pattern(
                            &swc_ast::Pat::Ident(assign.key.clone()),
                            dest,
                            block,
                            kind,
                        )?;
                    }
                }
                swc_ast::ObjectPatProp::Rest(rest) => {
                    // { ...rest } — 使用 ObjectRest builtin
                    let rest_dest = self.alloc_value();
                    let excluded_cid = self.module.add_constant(Constant::Undefined);
                    let excluded_val = self.alloc_value();
                    self.current_function.append_instruction(
                        block,
                        Instruction::Const {
                            dest: excluded_val,
                            constant: excluded_cid,
                        },
                    );
                    self.current_function.append_instruction(
                        block,
                        Instruction::CallBuiltin {
                            dest: Some(rest_dest),
                            builtin: Builtin::ObjectRest,
                            args: vec![src_val, excluded_val],
                        },
                    );
                    self.lower_destructure_pattern(&rest.arg, rest_dest, block, kind)?;
                }
            }
            // 确保 block 指向当前可用的基本块（可能已被 lower_default_value_check 等终结）
            block = self.resolve_store_block(block);
        }
        Ok(())
    }

    /// 数组解构: `[a, b, ...rest]`
    pub(crate) fn lower_array_destructure(
        &mut self,
        array_pat: &swc_ast::ArrayPat,
        src_val: ValueId,
        mut block: BasicBlockId,
        kind: VarKind,
    ) -> Result<(), LoweringError> {
        let mut idx: i32 = 0;
        let mut has_rest = false;

        for (_i, elem) in array_pat.elems.iter().enumerate() {
            let elem = match elem {
                Some(e) => e,
                None => {
                    idx += 1;
                    continue;
                }
            };

            if let swc_ast::Pat::Rest(rest) = elem {
                has_rest = true;
                // [...rest] — 使用 iterator 协议收集剩余元素
                let rest_val =
                    self.lower_array_rest_destructure(src_val, &rest.arg, idx, block, kind)?;
                // rest.arg 可能是 Pat::Ident 或嵌套 pattern
                // 但我们已经在 lower_array_rest_destructure 中处理了
                let _ = rest_val;
            } else {
                // 普通元素 get_elem
                let index_const = self.module.add_constant(Constant::Number(idx as f64));
                let index_val = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::Const {
                        dest: index_val,
                        constant: index_const,
                    },
                );

                // 如果有默认值，用 IteratorFrom/Next/Value/Done 或简单 undefined 检查
                if let swc_ast::Pat::Assign(assign) = elem {
                    // [a = default] — 需要检查数组对应位置是否为 undefined（不是 nullish）
                    let dest = self.alloc_value();
                    self.current_function.append_instruction(
                        block,
                        Instruction::GetElem {
                            dest,
                            object: src_val,
                            index: index_val,
                        },
                    );
                    let resolved = self.lower_default_value_check(dest, &assign.right, block)?;
                    self.lower_destructure_pattern(&assign.left, resolved, block, kind)?;
                } else {
                    let dest = self.alloc_value();
                    self.current_function.append_instruction(
                        block,
                        Instruction::GetElem {
                            dest,
                            object: src_val,
                            index: index_val,
                        },
                    );
                    self.lower_destructure_pattern(elem, dest, block, kind)?;
                }
                idx += 1;
            }
            block = self.resolve_store_block(block);
        }

        if has_rest {
            // 如果使用了 rest，需要关闭 iterator
            // 在此上下文中不需要显式 IteratorClose — 已由 lower_array_rest_destructure 处理
        }

        Ok(())
    }

    /// 数组解构中的 rest 元素: `[...rest]`
    /// 使用 iterator 协议从当前位置收集剩余元素到一个新数组
    pub(crate) fn lower_array_rest_destructure(
        &mut self,
        src_val: ValueId,
        rest_pat: &swc_ast::Pat,
        skip_count: i32,
        block: BasicBlockId,
        kind: VarKind,
    ) -> Result<ValueId, LoweringError> {
        // 1. 创建迭代器
        let iter_handle = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::CallBuiltin {
                dest: Some(iter_handle),
                builtin: Builtin::IteratorFrom,
                args: vec![src_val],
            },
        );

        // 2. 跳过已处理的元素
        for _ in 0..skip_count {
            self.current_function.append_instruction(
                block,
                Instruction::CallBuiltin {
                    dest: None,
                    builtin: Builtin::IteratorNext,
                    args: vec![iter_handle],
                },
            );
        }

        // 3. 创建结果数组
        let result_arr = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::NewArray {
                dest: result_arr,
                capacity: 0,
            },
        );

        // 4. 循环收集剩余元素
        let header = self.current_function.new_block();
        let loop_body = self.current_function.new_block();
        let exit = self.current_function.new_block();

        self.current_function
            .set_terminator(block, Terminator::Jump { target: header });

        // header: 检查 iterator done
        let done_val = self.alloc_value();
        self.current_function.append_instruction(
            header,
            Instruction::CallBuiltin {
                dest: Some(done_val),
                builtin: Builtin::IteratorDone,
                args: vec![iter_handle],
            },
        );
        let not_done = self.alloc_value();
        self.current_function.append_instruction(
            header,
            Instruction::Unary {
                dest: not_done,
                op: UnaryOp::Not,
                value: done_val,
            },
        );
        self.current_function.set_terminator(
            header,
            Terminator::Branch {
                condition: not_done,
                true_block: loop_body,
                false_block: exit,
            },
        );

        // body: 获取值，push 到数组
        let elem_val = self.alloc_value();
        self.current_function.append_instruction(
            loop_body,
            Instruction::CallBuiltin {
                dest: Some(elem_val),
                builtin: Builtin::IteratorValue,
                args: vec![iter_handle],
            },
        );
        self.current_function.append_instruction(
            loop_body,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::ArrayPush,
                args: vec![result_arr, elem_val],
            },
        );
        // 调用 iterator next 前进
        self.current_function.append_instruction(
            loop_body,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::IteratorNext,
                args: vec![iter_handle],
            },
        );
        self.current_function
            .set_terminator(loop_body, Terminator::Jump { target: header });

        // exit: 关闭迭代器
        self.current_function.append_instruction(
            exit,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::IteratorClose,
                args: vec![iter_handle],
            },
        );

        // 5. 将结果数组赋值给 rest pattern
        self.lower_destructure_pattern(rest_pat, result_arr, exit, kind)?;

        Ok(result_arr)
    }

    /// 默认值检查: `x = default`
    /// 语义：如果 value === undefined，使用 default 表达式；否则保留原值。
    pub(crate) fn lower_default_value_check(
        &mut self,
        value: ValueId,
        default_expr: &swc_ast::Expr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        // Compare { op: StrictEq, lhs: value, rhs: Undefined }
        let undef_cid = self.module.add_constant(Constant::Undefined);
        let undef_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: undef_val,
                constant: undef_cid,
            },
        );
        let is_undef = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Compare {
                dest: is_undef,
                op: CompareOp::StrictEq,
                lhs: value,
                rhs: undef_val,
            },
        );

        // Branch
        let then_block = self.current_function.new_block();
        let else_block = self.current_function.new_block();
        let merge_block = self.current_function.new_block();
        self.current_function.set_terminator(
            block,
            Terminator::Branch {
                condition: is_undef,
                true_block: then_block,
                false_block: else_block,
            },
        );

        // then_block: 求值默认表达式
        let default_val = self.lower_expr(default_expr, then_block)?;
        self.current_function.set_terminator(
            then_block,
            Terminator::Jump {
                target: merge_block,
            },
        );

        // else_block: 保留原值
        self.current_function.set_terminator(
            else_block,
            Terminator::Jump {
                target: merge_block,
            },
        );

        // merge_block: Phi
        let result = self.alloc_value();
        self.current_function.append_instruction(
            merge_block,
            Instruction::Phi {
                dest: result,
                sources: vec![
                    PhiSource {
                        predecessor: then_block,
                        value: default_val,
                    },
                    PhiSource {
                        predecessor: else_block,
                        value,
                    },
                ],
            },
        );

        Ok(result)
    }

}
