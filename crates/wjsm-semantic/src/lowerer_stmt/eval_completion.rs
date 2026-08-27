use super::*;

/// eval 完成值（completion value）追踪。
///
/// ECMAScript 规定 eval 返回脚本体的完成值：表达式语句产生值，声明产生 empty，
/// `if`/`try`/`switch`/循环/`with` 按 `UpdateEmpty(completion, undefined)` 归一。
/// SSA 值无法跨 try/catch、循环等任意控制流合法线程化（会产生支配性验证错误），
/// 因此完成值保存在专用内存槽（`$tmp.N` 变量）中：
///
/// - 模块入口把槽初始化为 undefined（空脚本与 empty 完成值即返回 undefined）；
/// - 表达式语句在正常路径把值写入槽；
/// - `if`/`try`/`switch`/循环/`with` 在语句执行前把槽重置为 undefined，
///   对应规范的 `UpdateEmpty(stmtCompletion, undefined)` 与循环的 `V = undefined`；
/// - catch 子句入口重置槽（`TryStatement` 中 B 为 throw 时 C 从 empty 起步）；
/// - finally 体求值前保存槽并重置为 undefined，正常完成后恢复
///   （规范丢弃 finally 的正常完成值；abrupt 完成则保留 finally 内部的线程化值）。
///
/// 追踪只作用于 eval 顶层代码：嵌套函数体（`function_stack` 非空）内的语句
/// 不参与 eval 完成值。
impl Lowerer {
    /// 当前语句是否处于 eval 顶层完成值追踪上下文。
    pub(crate) fn eval_completion_tracking(&self) -> bool {
        self.eval_mode && self.function_stack.is_empty()
    }

    /// 在 eval 模块入口初始化完成值槽为 undefined。
    pub(crate) fn init_eval_completion_var(&mut self, block: BasicBlockId) {
        let name = self.alloc_temp_name();
        let undef_val = self.alloc_undefined_value(block);
        self.current_function.append_instruction(
            block,
            Instruction::StoreVar {
                name: name.clone(),
                value: undef_val,
            },
        );
        self.eval_completion_var = Some(name);
    }

    fn eval_completion_var_name(&self) -> String {
        self.eval_completion_var
            .clone()
            .expect("eval completion var initialised at module entry")
    }

    /// 表达式语句的正常完成路径：把语句值写入完成值槽。
    pub(crate) fn emit_eval_completion_store(&mut self, block: BasicBlockId, value: ValueId) {
        let name = self.eval_completion_var_name();
        self.current_function
            .append_instruction(block, Instruction::StoreVar { name, value });
    }

    /// 把完成值槽重置为 undefined（`UpdateEmpty(completion, undefined)` 的 lowering 形态）。
    pub(crate) fn emit_eval_completion_reset(&mut self, block: BasicBlockId) {
        let undef_val = self.alloc_undefined_value(block);
        self.emit_eval_completion_store(block, undef_val);
    }

    /// 语句 dispatcher 钩子：完成值按规范被 `UpdateEmpty(·, undefined)` 归一的
    /// 语句在执行前重置槽。声明/块/标签语句保持槽不变（empty 完成值透传）。
    pub(crate) fn eval_completion_reset_for_stmt(&mut self, stmt: &swc_ast::Stmt, flow: StmtFlow) {
        if !self.eval_completion_tracking() {
            return;
        }
        let resets = matches!(
            stmt,
            swc_ast::Stmt::If(_)
                | swc_ast::Stmt::Try(_)
                | swc_ast::Stmt::Switch(_)
                | swc_ast::Stmt::While(_)
                | swc_ast::Stmt::DoWhile(_)
                | swc_ast::Stmt::For(_)
                | swc_ast::Stmt::ForIn(_)
                | swc_ast::Stmt::ForOf(_)
                | swc_ast::Stmt::With(_)
        );
        if resets && let StmtFlow::Open(block) = flow {
            self.emit_eval_completion_reset(block);
        }
    }

    /// catch 子句入口：重置完成值槽。try 体被 throw 中断时已写入的部分完成值
    /// 不得泄漏到 catch 路径（规范：`UpdateEmpty(C, undefined)` 中 C 从 empty 起步）。
    pub(crate) fn eval_completion_reset_catch_entry(&mut self, catch_entry: BasicBlockId) {
        if self.eval_completion_tracking() {
            self.emit_eval_completion_reset(catch_entry);
        }
    }

    /// finally 体求值前：把当前完成值保存到新临时槽并重置为 undefined。
    /// 返回保存槽名；非追踪上下文返回 None。
    pub(crate) fn eval_completion_save_for_finalizer(
        &mut self,
        block: BasicBlockId,
    ) -> Option<String> {
        if !self.eval_completion_tracking() {
            return None;
        }
        let current = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::LoadVar {
                dest: current,
                name: self.eval_completion_var_name(),
            },
        );
        let saved_name = self.alloc_temp_name();
        self.current_function.append_instruction(
            block,
            Instruction::StoreVar {
                name: saved_name.clone(),
                value: current,
            },
        );
        self.emit_eval_completion_reset(block);
        Some(saved_name)
    }

    /// finally 体正常完成后：从保存槽恢复完成值（丢弃 finally 的正常完成值）。
    /// finally 以 abrupt 完成（break/continue/return/throw）退出时不恢复，
    /// 槽保留 finally 内部线程化的值。
    pub(crate) fn eval_completion_restore_after_finalizer(
        &mut self,
        block: BasicBlockId,
        saved: Option<&str>,
    ) {
        let Some(saved_name) = saved else {
            return;
        };
        let restored = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::LoadVar {
                dest: restored,
                name: saved_name.to_string(),
            },
        );
        self.emit_eval_completion_store(block, restored);
    }

    /// eval 模块结尾：读取完成值槽作为返回值。
    pub(crate) fn emit_eval_completion_return_value(&mut self, block: BasicBlockId) -> ValueId {
        let dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::LoadVar {
                dest,
                name: self.eval_completion_var_name(),
            },
        );
        dest
    }
}
