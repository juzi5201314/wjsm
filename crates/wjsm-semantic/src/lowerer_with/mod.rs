//! WithStatement（§14.11）的降级与对象环境记录分派。
//!
//! `with (expr) body` 把 `ToObject(expr)` 作为对象环境记录挂到作用域链上：
//! body 内解析穿越 With 作用域的标识符必须在运行时先按 `HasBinding`
//! （HasProperty + `@@unscopables` 过滤，§9.1.1.2.1）逐层探测 with 对象，
//! 命中则读/写/调用都以该对象为基座（含调用的 this 绑定，§9.1.1.2.10），
//! 全链未命中才回退静态解析。
//!
//! 实现要点：
//! - 每个 With 作用域持有一个合成绑定 [`WITH_OBJECT_BINDING`]（名字含 `%`，
//!   与用户标识符无冲突）保存 with 对象；嵌套函数经现有闭包捕获机制拿到它，
//!   使 `with(o){ function f(){ return x } }` 的 f 捕获 with 环境。
//! - 分派链与规范 GetIdentifierReference 一致：每层至多一次 has 探测，
//!   命中即短路；Proxy has trap / `@@unscopables` getter 异常原样传播。
//! - 严格模式代码含 with 是 early error（§14.11.1），降级期直接拒绝。
//! - direct eval 互操作经 ScopeRecord 的 with 层（`ScopeRecordAddWithLayer`）
//!   由宿主 EvalGet/Set/HasBinding 在静态绑定与 with 对象之间正确插层。

use super::*;

pub(crate) mod strict_check;
mod with_calls;
mod with_reads;
mod with_writes;

pub(crate) use strict_check::find_with_in_strict_code;

/// With 作用域中保存 with 对象的合成绑定名。
/// `%` 不是合法标识符字符，用户代码永远无法遮蔽或读取该绑定。
pub(crate) const WITH_OBJECT_BINDING: &str = "%with";

impl Lowerer {
    /// 标识符解析需要穿越的 With 作用域（由内到外）；无需分派时为空。
    /// 程序不含 with 语句时零成本（不遍历作用域链）。
    pub(crate) fn with_scopes_for_ident(&self, name: &str) -> Vec<usize> {
        if self.with_scope_count == 0 {
            return Vec::new();
        }
        self.scopes.with_scopes_crossed(name).1
    }

    /// 当前作用域链上包围本站点的全部 With 作用域（由内到外），
    /// 供 direct eval 构建 ScopeRecord 的 with 层。
    pub(crate) fn enclosing_with_scopes(&self) -> Vec<usize> {
        if self.with_scope_count == 0 {
            return Vec::new();
        }
        let mut result = Vec::new();
        let mut cursor = Some(self.scopes.current);
        while let Some(id) = cursor {
            let scope = &self.scopes.arenas[id];
            if matches!(scope.kind, ScopeKind::With) {
                result.push(id);
            }
            cursor = scope.parent;
        }
        result
    }

    /// with 分派链的异常检查分叉：async / async-generator 状态机体内同样
    /// 立即分叉——分派链自身不含 await/yield，块结构与
    /// `async_await_yield` 中宿主 builtin 调用后的 IsException 分支模式一致，
    /// 经 `emit_throw_value` 走 promise rejection 路径；否则 Branch 会把
    /// TAG_EXCEPTION 哨兵当作 has 结果消费，异常被静默吞掉。
    /// 规范拥有者（动态 import 等）压制期间仍延迟交拥有者处理。
    pub(crate) fn fork_with_dispatch_exception(
        &mut self,
        block: BasicBlockId,
        value: ValueId,
    ) -> Result<BasicBlockId, LoweringError> {
        if self.exception_fork_suppressed() {
            return self.fork_or_defer_exception_branch(block, value);
        }
        self.lower_value_exception_branch(block, value)
    }

