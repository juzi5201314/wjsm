//! 运行时类型反馈驱动的单函数特化编译（Issue #390 阶段 3）。
//!
//! generic lowering 走 boxed 路径；本模块提供「profile → overlay wrapper +
//! 投机 body」入口：wrapper 以固定 `NativeSlowEntry` ABI 导出，先校验 `args_count`
//! 与每个 profile 参数的 tag，命中后调用同一 object 内部的 overlay body，
//! 失配则读取当前 base image 的 `function_table[function_index].slow_entry`
//! 回落 generic entry。overlay 只在进程内存在，不进入 `.wjsm`、磁盘 cache 或分发制品。

use std::collections::{BTreeSet, HashMap, HashSet};
use std::mem::{offset_of, size_of};

use anyhow::{Context, Result};
use cranelift_codegen::ir::{InstBuilder, MemFlagsData, UserFuncName, types};
use cranelift_codegen::isa::unwind::UnwindInfo;
use cranelift_control::ControlPlane;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_module::{Linkage, Module, ModuleReloc};
use cranelift_object::{ObjectBuilder, ObjectModule};
use wjsm_ir::{Builtin, Function, FunctionId, Instruction, Program, ValueId, constants};
use wjsm_native_abi::{NativeFeedbackTag, NativeFunctionEntry, NativeVmContext};

use crate::f64_analysis::infer_f64_values_with_param_seeds;
use crate::lower::{
    DeclaredBarrierThunks, DeclaredData, DeclaredFunction, FunctionCompileInput,
    allocate_feedback_slots, allocate_ic_slots, boxed_frame_local_names, compile_one_function,
    declare_barrier_thunks, declare_host_dispatcher, declare_math_thunks, declare_root_bitmaps,
    declare_string_add_thunk, declare_string_builder_finish_thunk, emit_feedback_tag_code,
    gimli_endian, libcall_name, root_frame_capacity, slow_entry_signature,
};
use crate::root_plan::RootPlan;
use crate::safepoint_free::infer_safepoint_free_functions;
use crate::template_meta::build_template_origin_maps;
use crate::unwind::{UnwindPolicy, UnwindRecord, validate_unwind_info, write_object_unwind};
use crate::{NativeCompilationDiagnostics, NativeCompileError, NativeObject};

/// 一条特化 profile：目标函数 + 每个实际参数（跳过 env/this）的反馈 tag。
///
/// tag 序列由反馈槽的 `last_tag_signature` 解码而来，与 wrapper 的入口守卫
/// 逐位一致。任意可证明收益的站点都可以编 overlay，不限于 Number 参数。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpecializationProfile {
    pub function: FunctionId,
    pub argument_tags: Box<[NativeFeedbackTag]>,
    /// 稳定 Number 二元/比较反馈对应的 SSA（dest 与操作数），供无参热函数重建。
    pub extra_numbers: HashSet<ValueId>,
    pub slot_map: Option<wjsm_optimize::SlotMap>,
    pub facts: wjsm_optimize::SpeculativeFacts,
}

