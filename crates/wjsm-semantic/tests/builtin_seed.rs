//! builtin 段种子 lower（hydration 核心）单元测试。
//!
//! 覆盖 [`wjsm_semantic::lower_modules_with_builtin_seed`] 与
//! [`wjsm_semantic::lower_modules_with_debug_meta`]：
//! - 段函数/常量预装（段函数在前、用户函数在后）；
//! - **用户 `$module_main` 入口块头部 Call `$builtin_main`**：builtin 模块作用域
//!   变量（`$N.x`）的 StoreVar 留在 `$builtin_main`，用户 LoadVar 留在
//!   `$module_main`，同槽名跨函数共享；
//! - 注入的 export_map 让用户 import 解析命中 builtin 导出。

use std::collections::{BTreeSet, HashMap};

use wjsm_ir::{
    BasicBlock, BasicBlockId, Constant, Function, FunctionId, ImportBinding, Instruction, ModuleId,
    Terminator, ValueId,
};
use wjsm_parser::parse_module;
use wjsm_semantic::{
    BuiltinSegment, ModuleKind, ModuleLoweringInput, ModuleMetadata,
    lower_modules_with_builtin_seed, lower_modules_with_debug_meta,
};

fn esm_input(id: u32, filename: &str, source: &str) -> ModuleLoweringInput {
    let dirname = filename.rsplit_once('/').map_or("/project", |(d, _)| d);
    ModuleLoweringInput {
        id: ModuleId(id),
        ast: parse_module(source).expect("source should parse"),
        metadata: ModuleMetadata {
            filename: filename.to_string(),
            dirname: dirname.to_string(),
            url: format!("file://{filename}"),
            kind: ModuleKind::Esm,
        },
        source: None,
    }
}

/// 手工构造一个最小 builtin 段：1 个 `$builtin_main` 函数
/// （`v0 = const 42; store var $1.answer = v0; return`）+ 1 个常量。
/// 作用域布局：root(0) + builtin 模块顶层作用域(1) → scope_count = 2。
/// 导出：模块 `ModuleId(100)` 导出 `answer` → `$1.answer`。
fn minimal_builtin_segment() -> BuiltinSegment {
    let mut program = wjsm_ir::Program::new();
    let num_const = program.add_constant(Constant::Number(42.0));

    let mut entry = Function::new("$builtin_main", BasicBlockId(0));
    let mut bb0 = BasicBlock::new(BasicBlockId(0));
    bb0.push_instruction(Instruction::Const {
        dest: ValueId(0),
        constant: num_const,
    });
    bb0.push_instruction(Instruction::StoreVar {
        name: "$1.answer".to_string(),
        value: ValueId(0),
    });
    bb0.set_terminator(Terminator::Return { value: None });
    entry.push_block(bb0);
    let entry_function_id = program.push_function(entry);
    assert_eq!(entry_function_id, FunctionId(0), "段入口应为段内第一个函数");

    let mut export_map = HashMap::new();
    export_map.insert(
        (ModuleId(100), "answer".to_string()),
        "$1.answer".to_string(),
    );
    let mut module_export_names = HashMap::new();
    module_export_names.insert(ModuleId(100), BTreeSet::from(["answer".to_string()]));
    let mut module_scopes = HashMap::new();
    module_scopes.insert(ModuleId(100), 1usize);

    BuiltinSegment {
        program,
        scope_count: 2,
        entry_function_id,
        export_map,
        module_export_names,
        module_scopes,
    }
}

fn builtin_import_map() -> HashMap<ModuleId, Vec<ImportBinding>> {
    let mut import_map = HashMap::new();
    import_map.insert(
        ModuleId(0),
        vec![ImportBinding {
            source_module: ModuleId(100),
            names: vec![("answer".to_string(), "answer".to_string())],
            specifier: "node:builtin-fixture".to_string(),
        }],
    );
    import_map
}

fn function_has_store(function: &Function, var_name: &str) -> bool {
    function.blocks().iter().flat_map(|block| block.instructions()).any(
        |instruction| matches!(instruction, Instruction::StoreVar { name, .. } if name == var_name),
    )
}

fn function_has_load(function: &Function, var_name: &str) -> bool {
    function.blocks().iter().flat_map(|block| block.instructions()).any(
        |instruction| matches!(instruction, Instruction::LoadVar { name, .. } if name == var_name),
    )
}

