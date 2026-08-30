//! object 声明与 compile_program_inner

#![allow(unused_imports)]
use super::*;
use anyhow::{Context, Result, bail};
use cranelift_codegen::ir::{self, InstBuilder, types};
use cranelift_frontend::FunctionBuilder;
use wjsm_ir::{Instruction, ValueId, constants, value};
use wjsm_native_abi::NativeRuntimeOp;

pub(crate) fn compile_program_inner(
    isa: cranelift_codegen::isa::OwnedTargetIsa,
    program: &Program,
    variable_slots: &HashMap<String, u32>,
    collect_diagnostics: bool,
) -> Result<crate::NativeCompilationDiagnostics, NativeCompileError> {
    if let Err(error) = program.verify() {
        let _ = error;
    }
    let module_isa = isa;
    let unwind_policy = UnwindPolicy::for_triple(module_isa.triple())?;
    // unwind 产出三平台统一归 `unwind` 模块所有：cranelift-object 的内置
    // `.eh_frame` 只在 `define_function_with_control_plane` 里被喂数据，而 codegen
    // 走并行 compile + `define_function_bytes`，内置路径拿不到 FDE。
    let builder = ObjectBuilder::new(
        module_isa.clone(),
        b"wjsm-native-image".to_vec(),
        Box::new(libcall_name),
    )
    .map_err(|error| NativeCompileError::Cranelift(error.to_string()))?;
    let mut module = ObjectModule::new(builder);
    let call_conv = module.isa().default_call_conv();
    let signature = slow_entry_signature(call_conv);
    let function_ids = declare_functions(&mut module, program, &signature)?;
    let mut fast_ids: Vec<Option<FuncId>> = Vec::with_capacity(program.functions().len());
    let mut fast_signatures: Vec<Option<Signature>> = Vec::with_capacity(program.functions().len());
    for (index, function) in program.functions().iter().enumerate() {
        if is_fast_body_eligible(function) {
            let fast_signature = fast_entry_signature(call_conv, js_param_count(function));
            let fast_id = module
                .declare_function(
                    &format!("wjsm_function_{index}_fast"),
                    Linkage::Local,
                    &fast_signature,
                )
                .map_err(|error| NativeCompileError::Cranelift(error.to_string()))?;
            fast_ids.push(Some(fast_id));
            fast_signatures.push(Some(fast_signature));
        } else {
            fast_ids.push(None);
            fast_signatures.push(None);
        }
    }
    let host_dispatcher = declare_host_dispatcher(&mut module)?;
    let string_add = declare_string_add_thunk(&mut module)?;
    let string_builder_finish = declare_string_builder_finish_thunk(&mut module)?;
    let (zgc_load_barrier, zgc_store_barrier) = declare_barrier_thunks(&mut module)?;
    let inferred_f64 = infer_f64_values(program);
    let math_thunks = declare_math_thunks(&mut module, program, &inferred_f64)?;
    let frame_locals = program.frame_local_variable_names_by_function();
    let boxed_frame_locals: Vec<BTreeSet<&str>> = program
        .functions()
        .iter()
        .enumerate()
        .zip(&frame_locals)
        .map(|((index, function), names)| {
            boxed_frame_local_names(function, names, &inferred_f64, index)
        })
        .collect();
    let root_plans: Vec<_> = program
        .functions()
        .par_iter()
        .enumerate()
        .map(|(index, function)| {
            let f64_values = inferred_f64
                .get(&FunctionId(
                    u32::try_from(index).expect("function index fits u32"),
                ))
                .expect("analysis covers every function");
            RootPlan::build(function, f64_values)
        })
        .collect();
    let root_capacities: Vec<_> = program
        .functions()
        .iter()
        .zip(&root_plans)
        .zip(&boxed_frame_locals)
        .map(|((function, plan), boxed)| root_frame_capacity(function, plan, boxed.len()))
        .collect();
    let max_roots = root_capacities.iter().copied().max().unwrap_or(0);
    let root_bitmaps = declare_root_bitmaps(&mut module, max_roots)?;

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

    // IC 槽预计算：常量字符串键的 GetProp 在编译期固定槽位，miss 回填由宿主完成。
    let (ic_slots, ic_slot_count) = allocate_ic_slots(program);
    let template_origins = build_template_origin_maps(program);
    // 反馈槽预计算：只按指令形态编号，保证与运行时特化 overlay 的编号一致。
    let feedback_plan = allocate_feedback_slots(program);

    // 每个函数的 lower + Cranelift compile 相互独立，只读上面的声明快照；
    // 合并进 object 的写入阶段仍然串行，保证 relocation / 符号表顺序确定。
    let function_decls: Vec<DeclaredFunction> = function_ids
        .iter()
        .enumerate()
        .map(|(index, slow_id)| {
            let id = fast_ids[index].unwrap_or(*slow_id);
            DeclaredFunction::snapshot(module.declarations(), id)
        })
        .collect();
    let fast_decls: Vec<Option<DeclaredFunction>> = fast_ids
        .iter()
        .copied()
        .map(|id| id.map(|id| DeclaredFunction::snapshot(module.declarations(), id)))
        .collect();
    // 类构造器排除在 Call 站点直调之外：[[Call]]（无 new）必须经宿主
    // PrepareCall 抛 TypeError（ES §10.2.1 步骤 2）；ConstructCall 站点
    // 恒走 PrepareConstruct，不受影响。
    let direct_callable_functions: HashSet<FunctionId> = program
        .functions()
        .iter()
        .enumerate()
        .filter(|(_, f)| f.direct_callable() && !f.is_class_constructor())
        .map(|(i, _)| FunctionId(u32::try_from(i).expect("function index fits u32")))
        .collect();
    let safepoint_free_functions = infer_safepoint_free_functions(program, variable_slots);
    let compiled: Vec<CompiledFunction> = program
        .functions()
        .par_iter()
        .enumerate()
        .map(|(index, function)| {
            let function_id = FunctionId(u32::try_from(index).expect("function index fits u32"));
            let safepoint_free = safepoint_free_functions.contains(&function_id);
            let body_signature = fast_signatures[index].as_ref().unwrap_or(&signature);
            let body_id = fast_ids[index].unwrap_or(function_ids[index]);
            compile_one_function(&FunctionCompileInput {
                isa: &module_isa,
                target_config,
                program,
                ir_function: function,
                index,
                signature: body_signature,
                function_id: body_id,
                dispatcher: &dispatcher_decl,
                string_add: &string_add_decl,
                string_builder_finish: &string_builder_finish_decl,
                barrier_thunks: &barrier_thunks,
                math_thunks: &math_thunk_decls,
                root_bitmaps: &bitmap_decls,
                f64_values: inferred_f64
                    .get(&FunctionId(
                        u32::try_from(index).expect("function index fits u32"),
                    ))
                    .expect("analysis covers every function"),
                // base 编译没有反馈推测，静态分析结果本身就是可靠集合。
                typed_f64_values: inferred_f64
                    .get(&FunctionId(
                        u32::try_from(index).expect("function index fits u32"),
                    ))
                    .expect("analysis covers every function"),
                variable_slots,
                root_plan: &root_plans[index],
                root_capacity: if safepoint_free {
                    0
                } else {
                    root_capacities[index]
                },
                frame_local_names: &frame_locals[index],
                boxed_local_names: &boxed_frame_locals[index],
                ic_slots: &ic_slots[index],
                template_origins: &template_origins[index],
                feedback_slots: feedback_plan.function_slots(index),
                specialized_tags: None,
                slot_map: None,
                int32_values: &HashSet::new(),
                function_decls: &function_decls,
                direct_callable_functions: &direct_callable_functions,
                safepoint_free,
                collect_diagnostics,
            })
        })
        .collect::<Result<Vec<_>, NativeCompileError>>()?;

    let trampolines: Vec<Option<CompiledFunction>> = program
        .functions()
        .par_iter()
        .enumerate()
        .map(|(index, function)| {
            let Some(body) = fast_decls[index].as_ref() else {
                return Ok(None);
            };
            let function_index = u32::try_from(index).expect("function index fits u32");
            compile_slow_trampoline(
                &module_isa,
                target_config,
                &signature,
                function_ids[index],
                body,
                js_param_count(function),
                function_index,
                function.name(),
                collect_diagnostics,
            )
            .map(Some)
        })
        .collect::<Result<Vec<_>, NativeCompileError>>()?;

    let mut frame_bytes = Vec::with_capacity(program.functions().len());
    let mut unwind_records: Vec<UnwindRecord> = Vec::with_capacity(
        program.functions().len() + fast_ids.iter().filter(|id| id.is_some()).count(),
    );
    let mut clif = String::new();
    let mut disassembly = String::new();

    for (index, (output, trampoline)) in compiled.into_iter().zip(trampolines).enumerate() {
        let body_id = fast_ids[index].unwrap_or(function_ids[index]);
        module
            .define_function_bytes(body_id, output.alignment, &output.bytes, &output.relocs)
            .map_err(|error| NativeCompileError::Cranelift(error.to_string()))?;
        frame_bytes.push(output.frame_bytes);
        unwind_records.push(UnwindRecord {
            function: body_id,
            code_len: output.code_len,
            info: output.unwind,
        });
        if collect_diagnostics {
            clif.push_str(&output.clif);
            disassembly.push_str(&output.disassembly);
        }
        if let Some(trampoline) = trampoline {
            module
                .define_function_bytes(
                    function_ids[index],
                    trampoline.alignment,
                    &trampoline.bytes,
                    &trampoline.relocs,
                )
                .map_err(|error| NativeCompileError::Cranelift(error.to_string()))?;
            unwind_records.push(UnwindRecord {
                function: function_ids[index],
                code_len: trampoline.code_len,
                info: trampoline.unwind,
            });
            if collect_diagnostics {
                clif.push_str(&trampoline.clif);
                disassembly.push_str(&trampoline.disassembly);
            }
        }
    }

    let mut product = module.finish();
    let systemv_cie = if unwind_policy == UnwindPolicy::WindowsPdata {
        None
    } else {
        Some(module_isa.create_systemv_cie().ok_or_else(|| {
            NativeCompileError::CompilerInvariant("ISA cannot create a System V CIE".into())
        })?)
    };
    write_object_unwind(
        &mut product,
        unwind_policy,
        unwind_records,
        systemv_cie,
        gimli_endian(module_isa.triple()),
    )?;
    let object = product
        .emit()
        .map_err(|error| NativeCompileError::Object(error.to_string()))?;
    Ok(crate::NativeCompilationDiagnostics {
        object: NativeObject {
            bytes: object.into(),
            frame_bytes,
            function_count: u32::try_from(program.functions().len())
                .map_err(|_| NativeCompileError::Capacity("function count"))?,
            ic_slot_count,
            feedback_slot_count: feedback_plan.total_slots(),
        },
        clif,
        disassembly,
    })
}

