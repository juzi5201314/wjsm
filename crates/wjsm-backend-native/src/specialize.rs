//! 运行时类型反馈驱动的单函数特化编译（Issue #390 阶段 3）。
//!
//! generic `lower.rs` 只有 boxed lowering；本模块提供「profile → typed wrapper +
//! typed body」入口：wrapper 以固定 `NativeSlowEntry` ABI 导出，先校验 `args_count`
//! 与每个 profile 参数的 tag，命中后调用同一 object 内部的 typed body（number
//! 参数从 call arena 直读、经种子 f64 分析消除守卫），失配则读取当前 base image
//! 的 `function_table[function_index].slow_entry` 以原始五参数回落 generic entry。
//! 特化 image 只在进程内存在，不进入 `.wjsm`、磁盘 cache 或分发制品。

use std::collections::{BTreeSet, HashMap};
use std::mem::{offset_of, size_of};

use anyhow::{Context, Result};
use cranelift_codegen::ir::{InstBuilder, MemFlagsData, UserFuncName, types};
use cranelift_codegen::isa::unwind::UnwindInfo;
use cranelift_control::ControlPlane;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_module::{Linkage, Module, ModuleReloc};
use cranelift_object::{ObjectBuilder, ObjectModule};
use wjsm_ir::{Builtin, FunctionId, Instruction, Program, constants};
use wjsm_native_abi::{NativeFeedbackTag, NativeFunctionEntry, NativeVmContext};

use crate::f64_analysis::{infer_f64_values, infer_f64_values_with_param_seeds};
use crate::lower::{
    DeclaredBarrierThunks, DeclaredData, DeclaredFunction, FunctionCompileInput,
    allocate_feedback_slots, allocate_ic_slots, boxed_frame_local_names, compile_one_function,
    declare_barrier_thunks, declare_host_dispatcher, declare_math_thunks, declare_root_bitmaps,
    declare_string_add_thunk, declare_string_builder_append_number_thunk,
    declare_string_builder_append_thunk, declare_string_builder_finish_thunk,
    emit_feedback_tag_code, gimli_endian, libcall_name, root_frame_capacity, slow_entry_signature,
};
use crate::root_plan::RootPlan;
use crate::unwind::{UnwindPolicy, UnwindRecord, validate_unwind_info, write_object_unwind};
use crate::{NativeCompilationDiagnostics, NativeCompileError, NativeObject};

