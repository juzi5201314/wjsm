//! builtin 段种子 lower（hydration 核心）单元测试。
//!
//! 覆盖 [`wjsm_semantic::lower_modules_with_builtin_seed`] 与
//! [`wjsm_semantic::lower_modules_with_debug_meta`]：
//! - 段函数/常量预装（段函数在前、用户函数在后）；
//! - **段入口函数体 inline 进用户 `$module_main` 入口块**：builtin 模块作用域变量
//!   （`$N.x`）的 StoreVar 与用户 LoadVar 落在同一函数（同一批 wasm local），
//!   跨函数可见性与 plain 路径一致（后端 Normal 模式 var 是每函数 local，跨函数
//!   LoadVar/StoreVar 无法共享，故不做 entry Call）；
//! - 注入的 export_map 让用户 import 解析命中 builtin 导出。

use std::collections::{BTreeSet, HashMap};

use wjsm_ir::{
    BasicBlock, BasicBlockId, Constant, Function, FunctionId, ImportBinding, Instruction,
    ModuleId, Terminator, ValueId,
};
use wjsm_parser::parse_module;
use wjsm_semantic::{
    BuiltinSegment, ModuleKind, ModuleLoweringInput, ModuleMetadata, lower_modules_with_builtin_seed,
    lower_modules_with_debug_meta,
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

/// 断言 `function` 内同时出现对 `var_name` 的 StoreVar（builtin 初始化写入）与
/// LoadVar（用户 import 读取）——跨函数可见性修复的核心不变量。
fn assert_same_function_store_and_load(function: &Function, var_name: &str) {
    let mut stores = 0;
    let mut loads = 0;
    for block in function.blocks() {
        for instruction in block.instructions() {
            match instruction {
                Instruction::StoreVar { name, .. } if name == var_name => stores += 1,
                Instruction::LoadVar { name, .. } if name == var_name => loads += 1,
                _ => {}
            }
        }
    }
    assert!(
        stores >= 1,
        "{var_name} 应在 {function:?} 内有 builtin 初始化的 StoreVar"
    );
    assert!(
        loads >= 1,
        "{var_name} 应在 {function:?} 内有用户 import 的 LoadVar"
    );
}

#[test]
fn seed_lower_inlines_builtin_entry_into_user_main() {
    let builtin = minimal_builtin_segment();

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
    // builtin 段函数在前（保持段内顺序），入口 $builtin_main 是 functions[0]（死代码保留）
    assert_eq!(functions[0].name(), "$builtin_main");
    // 用户函数在后，最后一个函数是用户 $module_main
    let main_fn = functions
        .last()
        .expect("合并程序至少含用户 $module_main");
    assert_eq!(main_fn.name(), wjsm_ir::MODULE_ENTRY_IR_NAME);
    assert!(functions.len() >= 2);

    // 入口块首部即 builtin 初始化（Const 42 → StoreVar $1.answer），随后才是用户内容。
    let bb0 = &main_fn.blocks()[0];
    let instructions = bb0.instructions();
    assert!(instructions.len() >= 2);
    match &instructions[0] {
        Instruction::Const { constant, .. } => {
            let idx = usize::try_from(constant.0).expect("ConstantId 索引在 usize 内");
            assert_eq!(program.constants()[idx], Constant::Number(42.0));
        }
        other => panic!("入口块首条应为 builtin 的 Const，实际 {other:?}"),
    }
    assert!(matches!(&instructions[1], Instruction::StoreVar { name, .. } if name == "$1.answer"));

    // 核心不变量：builtin 导出的 StoreVar 与用户 LoadVar 在同一函数（同一批 wasm local）。
    assert_same_function_store_and_load(main_fn, "$1.answer");

    // 无 entry Call：inline 取代了跨函数调用，用户 $module_main 不应再有 Call。
    let has_any_call = main_fn
        .blocks()
        .iter()
        .flat_map(|b| b.instructions())
        .any(|i| matches!(i, Instruction::Call { .. }));
    assert!(!has_any_call, "inline 后用户 $module_main 不应再有 Call（含 entry Call）");

    // 注入的 export_map 生效：console.log(answer) 解析为读取 builtin 导出 `$1.answer`。
    let reads_builtin_export = main_fn
        .blocks()
        .iter()
        .flat_map(|b| b.instructions())
        .any(|i| matches!(i, Instruction::LoadVar { name, .. } if name == "$1.answer"));
    assert!(reads_builtin_export);
}

/// 用户模块含顶层 await（TLA）时，入口块是 async main body entry（非 bb0）：
/// inline 的 builtin 初始化必须落在 body entry 首部，StoreVar/LoadVar 同在 main$async。
#[test]
fn seed_lower_with_tla_inlines_into_async_body_entry() {
    let builtin = minimal_builtin_segment();

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
    assert_eq!(functions[0].name(), "$builtin_main");
    assert_eq!(
        functions.last().unwrap().name(),
        wjsm_ir::MODULE_ENTRY_IR_NAME,
        "用户 $module_main 应为合并程序最后一个函数"
    );

    // TLA 形态：$module_main 是 wrapper；实际 inline 体在 main$async。
    let async_fn = functions
        .iter()
        .find(|f| f.name() == "main$async")
        .unwrap_or_else(|| panic!("TLA 种子 lower 应生成 main$async"));

    // 跨函数可见性不变量：builtin 导出 var 的 StoreVar 与用户 LoadVar 同在 main$async。
    assert_same_function_store_and_load(async_fn, "$1.answer");

    // 用户代码经注入的 export_map 读取 builtin 导出。
    let reads_builtin_export = async_fn
        .blocks()
        .iter()
        .flat_map(|b| b.instructions())
        .any(|i| matches!(i, Instruction::LoadVar { name, .. } if name == "$1.answer"));
    assert!(reads_builtin_export);
}

/// round-trip：用真实 multi-module lower 的产物构造 builtin 段（模拟 A2b 的
/// build_builtin_segment），回灌给种子 lower，验证跨函数变量可见性在真实段形状下成立。
#[test]
fn seed_lower_round_trip_real_segment_shares_module_vars() {
    // 1) 用 lower_modules_with_debug_meta 生成一个"builtin 段"（多模块 + 函数 +
    //    顶层控制流 if + 三目 phi，强制 inline 手术做块/phi 重映射）。
    let segment_source_a = "export function makeCounter() { let n = 0; return () => ++n; }\nexport const base = 10;\nlet warmed = false;\nif (base > 5) { warmed = true; }\nexport const label = base > 5 ? 'big' : 'small';\nexport function isWarmed() { return warmed; }\n";
    let segment_source_b = "import { base } from './seg_a.js';\nexport function addBase(x) { return x + base; }\n";
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
    segment_export_names.insert(ModuleId(1), BTreeSet::from(["base".to_string(), "makeCounter".to_string()]));
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
    let entry_function_id = FunctionId(
        u32::try_from(segment_program.functions().len() - 1).expect("函数数在 u32 内"),
    );
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

    program.verify().expect("merged round-trip program should verify");

    // 3) 跨函数可见性：builtin 段的模块作用域变量在用户 $module_main 内 store+load 同函数。
    //    段内模块作用域 id 来自 meta.module_scopes（seg_a 的顶层作用域）。
    let main_fn = program
        .functions()
        .iter()
        .find(|f| f.name() == wjsm_ir::MODULE_ENTRY_IR_NAME)
        .unwrap_or_else(|| panic!("应生成用户 $module_main"));
    // 导出 base 的 IR 变量名来自注入的 export_map（seg_a 模块顶层作用域）。
    let base_ir_name = "$1.base".to_string();
    assert_same_function_store_and_load(main_fn, &base_ir_name);
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