fn assert_entry_calls_builtin(program: &wjsm_ir::Program, function: &Function, entry_id: FunctionId) {
    let instructions = function
        .blocks()
        .iter()
        .map(|block| block.instructions())
        .find(|instructions| {
            matches!(
                instructions.first(),
                Some(Instruction::Const { constant, .. })
                    if program.constants()[usize::try_from(constant.0).expect("ConstantId 索引在 usize 内")]
                        == Constant::FunctionRef(entry_id)
            )
        })
        .expect("用户入口函数必须含 FunctionRef($builtin_main) 前缀块");
    assert!(
        instructions.len() >= 3,
        "用户入口块至少含 FunctionRef + Undefined + Call"
    );
    match &instructions[1] {
        Instruction::Const { constant, .. } => {
            let idx = usize::try_from(constant.0).expect("ConstantId 索引在 usize 内");
            assert_eq!(
                program.constants()[idx],
                Constant::Undefined,
                "入口块第二条应为 Const Undefined"
            );
        }
        other => panic!("入口块第二条应为 Const Undefined，实际 {other:?}"),
    }
    assert!(
        matches!(&instructions[2], Instruction::Call { args, .. } if args.is_empty()),
        "入口块第三条应为无参 Call"
    );
}

#[test]
fn seed_lower_calls_builtin_entry_from_user_main() {
    let builtin = minimal_builtin_segment();
    let entry_function_id = builtin.entry_function_id;

    let program = lower_modules_with_builtin_seed(
        vec![esm_input(
            0,
            "/project/main.js",
            "import { answer } from 'node:builtin-fixture';\nconsole.log(answer);\n",
        )],
        &builtin_import_map(),
        &HashMap::new(),
        &HashMap::new(),
        &HashMap::new(),
        &HashMap::new(),
        builtin,
        false,
    )
    .expect("seed lower should succeed");

    program.verify().expect("merged program should verify");

    let functions = program.functions();
    let builtin_main = functions
        .iter()
        .find(|function| function.name() == "$builtin_main")
        .expect("合并程序必须保留 $builtin_main");
    assert!(function_has_store(builtin_main, "$1.answer"));

    let main_fn = functions.last().expect("合并程序至少含用户 $module_main");
    assert_eq!(main_fn.name(), wjsm_ir::MODULE_ENTRY_IR_NAME);
    assert!(functions.len() >= 2);
    assert_entry_calls_builtin(&program, main_fn, entry_function_id);
    assert!(function_has_load(main_fn, "$1.answer"));
    assert!(!function_has_store(main_fn, "$1.answer"));
}

/// 用户模块含顶层 await（TLA）时，入口块是 async main body entry（非 bb0）：
/// Call `$builtin_main` 必须落在 body entry 首部；StoreVar 仍在 `$builtin_main`。
#[test]
fn seed_lower_with_tla_calls_builtin_from_async_body_entry() {
    let builtin = minimal_builtin_segment();
    let entry_function_id = builtin.entry_function_id;

    let program = lower_modules_with_builtin_seed(
        vec![esm_input(
            0,
            "/project/main.js",
            "import { answer } from 'node:builtin-fixture';\nconsole.log(answer);\nawait Promise.resolve();\n",
        )],
        &builtin_import_map(),
        &HashMap::new(),
        &HashMap::new(),
        &HashMap::new(),
        &HashMap::new(),
        builtin,
        false,
    )
    .expect("seed lower with TLA should succeed");

    program.verify().expect("merged TLA program should verify");

    let functions = program.functions();
    let builtin_main = functions
        .iter()
        .find(|function| function.name() == "$builtin_main")
        .expect("合并程序必须保留 $builtin_main");
    assert!(function_has_store(builtin_main, "$1.answer"));

    let async_fn = functions
        .iter()
        .find(|function| function.name() == "main$async")
        .unwrap_or_else(|| panic!("TLA 种子 lower 应生成 main$async"));
    assert_entry_calls_builtin(&program, async_fn, entry_function_id);
    assert!(function_has_load(async_fn, "$1.answer"));
    assert!(!function_has_store(async_fn, "$1.answer"));
}

