use swc_core::common::Span;
use swc_core::common::Spanned;
use swc_core::ecma::ast as swc_ast;
use thiserror::Error;
use wjsm_ir::{
    BasicBlock, BasicBlockId, BinaryOp, Builtin, Constant, Instruction, Module, Program,
    Terminator, ValueId,
};

pub fn lower_module(module: swc_ast::Module) -> Result<Program, LoweringError> {
    Lowerer::new().lower_module(&module)
}

struct Lowerer {
    module: Module,
    next_value: u32,
}

impl Lowerer {
    fn new() -> Self {
        Self {
            module: Module::new(),
            next_value: 0,
        }
    }

    fn lower_module(mut self, module: &swc_ast::Module) -> Result<Program, LoweringError> {
        let mut function = wjsm_ir::Function::new("main", BasicBlockId(0));
        let mut entry = BasicBlock::new(BasicBlockId(0), Terminator::Return { value: None });

        for item in &module.body {
            match item {
                swc_ast::ModuleItem::Stmt(stmt) => self.lower_stmt(stmt, &mut entry)?,
                swc_ast::ModuleItem::ModuleDecl(decl) => {
                    return Err(self.error(
                        decl.span(),
                        format!(
                            "unsupported module declaration kind `{}`",
                            module_decl_kind(decl)
                        ),
                    ));
                }
            }
        }

        function.push_block(entry);
        self.module.push_function(function);
        Ok(self.module)
    }

    fn lower_stmt(
        &mut self,
        stmt: &swc_ast::Stmt,
        block: &mut BasicBlock,
    ) -> Result<(), LoweringError> {
        match stmt {
            swc_ast::Stmt::Expr(expr_stmt) => {
                match expr_stmt.expr.as_ref() {
                    swc_ast::Expr::Call(call) => self.lower_console_log_stmt(call, block)?,
                    expr => {
                        let _ = self.lower_expr(expr, block)?;
                    }
                }
                Ok(())
            }
            _ => Err(self.error(
                stmt.span(),
                format!("unsupported statement kind `{}`", stmt_kind(stmt)),
            )),
        }
    }

    fn lower_expr(
        &mut self,
        expr: &swc_ast::Expr,
        block: &mut BasicBlock,
    ) -> Result<ValueId, LoweringError> {
        match expr {
            swc_ast::Expr::Bin(bin) => self.lower_binary(bin, block),
            swc_ast::Expr::Lit(lit) => self.lower_literal(lit, block),
            _ => Err(self.error(
                expr.span(),
                format!("unsupported expression kind `{}`", expr_kind(expr)),
            )),
        }
    }

    fn lower_console_log_stmt(
        &mut self,
        call: &swc_ast::CallExpr,
        block: &mut BasicBlock,
    ) -> Result<(), LoweringError> {
        if !is_console_log(call) {
            return Err(self.error(call.span(), "unsupported call expression"));
        }

        let first_arg = call
            .args
            .first()
            .ok_or_else(|| self.error(call.span(), "console.log requires at least 1 argument"))?;

        let value = self.lower_expr(first_arg.expr.as_ref(), block)?;
        block.push_instruction(Instruction::CallBuiltin {
            dest: None,
            builtin: Builtin::ConsoleLog,
            args: vec![value],
        });
        Ok(())
    }

    fn lower_binary(
        &mut self,
        bin: &swc_ast::BinExpr,
        block: &mut BasicBlock,
    ) -> Result<ValueId, LoweringError> {
        let lhs = self.lower_expr(bin.left.as_ref(), block)?;
        let rhs = self.lower_expr(bin.right.as_ref(), block)?;
        let dest = self.alloc_value();
        let op = match bin.op {
            swc_ast::BinaryOp::Add => BinaryOp::Add,
            swc_ast::BinaryOp::Sub => BinaryOp::Sub,
            swc_ast::BinaryOp::Mul => BinaryOp::Mul,
            swc_ast::BinaryOp::Div => BinaryOp::Div,
            _ => {
                return Err(self.error(
                    bin.span(),
                    format!("unsupported binary operator `{}`", binary_op_name(bin.op)),
                ));
            }
        };

        block.push_instruction(Instruction::Binary { dest, op, lhs, rhs });
        Ok(dest)
    }

