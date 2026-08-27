//! with 分派的调用侧：裸标识符 callee 的解析与 this 绑定（§9.1.1.2.10）。

use super::*;
use with_reads::WithFallback;

impl Lowerer {
    /// 裸标识符调用经 with 分派：命中 with 对象时 callee 取对象属性且
    /// this 绑定为该对象（WithBaseObject）；未命中回退静态解析、this=undefined。
    /// `eval` 名字未命中时回退 direct eval（含 with 层的 ScopeRecord）。
    pub(crate) fn lower_with_ident_call(
        &mut self,
        call: &swc_ast::CallExpr,
        ident: &swc_ast::Ident,
        crossed: &[usize],
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        if ident.sym.as_ref() == "eval" && self.scopes.lookup("eval").is_err() {
            return self.lower_with_eval_call(call, crossed, block);
        }
        let (callee, this_val, post) = self.lower_with_callee_resolution(ident, crossed, block)?;
        let (dest, end_block) = self.emit_with_dynamic_call(call, callee, this_val, post)?;
        self.expr_merge_block = Some(end_block);
        Ok(dest)
    }

    /// 解析 callee 与 this：返回 `(callee, this, 落点 block)`。
    pub(crate) fn lower_with_callee_resolution(
        &mut self,
        ident: &swc_ast::Ident,
        crossed: &[usize],
        block: BasicBlockId,
    ) -> Result<(ValueId, ValueId, BasicBlockId), LoweringError> {
        let name = ident.sym.to_string();
        let (base, post) = self.lower_with_resolution_chain(&name, crossed, block)?;
        let (miss, hit) = self.branch_on_with_base(base, post);

        // 命中：callee = [[Get]](base, name)，this = base。
        let key = self.append_string_const(hit, &name);
        let hit_callee = self.alloc_value();
        self.current_function.append_instruction(
            hit,
            Instruction::GetProp {
                dest: hit_callee,
                object: base,
                key,
            },
        );
        let hit_end = self.fork_or_defer_exception_branch(hit, hit_callee)?;

        // 未命中：静态解析 callee，this = undefined。
        let mut miss_end = miss;
        let (miss_callee, miss_open) = match self.with_read_fallback_kind(&name) {
            WithFallback::Static => {
                let value = self.lower_ident_static(ident, miss)?;
                self.resolve_expr_continuations(&mut miss_end);
                (value, true)
            }
            WithFallback::Undeclared => {
                // 未声明名查运行时全局对象（隐式全局），缺失才 ReferenceError。
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
        let miss_this = self.append_undefined_const(miss_end);

        // 双 Phi 合并 callee 与 this（同块多个 Phi 由 codegen 按块首连续消费）。
        let merge = self.current_function.new_block();
        self.current_function
            .set_terminator(hit_end, Terminator::Jump { target: merge });
        let mut callee_sources = vec![PhiSource {
            predecessor: hit_end,
            value: hit_callee,
        }];
        let mut this_sources = vec![PhiSource {
            predecessor: hit_end,
            value: base,
        }];
        if miss_open {
            self.current_function
                .set_terminator(miss_end, Terminator::Jump { target: merge });
            callee_sources.push(PhiSource {
                predecessor: miss_end,
                value: miss_callee,
            });
            this_sources.push(PhiSource {
                predecessor: miss_end,
                value: miss_this,
            });
        }
        let callee = self.alloc_value();
        self.current_function.append_instruction(
            merge,
            Instruction::Phi {
                dest: callee,
                sources: callee_sources,
            },
        );
        let this_val = self.alloc_value();
        self.current_function.append_instruction(
            merge,
            Instruction::Phi {
                dest: this_val,
                sources: this_sources,
            },
        );
        let post = self.current_function.new_block();
        self.current_function
            .set_terminator(merge, Terminator::Jump { target: post });
        Ok((callee, this_val, post))
    }

    /// 发射通用动态调用（spread → FuncApply，否则 Call），
    /// 返回 `(结果, 落点 block)`。
    pub(crate) fn emit_with_dynamic_call(
        &mut self,
        call: &swc_ast::CallExpr,
        callee: ValueId,
        this_val: ValueId,
        block: BasicBlockId,
    ) -> Result<(ValueId, BasicBlockId), LoweringError> {
        let mut call_block = block;
        let dest;
        if Self::call_args_have_spread(&call.args) {
            let (args_array, end_block) = self.lower_call_args_to_array(&call.args, call_block)?;
            call_block = end_block;
            dest = self.alloc_value();
            self.current_function.append_instruction(
                call_block,
                Instruction::CallBuiltin {
                    dest: Some(dest),
                    builtin: Builtin::FuncApply,
                    args: vec![callee, this_val, args_array],
                },
            );
        } else {
            let mut args = Vec::with_capacity(call.args.len());
            for arg in &call.args {
                args.push(self.lower_call_operand_then_continue(&arg.expr, &mut call_block)?);
            }
            dest = self.alloc_value();
            self.current_function.append_instruction(
                call_block,
                Instruction::Call {
                    dest: Some(dest),
                    callee,
                    this_val,
                    args,
                },
            );
        }
        Ok((dest, call_block))
    }

    /// `with(o){ eval(...) }`：o 提供 `eval` 属性时按普通函数调用（this=o），
    /// 未提供时保持 direct eval 语义（ScopeRecord 携带 with 层）。
    fn lower_with_eval_call(
        &mut self,
        call: &swc_ast::CallExpr,
        crossed: &[usize],
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let (base, post) = self.lower_with_resolution_chain("eval", crossed, block)?;
        let (miss, hit) = self.branch_on_with_base(base, post);

        let key = self.append_string_const(hit, "eval");
        let hit_callee = self.alloc_value();
        self.current_function.append_instruction(
            hit,
            Instruction::GetProp {
                dest: hit_callee,
                object: base,
                key,
            },
        );
        let hit_entry = self.fork_or_defer_exception_branch(hit, hit_callee)?;
        let (hit_val, hit_end) = self.emit_with_dynamic_call(call, hit_callee, base, hit_entry)?;

        let (miss_val, miss_end) = self.lower_direct_eval_call(call, miss)?;

        let (result, out) = self.merge_with_dispatch_results(&[
            (hit_end, hit_val, true),
            (miss_end, miss_val, true),
        ]);
        self.expr_merge_block = Some(out);
        Ok(result)
    }
}
