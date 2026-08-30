//! 单函数入口 lowering

#![allow(unused_imports)]
use super::*;
use anyhow::{Context, Result, bail};
use cranelift_codegen::ir::{self, InstBuilder, types};
use cranelift_frontend::FunctionBuilder;
use wjsm_ir::FunctionId;
use wjsm_ir::{Instruction, ValueId, constants, value};
use wjsm_native_abi::NativeRuntimeOp;

pub(crate) fn lower_function(
    function: &mut Function,
    builder_context: &mut FunctionBuilderContext,
    input: &FunctionCompileInput<'_, '_>,
) -> Result<()> {
    let target_config = input.target_config;
    let program = input.program;
    let ir_function = input.ir_function;
    let function_index = u32::try_from(input.index).context("function index exceeds u32")?;
    let host_dispatcher = input.dispatcher;
    let string_add = input.string_add;
    let string_builder_finish = input.string_builder_finish;
    let math_thunks = input.math_thunks;
    let f64_values = input.f64_values;
    let int32_values = input.int32_values;
    let variable_slots = input.variable_slots;
    let root_plan = input.root_plan;
    let root_capacity = input.root_capacity;
    let root_bitmaps = input.root_bitmaps;
    let frame_local_names = input.frame_local_names;
    let boxed_local_names = input.boxed_local_names;
    let ic_slots = input.ic_slots;
    let template_origins = input.template_origins;
    let feedback_slots = input.feedback_slots;
    let specialized_tags = input.specialized_tags;
    let safepoint_free = input.safepoint_free;
    let slow_call_signature = slow_entry_signature(function.signature.call_conv);
    let mut builder = FunctionBuilder::new(function, builder_context);
    let mut blocks = HashMap::with_capacity(ir_function.blocks().len());
    for block in ir_function.blocks() {
        blocks.insert(block.id(), builder.create_block());
    }
    let entry = *blocks
        .get(&ir_function.entry())
        .context("entry block is missing")?;
    builder.append_block_params_for_function_params(entry);

    let value_ids = collect_value_ids(ir_function);
    let has_suspend = ir_function.blocks().iter().any(|block| {
        block.instructions().iter().any(|instruction| {
            matches!(
                instruction,
                Instruction::Suspend { .. } | Instruction::GeneratorSuspend { .. }
            )
        })
    });
    let mut variables = ValueRepr::plan(
        ir_function,
        input.typed_f64_values,
        frame_local_names,
        has_suspend,
    );
    variables.declare_values(&mut builder, &value_ids);
    let mut frame_locals = frame_local_variables(frame_local_names);
    let boxed_local_order = boxed_local_order(boxed_local_names);
    let boxed_local_indices = frame_local_indices(&boxed_local_order);
    let phi_edges = collect_phi_edges(ir_function);
    let dispatcher_ref = host_dispatcher.import(builder.func);
    let string_add_ref = string_add.import(builder.func);
    let string_builder_finish_ref = string_builder_finish.import(builder.func);
    let mut imported_math_thunks: HashMap<Builtin, ir::FuncRef> =
        HashMap::with_capacity(math_thunks.len());
    let barrier_thunks = input.barrier_thunks.import(builder.func);
    let slow_call_signature = builder.import_signature(slow_call_signature);
    let ctx_value = builder.block_params(entry)[0];
    let env_param = builder.block_params(entry)[1];
    let this_param = builder.block_params(entry)[2];
    let constants = program.constants();
    let boolean_values = infer_boolean_values(ir_function, constants);
    let constant_defs = ir_function
        .blocks()
        .iter()
        .flat_map(|block| block.instructions())
        .filter_map(|instruction| match instruction {
            Instruction::Const { dest, constant } => Some((*dest, *constant)),
            _ => None,
        })
        .collect::<HashMap<_, _>>();
    let immutable_constant_ids = immutable_constant_ids(ir_function, constants);
    // root frame 的基址值必须在入口块物化：入口块支配其余所有块，基址可跨块复用。

    builder.switch_to_block(entry);
    let mut root_frame = if safepoint_free {
        None
    } else {
        let frame = FrameLowering::new(&mut builder, root_bitmaps, root_capacity, ctx_value)?;
        frame.link(&mut builder, ctx_value)?;
        Some(frame)
    };
    initialize_frame_locals(&mut builder, &mut frame_locals, &variables);
    if let Some(root_frame) = root_frame.as_mut() {
        pin_initialized_frame_locals(root_frame, &mut builder, &frame_locals, &boxed_local_order)?;
    }
    let pointer_type = builder.func.dfg.value_type(ctx_value);
    let ht_base = builder.ins().load(
        pointer_type,
        MemFlagsData::trusted(),
        ctx_value,
        vmctx_offset(offset_of!(NativeVmContext, handle_table_base))?,
    );
    let ic_base = builder.ins().load(
        pointer_type,
        MemFlagsData::trusted(),
        ctx_value,
        vmctx_offset(offset_of!(NativeVmContext, ic_slots_base))?,
    );
    let barrier_state = builder.ins().load(
        pointer_type,
        MemFlagsData::trusted(),
        ctx_value,
        vmctx_offset(offset_of!(NativeVmContext, barrier_state))?,
    );
    {
        let mut cx = LoweringCx {
            builder: &mut builder,
            variables: &variables,
            root_frame: root_frame.as_mut(),
            dispatcher: dispatcher_ref,
            string_add: string_add_ref,
            string_builder_finish: string_builder_finish_ref,
            ctx: ctx_value,
            env: env_param,
            this_value: this_param,
            function_index,
            current_block: ir_function.entry(),
            target_config,
            ht_base,
            ic_base,
            barrier_state,
            current_instruction: 0,
            feedback_ptr: None,
        };
        lower_function_parameters(
            &mut cx,
            ir_function,
            variable_slots,
            &frame_locals,
            &boxed_local_indices,
            specialized_tags,
        )?;
        cx.stage(root_plan.before_instruction(ir_function.entry(), 0));
        let mut hoisted_constants = HashMap::with_capacity(immutable_constant_ids.len());
        for constant_id in &immutable_constant_ids {
            let constant = &constants
                [usize::try_from(constant_id.0).context("constant index does not fit usize")?];
            let result = match constant {
                // 字符串常量 install 期已发布进 vmctx 常量数组：入口两条 load 直读，
                // 替代旧的 MaterializeString 宿主往返（每次函数调用 ~40ns/常量）。
                // Utf16String（含孤立代理项）与 String 共用同一发布表。
                Constant::String(_) | Constant::Utf16String(_) => {
                    let pointer_type = cx.builder.func.dfg.value_type(cx.ctx);
                    let base = cx.builder.ins().load(
                        pointer_type,
                        MemFlagsData::trusted(),
                        cx.ctx,
                        vmctx_offset(offset_of!(NativeVmContext, string_constants_base))?,
                    );
                    let address = cx
                        .builder
                        .ins()
                        .iadd_imm_u(base, i64::from(constant_id.0) * 8);
                    cx.builder
                        .ins()
                        .load(types::I64, MemFlagsData::trusted(), address, 0)
                }
                Constant::BigInt(_) => {
                    let index = cx
                        .builder
                        .ins()
                        .iconst(types::I64, i64::from(constant_id.0));
                    cx.call(NativeRuntimeOp::MaterializeBigInt.id(), &[index], None)?
                }
                _ => unreachable!("immutable constant collector only returns strings and BigInts"),
            };
            hoisted_constants.insert(*constant_id, result);
        }
        let hypot_property_names = input.hypot_property_names;
        let mut imported_function_decls: HashMap<FunctionId, ir::FuncRef> = HashMap::new();
        let mut tables = InstructionTables {
            program,
            ir_function,
            has_env_layout: !ir_function.env_layout_keys().is_empty(),
            constants,
            function_index,
            barrier_thunks: &barrier_thunks,
            f64_values,
            int32_values,
            speculative: input.specialized_tags.is_some(),
            constant_defs: &constant_defs,
            math_thunks,
            hoisted_constants: &hoisted_constants,
            imported_math_thunks: &mut imported_math_thunks,
            slow_call_signature,
            variable_slots,
            frame_locals: &frame_locals,
            frame_local_indices: &boxed_local_indices,
            ic_slots,
            template_origins,
            function_decls: input.function_decls,
            imported_function_decls: &mut imported_function_decls,
            direct_callable_functions: input.direct_callable_functions,
            hypot_property_names: &hypot_property_names,
        };

        let headers = wjsm_ir::typed_cfg::loop_headers(ir_function);
        let poll_edges = wjsm_ir::typed_cfg::dfs_back_edges(ir_function);
        let mut resume_pads = HashMap::new();
        for block in ir_function.blocks() {
            for (instruction_index, instruction) in block.instructions().iter().enumerate() {
                if is_resume_target(instruction) {
                    let pad = cx.builder.create_block();
                    resume_pads.insert((block.id(), instruction_index as u32), pad);
                }
            }
        }
        let entry_body = cx.builder.create_block();
        emit_resume_dispatch(
            &mut cx,
            program,
            ir_function,
            function_index,
            &blocks,
            &resume_pads,
            &headers,
            entry_body,
        )?;
        blocks.insert(ir_function.entry(), entry_body);

        for block in ir_function.blocks() {
            let clif_block = blocks[&block.id()];
            cx.builder.switch_to_block(clif_block);
            cx.current_block = block.id();
            cx.enter_root_block();
            let has_suspend = block.instructions().iter().any(|instruction| {
                matches!(
                    instruction,
                    Instruction::Suspend { .. } | Instruction::GeneratorSuspend { .. }
                )
            });
            let first_non_phi = block
                .instructions()
                .iter()
                .position(|instruction| !matches!(instruction, Instruction::Phi { .. }))
                .unwrap_or(block.instructions().len());
            let is_header = headers.contains(&block.id());
            for (instruction_index, instruction) in block.instructions().iter().enumerate() {
                if matches!(instruction, Instruction::Phi { .. }) {
                    continue;
                }
                if let Some(pad) = resume_pads.get(&(block.id(), instruction_index as u32)) {
                    cx.builder.ins().jump(*pad, &[]);
                    cx.builder.switch_to_block(*pad);
                }
                cx.current_instruction = instruction_index as u32;
                if is_header && instruction_index == first_non_phi {
                    let lives = wjsm_optimize::live_values_at(
                        program,
                        FunctionId(function_index),
                        block.id(),
                        instruction_index,
                    );
                    if input.specialized_tags.is_some() {
                        emit_overlay_header_guards(&mut cx, &tables, block.id(), &lives)?;
                    } else {
                        emit_osr_poll(
                            &mut cx,
                            &tables,
                            block.id(),
                            instruction_index as u32,
                            &lives,
                        )?;
                    }
                }
                let roots = root_plan.before_instruction(block.id(), instruction_index);
                cx.stage(roots);
                let ctx = cx.ctx;
                let overlay_slot = input.slot_map.and_then(|map| {
                    map.sites.iter().find(|site| {
                        site.overlay_block == block.id()
                            && site.overlay_instruction == instruction_index as u32
                    })
                });
                let feedback_key = overlay_slot
                    .map(|site| (site.generic_block, site.generic_instruction as usize))
                    .unwrap_or((block.id(), instruction_index));
                let feedback_ptr = if lowering_uses_feedback_ptr(instruction, f64_values) {
                    feedback_slots
                        .get(&feedback_key)
                        .map(|slot| emit_feedback_slot_ptr(cx.builder, ctx, *slot))
                        .transpose()?
                } else {
                    None
                };
                cx.feedback_ptr = feedback_ptr;
                lower_instruction(&mut cx, &mut tables, instruction, roots, feedback_ptr)?;
            }
            if has_suspend {
                continue;
            }
            cx.stage(root_plan.before_terminator(block.id()));
            lower_terminator(
                &mut cx,
                block.id(),
                block.terminator(),
                constants,
                &boolean_values,
                &blocks,
                &phi_edges,
                &poll_edges,
            )?;
        }
        cx.finish_roots();
        cx.builder.seal_all_blocks();
    }

    fn immutable_constant_ids(
        function: &wjsm_ir::Function,
        constants: &[Constant],
    ) -> Vec<ConstantId> {
        let mut ids = HashSet::new();
        for block in function.blocks() {
            for instruction in block.instructions() {
                let Instruction::Const { constant, .. } = instruction else {
                    continue;
                };
                let Some(value) = constants.get(constant.0 as usize) else {
                    continue;
                };
                if matches!(
                    value,
                    Constant::String(_) | Constant::Utf16String(_) | Constant::BigInt(_)
                ) {
                    ids.insert(*constant);
                }
            }
        }
        let mut ids: Vec<_> = ids.into_iter().collect();
        ids.sort_unstable_by_key(|constant| constant.0);

        ids
    }
    builder.finalize(target_config);
    Ok(())
}

