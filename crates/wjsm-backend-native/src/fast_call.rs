//! 模块内直调的寄存器传参 ABI（≤4 个 JS 形参）。
//!
//! 宿主入口仍是 `NativeSlowEntry`；合格函数额外编译 Local fast body，
//! 直调走 Cranelift 原生 callconv，slow 导出只做 arena → 寄存器 trampoline。

use std::mem::{offset_of, size_of};

use anyhow::{Context, Result};
use cranelift_codegen::ir::{
    self, AbiParam, InstBuilder, MemFlagsData, Signature, UserFuncName, types,
};
use cranelift_codegen::isa::{CallConv, TargetFrontendConfig};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_module::FuncId;
use wjsm_ir::{Function, FunctionId, value};
use wjsm_native_abi::NativeVmContext;

use crate::NativeCompileError;
use crate::lower::{CompiledFunction, DeclaredFunction, finish_compiled_function, vmctx_offset};

/// 寄存器 ABI 允许的最大 JS 形参数（不含 `$env` / `$this`）。
pub(crate) const MAX_FAST_JS_PARAMS: usize = 4;

pub(crate) fn js_param_count(function: &Function) -> usize {
    function.params().len().saturating_sub(2)
}

/// 是否编译寄存器 fast body（含带捕获闭包内层函数）。
pub(crate) fn is_fast_body_eligible(function: &Function) -> bool {
    let name = function.name();
    if name.ends_with("$async") || name.ends_with("$asyncgen") {
        return false;
    }
    if function.is_class_constructor() {
        return false;
    }
    if function.has_eval() {
        return false;
    }
    if js_param_count(function) > MAX_FAST_JS_PARAMS {
        return false;
    }
    function.direct_callable()
        || !function.captured_names().is_empty()
        || function.home_object.is_some()
}

pub(crate) fn is_fast_call_eligible(function: &Function) -> bool {
    is_fast_body_eligible(function)
}

pub(crate) fn fast_entry_signature(call_conv: CallConv, js_params: usize) -> Signature {
    debug_assert!(js_params <= MAX_FAST_JS_PARAMS);
    let mut signature = Signature::new(call_conv);
    signature.params.push(AbiParam::new(types::I64));
    signature.params.push(AbiParam::new(types::I64));
    signature.params.push(AbiParam::new(types::I64));
    for _ in 0..js_params {
        signature.params.push(AbiParam::new(types::I64));
    }
    signature.returns.push(AbiParam::new(types::I64));
    signature
}

/// Fast 签名的 JS 形参数；slow 五参数入口返回 `None`。
pub(crate) fn fast_js_arity(signature: &Signature) -> Option<usize> {
    let params = &signature.params;
    match params.get(3) {
        Some(param) if param.value_type == types::I32 => None,
        Some(_) => Some(params.len().saturating_sub(3)),
        None if params.len() == 3 => Some(0),
        None => None,
    }
}

/// 宿主 `NativeSlowEntry` trampoline：从 call arena 装入寄存器后调用 fast body。
pub(crate) fn compile_slow_trampoline(
    isa: &cranelift_codegen::isa::OwnedTargetIsa,
    target_config: TargetFrontendConfig,
    slow_signature: &Signature,
    trampoline_id: FuncId,
    body: &DeclaredFunction,
    js_param_count: usize,
    function_index: u32,
    ir_name: &str,
    collect_diagnostics: bool,
) -> Result<CompiledFunction, NativeCompileError> {
    let mut context = cranelift_codegen::Context::new();
    let mut builder_context = FunctionBuilderContext::new();
    context.set_disasm(collect_diagnostics);
    context.func.signature = slow_signature.clone();
    context.func.name = UserFuncName::user(1, function_index);
    lower_slow_trampoline(
        &mut context.func,
        &mut builder_context,
        target_config,
        body,
        js_param_count,
    )
    .map_err(|error| NativeCompileError::Lowering {
        function: FunctionId(function_index),
        message: error.to_string(),
    })?;
    let clif = if collect_diagnostics {
        format!(
            ";; trampoline {}: {}\n{}\n",
            function_index,
            ir_name,
            context.func.display()
        )
    } else {
        String::new()
    };
    finish_compiled_function(
        context,
        isa.as_ref(),
        trampoline_id,
        function_index,
        clif,
        collect_diagnostics,
        ir_name,
        true,
    )
}