    fn lower_literal(
        &mut self,
        lit: &swc_ast::Lit,
        block: &mut BasicBlock,
    ) -> Result<ValueId, LoweringError> {
        let constant = match lit {
            swc_ast::Lit::Num(num) => Constant::Number(num.value),
            swc_ast::Lit::Str(string) => {
                Constant::String(string.value.to_string_lossy().into_owned())
            }
            _ => {
                return Err(self.error(
                    lit.span(),
                    format!("unsupported literal kind `{}`", literal_kind(lit)),
                ));
            }
        };

        let constant = self.module.add_constant(constant);
        let dest = self.alloc_value();
        block.push_instruction(Instruction::Const { dest, constant });
        Ok(dest)
    }

    fn alloc_value(&mut self) -> ValueId {
        let id = ValueId(self.next_value);
        self.next_value += 1;
        id
    }

    fn error(&self, span: Span, message: impl Into<String>) -> LoweringError {
        LoweringError::Diagnostic(Diagnostic::new(span.lo.0, span.hi.0, message))
    }
}

fn is_console_log(call: &swc_ast::CallExpr) -> bool {
    let swc_ast::Callee::Expr(callee_expr) = &call.callee else {
        return false;
    };
    let swc_ast::Expr::Member(member) = callee_expr.as_ref() else {
        return false;
    };
    let swc_ast::Expr::Ident(object) = member.obj.as_ref() else {
        return false;
    };
    let swc_ast::MemberProp::Ident(property) = &member.prop else {
        return false;
    };

    object.sym == "console" && property.sym == "log"
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum LoweringError {
    #[error("{0}")]
    Diagnostic(Diagnostic),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub start: u32,
    pub end: u32,
    pub message: String,
}

impl Diagnostic {
    fn new(start: u32, end: u32, message: impl Into<String>) -> Self {
        Self {
            start,
            end,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for Diagnostic {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "semantic lowering error [{}..{}]: {}",
            self.start, self.end, self.message
        )
    }
}

fn stmt_kind(stmt: &swc_ast::Stmt) -> &'static str {
    match stmt {
        swc_ast::Stmt::Block(_) => "block",
        swc_ast::Stmt::Empty(_) => "empty",
        swc_ast::Stmt::Debugger(_) => "debugger",
        swc_ast::Stmt::With(_) => "with",
        swc_ast::Stmt::Return(_) => "return",
        swc_ast::Stmt::Labeled(_) => "labeled",
        swc_ast::Stmt::Break(_) => "break",
        swc_ast::Stmt::Continue(_) => "continue",
        swc_ast::Stmt::If(_) => "if",
        swc_ast::Stmt::Switch(_) => "switch",
        swc_ast::Stmt::Throw(_) => "throw",
        swc_ast::Stmt::Try(_) => "try",
        swc_ast::Stmt::While(_) => "while",
        swc_ast::Stmt::DoWhile(_) => "do-while",
        swc_ast::Stmt::For(_) => "for",
        swc_ast::Stmt::ForIn(_) => "for-in",
        swc_ast::Stmt::ForOf(_) => "for-of",
        swc_ast::Stmt::Decl(_) => "decl",
        swc_ast::Stmt::Expr(_) => "expr",
    }
}

fn expr_kind(expr: &swc_ast::Expr) -> &'static str {
    match expr {
        swc_ast::Expr::This(_) => "this",
        swc_ast::Expr::Array(_) => "array",
        swc_ast::Expr::Object(_) => "object",
        swc_ast::Expr::Fn(_) => "function",
        swc_ast::Expr::Unary(_) => "unary",
        swc_ast::Expr::Update(_) => "update",
        swc_ast::Expr::Bin(_) => "binary",
        swc_ast::Expr::Assign(_) => "assign",
        swc_ast::Expr::Member(_) => "member",
        swc_ast::Expr::SuperProp(_) => "super-prop",
        swc_ast::Expr::Cond(_) => "conditional",
        swc_ast::Expr::Call(_) => "call",
        swc_ast::Expr::New(_) => "new",
        swc_ast::Expr::Seq(_) => "sequence",
        swc_ast::Expr::Ident(_) => "identifier",
        swc_ast::Expr::Lit(_) => "literal",
        swc_ast::Expr::Tpl(_) => "template",
        swc_ast::Expr::TaggedTpl(_) => "tagged-template",
        swc_ast::Expr::Arrow(_) => "arrow",
        swc_ast::Expr::Class(_) => "class",
        swc_ast::Expr::Yield(_) => "yield",
        swc_ast::Expr::MetaProp(_) => "meta-prop",
        swc_ast::Expr::Await(_) => "await",
        swc_ast::Expr::Paren(_) => "paren",
        swc_ast::Expr::JSXMember(_) => "jsx-member",
        swc_ast::Expr::JSXNamespacedName(_) => "jsx-namespaced-name",
        swc_ast::Expr::JSXEmpty(_) => "jsx-empty",
        swc_ast::Expr::JSXElement(_) => "jsx-element",
        swc_ast::Expr::JSXFragment(_) => "jsx-fragment",
        swc_ast::Expr::TsTypeAssertion(_) => "ts-type-assertion",
        swc_ast::Expr::TsConstAssertion(_) => "ts-const-assertion",
        swc_ast::Expr::TsNonNull(_) => "ts-non-null",
        swc_ast::Expr::TsAs(_) => "ts-as",
        swc_ast::Expr::TsInstantiation(_) => "ts-instantiation",
        swc_ast::Expr::TsSatisfies(_) => "ts-satisfies",
        swc_ast::Expr::PrivateName(_) => "private-name",
        swc_ast::Expr::OptChain(_) => "optional-chain",
        swc_ast::Expr::Invalid(_) => "invalid",
    }
}