/// 一条特化 profile：目标函数 + 每个实际参数（跳过 env/this）的反馈 tag。
///
/// tag 序列由反馈槽的 `last_tag_signature` 解码而来，与 wrapper 的入口守卫
/// 逐位一致；`Number` tag 是唯一可安全提升的类别，其余 tag 只允许 wrapper
/// 直读（不再发 LoadArgument），不会让分析证明任何 f64 值。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpecializationProfile {
    pub function: FunctionId,
    pub argument_tags: Box<[NativeFeedbackTag]>,
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
    if profile.argument_tags.is_empty()
        || profile.argument_tags.len() > constants::FEEDBACK_MAX_TAGS as usize
    {
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
    if !profile.argument_tags.contains(&NativeFeedbackTag::Number) {
        return Err(SpecializationError::NoBenefit(
            "profile has no number argument",
        ));
    }
    let name = ir_function.name();
    if name.ends_with("$async") || name.ends_with("$asyncgen") {
        return Err(SpecializationError::NoBenefit(
            "async and generator functions resume through dedicated entries",
        ));
    }
    for block in ir_function.blocks() {
        for instruction in block.instructions() {
            match instruction {
                Instruction::Suspend { .. } | Instruction::GeneratorSuspend { .. } => {
                    return Err(SpecializationError::NoBenefit(
                        "suspending functions resume through dedicated entries",
                    ));
                }
                // mapped arguments 对象在语义上应与参数槽别名；当前实现虽未建立
                // 别名，仍保守拒绝，避免未来补齐语义时 overlay 静默偏离。
                Instruction::CallBuiltin {
                    builtin: Builtin::CreateMappedArgumentsObject,
                    ..
                } => {
                    return Err(SpecializationError::NoBenefit(
                        "mapped arguments objects alias parameter slots",
                    ));
                }
                _ => {}
            }
        }
    }

    // 种子分析：Number 参数在 wrapper 守卫背书下作为 f64 起点；与无种子结果
    // 相比没有新增证明时，该 profile 无收益，宿主保持 generic。
    let mut seeds: HashMap<FunctionId, Vec<bool>> = HashMap::new();
    seeds.insert(
        profile.function,
        profile
            .argument_tags
            .iter()
            .map(|tag| *tag == NativeFeedbackTag::Number)
            .collect(),
    );
    let unseeded = infer_f64_values(program);
    let seeded = infer_f64_values_with_param_seeds(program, &seeds);
    let target_function_id =
        FunctionId(u32::try_from(target_index).map_err(|_| {
            SpecializationError::NoBenefit("target function index does not fit u32")
        })?);
    let unseeded_values = unseeded
        .get(&target_function_id)
        .cloned()
        .unwrap_or_default();
    let seeded_values = seeded.get(&target_function_id).cloned().unwrap_or_default();
    if seeded_values.len() <= unseeded_values.len() {
        return Err(SpecializationError::NoBenefit(
            "seeded analysis proves no additional f64 values",
        ));
    }

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
    let string_builder_append = declare_string_builder_append_thunk(&mut module)?;
    let string_builder_append_number = declare_string_builder_append_number_thunk(&mut module)?;
    let string_builder_finish = declare_string_builder_finish_thunk(&mut module)?;
    let (zgc_load_barrier, zgc_store_barrier) = declare_barrier_thunks(&mut module)?;
    let math_thunks = declare_math_thunks(&mut module, program, &seeded)?;

    let frame_locals: BTreeSet<&str> = program
        .frame_local_variable_names_by_function()
        .get(target_index)
        .cloned()
        .unwrap_or_default();
    let boxed_frame_locals =
        boxed_frame_local_names(ir_function, &frame_locals, &seeded, target_index);
    let root_plan = RootPlan::build(ir_function, &seeded_values);
    let root_capacity = root_frame_capacity(ir_function, &root_plan, boxed_frame_locals.len());
    let root_bitmaps = declare_root_bitmaps(&mut module, root_capacity)?;
    let dispatcher_decl = DeclaredFunction::snapshot(module.declarations(), host_dispatcher);
    let string_add_decl = DeclaredFunction::snapshot(module.declarations(), string_add);
    let string_builder_append_decl =
        DeclaredFunction::snapshot(module.declarations(), string_builder_append);
    let string_builder_append_number_decl =
        DeclaredFunction::snapshot(module.declarations(), string_builder_append_number);
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
    let feedback_plan = allocate_feedback_slots(program);

    let body = compile_one_function(&FunctionCompileInput {
        isa: &isa,
        target_config,
        program,
        ir_function,
        index: target_index,
        signature: &signature,
        function_id: body_id,
        dispatcher: &dispatcher_decl,
        string_add: &string_add_decl,
        string_builder_append: &string_builder_append_decl,
        string_builder_append_number: &string_builder_append_number_decl,
        string_builder_finish: &string_builder_finish_decl,
        barrier_thunks: &barrier_thunks,
        math_thunks: &math_thunk_decls,
        root_bitmaps: &bitmap_decls,
        f64_values: &seeded_values,
        variable_slots,
        root_plan: &root_plan,
        root_capacity,
        frame_local_names: &frame_locals,
        boxed_local_names: &boxed_frame_locals,
        ic_slots: &ic_slots[target_index],
        feedback_slots: feedback_plan.function_slots(target_index),
        specialized_tags: Some(profile.argument_tags.as_ref()),
        collect_diagnostics,
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
        .map_err(|error| NativeCompileError::Cranelift(error.inner.to_string()))?;
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
