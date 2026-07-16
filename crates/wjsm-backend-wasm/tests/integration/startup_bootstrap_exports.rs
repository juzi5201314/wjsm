use std::collections::HashSet;

#[test]
fn startup_bootstrap_exports_are_present() {
    let module = wjsm_parser::parse_module("console.log('startup');").expect("parse");
    let program = wjsm_semantic::lower_module(module, false).expect("lower");
    let wasm = wjsm_backend_wasm::compile(&program).expect("compile");

    let mut exports = HashSet::new();
    for payload in wasmparser::Parser::new(0).parse_all(&wasm) {
        let wasmparser::Payload::ExportSection(section) = payload.expect("payload") else {
            continue;
        };
        for export in section {
            let export = export.expect("export");
            exports.insert(export.name.to_string());
        }
    }

    for required in [
        "__wjsm_bootstrap_once",
        "__wjsm_init_function_props",
        "__function_props_base",
        "__bootstrap_done",
        "__function_props_done",
        "__arr_proto_table_base",
        "__arr_proto_table_len",
        "__arr_proto_table_hash",
    ] {
        assert!(exports.contains(required), "missing export {required}");
    }
}
