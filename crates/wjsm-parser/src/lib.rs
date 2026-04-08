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
        Syntax::Typescript(Default::default()),
        Default::default(),
        StringInput::from(&*fm),
        None,
    );

    let mut parser = Parser::new_from(lexer);
    parser
        .parse_module()
        .map_err(|error| anyhow::anyhow!("Parse error: {:?}", error))
}
