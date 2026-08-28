//! `Function` 动态函数构造器的语法准备与早错误校验（§20.2.1.1.1
//! CreateDynamicFunction）。
//!
//! 规范要求三次独立解析：形参串以 FormalParameters 目标符号解析、函数体以
//! FunctionBody 目标符号解析、拼接后的 sourceText 以函数表达式解析。任何一次
//! 失败都是 SyntaxError。三次解析共同构成注入防护：形参/函数体中的注释、
//! 提前闭合的括号或额外声明都无法逃出函数边界（如 `new Function("/*", "*/) {")`
//! 与 `new Function("a", "}, function evil() {")` 必须抛 SyntaxError）。
//!
//! swc 没有独立的 FormalParameters/FunctionBody 目标符号入口，这里用规范同款
//! 包装文本模拟：把形参（或函数体）单独嵌入 `function anonymous(…\n) {\n…\n}`
//! 再要求整个脚本恰好是这一个函数声明。换行符的位置与规范一致，保证形参内的
//! 行注释不会吞掉 `)`，而未闭合的块注释无法借另一段文本闭合。

use swc_core::ecma::ast as swc_ast;

use crate::lowerer_with::strict_check::stmts_have_use_strict;

/// CreateDynamicFunction 的语法产物：拼接源码与函数元数据。
pub struct DynamicFunctionSource {
    /// 实际编译执行的脚本：匿名函数表达式语句，其完成值即目标闭包。
    /// 不带 `anonymous` 名字——规范经 OrdinaryFunctionCreate 创建函数，
    /// 函数体内不存在指向自身的 `anonymous` 绑定（Node 中
    /// `new Function("return typeof anonymous")()` 为 `"undefined"`）。
    pub compile_source: String,
    /// ExpectedArgumentCount（§15.1.5）：首个带默认值或 rest 形参之前的形参数。
    pub expected_length: u32,
}

/// 校验并拼接动态函数源码。`Err` 携带 SyntaxError 消息。
pub fn prepare_dynamic_function(
    parameters: &str,
    body: &str,
) -> Result<DynamicFunctionSource, String> {
    // 形参独立解析（FormalParameters 目标符号）：函数体为空，形参中未闭合的
    // 注释/括号无法借函数体文本补全。
    parse_exact_anonymous_function(&format!("function anonymous({parameters}\n) {{\n}}"))
        .map_err(|error| format!("invalid formal parameters for Function constructor: {error}"))?;
    // 函数体独立解析（FunctionBody 目标符号）：形参为空，函数体无法借形参文本
    // 提前闭合函数边界。
    parse_exact_anonymous_function(&format!("function anonymous(\n) {{\n{body}\n}}"))
        .map_err(|error| format!("invalid function body for Function constructor: {error}"))?;
    // 规范 sourceText 整体解析：形参与函数体的组合仍须恰好构成一个函数。
    let function =
        parse_exact_anonymous_function(&format!("function anonymous({parameters}\n) {{\n{body}\n}}"))
            .map_err(|error| format!("invalid Function constructor source: {error}"))?;
    check_strict_early_errors(&function)?;
    Ok(DynamicFunctionSource {
        compile_source: format!("(function({parameters}\n) {{\n{body}\n}});"),
        expected_length: expected_argument_count(&function.params),
    })
}

/// 解析 `source` 并要求其恰好是一个名为 `anonymous` 的普通函数声明。
fn parse_exact_anonymous_function(source: &str) -> Result<swc_ast::Function, String> {
    let module =
        wjsm_parser::parse_script_as_module(source).map_err(|error| error.to_string())?;
    let mut items = module.body.into_iter();
    let (Some(item), None) = (items.next(), items.next()) else {
        return Err("source text is not a single function".into());
    };
    let swc_ast::ModuleItem::Stmt(swc_ast::Stmt::Decl(swc_ast::Decl::Fn(fn_decl))) = item else {
        return Err("source text is not a single function".into());
    };
    if fn_decl.ident.sym.as_ref() != "anonymous" {
        return Err("source text is not a single function".into());
    }
    Ok(*fn_decl.function)
}

/// 函数体带 `"use strict"` 指令序言时的形参早错误（§15.2.1、§20.2.1.1.1
/// 步骤 20）：非简单形参列表、重复形参名、`eval`/`arguments` 形参名。
fn check_strict_early_errors(function: &swc_ast::Function) -> Result<(), String> {
    let body_is_strict = function
        .body
        .as_ref()
        .is_some_and(|body| stmts_have_use_strict(&body.stmts));
    if !body_is_strict {
        return Ok(());
    }
    let mut names: Vec<&str> = Vec::with_capacity(function.params.len());
    for param in &function.params {
        let swc_ast::Pat::Ident(binding) = &param.pat else {
            return Err(
                "Illegal 'use strict' directive in function with non-simple parameter list".into(),
            );
        };
        names.push(binding.id.sym.as_ref());
    }
    for (index, name) in names.iter().enumerate() {
        if matches!(*name, "eval" | "arguments") {
            return Err("Unexpected eval or arguments in strict mode".into());
        }
        if names[..index].contains(name) {
            return Err("Duplicate parameter name not allowed in this context".into());
        }
    }
    Ok(())
}

/// ExpectedArgumentCount：统计首个带初始化器或 rest 形参之前的形参个数。
fn expected_argument_count(params: &[swc_ast::Param]) -> u32 {
    let mut count = 0u32;
    for param in params {
        if matches!(
            param.pat,
            swc_ast::Pat::Assign(_) | swc_ast::Pat::Rest(_)
        ) {
            break;
        }
        count += 1;
    }
    count
}

#[cfg(test)]
mod tests {
    use super::prepare_dynamic_function;

    #[test]
    fn accepts_plain_parameters_and_body() {
        let prepared = prepare_dynamic_function("a, b", "return a + b").expect("should parse");
        assert_eq!(prepared.expected_length, 2);
        assert_eq!(
            prepared.compile_source,
            "(function(a, b\n) {\nreturn a + b\n});"
        );
    }

    #[test]
    fn expected_length_stops_at_default_and_rest() {
        assert_eq!(
            prepare_dynamic_function("a,b=1,c", "return 1")
                .expect("should parse")
                .expected_length,
            1
        );
        assert_eq!(
            prepare_dynamic_function("x,...rest", "return 1")
                .expect("should parse")
                .expected_length,
            1
        );
        assert_eq!(
            prepare_dynamic_function("{x},[y]", "return 1")
                .expect("should parse")
                .expected_length,
            2
        );
    }

    #[test]
    fn rejects_comment_injection_through_parameters() {
        assert!(prepare_dynamic_function("/*", "*/) {").is_err());
    }

    #[test]
    fn rejects_body_escaping_function_boundary() {
        assert!(prepare_dynamic_function("a", "}, function evil() {").is_err());
        assert!(prepare_dynamic_function("a", "} function evil() {").is_err());
    }

    #[test]
    fn rejects_parameter_escaping_function_boundary() {
        assert!(prepare_dynamic_function("a\n) {} function evil(", "return 1").is_err());
    }

    #[test]
    fn allows_line_comment_in_parameters() {
        assert!(prepare_dynamic_function("a // comment", "return a").is_ok());
    }

    #[test]
    fn strict_body_rejects_non_simple_duplicate_and_eval_parameters() {
        assert!(prepare_dynamic_function("a = 1", "'use strict'; return a").is_err());
        assert!(prepare_dynamic_function("a, a", "'use strict'; return a").is_err());
        assert!(prepare_dynamic_function("eval", "'use strict';").is_err());
        assert!(prepare_dynamic_function("a, a", "return a").is_ok());
    }
}