/// 单个函数的完整 codegen：IR → CLIF → 机器码 + relocation + unwind info。
/// 不接触 `ObjectModule`，可安全并行执行。
pub(crate) fn compile_one_function(
    input: &FunctionCompileInput<'_, '_>,
) -> Result<CompiledFunction, NativeCompileError> {
    let function_index =
        u32::try_from(input.index).map_err(|_| NativeCompileError::Capacity("function IDs"))?;
    let mut context = cranelift_codegen::Context::new();
    let mut builder_context = FunctionBuilderContext::new();
    context.set_disasm(input.collect_diagnostics);
    context.func.signature = input.signature.clone();
    context.func.name = UserFuncName::user(0, function_index);
    lower_function(&mut context.func, &mut builder_context, input).map_err(|error| {
        NativeCompileError::Lowering {
            function: FunctionId(function_index),
            message: error.to_string(),
        }
    })?;
    let clif = if input.collect_diagnostics {
        format!(
            ";; function {}: {}\n{}\n",
            input.index,
            input.ir_function.name(),
            context.func.display()
        )
    } else {
        String::new()
    };
    finish_compiled_function(
        context,
        input.isa.as_ref(),
        input.function_id,
        function_index,
        clif,
        input.collect_diagnostics,
        input.ir_function.name(),
        false,
    )
}