/// 特化编译的内部失败：`NoBenefit` 表示宿主应保持 generic，不是 JS 异常。
#[derive(Debug, thiserror::Error)]
pub enum SpecializationError {
    #[error("specialization has no provable benefit: {0}")]
    NoBenefit(&'static str),
    #[error(transparent)]
    Compile(#[from] NativeCompileError),
}

/// 编译一个特化 overlay object：wrapper（`wjsm_function_{N}`，唯一 entry）+
/// typed body（模块内部直调，不注册为 JS function table entry）。
pub(crate) fn compile_specialized(
    isa: cranelift_codegen::isa::OwnedTargetIsa,
    program: &Program,
    variable_slots: &HashMap<String, u32>,
    profile: &SpecializationProfile,
    collect_diagnostics: bool,
) -> Result<NativeCompilationDiagnostics, SpecializationError> {
    let target_index = usize::try_from(profile.function.0)
        .map_err(|_| SpecializationError::NoBenefit("target function index does not fit usize"))?;
    let Some(ir_function) = program.functions().get(target_index) else {
        return Err(SpecializationError::NoBenefit("target function is missing"));
    };
    if profile.argument_tags.len() > constants::FEEDBACK_MAX_TAGS as usize {
        return Err(SpecializationError::NoBenefit(
            "profile arity is out of range",
        ));
    }
    let js_param_count = ir_function.params().len().saturating_sub(2);
    if profile.argument_tags.len() > js_param_count {
        return Err(SpecializationError::NoBenefit(
            "profile tags exceed the parameter count",
        ));
    }

    let mut derived = program.clone();
    let unit = wjsm_optimize::optimize_speculative(&mut derived, &profile.facts);
    let ir_function = derived
        .functions()
        .get(target_index)
        .ok_or(SpecializationError::NoBenefit("cloned target is missing"))?;
    let has_overlay_shape = unit.deopt_map.points.iter().any(|_| true)
        || !profile.facts.get_props.is_empty()
        || !profile.facts.set_props.is_empty()
        || !profile.facts.get_elems.is_empty()
        || !profile.facts.calls.is_empty()
        || !profile.extra_numbers.is_empty()
        || profile
            .argument_tags
            .iter()
            .any(|tag| *tag == NativeFeedbackTag::Number);
    if !has_overlay_shape {
        return Err(SpecializationError::NoBenefit(
            "profile has no speculative sites",
        ));
    }
    let class_seeds = wjsm_ir::value_class::FunctionSeeds {
        param_is_number: profile
            .argument_tags
            .iter()
            .map(|tag| *tag == NativeFeedbackTag::Number)
            .collect(),
        extra_numbers: profile.extra_numbers.clone(),
    };
    let frame_locals: BTreeSet<&str> = derived.frame_local_variable_names(ir_function);
    let classes =
        wjsm_ir::value_class::infer_function(&derived, ir_function, &frame_locals, &class_seeds);
    let mut seeds: HashMap<FunctionId, Vec<bool>> = HashMap::new();
    seeds.insert(profile.function, class_seeds.param_is_number.clone());
    let mut seeded = infer_f64_values_with_param_seeds(&derived, &seeds);
    let target_function_id =
        FunctionId(u32::try_from(target_index).map_err(|_| {
            SpecializationError::NoBenefit("target function index does not fit u32")
        })?);
    let typed_f64_values = seeded.get(&target_function_id).cloned().unwrap_or_default();
    seeded
        .entry(target_function_id)
        .or_default()
        .extend(classes.numbers.iter().copied());
    let seeded_values = seeded.get(&target_function_id).cloned().unwrap_or_default();

    let unwind_policy = UnwindPolicy::for_triple(isa.triple())?;
    let builder = ObjectBuilder::new(
        isa.clone(),
        b"wjsm-native-specialized".to_vec(),
        Box::new(libcall_name),
    )
    .map_err(|error| NativeCompileError::Cranelift(error.to_string()))?;
    let mut module = ObjectModule::new(builder);
    let signature = slow_entry_signature(module.isa().default_call_conv());
    let wrapper_id = module
        .declare_function(
            &format!("wjsm_function_{}", profile.function.0),
            Linkage::Export,
            &signature,
        )
        .map_err(|error| NativeCompileError::Cranelift(error.to_string()))?;
    let body_id = module
        .declare_function("wjsm_specialized_body", Linkage::Export, &signature)
        .map_err(|error| NativeCompileError::Cranelift(error.to_string()))?;
    let host_dispatcher = declare_host_dispatcher(&mut module)?;
    let string_add = declare_string_add_thunk(&mut module)?;
    let string_builder_finish = declare_string_builder_finish_thunk(&mut module)?;
    let (zgc_load_barrier, zgc_store_barrier) = declare_barrier_thunks(&mut module)?;
    let math_thunks = declare_math_thunks(&mut module, &derived, &seeded)?;
    let hypot_property_names: HashSet<String> = wjsm_optimize::collect_hypot_getters(&derived)
        .into_iter()
        .map(|getter| getter.property)
        .collect();

    let boxed_frame_locals =
        boxed_frame_local_names(ir_function, &frame_locals, &seeded, target_index);
    let int32_values = classes.int32s;
    let root_plan = RootPlan::build(ir_function, &seeded_values);
    let root_capacity = root_frame_capacity(ir_function, &root_plan, boxed_frame_locals.len());
    let safepoint_free =
        infer_safepoint_free_functions(&derived, variable_slots).contains(&FunctionId(
            u32::try_from(target_index).map_err(|_| {
                NativeCompileError::Capacity("specialized function index exceeds u32")
            })?,
        ));
    let root_capacity = if safepoint_free { 0 } else { root_capacity };
    let root_bitmaps = declare_root_bitmaps(&mut module, root_capacity)?;
    let dispatcher_decl = DeclaredFunction::snapshot(module.declarations(), host_dispatcher);
    let string_add_decl = DeclaredFunction::snapshot(module.declarations(), string_add);
    let string_builder_finish_decl =
        DeclaredFunction::snapshot(module.declarations(), string_builder_finish);
    let barrier_thunks =
        DeclaredBarrierThunks::snapshot(module.declarations(), zgc_load_barrier, zgc_store_barrier);
    let math_thunk_decls: HashMap<Builtin, DeclaredFunction> = math_thunks
        .iter()
        .map(|(builtin, func_id)| {
            (
                *builtin,
                DeclaredFunction::snapshot(module.declarations(), *func_id),
            )
        })
        .collect();
    let bitmap_decls: Vec<DeclaredData> = root_bitmaps
        .iter()
        .map(|bitmap| DeclaredData::snapshot(module.declarations(), *bitmap))
        .collect();
    let target_config = module.target_config();

    // IC/反馈槽沿用全 Program 编号：overlay 生成代码经由 vmctx 继续写 base
    // image 的 IC/反馈区，编号必须与 base 编译完全一致。
    let (ic_slots, _) = allocate_ic_slots(program);
    let template_origins = build_template_origin_maps(program);
    let feedback_plan = allocate_feedback_slots(program);

    let body = compile_one_function(&FunctionCompileInput {
        isa: &isa,
        target_config,
        program: &derived,
        ir_function,
        index: target_index,
        signature: &signature,
        function_id: body_id,
        dispatcher: &dispatcher_decl,
        string_add: &string_add_decl,
        string_builder_finish: &string_builder_finish_decl,
        barrier_thunks: &barrier_thunks,
        math_thunks: &math_thunk_decls,
        root_bitmaps: &bitmap_decls,
        f64_values: &seeded_values,
        typed_f64_values: &typed_f64_values,
        variable_slots,
        root_plan: &root_plan,
        root_capacity,
        frame_local_names: &frame_locals,
        boxed_local_names: &boxed_frame_locals,
        ic_slots: &ic_slots[target_index],
        template_origins: &template_origins[target_index],
        feedback_slots: feedback_plan.function_slots(target_index),
        specialized_tags: Some(profile.argument_tags.as_ref()),
        slot_map: Some(&unit.slot_map),
        int32_values: &int32_values,
        function_decls: &[],
        direct_callable_functions: &HashSet::new(),
        safepoint_free,
        collect_diagnostics,
        hypot_property_names: &hypot_property_names,
    })?;
    let wrapper = compile_wrapper(
        &isa,
        &signature,
        &module,
        wrapper_id,
        body_id,
        profile,
        collect_diagnostics,
    )?;

    let mut unwind_records: Vec<UnwindRecord> = Vec::with_capacity(2);
    let mut frame_bytes = Vec::with_capacity(2);
    let mut clif = String::new();
    let mut disassembly = String::new();
    for (function_id, output) in [(wrapper_id, wrapper), (body_id, body)] {
        module
            .define_function_bytes(function_id, output.alignment, &output.bytes, &output.relocs)
            .map_err(|error| NativeCompileError::Cranelift(error.to_string()))?;
        frame_bytes.push(output.frame_bytes);
        unwind_records.push(UnwindRecord {
            function: function_id,
            code_len: output.code_len,
            info: output.unwind,
        });
        if collect_diagnostics {
            clif.push_str(&output.clif);
            disassembly.push_str(&output.disassembly);
        }
    }

    let mut product = module.finish();
    let systemv_cie = if unwind_policy == UnwindPolicy::WindowsPdata {
        None
    } else {
        Some(isa.create_systemv_cie().ok_or_else(|| {
            NativeCompileError::CompilerInvariant("ISA cannot create a System V CIE".into())
        })?)
    };
    write_object_unwind(
        &mut product,
        unwind_policy,
        unwind_records,
        systemv_cie,
        gimli_endian(isa.triple()),
    )?;
    let object = product
        .emit()
        .map_err(|error| NativeCompileError::Object(error.to_string()))?;
    Ok(NativeCompilationDiagnostics {
        object: NativeObject {
            bytes: object.into(),
            frame_bytes,
            function_count: 2,
            ic_slot_count: 0,
            feedback_slot_count: 0,
        },
        clif,
        disassembly,
    })
}

/// 编译 wrapper：入口 tag 守卫 → 直调 typed body；失配回落 base generic entry。
fn compile_wrapper(
    isa: &cranelift_codegen::isa::OwnedTargetIsa,
    signature: &cranelift_codegen::ir::Signature,
    module: &ObjectModule,
    wrapper_id: cranelift_module::FuncId,
    body_id: cranelift_module::FuncId,
    profile: &SpecializationProfile,
    collect_diagnostics: bool,
) -> Result<crate::lower::CompiledFunction, NativeCompileError> {
    let mut context = cranelift_codegen::Context::new();
    let mut builder_context = FunctionBuilderContext::new();
    context.set_disasm(collect_diagnostics);
    context.func.signature = signature.clone();
    context.func.name = UserFuncName::user(0, profile.function.0);
    let mut builder = FunctionBuilder::new(&mut context.func, &mut builder_context);
    let entry_block = builder.create_block();
    builder.switch_to_block(entry_block);
    builder.append_block_params_for_function_params(entry_block);
    let params = builder.block_params(entry_block).to_vec();
    let ctx = params[0];
    let env = params[1];
    let this_value = params[2];
    let args_base = params[3];
    let args_count = params[4];
    let pointer_type = builder.func.dfg.value_type(ctx);

    // 守卫 1：实际参数数量必须覆盖全部 tagged 参数（缺失参数语义是 undefined，
    // 与 profile 不符时回落）。
    let tagged_len = i64::try_from(profile.argument_tags.len())
        .map_err(|_| NativeCompileError::Capacity("profile arity"))?;
    let args_count_i64 = builder.ins().uextend(types::I64, args_count);
    let args_count_ok = builder.ins().icmp_imm_u(
        cranelift_codegen::ir::condcodes::IntCC::UnsignedGreaterThanOrEqual,
        args_count_i64,
        tagged_len,
    );

    // 守卫 2：每个 tagged 参数的实际 tag 与 profile 一致。
    let arena_base = builder.ins().load(
        pointer_type,
        MemFlagsData::trusted(),
        ctx,
        i32::try_from(offset_of!(NativeVmContext, call_arena_slots))
            .context("call arena offset exceeds i32")
            .map_err(|error| NativeCompileError::Lowering {
                function: profile.function,
                message: error.to_string(),
            })?,
    );
    let mut all_tags_ok = args_count_ok;
    let args_base_i64 = builder.ins().uextend(types::I64, args_base);
    let args_base_bytes = builder.ins().ishl_imm_u(args_base_i64, 3);
    for (index, tag) in profile.argument_tags.iter().enumerate() {
        let byte_offset = i64::try_from(index)
            .map_err(|_| NativeCompileError::Capacity("profile arity"))?
            .checked_mul(size_of::<i64>() as i64)
            .ok_or(NativeCompileError::Capacity("call arena offset"))?;
        let address = builder.ins().iadd_imm_s(args_base_bytes, byte_offset);
        let address = builder.ins().iadd(arena_base, address);
        let argument = builder
            .ins()
            .load(types::I64, MemFlagsData::trusted(), address, 0);
        let code = emit_feedback_tag_code(&mut builder, argument);
        let expected = builder.ins().iconst(types::I64, i64::from(tag.code()));
        let matches = builder.ins().icmp(
            cranelift_codegen::ir::condcodes::IntCC::Equal,
            code,
            expected,
        );
        all_tags_ok = builder.ins().band(all_tags_ok, matches);
    }

    let body_block = builder.create_block();
    let fallback_block = builder.create_block();
    builder
        .ins()
        .brif(all_tags_ok, body_block, &[], fallback_block, &[]);

    // 命中：直调 typed body（同 object 内部调用，不经函数表）。
    builder.switch_to_block(body_block);
    builder.seal_block(body_block);
    let body_decl = crate::lower::DeclaredFunction::snapshot(module.declarations(), body_id);
    let body_ref = body_decl.import(builder.func);
    let call = builder
        .ins()
        .call(body_ref, &[ctx, env, this_value, args_base, args_count]);
    let result = *builder
        .inst_results(call)
        .first()
        .context("typed body returned no result")
        .map_err(|error| NativeCompileError::Lowering {
            function: profile.function,
            message: error.to_string(),
        })?;
    builder.ins().return_(&[result]);

    // 失配：读取当前 base image 的 function_table[function_index].slow_entry，
    // 以原始五参数调用 generic entry 并返回。这不是帧重建——激活/变量保存协议
    // 已在 PrepareCall 中完成，此处只是替换间接目标。
    builder.switch_to_block(fallback_block);
    builder.seal_block(fallback_block);
    let function_table = builder.ins().load(
        pointer_type,
        MemFlagsData::trusted(),
        ctx,
        i32::try_from(offset_of!(NativeVmContext, function_table))
            .context("function table offset exceeds i32")
            .map_err(|error| NativeCompileError::Lowering {
                function: profile.function,
                message: error.to_string(),
            })?,
    );
    let entry_offset = i64::from(profile.function.0)
        .checked_mul(size_of::<NativeFunctionEntry>() as i64)
        .ok_or(NativeCompileError::Capacity("function table offset"))?;
    let entry_address = builder.ins().iadd_imm_s(function_table, entry_offset);
    let slow_entry = builder.ins().load(
        pointer_type,
        MemFlagsData::trusted(),
        entry_address,
        offset_of!(NativeFunctionEntry, slow_entry) as i32,
    );
    let slow_signature = builder.import_signature(signature.clone());
    let call = builder.ins().call_indirect(
        slow_signature,
        slow_entry,
        &[ctx, env, this_value, args_base, args_count],
    );
    let fallback_result = *builder
        .inst_results(call)
        .first()
        .context("generic entry returned no result")
        .map_err(|error| NativeCompileError::Lowering {
            function: profile.function,
            message: error.to_string(),
        })?;
    builder.ins().return_(&[fallback_result]);

    builder.seal_all_blocks();
    builder.finalize(isa.frontend_config());

    let clif = if collect_diagnostics {
        format!(
            ";; specialized wrapper for function {}\n{}\n",
            profile.function.0,
            context.func.display()
        )
    } else {
        String::new()
    };
    context
        .compile(isa.as_ref(), &mut ControlPlane::default())
        .map_err(|error| NativeCompileError::Cranelift(format!("{:#?}", error.inner)))?;
    let compiled = context
        .compiled_code()
        .ok_or_else(|| NativeCompileError::CompilerInvariant("missing compiled code".into()))?;
    if !compiled.buffer.traps().is_empty() {
        return Err(NativeCompileError::CompilerInvariant(
            "specialized wrapper contains a machine trap".into(),
        ));
    }
    let disassembly = if collect_diagnostics {
        compiled.vcode.as_deref().unwrap_or("").to_owned()
    } else {
        String::new()
    };
    let frame_bytes = compiled
        .buffer
        .frame_layout()
        .ok_or_else(|| {
            NativeCompileError::CompilerInvariant(
                "specialized wrapper is missing frame metadata".into(),
            )
        })?
        .frame_to_fp_offset;
    let unwind: UnwindInfo = compiled
        .create_unwind_info(isa.as_ref())
        .map_err(|error| NativeCompileError::Cranelift(error.to_string()))?
        .ok_or(NativeCompileError::MissingUnwindInfo(profile.function))?;
    validate_unwind_info(isa.triple(), &unwind, profile.function)?;
    let relocs: Vec<ModuleReloc> = compiled
        .buffer
        .relocs()
        .iter()
        .map(|reloc| ModuleReloc::from_mach_reloc(reloc, &context.func, wrapper_id))
        .collect();
    Ok(crate::lower::CompiledFunction {
        alignment: u64::from(compiled.buffer.alignment),
        bytes: compiled.buffer.data().to_vec(),
        relocs,
        frame_bytes,
        code_len: u64::from(compiled.buffer.total_size()),
        unwind,
        clif,
        disassembly,
    })
}

/// 按与 generic 相同的槽编号取出该站点的 IR 位置与指令。
pub(crate) fn feedback_instruction_at(
    program: &Program,
    function_id: FunctionId,
    site_index: u32,
) -> Option<(wjsm_ir::BasicBlockId, u32, Instruction)> {
    let plan = allocate_feedback_slots(program);
    let index = function_id.0 as usize;
    let function = program.functions().get(index)?;
    for ((block_id, instruction_index), slot) in plan.function_slots(index) {
        if *slot != site_index {
            continue;
        }
        let block = function
            .blocks()
            .iter()
            .find(|block| block.id() == *block_id)?;
        let instruction = block.instructions().get(*instruction_index)?.clone();
        return Some((*block_id, *instruction_index as u32, instruction));
    }
    None
}

/// 按与 generic 相同的槽编号取出该站点的 SSA 操作数与 dest。
pub(crate) fn extra_numbers_at_site(
    program: &Program,
    function_id: FunctionId,
    site_index: u32,
) -> HashSet<ValueId> {
    let plan = allocate_feedback_slots(program);
    let index = function_id.0 as usize;
    let Some(function) = program.functions().get(index) else {
        return HashSet::new();
    };
    let mut seeds = HashSet::new();
    for ((block_id, instruction_index), slot) in plan.function_slots(index) {
        if *slot != site_index {
            continue;
        }
        let Some(block) = function
            .blocks()
            .iter()
            .find(|block| block.id() == *block_id)
        else {
            continue;
        };
        let Some(instruction) = block.instructions().get(*instruction_index) else {
            continue;
        };
        match instruction {
            Instruction::Binary { lhs, rhs, .. } | Instruction::Compare { lhs, rhs, .. } => {
                insert_seedable(&mut seeds, function, *lhs);
                insert_seedable(&mut seeds, function, *rhs);
            }
            Instruction::Unary { value, .. } => {
                insert_seedable(&mut seeds, function, *value);
            }
            _ => {}
        }
    }
    seeds
}

fn insert_seedable(seeds: &mut HashSet<ValueId>, function: &Function, value: ValueId) {
    for block in function.blocks() {
        for instruction in block.instructions() {
            match instruction {
                Instruction::Const { dest, .. }
                | Instruction::LoadVar { dest, .. }
                | Instruction::Phi { dest, .. }
                    if *dest == value =>
                {
                    seeds.insert(value);
                    return;
                }
                _ => {}
            }
        }
    }
}
