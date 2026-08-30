//! 闭包 `$env` 固定槽读写：宿主内联 shape 校验 + 直读槽，miss 回退 GetProp/SetProp。

use super::*;
use anyhow::Result;
use cranelift_codegen::ir::{self, InstBuilder, types};
use wjsm_ir::ValueId;
use wjsm_native_abi::NativeRuntimeOp;

pub(crate) fn lower_load_env_slot(
    cx: &mut LoweringCx<'_, '_>,
    dest: ValueId,
    env: ValueId,
    slot: u32,
    key: ValueId,
) -> Result<()> {
    let env_val = use_value_boxed(cx.builder, cx.variables, env)?;
    let slot_val = cx.builder.ins().iconst(types::I64, i64::from(slot));
    let key_val = use_value_boxed(cx.builder, cx.variables, key)?;
    let result = cx.call(
        NativeRuntimeOp::LoadEnvSlot.id(),
        &[env_val, slot_val, key_val],
        None,
    )?;
    define_value_boxed(cx.builder, cx.variables, dest, result)
}

pub(crate) fn lower_store_env_slot(
    cx: &mut LoweringCx<'_, '_>,
    dest: Option<ValueId>,
    env: ValueId,
    slot: u32,
    value: ValueId,
    key: ValueId,
    strict: bool,
) -> Result<()> {
    let env_val = use_value_boxed(cx.builder, cx.variables, env)?;
    let slot_val = cx.builder.ins().iconst(types::I64, i64::from(slot));
    let stored = use_value_boxed(cx.builder, cx.variables, value)?;
    let key_val = use_value_boxed(cx.builder, cx.variables, key)?;
    let op = if strict {
        NativeRuntimeOp::StoreEnvSlotStrict
    } else {
        NativeRuntimeOp::StoreEnvSlot
    };
    let result = cx.call(op.id(), &[env_val, slot_val, stored, key_val], None)?;
    if let Some(dest) = dest {
        define_value_boxed(cx.builder, cx.variables, dest, result)?;
    }
    Ok(())
}