fn lower_slow_trampoline(
    function: &mut ir::Function,
    builder_context: &mut FunctionBuilderContext,
    target_config: TargetFrontendConfig,
    body: &DeclaredFunction,
    js_param_count: usize,
) -> Result<()> {
    let mut builder = FunctionBuilder::new(function, builder_context);
    let entry = builder.create_block();
    builder.append_block_params_for_function_params(entry);
    builder.switch_to_block(entry);
    builder.seal_block(entry);

    let params = builder.block_params(entry).to_vec();
    let ctx = params[0];
    let env = params[1];
    let this_value = params[2];
    let args_base = params[3];
    let args_count = params[4];

    let pointer_type = builder.func.dfg.value_type(ctx);
    let arena_base = builder.ins().load(
        pointer_type,
        MemFlagsData::trusted(),
        ctx,
        vmctx_offset(offset_of!(NativeVmContext, call_arena_slots))?,
    );
    let args_base_u64 = builder.ins().uextend(types::I64, args_base);
    let args_base_bytes = builder.ins().ishl_imm_u(args_base_u64, 3);
    let base_addr = builder.ins().iadd(arena_base, args_base_bytes);
    let undefined = builder.ins().iconst(types::I64, value::encode_undefined());

    let mut call_args = Vec::with_capacity(3 + js_param_count);
    call_args.push(ctx);
    call_args.push(env);
    call_args.push(this_value);
    for index in 0..js_param_count {
        let param_idx = u32::try_from(index).context("fast-call parameter index exceeds u32")?;
        let in_bounds = builder.ins().icmp_imm_u(
            ir::condcodes::IntCC::UnsignedGreaterThan,
            args_count,
            i64::from(param_idx),
        );
        let slot_offset = i64::from(param_idx)
            .checked_mul(size_of::<i64>() as i64)
            .context("call arena offset overflows")?;
        let offset = i32::try_from(slot_offset).context("call arena offset exceeds i32")?;
        let loaded = builder
            .ins()
            .load(types::I64, MemFlagsData::trusted(), base_addr, offset);
        call_args.push(builder.ins().select(in_bounds, loaded, undefined));
    }

    let target = body.import(builder.func);
    let call = builder.ins().call(target, &call_args);
    let result = builder.inst_results(call)[0];
    builder.ins().return_(&[result]);
    builder.finalize(target_config);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wjsm_ir::{BasicBlock, BasicBlockId, Function};

    fn function_with_js_params(n: usize, direct: bool, name: &str) -> Function {
        let mut params = vec!["$env".into(), "$this".into()];
        for index in 0..n {
            params.push(format!("$1.p{index}"));
        }
        let mut function = Function::new(name, BasicBlockId(0));
        function.set_params(params);
        function.set_direct_callable(direct);
        function.push_block(BasicBlock::new(BasicBlockId(0)));
        function
    }

    fn captured_function(name: &str) -> Function {
        let mut function = function_with_js_params(0, false, name);
        function.set_captured_names(vec!["$1.x".into()]);
        function
    }

    #[test]
    fn captured_inner_functions_get_fast_body() {
        assert!(is_fast_body_eligible(&captured_function("increment")));
        let mut no_capture = function_with_js_params(0, false, "needs_env");
        no_capture.set_captured_names(vec![]);
        assert!(!is_fast_body_eligible(&no_capture));
    }

    #[test]
    fn eligibility_requires_direct_callable_and_arity() {
        assert!(is_fast_call_eligible(&function_with_js_params(
            4, true, "ok"
        )));
        assert!(!is_fast_call_eligible(&function_with_js_params(
            5, true, "wide"
        )));
        assert!(!is_fast_call_eligible(&function_with_js_params(
            1, false, "indirect"
        )));
        assert!(!is_fast_call_eligible(&function_with_js_params(
            1,
            true,
            "work$async"
        )));
    }

    #[test]
    fn fast_signature_arity_roundtrip() {
        let conv = CallConv::SystemV;
        assert_eq!(fast_js_arity(&fast_entry_signature(conv, 0)), Some(0));
        assert_eq!(fast_js_arity(&fast_entry_signature(conv, 2)), Some(2));
        assert_eq!(fast_js_arity(&fast_entry_signature(conv, 4)), Some(4));
        let slow = crate::lower::slow_entry_signature(conv);
        assert_eq!(fast_js_arity(&slow), None);
    }
}
