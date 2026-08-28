use swc_core::common::DUMMY_SP;
use swc_core::common::Span;
use swc_core::common::Spanned;
use swc_core::ecma::ast as swc_ast;
use thiserror::Error;
use wjsm_ir::{
    BasicBlock, BasicBlockId, BinaryOp, Builtin, CompareOp, Constant, EVAL_SCOPE_ENV_PARAM,
    Function, FunctionId, HomeObject, Instruction, MODULE_ENTRY_IR_NAME, Module, PhiSource,
    Program, SourceSpan, SwitchCaseTarget, Terminator, UnaryOp, ValueId,
};

use wjsm_ir::wk_symbol;
const WK_SYMBOL_DISPOSE: u32 = wk_symbol::DISPOSE;
const WK_SYMBOL_ASYNC_DISPOSE: u32 = wk_symbol::ASYNC_DISPOSE;

// ── 提取到子模块的类型 ──────────────────────────────────────────────────
mod function_builder;
mod lowerer_modules;
mod lowerer_types;
mod regexp_early;
mod scan_await;
mod scope;
mod wk_symbol_map;
pub(crate) use function_builder::*;
pub use lowerer_modules::{
    BuiltinSegment, LoweringMetadata, ModuleKind, ModuleLinking, ModuleLoweringInput,
    ModuleMetadata, lower_modules, lower_modules_with_builtin_seed, lower_modules_with_debug,
    lower_modules_with_debug_meta,
};
pub(crate) use lowerer_types::*;
pub(crate) use scan_await::has_top_level_await;

/// 检测模块体是否包含 top-level `await`（不递归进入函数/类体边界）。
/// 供 wjsm-module 判断用户程序是否 TLA（TLA 程序禁用 builtin 段缓存，见
/// bundler::lower_bundle_cached——builtin 顶层代码 inline 进 async 状态机后
/// 无法触发 ContinuationSaveVar 生成，跨 await 的模块变量会丢失）。
pub use scan_await::has_top_level_await as program_has_top_level_await;
pub(crate) use scope::*;

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
    lower_module_with_debug_source(module, script, source, filename, false)
}

/// 与 [`lower_module_with_source`] 相同，并可选在语句入口发射 `debug_check`。
///
/// `emit_debug_checks` 为 true 时需要提供 `source`，否则行/列无法解析，指令会被跳过。
pub fn lower_module_with_debug_source(
    module: swc_ast::Module,
    script: bool,
    source: Option<std::sync::Arc<str>>,
    filename: impl Into<String>,
    emit_debug_checks: bool,
) -> Result<Program, LoweringError> {
    let mut lowerer = Lowerer::new();
    lowerer.script_mode = script;
    lowerer.diagnostic_source = source;
    lowerer.diagnostic_filename = filename.into();
    lowerer.emit_debug_checks = emit_debug_checks;
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
    lower_eval_module_with_scope_and_strict(module, has_scope_bridge, var_writes_to_scope, false)
}

pub fn lower_eval_module_with_scope_and_strict(
    module: swc_ast::Module,
    has_scope_bridge: bool,
    var_writes_to_scope: bool,
    inherited_strict: bool,
) -> Result<Program, LoweringError> {
    let mut lowerer = Lowerer::new();
    lowerer.eval_mode = true;
    lowerer.eval_has_scope_bridge = has_scope_bridge;
    lowerer.eval_scope_record = true;
    lowerer.strict_mode = inherited_strict || module_has_use_strict_directive(&module);
    lowerer.eval_var_writes_to_scope = var_writes_to_scope && !lowerer.strict_mode;
    lowerer.lower_module(&module)
}

pub fn eval_module_has_use_strict_directive(module: &swc_ast::Module) -> bool {
    module_has_use_strict_directive(module)
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
mod lowerer_with;
mod passes;

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
        if let Some(src) = &self.source {
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
mod dynamic_function;
mod eval_scan;

use ast_kinds::*;
use builtins::*;
pub use dynamic_function::{DynamicFunctionSource, prepare_dynamic_function};
pub use eval_scan::eval_literal_binding_names;
use eval_scan::*;
/// 判断表达式是否为 Array 构造结果（数组字面量或 `new Array(...)`）。
fn is_array_constructor_expr(expr: &swc_ast::Expr) -> bool {
    match expr {
        swc_ast::Expr::Array(_) => true,
        swc_ast::Expr::New(new_expr) => {
            if let swc_ast::Expr::Ident(ident) = new_expr.callee.as_ref() {
                ident.sym.as_ref() == "Array"
            } else {
                false
            }
        }
        _ => false,
    }
}

/// 判断表达式是否为 `Array.from(...)` / `Array.of(...)` 静态调用。
/// 二者恒返回数组（否则抛异常）；是否真的落到内置 Array 由调用方用作用域判影。
fn is_array_from_of_call(expr: &swc_ast::Expr) -> bool {
    let swc_ast::Expr::Call(call) = expr else {
        return false;
    };
    let swc_ast::Callee::Expr(callee) = &call.callee else {
        return false;
    };
    let swc_ast::Expr::Member(member) = callee.as_ref() else {
        return false;
    };
    let swc_ast::Expr::Ident(obj) = member.obj.as_ref() else {
        return false;
    };
    if obj.sym.as_ref() != "Array" {
        return false;
    }
    matches!(
        &member.prop,
        swc_ast::MemberProp::Ident(prop) if matches!(prop.sym.as_ref(), "from" | "of")
    )
}
/// 判断方法名是否为「receiver 为字符串时恒返回字符串」的 String.prototype 方法。
///
/// 排除 `at`（越界返回 undefined）、`split`（返回数组）、`match` 等返回对象或
/// 可空值的方法；链式 receiver 判定依赖「恒为字符串」这一点。
fn is_string_returning_proto_method(name: &str) -> bool {
    matches!(
        name,
        "slice"
            | "substring"
            | "substr"
            | "toUpperCase"
            | "toLowerCase"
            | "trim"
            | "trimStart"
            | "trimEnd"
            | "padStart"
            | "padEnd"
            | "repeat"
            | "replace"
            | "replaceAll"
            | "concat"
            | "charAt"
            | "normalize"
            | "toString"
            | "valueOf"
    )
}

/// 判断方法名是否为「返回数组」的 Array.prototype 方法（`map`/`filter`/`slice`
/// 等）。用于链式数组高阶函数 receiver 判定：这类方法在 receiver 为数组时
/// 恒返回数组，其结果可作为下一链节的内建 receiver 继续展开。
fn is_array_producing_proto_method(name: &str) -> bool {
    matches!(
        name,
        "map"
            | "filter"
            | "flatMap"
            | "flat"
            | "concat"
            | "splice"
            | "sort"
            | "copyWithin"
            | "toSorted"
            | "toReversed"
            | "toSpliced"
            | "with"
    )
}

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

/// 判断表达式是否为 Map 构造函数调用（`new Map(...)` 形式）。
fn is_map_constructor_expr(expr: &swc_ast::Expr) -> bool {
    if let swc_ast::Expr::New(new_expr) = expr
        && let swc_ast::Expr::Ident(ident) = new_expr.callee.as_ref()
    {
        return ident.sym.as_ref() == "Map";
    }
    false
}

/// 判断表达式是否为 Set 构造函数调用（`new Set(...)` 形式）。
fn is_set_constructor_expr(expr: &swc_ast::Expr) -> bool {
    if let swc_ast::Expr::New(new_expr) = expr
        && let swc_ast::Expr::Ident(ident) = new_expr.callee.as_ref()
    {
        return ident.sym.as_ref() == "Set";
    }
    false
}
