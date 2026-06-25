use swc_core::common::DUMMY_SP;
use swc_core::common::Span;
use swc_core::common::Spanned;
use swc_core::ecma::ast as swc_ast;
use thiserror::Error;
use wjsm_ir::{
    BasicBlock, BasicBlockId, BinaryOp, Builtin, CompareOp, Constant, ConstantId, Function,
    FunctionId, HomeObject, Instruction, MODULE_ENTRY_IR_NAME, Module, PhiSource, Program,
    SwitchCaseTarget, Terminator, UnaryOp, ValueId,
};

const EVAL_SCOPE_ENV_PARAM: &str = "$eval_env";

use wjsm_ir::wk_symbol;
// 保留旧常量名作为别名，避免改动所有引用点
const WK_SYMBOL_ITERATOR: u32 = wk_symbol::ITERATOR;
const WK_SYMBOL_SPECIES: u32 = wk_symbol::SPECIES;
const WK_SYMBOL_TO_STRING_TAG: u32 = wk_symbol::TO_STRING_TAG;
const WK_SYMBOL_ASYNC_ITERATOR: u32 = wk_symbol::ASYNC_ITERATOR;
const WK_SYMBOL_HAS_INSTANCE: u32 = wk_symbol::HAS_INSTANCE;
const WK_SYMBOL_TO_PRIMITIVE: u32 = wk_symbol::TO_PRIMITIVE;
const WK_SYMBOL_DISPOSE: u32 = wk_symbol::DISPOSE;
const WK_SYMBOL_MATCH: u32 = wk_symbol::MATCH;
const WK_SYMBOL_ASYNC_DISPOSE: u32 = wk_symbol::ASYNC_DISPOSE;

// ── 提取到子模块的类型 ──────────────────────────────────────────────────
mod scope;
mod function_builder;
mod lowerer_types;
mod lowerer_modules;
mod scan_await;

pub(crate) use scope::*;
pub(crate) use function_builder::*;
pub(crate) use lowerer_types::*;
pub use lowerer_modules::lower_modules;
pub(crate) use scan_await::has_top_level_await;

// ── Public API ──────────────────────────────────────────────────────────

pub fn lower_module(module: swc_ast::Module, script: bool) -> Result<Program, LoweringError> {
    lower_module_with_source(module, script, None, "input")
}

/// 带源码上下文以降低错误诊断；`source` 为完整源文本，`filename` 用于错误展示。
pub fn lower_module_with_source(
    module: swc_ast::Module,
    script: bool,
    source: Option<std::sync::Arc<str>>,
    filename: impl Into<String>,
) -> Result<Program, LoweringError> {
    let mut lowerer = Lowerer::new();
    lowerer.script_mode = script;
    lowerer.diagnostic_source = source;
    lowerer.diagnostic_filename = filename.into();
    lowerer.lower_module(&module)
}

pub fn lower_eval_module(module: swc_ast::Module) -> Result<Program, LoweringError> {
    lower_eval_module_with_scope(module, false, false)
}

pub fn lower_eval_module_with_scope(
    module: swc_ast::Module,
    has_scope_bridge: bool,
    var_writes_to_scope: bool,
) -> Result<Program, LoweringError> {
    let mut lowerer = Lowerer::new();
    lowerer.eval_mode = true;
    lowerer.eval_has_scope_bridge = has_scope_bridge;
    lowerer.eval_var_writes_to_scope = var_writes_to_scope;
    lowerer.eval_scope_record = true;
    lowerer.strict_mode = module_has_use_strict_directive(&module);
    lowerer.lower_module(&module)
}

mod lowerer_arrows;
mod lowerer_assignments;
mod lowerer_async_eval;
mod lowerer_binary_expr;
mod lowerer_branching;
mod lowerer_calls_eval;
mod lowerer_classes_ts;
mod lowerer_construct;
mod lowerer_core;
mod lowerer_declarations;
mod lowerer_function_decls;
mod lowerer_functions;
mod lowerer_jsx_objects;
mod lowerer_predeclare;
mod lowerer_stmt;
mod lowerer_ts;

// ── Error types ─────────────────────────────────────────────────────────

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
    /// 用于将字节偏移格式化为行/列；多模块编译时可能为空。
    pub(crate) source: Option<std::sync::Arc<str>>,
    pub(crate) filename: String,
}

impl Diagnostic {
    pub(crate) fn new(start: u32, end: u32, message: impl Into<String>) -> Self {
        Self {
            start,
            end: if end > start { end } else { start + 1 },
            message: message.into(),
            source: None,
            filename: "input".into(),
        }
    }

    pub(crate) fn with_source_context(
        start: u32,
        end: u32,
        message: impl Into<String>,
        source: Option<std::sync::Arc<str>>,
        filename: impl Into<String>,
    ) -> Self {
        Self {
            start,
            end: if end > start { end } else { start + 1 },
            message: message.into(),
            source,
            filename: filename.into(),
        }
    }
}

impl std::fmt::Display for Diagnostic {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(ref src) = self.source {
            write!(
                formatter,
                "{}",
                wjsm_parser::format_byte_diagnostic(
                    &self.filename,
                    src,
                    &self.message,
                    self.start,
                    self.end,
                )
            )
        } else {
            write!(
                formatter,
                "error: {}\n --> {}:{}:1",
                self.message, self.filename, self.start
            )
        }
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────

mod ast_kinds;
mod builtins;
mod eval_scan;

use ast_kinds::*;
use builtins::*;
pub use eval_scan::eval_literal_binding_names;
use eval_scan::*;
/// 判断表达式是否为 TypedArray 构造函数调用（`new Int8Array(...)` 等形式）。
fn is_typedarray_constructor_expr(expr: &swc_ast::Expr) -> bool {
    if let swc_ast::Expr::New(new_expr) = expr
        && let swc_ast::Expr::Ident(ident) = new_expr.callee.as_ref()
    {
        return matches!(
            ident.sym.as_ref(),
            "Int8Array"
                | "Uint8Array"
                | "Uint8ClampedArray"
                | "Int16Array"
                | "Uint16Array"
                | "Int32Array"
                | "Uint32Array"
                | "Float32Array"
                | "Float64Array"
                | "BigInt64Array"
                | "BigUint64Array"
        );
    }
    false
}
/// 判断表达式是否为 SharedArrayBuffer 构造函数调用（`new SharedArrayBuffer(...)` 形式）。
fn is_sharedarraybuffer_constructor_expr(expr: &swc_ast::Expr) -> bool {
    if let swc_ast::Expr::New(new_expr) = expr
        && let swc_ast::Expr::Ident(ident) = new_expr.callee.as_ref()
    {
        return ident.sym.as_ref() == "SharedArrayBuffer";
    }
    false
}
/// 判断表达式是否为 DataView 构造函数调用（`new DataView(...)` 形式）。
fn is_dataview_constructor_expr(expr: &swc_ast::Expr) -> bool {
    if let swc_ast::Expr::New(new_expr) = expr
        && let swc_ast::Expr::Ident(ident) = new_expr.callee.as_ref()
    {
        return ident.sym.as_ref() == "DataView";
    }
    false
}