fn literal_kind(lit: &swc_ast::Lit) -> &'static str {
    match lit {
        swc_ast::Lit::Str(_) => "string",
        swc_ast::Lit::Bool(_) => "bool",
        swc_ast::Lit::Null(_) => "null",
        swc_ast::Lit::Num(_) => "number",
        swc_ast::Lit::BigInt(_) => "bigint",
        swc_ast::Lit::Regex(_) => "regex",
        swc_ast::Lit::JSXText(_) => "jsx-text",
    }
}

fn module_decl_kind(decl: &swc_ast::ModuleDecl) -> &'static str {
    match decl {
        swc_ast::ModuleDecl::Import(_) => "import",
        swc_ast::ModuleDecl::ExportDecl(_) => "export-decl",
        swc_ast::ModuleDecl::ExportNamed(_) => "export-named",
        swc_ast::ModuleDecl::ExportDefaultDecl(_) => "export-default-decl",
        swc_ast::ModuleDecl::ExportDefaultExpr(_) => "export-default-expr",
        swc_ast::ModuleDecl::ExportAll(_) => "export-all",
        swc_ast::ModuleDecl::TsImportEquals(_) => "ts-import-equals",
        swc_ast::ModuleDecl::TsExportAssignment(_) => "ts-export-assignment",
        swc_ast::ModuleDecl::TsNamespaceExport(_) => "ts-namespace-export",
    }
}

fn binary_op_name(op: swc_ast::BinaryOp) -> &'static str {
    match op {
        swc_ast::BinaryOp::EqEq => "==",
        swc_ast::BinaryOp::NotEq => "!=",
        swc_ast::BinaryOp::EqEqEq => "===",
        swc_ast::BinaryOp::NotEqEq => "!==",
        swc_ast::BinaryOp::Lt => "<",
        swc_ast::BinaryOp::LtEq => "<=",
        swc_ast::BinaryOp::Gt => ">",
        swc_ast::BinaryOp::GtEq => ">=",
        swc_ast::BinaryOp::LShift => "<<",
        swc_ast::BinaryOp::RShift => ">>",
        swc_ast::BinaryOp::ZeroFillRShift => ">>>",
        swc_ast::BinaryOp::Add => "+",
        swc_ast::BinaryOp::Sub => "-",
        swc_ast::BinaryOp::Mul => "*",
        swc_ast::BinaryOp::Div => "/",
        swc_ast::BinaryOp::Mod => "%",
        swc_ast::BinaryOp::BitOr => "|",
        swc_ast::BinaryOp::BitXor => "^",
        swc_ast::BinaryOp::BitAnd => "&",
        swc_ast::BinaryOp::LogicalOr => "||",
        swc_ast::BinaryOp::LogicalAnd => "&&",
        swc_ast::BinaryOp::In => "in",
        swc_ast::BinaryOp::InstanceOf => "instanceof",
        swc_ast::BinaryOp::Exp => "**",
        swc_ast::BinaryOp::NullishCoalescing => "??",
    }
}