    /// WithStatement 语句入口。严格模式 early error（§14.11.1）已由降级前的
    /// AST 校验 [`validate_with_strict`] 统一覆盖（含函数级指令与类体）。
    pub(crate) fn lower_with(
        &mut self,
        with_stmt: &swc_ast::WithStmt,
        flow: StmtFlow,
    ) -> Result<StmtFlow, LoweringError> {
        let block = self.ensure_open(flow)?;
        let mut current_block = block;
        let obj_val = self.lower_call_operand_then_continue(&with_stmt.obj, &mut current_block)?;

        // §14.11.2 步骤 2：ToObject(value)。null/undefined 抛 TypeError，
        // 原语装箱为包装对象；异常按表达式级分叉传播。
        let coerced = self.alloc_value();
        self.current_function.append_instruction(
            current_block,
            Instruction::CallBuiltin {
                dest: Some(coerced),
                builtin: Builtin::WithToObject,
                args: vec![obj_val],
            },
        );
        current_block = self.fork_with_dispatch_exception(current_block, coerced)?;

        // 对象环境记录：合成绑定保存 with 对象，嵌套闭包按常规捕获协议访问。
        self.with_scope_count += 1;
        self.scopes.push_scope(ScopeKind::With);
        let with_scope_id = self.scopes.current_scope_id();
        self.scopes
            .declare(WITH_OBJECT_BINDING, VarKind::Const, true)
            .map_err(|msg| self.error(with_stmt.span, msg))?;
        let binding = CapturedBinding::new(WITH_OBJECT_BINDING, with_scope_id);
        let body_entry =
            self.store_binding_value(current_block, &binding, coerced, with_stmt.span, true)?;

        // 完成值：lower_stmt 入口已按 UpdateEmpty(·, undefined) 重置槽，
        // body 语句自行写槽，与 eval 完成值内存槽模型天然兼容。
        let body_flow = self.lower_stmt(&with_stmt.body, StmtFlow::Open(body_entry));
        self.scopes.pop_scope();
        body_flow
    }

    /// 消化表达式降级留下的续接标志（new/await/eval/merge），推进插入点。
    /// 与 `lower_expr_then_continue` 的续接循环一致，供分派链内部使用。
    pub(crate) fn resolve_expr_continuations(&mut self, block: &mut BasicBlockId) {
        while self.eval_continue_block.is_some()
            || self.new_expr_continue_block.is_some()
            || self.await_continue_block.is_some()
            || self.expr_merge_block.is_some()
        {
            let next = self.resolve_store_block(*block);
            if next != *block {
                *block = next;
            }
        }
    }

    /// 读取指定 With 作用域的 with 对象（本函数局部或经闭包环境捕获）。
    pub(crate) fn load_with_object(
        &mut self,
        block: &mut BasicBlockId,
        scope_id: usize,
    ) -> Result<ValueId, LoweringError> {
        let binding = CapturedBinding::new(WITH_OBJECT_BINDING, scope_id);
        if !self.binding_belongs_to_current_function(&binding) || self.is_shared_binding(&binding) {
            let value = self.load_captured_binding(*block, &binding)?;
            self.resolve_expr_continuations(block);
            return Ok(value);
        }
        let dest = self.alloc_value();
        self.current_function.append_instruction(
            *block,
            Instruction::LoadVar {
                dest,
                name: binding.var_ir_name(),
            },
        );
        Ok(dest)
    }

    /// 发射字符串常量并返回其值。
    pub(crate) fn append_string_const(&mut self, block: BasicBlockId, text: &str) -> ValueId {
        let constant = self.module.add_constant(Constant::String(text.to_string()));
        let dest = self.alloc_value();
        self.current_function
            .append_instruction(block, Instruction::Const { dest, constant });
        dest
    }

