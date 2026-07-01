mod diagnostic;

use anyhow::Result;
use swc_core::common::sync::Lrc;
use swc_core::common::{FileName, SourceMap};
use swc_core::ecma::ast as swc_ast;
use swc_core::ecma::parser::{EsSyntax, Parser, StringInput, Syntax, TsSyntax, lexer::Lexer};

pub use diagnostic::format_byte_diagnostic;

fn parse_module_inner(
    cm: &Lrc<SourceMap>,
    filename: &str,
    source: &str,
    syntax: Syntax,
    script: bool,
) -> Result<swc_ast::Module> {
    let fm = cm.new_source_file(
        FileName::Custom(filename.to_string()).into(),
        source.to_string(),
    );

    let lexer = Lexer::new(syntax, Default::default(), StringInput::from(&*fm), None);

    let mut parser = Parser::new_from(lexer);
    if script {
        let script_ast = parser.parse_script().map_err(|error| {
            anyhow::anyhow!(diagnostic::format_parse_error(cm, filename, source, error))
        })?;
        Ok(swc_ast::Module {
            span: script_ast.span,
            body: script_ast
                .body
                .into_iter()
                .map(swc_ast::ModuleItem::Stmt)
                .collect(),
            shebang: script_ast.shebang,
        })
    } else {
        parser.parse_module().map_err(|error| {
            anyhow::anyhow!(diagnostic::format_parse_error(cm, filename, source, error))
        })
    }
}

pub fn parse_module(source: &str) -> Result<swc_ast::Module> {
    let cm: Lrc<SourceMap> = Default::default();
    parse_module_inner(
        &cm,
        "input.ts",
        source,
        Syntax::Typescript(swc_core::ecma::parser::TsSyntax {
            tsx: true,
            ..Default::default()
        }),
        false,
    )
}

/// 根据文件路径选择 SWC 语法模式。
fn syntax_for_path(path: &std::path::Path) -> Syntax {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());

    match ext.as_deref() {
        Some("ts") => Syntax::Typescript(TsSyntax {
            tsx: false,
            ..Default::default()
        }),
        Some("tsx") => Syntax::Typescript(TsSyntax {
            tsx: true,
            ..Default::default()
        }),
        Some("jsx") => Syntax::Es(EsSyntax {
            jsx: true,
            allow_super_outside_method: true,
            ..Default::default()
        }),
        Some("js") | Some("mjs") | Some("cjs") => Syntax::Es(EsSyntax {
            jsx: false,
            allow_super_outside_method: true,
            ..Default::default()
        }),
        _ => Syntax::Typescript(TsSyntax {
            tsx: true,
            ..Default::default()
        }),
    }
}

fn syntax_for_filename(filename: &str) -> Syntax {
    syntax_for_path(std::path::Path::new(filename))
}

/// 按文件名扩展名选择 TypeScript / ECMAScript 语法解析模块。
pub fn parse_module_with_filename(source: &str, filename: &str) -> Result<swc_ast::Module> {
    let cm: Lrc<SourceMap> = Default::default();
    parse_module_inner(&cm, filename, source, syntax_for_filename(filename), false)
}

/// 按路径扩展名选择 TypeScript / ECMAScript 语法解析模块。
pub fn parse_module_with_path(source: &str, path: &std::path::Path) -> Result<swc_ast::Module> {
    let cm: Lrc<SourceMap> = Default::default();
    let filename = path.display().to_string();
    parse_module_inner(&cm, &filename, source, syntax_for_path(path), false)
}
/// 以 Script 模式解析源码并转换为 Module。
/// Script 模式下 `await` 在非 async 上下文中是合法标识符，
/// 适用于 test262 等需要严格 ECMAScript 合规性的场景。
pub fn parse_script_as_module(source: &str) -> Result<swc_ast::Module> {
    let cm: Lrc<SourceMap> = Default::default();
    parse_module_inner(
        &cm,
        "input.js",
        source,
        Syntax::Es(swc_core::ecma::parser::EsSyntax {
            allow_super_outside_method: true,
            ..Default::default()
        }),
        true,
    )
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
        assert!(message.starts_with("error: "));
        assert!(message.contains(" --> input.ts:"));
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
