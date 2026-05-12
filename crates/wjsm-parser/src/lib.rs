use anyhow::Result;
use swc_core::common::sync::Lrc;
use swc_core::common::{FileName, SourceMap};
use swc_core::ecma::ast as swc_ast;
use swc_core::ecma::parser::{Parser, StringInput, Syntax, lexer::Lexer};

pub fn parse_module(source: &str) -> Result<swc_ast::Module> {
    let cm: Lrc<SourceMap> = Default::default();
    let fm = cm.new_source_file(
        FileName::Custom("input.ts".into()).into(),
        source.to_string(),
    );

    let lexer = Lexer::new(
        Syntax::Typescript(swc_core::ecma::parser::TsSyntax {
            tsx: true,
            ..Default::default()
        }),
        Default::default(),
        StringInput::from(&*fm),
        None,
    );

    let mut parser = Parser::new_from(lexer);
    parser
        .parse_module()
        .map_err(|error| anyhow::anyhow!("Parse error: {:?}", error))
}
/// 以 Script 模式解析源码并转换为 Module。
/// Script 模式下 `await` 在非 async 上下文中是合法标识符，
/// 适用于 test262 等需要严格 ECMAScript 合规性的场景。
pub fn parse_script_as_module(source: &str) -> Result<swc_ast::Module> {
    let cm: Lrc<SourceMap> = Default::default();
    let fm = cm.new_source_file(
        FileName::Custom("input.js".into()).into(),
        source.to_string(),
    );

    let lexer = Lexer::new(
        Syntax::Es(Default::default()),
        Default::default(),
        StringInput::from(&*fm),
        None,
    );

    let mut parser = Parser::new_from(lexer);
    let script = parser
        .parse_script()
        .map_err(|error| anyhow::anyhow!("Parse error: {:?}", error))?;

    Ok(swc_ast::Module {
        span: script.span,
        body: script.body.into_iter().map(swc_ast::ModuleItem::Stmt).collect(),
        shebang: script.shebang,
    })
}

#[cfg(test)]
mod tests {
    use super::parse_module;
    use swc_core::ecma::ast as swc_ast;

    fn first_module_decl(source: &str) -> swc_ast::ModuleDecl {
        let module = parse_module(source).expect("parser should accept fixture");
        match module
            .body
            .into_iter()
            .next()
            .expect("fixture should produce one module item")
        {
            swc_ast::ModuleItem::ModuleDecl(decl) => decl,
            swc_ast::ModuleItem::Stmt(_) => panic!("expected module declaration"),
        }
    }

    #[test]
    fn parses_console_log_module() {
        let module =
            parse_module(r#"console.log("hello");"#).expect("parser should accept fixture");
        assert_eq!(module.body.len(), 1);
    }

    #[test]
    fn normalizes_parse_errors() {
        let error = parse_module(r#"console.log("missing closing paren";"#)
            .expect_err("parser should reject invalid syntax");

        let message = error.to_string();
        assert!(message.starts_with("Parse error: "));
        assert!(message.contains("Expected"));
    }

    #[test]
    fn parses_import_named_decl() {
        let decl = first_module_decl(r#"import { foo } from "./lib.js";"#);
        assert!(matches!(decl, swc_ast::ModuleDecl::Import(_)));
    }

    #[test]
    fn parses_import_default_decl() {
        let decl = first_module_decl(r#"import foo from "./lib.js";"#);
        assert!(matches!(decl, swc_ast::ModuleDecl::Import(_)));
    }

    #[test]
    fn parses_export_decl() {
        let decl = first_module_decl(r#"export const answer = 42;"#);
        assert!(matches!(decl, swc_ast::ModuleDecl::ExportDecl(_)));
    }

    #[test]
    fn parses_export_named_decl() {
        let decl = first_module_decl(r#"export { foo };"#);
        assert!(matches!(decl, swc_ast::ModuleDecl::ExportNamed(_)));
    }

    #[test]
    fn parses_export_default_decl() {
        let decl = first_module_decl(r#"export default 42;"#);
        assert!(matches!(
            decl,
            swc_ast::ModuleDecl::ExportDefaultExpr(_) | swc_ast::ModuleDecl::ExportDefaultDecl(_)
        ));
    }
}