pub(crate) fn lower_function_parameters(
    cx: &mut LoweringCx<'_, '_>,
    function: &wjsm_ir::Function,
    variable_slots: &HashMap<String, u32>,
    frame_locals: &HashMap<String, Variable>,
    frame_local_indices: &HashMap<String, usize>,
    specialized_tags: Option<&[wjsm_native_abi::NativeFeedbackTag]>,
) -> Result<()> {
    let native_params = cx
        .builder
        .block_params(cx.builder.current_block().context("missing entry block")?)
        .to_vec();
    let env = native_params[1];
    let this_value = native_params[2];
    let fast_arity = fast_js_arity(&cx.builder.func.signature);
    let entry_roots: &[ir::Value] = if function.params().len() >= 2 {
        &[env, this_value]
    } else if function.params().len() == 1 {
        &[env]
    } else {
        &[]
    };
    cx.publish_roots(&[], entry_roots)?;
    let uses_canonical_this = function_uses_local_var(function, "$this");
    let mut param_values = Vec::with_capacity(function.params().len());
    for (index, _) in function.params().iter().enumerate() {
        let value = match index {
            0 => env,
            1 => this_value,
            _ => load_js_parameter(cx, &native_params, index, fast_arity, specialized_tags)?,
        };
        param_values.push(value);
    }
    for (index, name) in function.params().iter().enumerate() {
        let storage_name = if index == 0
            && (function.name().ends_with("$async")
                || function.name().ends_with("$asyncgen")
                || name == EVAL_SCOPE_ENV_PARAM)
        {
            name
        } else if index == 0 {
            "$env"
        } else if index == 1
            && uses_canonical_this
            && !function.name().ends_with("$async")
            && !function.name().ends_with("$asyncgen")
        {
            "$this"
        } else {
            name
        };
        let value = param_values[index];
        if let Some(local) = frame_locals.get(storage_name).copied() {
            // typed 局部只在「该名字的全部 load 都已证明 f64」时成立，入参的
            // NaN-Box 位模式此时就是 double，入口一次 bitcast 即可。
            let native = if cx.variables.is_typed_local(storage_name) {
                unbox_f64(cx.builder, value)
            } else {
                value
            };
            cx.builder.def_var(local, native);
            if let Some(index) = frame_local_indices.get(storage_name).copied() {
                cx.update_pinned_local(index, native)?;
            }
            continue;
        }
        let Some(slot) = variable_slots.get(storage_name).copied() else {
            continue;
        };
        // 与 safepoint-free 判定一致：体内未读写的形参不发 StoreVar，否则
        // 无 root frame 的函数会在入口就要求宿主参数区。
        if !function_uses_local_var(function, storage_name) {
            continue;
        }
        cx.publish_roots(&[], &[value])?;
        let slot = cx.builder.ins().iconst(types::I64, i64::from(slot));
        let _ = cx.call(NativeRuntimeOp::StoreVar.id(), &[slot, value], None)?;
    }
    Ok(())
}

