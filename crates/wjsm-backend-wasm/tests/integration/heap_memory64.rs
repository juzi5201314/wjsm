use wasmparser::{Parser, Payload, TypeRef, ValType};
use wjsm_backend_wasm::{GcFlavor, compile, emit_support_module};

const HEAP_MEMORY_NAME: &str = "__heap_memory";
const HEAP_MEMORY_MIN_PAGES: u64 = 32 * 1024 * 1024 * 1024 / (64 * 1024);

#[test]
fn heap_memory64_user_and_support_modules_share_dynamic_heap_import() {
    let user = compile_source("const x = { a: [1, 2, 3] }; console.log(x.a[1]);");
    let support = emit_support_module(GcFlavor::MarkSweep).unwrap();

    for (name, wasm) in [("user", user), ("support", support)] {
        let heap_memory = memory_import(&wasm, HEAP_MEMORY_NAME)
            .unwrap_or_else(|| panic!("{name} module must import env.{HEAP_MEMORY_NAME}"));
        assert!(heap_memory.memory64, "{name} heap memory must be memory64");
        assert!(heap_memory.shared, "{name} heap memory must be shared");
        assert_eq!(heap_memory.initial, HEAP_MEMORY_MIN_PAGES);
        assert_eq!(heap_memory.maximum, Some(1_u64 << 32));
        for global_name in [
            "__heap_alloc_ptr",
            "__heap_alloc_end",
            "__heap_object_start",
            "__heap_limit_v2",
        ] {
            let heap_global = global_import(&wasm, global_name).unwrap_or_else(|| {
                panic!("{name} module must import env.{global_name} as an i64 global")
            });
            assert_eq!(heap_global.content_type, ValType::I64);
            assert!(heap_global.mutable);
        }
    }
}

#[test]
fn support_module_emit_memory64_i64_accesses() {
    let wasm = emit_support_module(GcFlavor::MarkSweep).unwrap();
    let mut has_heap_i64_access = false;

    for payload in Parser::new(0).parse_all(&wasm) {
        let Payload::CodeSectionEntry(body) = payload.unwrap() else {
            continue;
        };
        for operator in body.get_operators_reader().unwrap().into_iter() {
            match operator.unwrap() {
                wasmparser::Operator::I64Load { memarg }
                | wasmparser::Operator::I64Store { memarg }
                | wasmparser::Operator::I64AtomicLoad { memarg }
                | wasmparser::Operator::I64AtomicStore { memarg }
                    if memarg.memory == 2 =>
                {
                    has_heap_i64_access = true;
                }
                _ => {}
            }
        }
    }

    assert!(
        has_heap_i64_access,
        "V2 support helpers must access shared memory64 through i64 addresses"
    );
}

fn compile_source(source: &str) -> Vec<u8> {
    let module = wjsm_parser::parse_module(source).unwrap();
    let program = wjsm_semantic::lower_module(module, false).unwrap();
    compile(&program).unwrap()
}

fn memory_import(wasm: &[u8], expected_name: &str) -> Option<wasmparser::MemoryType> {
    for payload in Parser::new(0).parse_all(wasm) {
        let Payload::ImportSection(section) = payload.unwrap() else {
            continue;
        };
        for import in section.into_imports() {
            let import = import.unwrap();
            if import.module == "env" && import.name == expected_name {
                let TypeRef::Memory(memory) = import.ty else {
                    panic!("env.{expected_name} must be a memory import");
                };
                return Some(memory);
            }
        }
    }
    None
}

fn global_import(wasm: &[u8], expected_name: &str) -> Option<wasmparser::GlobalType> {
    for payload in Parser::new(0).parse_all(wasm) {
        let Payload::ImportSection(section) = payload.unwrap() else {
            continue;
        };
        for import in section.into_imports() {
            let import = import.unwrap();
            if import.module == "env" && import.name == expected_name {
                let TypeRef::Global(global) = import.ty else {
                    panic!("env.{expected_name} must be a global import");
                };
                return Some(global);
            }
        }
    }
    None
}