    /// 发射 with 分派链（§9.1.1.2.1 HasBinding，由内到外短路）：
    /// 返回 `(base, 落点 block)`。base 为命中的 with 对象，全链未命中为
    /// undefined（WithToObject 保证 with 对象恒非 undefined，可作哨兵）。
    /// 每层至多一次 has 探测；trap / getter 异常在链中直接抛出传播。
    pub(crate) fn lower_with_resolution_chain(
        &mut self,
        name: &str,
        crossed: &[usize],
        block: BasicBlockId,
    ) -> Result<(ValueId, BasicBlockId), LoweringError> {
        let merge = self.current_function.new_block();
        let mut sources = Vec::with_capacity(crossed.len() + 1);
        let mut cursor = block;
        for scope_id in crossed {
            let object = self.load_with_object(&mut cursor, *scope_id)?;
            let key = self.append_string_const(cursor, name);
            let has = self.alloc_value();
            self.current_function.append_instruction(
                cursor,
                Instruction::CallBuiltin {
                    dest: Some(has),
                    builtin: Builtin::WithHasBinding,
                    args: vec![object, key],
                },
            );
            cursor = self.fork_with_dispatch_exception(cursor, has)?;
            let hit = self.current_function.new_block();
            let next = self.current_function.new_block();
            self.current_function.set_terminator(
                cursor,
                Terminator::Branch {
                    condition: has,
                    true_block: hit,
                    false_block: next,
                },
            );
            self.current_function
                .set_terminator(hit, Terminator::Jump { target: merge });
            sources.push(PhiSource {
                predecessor: hit,
                value: object,
            });
            cursor = next;
        }

        let undef = self.append_undefined_const(cursor);
        self.current_function
            .set_terminator(cursor, Terminator::Jump { target: merge });
        sources.push(PhiSource {
            predecessor: cursor,
            value: undef,
        });

        let base = self.alloc_value();
        self.current_function.append_instruction(
            merge,
            Instruction::Phi {
                dest: base,
                sources,
            },
        );
        // Phi 所在块不得再挂 Branch 终结器（CFG codegen 契约），插入续接块。
        let post = self.current_function.new_block();
        self.current_function
            .set_terminator(merge, Terminator::Jump { target: post });
        Ok((base, post))
    }

    /// 在 `block` 上按 `base === undefined` 分叉：返回 (miss, hit) 两块。
    pub(crate) fn branch_on_with_base(
        &mut self,
        base: ValueId,
        block: BasicBlockId,
    ) -> (BasicBlockId, BasicBlockId) {
        let undef = self.append_undefined_const(block);
        let missing = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Compare {
                dest: missing,
                op: CompareOp::StrictEq,
                lhs: base,
                rhs: undef,
            },
        );
        let miss = self.current_function.new_block();
        let hit = self.current_function.new_block();
        self.current_function.set_terminator(
            block,
            Terminator::Branch {
                condition: missing,
                true_block: miss,
                false_block: hit,
            },
        );
        (miss, hit)
    }

    /// 发射 undefined 常量。
    pub(crate) fn append_undefined_const(&mut self, block: BasicBlockId) -> ValueId {
        let constant = self.module.add_constant(Constant::Undefined);
        let dest = self.alloc_value();
        self.current_function
            .append_instruction(block, Instruction::Const { dest, constant });
        dest
    }

    /// 合并 hit / miss 两路的结果值：两端各自 Jump 到新 merge，Phi 出结果。
    /// 任一端已终结（throw）时退化为单前驱 Phi。
    pub(crate) fn merge_with_dispatch_results(
        &mut self,
        arms: &[(BasicBlockId, ValueId, bool)],
    ) -> (ValueId, BasicBlockId) {
        let merge = self.current_function.new_block();
        let mut sources = Vec::new();
        for (end_block, value, open) in arms {
            if !*open {
                continue;
            }
            self.current_function
                .set_terminator(*end_block, Terminator::Jump { target: merge });
            sources.push(PhiSource {
                predecessor: *end_block,
                value: *value,
            });
        }
        let result = self.alloc_value();
        self.current_function.append_instruction(
            merge,
            Instruction::Phi {
                dest: result,
                sources,
            },
        );
        let post = self.current_function.new_block();
        self.current_function
            .set_terminator(merge, Terminator::Jump { target: post });
        (result, post)
    }

    /// 发射运行时错误构造 + throw，返回哨兵值（throw 后不可达，仅满足 SSA）。
    pub(crate) fn emit_runtime_error_throw(
        &mut self,
        block: BasicBlockId,
        constructor: Builtin,
        message: &str,
    ) -> Result<ValueId, LoweringError> {
        let msg_val = self.append_string_const(block, message);
        let error_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::CallBuiltin {
                dest: Some(error_val),
                builtin: constructor,
                args: vec![msg_val],
            },
        );
        // 哨兵须在 throw 终结块之前分配，避免值引用落入不可达块。
        let dummy = self.append_undefined_const(block);
        self.emit_throw_value(block, error_val)?;
        Ok(dummy)
    }
}