pub(crate) fn finish_compiled_function(
    mut context: cranelift_codegen::Context,
    isa: &dyn cranelift_codegen::isa::TargetIsa,
    function_id: FuncId,
    function_index: u32,
    clif: String,
    collect_diagnostics: bool,
    ir_name: &str,
    trampoline: bool,
) -> Result<CompiledFunction, NativeCompileError> {
    context
        .compile(isa, &mut ControlPlane::default())
        .map_err(|error| NativeCompileError::Cranelift(format!("{:#?}", error.inner)))?;
    let compiled = context
        .compiled_code()
        .ok_or_else(|| NativeCompileError::CompilerInvariant("missing compiled code".into()))?;
    if !compiled.buffer.traps().is_empty() {
        return Err(NativeCompileError::CompilerInvariant(format!(
            "function {} contains a machine trap",
            function_index
        )));
    }
    let kind = if trampoline { "trampoline" } else { "function" };
    let disassembly = if collect_diagnostics {
        format!(
            ";; {kind} {}: {}\n{}\n",
            function_index,
            ir_name,
            compiled.vcode.as_deref().unwrap_or("")
        )
    } else {
        String::new()
    };
    let frame_bytes = compiled
        .buffer
        .frame_layout()
        .ok_or_else(|| {
            NativeCompileError::CompilerInvariant(format!(
                "function {function_index} is missing frame metadata"
            ))
        })?
        .frame_to_fp_offset;
    let unwind = compiled
        .create_unwind_info(isa)
        .map_err(|error| NativeCompileError::Cranelift(error.to_string()))?
        .ok_or(NativeCompileError::MissingUnwindInfo(FunctionId(
            function_index,
        )))?;
    validate_unwind_info(isa.triple(), &unwind, FunctionId(function_index))?;
    let alignment = u64::from(compiled.buffer.alignment);
    let code_len = u64::from(compiled.buffer.total_size());
    let bytes = compiled.buffer.data().to_vec();
    let relocs: Vec<ModuleReloc> = compiled
        .buffer
        .relocs()
        .iter()
        .map(|reloc| ModuleReloc::from_mach_reloc(reloc, &context.func, function_id))
        .collect();
    Ok(CompiledFunction {
        alignment,
        bytes,
        relocs,
        frame_bytes,
        code_len,
        unwind,
        clif,
        disassembly,
    })
}