/// round-trip：用真实 multi-module lower 的产物构造 builtin 段（模拟 A2b 的
/// build_builtin_segment），回灌给种子 lower，验证跨函数变量可见性在真实段形状下成立。
#[test]
fn seed_lower_round_trip_real_segment_shares_module_vars() {
    // 1) 用 lower_modules_with_debug_meta 生成一个"builtin 段"（多模块 + 函数 +
    //    顶层控制流 if + 三目 phi，强制 inline 手术做块/phi 重映射）。
    let segment_source_a = "export function makeCounter() { let n = 0; return () => ++n; }\nexport const base = 10;\nlet warmed = false;\nif (base > 5) { warmed = true; }\nexport const label = base > 5 ? 'big' : 'small';\nexport function isWarmed() { return warmed; }\n";
    let segment_source_b =
        "import { base } from './seg_a.js';\nexport function addBase(x) { return x + base; }\n";
    let mut segment_import_map = HashMap::new();
    segment_import_map.insert(
        ModuleId(2),
        vec![ImportBinding {
            source_module: ModuleId(1),
            names: vec![("base".to_string(), "base".to_string())],
            specifier: "./seg_a.js".to_string(),
        }],
    );
    let mut segment_export_names = HashMap::new();
    segment_export_names.insert(
        ModuleId(1),
        BTreeSet::from(["base".to_string(), "makeCounter".to_string()]),
    );
    segment_export_names.insert(ModuleId(2), BTreeSet::from(["addBase".to_string()]));

    let (segment_program, meta) = lower_modules_with_debug_meta(
        vec![
            esm_input(1, "/__wjsm_builtin__/node/seg_a.mjs", segment_source_a),
            esm_input(2, "/__wjsm_builtin__/node/seg_b.mjs", segment_source_b),
        ],
        &segment_import_map,
        &HashMap::new(),
        &segment_export_names,
        &HashMap::new(),
        &HashMap::new(),
        false,
    )
    .expect("segment lower should succeed");
    segment_program
        .verify()
        .expect("segment program should verify");

    // 段入口 = 最后一个函数（finalize 最后 push 的 $module_main），改名为 $builtin_main。
    let entry_function_id =
        FunctionId(u32::try_from(segment_program.functions().len() - 1).expect("函数数在 u32 内"));
    let mut segment_program = segment_program;
    segment_program
        .function_mut(entry_function_id)
        .expect("入口函数存在")
        .set_name("$builtin_main");

    let builtin = BuiltinSegment {
        program: segment_program,
        scope_count: u32::try_from(meta.scope_count).expect("scope 数在 u32 内"),
        entry_function_id,
        export_map: meta.export_map,
        module_export_names: segment_export_names,
        module_scopes: meta.module_scopes,
    };

    // 2) 用户模块 import 该段的导出。
    let mut user_import_map = HashMap::new();
    user_import_map.insert(
        ModuleId(0),
        vec![ImportBinding {
            source_module: ModuleId(1),
            names: vec![
                ("makeCounter".to_string(), "makeCounter".to_string()),
                ("base".to_string(), "base".to_string()),
            ],
            specifier: "node:seg_a".to_string(),
        }],
    );

    let program = lower_modules_with_builtin_seed(
        vec![esm_input(
            0,
            "/project/main.js",
            "import { makeCounter, base } from 'node:seg_a';\nconst c = makeCounter();\nconsole.log(c(), base);\n",
        )],
        &user_import_map,
        &HashMap::new(),
        &HashMap::new(),
        &HashMap::new(),
        &HashMap::new(),
        builtin,
        false,
    )
    .expect("round-trip seed lower should succeed");

    program
        .verify()
        .expect("merged round-trip program should verify");

    // 3) 跨函数、同槽名：builtin 段 StoreVar `$1.base`，用户 `$module_main` LoadVar 同名。
    let builtin_main = program
        .functions()
        .iter()
        .find(|function| function.name() == "$builtin_main")
        .expect("应保留 $builtin_main");
    let main_fn = program
        .functions()
        .iter()
        .find(|function| function.name() == wjsm_ir::MODULE_ENTRY_IR_NAME)
        .unwrap_or_else(|| panic!("应生成用户 $module_main"));
    let base_ir_name = "$1.base";
    assert!(function_has_store(builtin_main, base_ir_name));
    assert!(function_has_load(main_fn, base_ir_name));
    assert!(!function_has_store(main_fn, base_ir_name));
}

#[test]
fn debug_meta_collects_export_map_module_scopes_and_scope_count() {
    let mut import_map = HashMap::new();
    import_map.insert(
        ModuleId(0),
        vec![ImportBinding {
            source_module: ModuleId(1),
            names: vec![("v".to_string(), "v".to_string())],
            specifier: "./dep.js".to_string(),
        }],
    );
    let mut export_names = HashMap::new();
    export_names.insert(ModuleId(1), BTreeSet::from(["v".to_string()]));

    let (program, metadata) = lower_modules_with_debug_meta(
        vec![
            esm_input(1, "/project/dep.js", "export const v = 7;\n"),
            esm_input(
                0,
                "/project/main.js",
                "import { v } from './dep.js';\nconsole.log(v);\n",
            ),
        ],
        &import_map,
        &HashMap::new(),
        &export_names,
        &HashMap::new(),
        &HashMap::new(),
        false,
    )
    .expect("meta lower should succeed");

    program.verify().expect("program should verify");

    // export_map 含 dep 模块导出（dep 先 predeclare → 顶层作用域 id 1）
    assert_eq!(
        metadata.export_map.get(&(ModuleId(1), "v".to_string())),
        Some(&"$1.v".to_string())
    );
    // module_scopes 覆盖两个用户模块
    assert!(metadata.module_scopes.contains_key(&ModuleId(0)));
    assert!(metadata.module_scopes.contains_key(&ModuleId(1)));
    // scope_count = root + 2 个模块作用域（含可能的分支作用域）≥ 3
    assert!(metadata.scope_count >= 3);
}