pub(crate) fn load_js_parameter(
    cx: &mut LoweringCx<'_, '_>,
    native_params: &[ir::Value],
    index: usize,
    fast_arity: Option<usize>,
    specialized_tags: Option<&[wjsm_native_abi::NativeFeedbackTag]>,
) -> Result<ir::Value> {
    let param_idx = index - 2;
    if let Some(arity) = fast_arity {
        if param_idx < arity
            && let Some(value) = native_params.get(3 + param_idx).copied()
        {
            return Ok(value);
        }
        return Ok(cx
            .builder
            .ins()
            .iconst(types::I64, value::encode_undefined()));
    }
    let pointer_type = cx.builder.func.dfg.value_type(cx.ctx);
    let args_base_u32 = cx.builder.ins().uextend(types::I64, native_params[3]);
    let arena_base = cx.builder.ins().load(
        pointer_type,
        MemFlagsData::trusted(),
        cx.ctx,
        vmctx_offset(offset_of!(NativeVmContext, call_arena_slots))?,
    );
    let param_idx_u32 = u32::try_from(param_idx).context("parameter index exceeds u32")?;
    let slot_offset = i64::from(param_idx_u32)
        .checked_mul(size_of::<i64>() as i64)
        .context("call arena offset overflows")?;
    let args_base_bytes = cx.builder.ins().ishl_imm_u(args_base_u32, 3);
    let param_bytes = cx.builder.ins().iadd_imm_s(args_base_bytes, slot_offset);
    if let Some(tags) = specialized_tags
        && tags.get(param_idx).is_some()
    {
        let address = cx.builder.ins().iadd(arena_base, param_bytes);
        return Ok(cx
            .builder
            .ins()
            .load(types::I64, MemFlagsData::trusted(), address, 0));
    }
    let in_bounds = cx.builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::UnsignedGreaterThan,
        native_params[4],
        i64::from(param_idx_u32),
    );
    let address = cx.builder.ins().iadd(arena_base, param_bytes);
    let loaded = cx
        .builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), address, 0);
    let undefined = cx
        .builder
        .ins()
        .iconst(types::I64, value::encode_undefined());
    Ok(cx.builder.ins().select(in_bounds, loaded, undefined))
}
