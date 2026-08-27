//! with 分派的标识符读取侧：读值、typeof、delete。

use super::*;

/// with 分派链未命中时静态回退的类别。
pub(crate) enum WithFallback {
    /// 名字可静态解析（含捕获 / builtin global / eval 桥接 / 模块导入），
    /// 沿用原静态降级路径。
    Static,
    /// 静态不可解析：运行时 ReferenceError（with 对象可能在运行时提供绑定，
    /// 编译期不得拒绝）。
    Undeclared,
    /// 同函数直线 TDZ 读取：静态路径本会编译期拒绝，with 遮蔽下改为运行时
    /// ReferenceError（命中 with 对象时不抛）。
    Tdz,
}

impl Lowerer {
    /// 未声明名的运行时全局回退（与 eval 桥接的 outer=global 语义一致）：
    /// 读全局对象属性；属性值为 undefined 且属性不存在时抛 ReferenceError。
    /// 返回 `(读到的值, 落点 block)`。
    pub(crate) fn lower_with_global_read(
        &mut self,
        name: &str,
        block: BasicBlockId,
    ) -> Result<(ValueId, BasicBlockId), LoweringError> {
        let (global, key, loaded) = self.emit_global_prop_load(name, block);
        let undef = self.append_undefined_const(block);
        let is_missing = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Compare {
                dest: is_missing,
                op: CompareOp::StrictEq,
                lhs: loaded,
                rhs: undef,
            },
        );
        let has_check = self.current_function.new_block();
        let merge = self.current_function.new_block();
        self.current_function.set_terminator(
            block,
            Terminator::Branch {
                condition: is_missing,
                true_block: has_check,
                false_block: merge,
            },
        );
        // 属性值为 undefined：区分「存在但为 undefined」与「不存在 → 未解析引用」。
        let has = self.alloc_value();
        self.current_function.append_instruction(
            has_check,
            Instruction::CallBuiltin {
                dest: Some(has),
                builtin: Builtin::In,
                args: vec![global, key],
            },
        );
        let throw_block = self.current_function.new_block();
        self.current_function.set_terminator(
            has_check,
            Terminator::Branch {
                condition: has,
                true_block: merge,
                false_block: throw_block,
            },
        );
        let _ = self.emit_runtime_error_throw(
            throw_block,
            Builtin::ReferenceErrorConstructor,
            &format!("{name} is not defined"),
        )?;
        Ok((loaded, merge))
    }

    /// 发射全局对象属性读取，返回 `(全局对象, key, 读到的值)`。
    fn emit_global_prop_load(
        &mut self,
        name: &str,
        block: BasicBlockId,
    ) -> (ValueId, ValueId, ValueId) {
        let global = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::LoadVar {
                dest: global,
                name: "$0.$global".to_string(),
            },
        );
        let key = self.append_string_const(block, name);
        let loaded = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::GetProp {
                dest: loaded,
                object: global,
                key,
            },
        );
        (global, key, loaded)
    }

    /// 判定 with 未命中回退的处理类别。
    pub(crate) fn with_read_fallback_kind(&self, name: &str) -> WithFallback {
        if self.eval_scope_bridge_active() {
            // eval 桥接对自由名走 EvalGetBinding，运行时自行报 ReferenceError。
            return WithFallback::Static;
        }
        if let Some(module_id) = self.current_module_id
            && (self
                .static_namespace_import_objects
                .contains_key(&(module_id, name.to_string()))
                || self
                    .import_aliases
                    .contains_key(&(module_id, name.to_string())))
        {
            return WithFallback::Static;
        }
        if name == "eval" {
            return WithFallback::Static;
        }
        match self.lookup_binding_for_read(name) {
            Ok(_) => WithFallback::Static,
            Err(msg) if msg.starts_with("undeclared identifier") => {
                if is_builtin_global(name) {
                    WithFallback::Static
                } else {
                    WithFallback::Undeclared
                }
            }
            Err(_) => {
                if self.runtime_tdz_binding(name).is_some() {
                    WithFallback::Static
                } else {
                    WithFallback::Tdz
                }
            }
        }
    }

    /// with 分派读取：命中 with 对象经 [[Get]]，未命中回退静态路径。
    pub(crate) fn lower_with_ident_read(
        &mut self,
        ident: &swc_ast::Ident,
        crossed: &[usize],
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let (_, result, out) = self.lower_with_read_resolved(ident, crossed, block)?;
        self.expr_merge_block = Some(out);
        Ok(result)
    }

    /// with 分派读取（保留 base）：返回 `(base, 读到的值, 落点 block)`。
    /// 复合赋值/update 用同一 base 完成后续写回（规范：Reference 只解析一次，
    /// RHS 求值期间对象属性变化不影响写回基座）。
    pub(crate) fn lower_with_read_resolved(
        &mut self,
        ident: &swc_ast::Ident,
        crossed: &[usize],
        block: BasicBlockId,
    ) -> Result<(ValueId, ValueId, BasicBlockId), LoweringError> {
        let name = ident.sym.to_string();
        let (base, post) = self.lower_with_resolution_chain(&name, crossed, block)?;
        let (miss, hit) = self.branch_on_with_base(base, post);

        // 命中：GetValue → [[Get]]（getter / proxy get 异常传播）。
        let key = self.append_string_const(hit, &name);
        let hit_val = self.alloc_value();
        self.current_function.append_instruction(
            hit,
            Instruction::GetProp {
                dest: hit_val,
                object: base,
                key,
            },
        );
        let hit_end = self.fork_or_defer_exception_branch(hit, hit_val)?;

        // 未命中：静态回退、运行时全局回退或运行时 ReferenceError。
        let mut miss_end = miss;
        let (miss_val, miss_open) = match self.with_read_fallback_kind(&name) {
            WithFallback::Static => {
                let value = self.lower_ident_static(ident, miss)?;
                self.resolve_expr_continuations(&mut miss_end);
                (value, true)
            }
            WithFallback::Undeclared => {
                // 未声明名查运行时全局对象（隐式全局可在运行期出现），
                // 属性不存在才抛 ReferenceError。
                let (value, end) = self.lower_with_global_read(&name, miss)?;
                miss_end = end;
                (value, true)
            }
            WithFallback::Tdz => {
                let dummy = self.emit_runtime_error_throw(
                    miss,
                    Builtin::ReferenceErrorConstructor,
                    &format!("Cannot access '{name}' before initialization"),
                )?;
                (dummy, false)
            }
        };

        let (result, out) = self.merge_with_dispatch_results(&[
            (hit_end, hit_val, true),
            (miss_end, miss_val, miss_open),
        ]);
        Ok((base, result, out))
    }

    /// with 分派 typeof：未解析名产出 "undefined" 而非抛错（§13.5.3）。
    pub(crate) fn lower_with_typeof(
        &mut self,
        ident: &swc_ast::Ident,
        crossed: &[usize],
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let name = ident.sym.to_string();
        let (base, post) = self.lower_with_resolution_chain(&name, crossed, block)?;
        let (miss, hit) = self.branch_on_with_base(base, post);

        let key = self.append_string_const(hit, &name);
        let loaded = self.alloc_value();
        self.current_function.append_instruction(
            hit,
            Instruction::GetProp {
                dest: loaded,
                object: base,
                key,
            },
        );
        let hit_end = self.fork_or_defer_exception_branch(hit, loaded)?;
        let hit_val = self.alloc_value();
        self.current_function.append_instruction(
            hit_end,
            Instruction::CallBuiltin {
                dest: Some(hit_val),
                builtin: Builtin::TypeOf,
                args: vec![loaded],
            },
        );

        let mut miss_end = miss;
        let (miss_val, miss_open) = if self.eval_scope_bridge_active()
            && self.scopes.lookup(&name).is_err()
        {
            let value = self.lower_eval_typeof_binding(&name, miss)?;
            self.resolve_expr_continuations(&mut miss_end);
            (value, true)
        } else {
            match self.with_read_fallback_kind(&name) {
                WithFallback::Static => {
                    let value = self.lower_ident_static(ident, miss)?;
                    self.resolve_expr_continuations(&mut miss_end);
                    let ty = self.alloc_value();
                    self.current_function.append_instruction(
                        miss_end,
                        Instruction::CallBuiltin {
                            dest: Some(ty),
                            builtin: Builtin::TypeOf,
                            args: vec![value],
                        },
                    );
                    (ty, true)
                }
                WithFallback::Undeclared => {
                    // typeof 未解析名不抛错（§13.5.3）：读运行时全局对象属性，
                    // 缺失时 GetProp 产出 undefined → "undefined"。
                    let (_, _, loaded) = self.emit_global_prop_load(&name, miss_end);
                    let ty = self.alloc_value();
                    self.current_function.append_instruction(
                        miss_end,
                        Instruction::CallBuiltin {
                            dest: Some(ty),
                            builtin: Builtin::TypeOf,
                            args: vec![loaded],
                        },
                    );
                    (ty, true)
                }
                WithFallback::Tdz => {
                    let dummy = self.emit_runtime_error_throw(
                        miss,
                        Builtin::ReferenceErrorConstructor,
                        &format!("Cannot access '{name}' before initialization"),
                    )?;
                    (dummy, false)
                }
            }
        };

        let (result, out) = self.merge_with_dispatch_results(&[
            (hit_end, hit_val, true),
            (miss_end, miss_val, miss_open),
        ]);
        self.expr_merge_block = Some(out);
        Ok(result)
    }

    /// with 分派 delete：命中对象环境记录时执行 [[Delete]]（§9.1.1.2.7），
    /// 未命中沿用引擎既有的裸标识符 delete 语义（恒 true）。
    pub(crate) fn lower_with_delete(
        &mut self,
        ident: &swc_ast::Ident,
        crossed: &[usize],
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let name = ident.sym.to_string();
        let (base, post) = self.lower_with_resolution_chain(&name, crossed, block)?;
        let (miss, hit) = self.branch_on_with_base(base, post);

        let key = self.append_string_const(hit, &name);
        let deleted = self.alloc_value();
        self.current_function.append_instruction(
            hit,
            Instruction::DeleteProp {
                dest: deleted,
                object: base,
                key,
            },
        );
        let hit_end = self.fork_or_defer_exception_branch(hit, deleted)?;

        // 未命中：未声明名对隐式全局属性执行 [[Delete]]（缺失属性也返回 true），
        // 静态绑定沿用引擎既有裸标识符 delete 语义（恒 true）。
        let miss_val = if matches!(self.with_read_fallback_kind(&name), WithFallback::Undeclared) {
            let global = self.alloc_value();
            self.current_function.append_instruction(
                miss,
                Instruction::LoadVar {
                    dest: global,
                    name: "$0.$global".to_string(),
                },
            );
            let miss_key = self.append_string_const(miss, &name);
            let deleted_global = self.alloc_value();
            self.current_function.append_instruction(
                miss,
                Instruction::DeleteProp {
                    dest: deleted_global,
                    object: global,
                    key: miss_key,
                },
            );
            deleted_global
        } else {
            let true_const = self.module.add_constant(Constant::Bool(true));
            let miss_val = self.alloc_value();
            self.current_function.append_instruction(
                miss,
                Instruction::Const {
                    dest: miss_val,
                    constant: true_const,
                },
            );
            miss_val
        };

        let (result, out) = self
            .merge_with_dispatch_results(&[(hit_end, deleted, true), (miss, miss_val, true)]);
        self.expr_merge_block = Some(out);
        Ok(result)
    }
}