pub(crate) fn declare_functions(
    module: &mut ObjectModule,
    program: &Program,
    signature: &Signature,
) -> Result<Vec<FuncId>, NativeCompileError> {
    program
        .functions()
        .iter()
        .enumerate()
        .map(|(index, _)| {
            module
                .declare_function(
                    &format!("wjsm_function_{index}"),
                    Linkage::Export,
                    signature,
                )
                .map_err(|error| NativeCompileError::Cranelift(error.to_string()))
        })
        .collect()
}

pub(crate) fn declare_host_dispatcher(
    module: &mut ObjectModule,
) -> Result<FuncId, NativeCompileError> {
    let pointer_type = module.target_config().pointer_type();
    let mut signature = module.make_signature();
    signature.params.push(AbiParam::new(pointer_type));
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(pointer_type));
    signature.params.push(AbiParam::new(types::I32));
    // ABI v10：反馈槽指针；非反馈调用点传 null。
    signature.params.push(AbiParam::new(pointer_type));
    signature.returns.push(AbiParam::new(types::I64));
    module
        .declare_function(
            HOST_OPERATION_SYMBOL.symbol_name(),
            Linkage::Import,
            &signature,
        )
        .map_err(|error| NativeCompileError::Cranelift(error.to_string()))
}

pub(crate) fn declare_string_add_thunk(
    module: &mut ObjectModule,
) -> Result<FuncId, NativeCompileError> {
    let pointer_type = module.target_config().pointer_type();
    let mut signature = module.make_signature();
    signature.params.push(AbiParam::new(pointer_type));
    signature.params.push(AbiParam::new(types::I64));
    signature.params.push(AbiParam::new(types::I64));
    signature.returns.push(AbiParam::new(types::I64));
    module
        .declare_function(STRING_ADD_SYMBOL.symbol_name(), Linkage::Import, &signature)
        .map_err(|error| NativeCompileError::Cranelift(error.to_string()))
}
pub(crate) fn declare_string_builder_finish_thunk(
    module: &mut ObjectModule,
) -> Result<FuncId, NativeCompileError> {
    let pointer_type = module.target_config().pointer_type();
    let mut signature = module.make_signature();
    signature.params.push(AbiParam::new(pointer_type));
    signature.params.push(AbiParam::new(types::I64));
    signature.returns.push(AbiParam::new(types::I64));
    module
        .declare_function(
            STRING_BUILDER_FINISH_SYMBOL.symbol_name(),
            Linkage::Import,
            &signature,
        )
        .map_err(|error| NativeCompileError::Cranelift(error.to_string()))
}

pub(crate) fn declare_barrier_thunks(
    module: &mut ObjectModule,
) -> Result<(FuncId, FuncId), NativeCompileError> {
    let pointer_type = module.target_config().pointer_type();
    let mut load_signature = module.make_signature();
    load_signature.params.push(AbiParam::new(pointer_type));
    load_signature.params.push(AbiParam::new(types::I32));
    load_signature.returns.push(AbiParam::new(types::I64));
    let load = module
        .declare_function(
            NativeHostSymbol::ZgcLoadBarrierAssist.symbol_name(),
            Linkage::Import,
            &load_signature,
        )
        .map_err(|error| NativeCompileError::Cranelift(error.to_string()))?;

    let mut store_signature = module.make_signature();
    store_signature.params.push(AbiParam::new(pointer_type));
    store_signature.params.push(AbiParam::new(types::I32));
    store_signature.params.push(AbiParam::new(types::I64));
    store_signature.params.push(AbiParam::new(types::I64));
    store_signature.returns.push(AbiParam::new(types::I32));
    let store = module
        .declare_function(
            NativeHostSymbol::ZgcStoreBarrier.symbol_name(),
            Linkage::Import,
            &store_signature,
        )
        .map_err(|error| NativeCompileError::Cranelift(error.to_string()))?;
    Ok((load, store))
}

