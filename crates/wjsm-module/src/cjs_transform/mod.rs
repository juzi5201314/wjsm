mod collector;
mod detector;
mod helpers;
mod transformer;

use swc_core::common::{DUMMY_SP, SyntaxContext};
use swc_core::ecma::ast;
use swc_core::ecma::visit::VisitWith;

pub fn is_commonjs_module(module: &ast::Module) -> bool {
    let mut detector = detector::CjsDetector { found: false };
    module.visit_with(&mut detector);
    detector.found
}

pub fn transform(module: &ast::Module) -> ast::Module {
    transform_with_prefix(module, "")
}

pub fn transform_with_prefix(module: &ast::Module, export_prefix: &str) -> ast::Module {
    let mut collector = collector::RequireCollector {
        require_map: std::collections::BTreeMap::new(),
        next_req_id: 0,
        direct_imports: std::collections::HashSet::new(),
    };
    module.visit_with(&mut collector);

    let mut transformer = transformer::CjsTransformer {
        require_map: collector.require_map,
        direct_imports: collector.direct_imports,
        export_names: Vec::new(),
        has_default_export: false,
        export_prefix: export_prefix.to_string(),
    };
    let mut new_module = transformer.transform_module(module);

    if !transformer.export_names.is_empty() {
        if transformer.has_default_export {
            for (prop_name, var_name) in &transformer.export_names {
                let export_spec = ast::ExportNamedSpecifier {
                    span: DUMMY_SP,
                    orig: ast::ModuleExportName::Ident(ast::Ident::new(
                        var_name.clone().into(),
                        DUMMY_SP,
                        SyntaxContext::default(),
                    )),
                    exported: Some(ast::ModuleExportName::Ident(ast::Ident::new(
                        prop_name.clone().into(),
                        DUMMY_SP,
                        SyntaxContext::default(),
                    ))),
                    is_type_only: false,
                };
                new_module
                    .body
                    .push(ast::ModuleItem::ModuleDecl(ast::ModuleDecl::ExportNamed(
                        ast::NamedExport {
                            span: DUMMY_SP,
                            specifiers: vec![ast::ExportSpecifier::Named(export_spec)],
                            src: None,
                            type_only: false,
                            with: None,
                        },
                    )));
            }
        } else {
            let default_export_expr =
                helpers::create_synthetic_default_export(&transformer.export_names);
            new_module.body.push(ast::ModuleItem::ModuleDecl(
                ast::ModuleDecl::ExportDefaultExpr(ast::ExportDefaultExpr {
                    span: DUMMY_SP,
                    expr: Box::new(default_export_expr),
                }),
            ));
        }
    }

    new_module
}

#[cfg(test)]
mod tests;