/// 按需声明本程序真正走 typed 路径的 math thunk（模块级声明一次；函数内 import
/// 复用之，避免每条调用重建签名）。只有 `infer_f64_values` 已证明 dest 且实参
/// arity 与 thunk 签名一致的调用点才需要声明。
pub(crate) fn declare_math_thunks(
    module: &mut ObjectModule,
    program: &Program,
    inferred_f64: &HashMap<FunctionId, HashSet<ValueId>>,
) -> Result<HashMap<Builtin, FuncId>, NativeCompileError> {
    let mut used = HashSet::new();
    for (index, function) in program.functions().iter().enumerate() {
        let function_id = FunctionId(u32::try_from(index).expect("function index fits u32"));
        let f64_values = inferred_f64
            .get(&function_id)
            .expect("analysis covers every function");
        for block in function.blocks() {
            for instruction in block.instructions() {
                if let Instruction::CallBuiltin {
                    dest: Some(dest),
                    builtin,
                    args,
                } = instruction
                    && f64_values.contains(dest)
                    && NativeHostSymbol::for_builtin(*builtin).is_some_and(|symbol| {
                        args.len() == usize::from(symbol.signature().argument_count())
                    })
                {
                    used.insert(*builtin);
                }
            }
        }
    }
    let mut used: Vec<_> = used.into_iter().collect();
    used.sort_by_key(|builtin| builtin.wire_id());
    let unary_signature = math_thunk_signature(module, NativeSignature::F64Unary);
    let binary_signature = math_thunk_signature(module, NativeSignature::F64Binary);
    let mut declared = HashMap::with_capacity(used.len());
    for builtin in used {
        let symbol = NativeHostSymbol::for_builtin(builtin).expect("used 集合由 for_builtin 过滤");
        // 同一 arity 的全部 thunk 共享同一份 Cranelift 签名，避免逐符号重建。
        let signature = match symbol.signature() {
            NativeSignature::F64Unary => &unary_signature,
            NativeSignature::F64Binary => &binary_signature,
            NativeSignature::HostOperation
            | NativeSignature::ValueBinary
            | NativeSignature::ValueUnary
            | NativeSignature::ValueTernary
            | NativeSignature::ValueBinaryF64
            | NativeSignature::ZgcLoadBarrier
            | NativeSignature::ZgcStoreBarrier => {
                unreachable!("math thunk 不存在 host 或 ZGC 屏障签名")
            }
        };
        let func_id = module
            .declare_function(symbol.symbol_name(), Linkage::Import, signature)
            .map_err(|error| NativeCompileError::Cranelift(error.to_string()))?;
        declared.insert(builtin, func_id);
    }
    Ok(declared)
}

pub(crate) fn math_thunk_signature(module: &ObjectModule, signature: NativeSignature) -> Signature {
    let mut clif_signature = module.make_signature();
    match signature {
        NativeSignature::F64Unary => {
            clif_signature.params.push(AbiParam::new(types::F64));
        }
        NativeSignature::F64Binary => {
            clif_signature.params.push(AbiParam::new(types::F64));
            clif_signature.params.push(AbiParam::new(types::F64));
        }
        NativeSignature::HostOperation
        | NativeSignature::ValueBinary
        | NativeSignature::ValueUnary
        | NativeSignature::ValueTernary
        | NativeSignature::ValueBinaryF64
        | NativeSignature::ZgcLoadBarrier
        | NativeSignature::ZgcStoreBarrier => {
            unreachable!("math thunk 不存在 host 或 ZGC 屏障签名")
        }
    }
    clif_signature.returns.push(AbiParam::new(types::F64));
    clif_signature
}

pub(crate) fn slow_entry_signature(call_conv: CallConv) -> Signature {
    let mut signature = Signature::new(call_conv);
    signature.params.push(AbiParam::new(types::I64));
    signature.params.push(AbiParam::new(types::I64));
    signature.params.push(AbiParam::new(types::I64));
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I32));
    signature.returns.push(AbiParam::new(types::I64));
    signature
}
