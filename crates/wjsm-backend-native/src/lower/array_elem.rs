//! GetElem / SetElem / array push

#![allow(unused_imports)]
use super::*;
use anyhow::{Context, Result, bail};
use cranelift_codegen::ir::{self, InstBuilder, MemFlagsData, types};
use cranelift_frontend::FunctionBuilder;
use std::mem::offset_of;
use wjsm_ir::{
    BinaryOp, Builtin, CompareOp, Constant, ConstantId, Instruction, UnaryOp, ValueId, constants,
    value,
};
use wjsm_native_abi::{NativeRuntimeOp, NativeVmContext};

pub(crate) fn lower_string_element(
    cx: &mut LoweringCx<'_, '_>,
    barrier_thunks: &BarrierThunks,
    dest: ValueId,
    object: ValueId,
    index: ValueId,
    guard: Option<ValueId>,
    speculative: bool,
) -> Result<()> {
    let object_id = object;
    let index_id = index;
    let object = use_value_boxed(cx.builder, cx.variables, object)?;
    let encoded_index = use_value_boxed(cx.builder, cx.variables, index)?;
    let (index, valid_index) = emit_nonnegative_integer_index(cx.builder, encoded_index);
    let index_block = cx.builder.create_block();
    let inline_string_block = cx.builder.create_block();
    let inline_char_block = cx.builder.create_block();
    let string_block = cx.builder.create_block();
    let dispatch_block = cx.builder.create_block();
    let array_dispatch_block = cx.builder.create_block();
    let array_block = cx.builder.create_block();
    let miss_block = cx.builder.create_block();
    let out_of_bounds_block = cx.builder.create_block();
    let merge_block = cx.builder.create_block();
    cx.builder
        .ins()
        .brif(valid_index, index_block, &[], miss_block, &[]);

    cx.builder.switch_to_block(index_block);
    cx.builder.seal_block(index_block);
    let is_inline = emit_inline_string_predicate(cx.builder, object);
    cx.builder
        .ins()
        .brif(is_inline, inline_string_block, &[], dispatch_block, &[]);

    cx.builder.switch_to_block(inline_string_block);
    cx.builder.seal_block(inline_string_block);
    let inline_length = cx
        .builder
        .ins()
        .ushr_imm_u(object, i64::from(value::INLINE_STRING_LENGTH_SHIFT));
    let inline_length = cx.builder.ins().band_imm_u(inline_length, 0b111);
    let inline_in_bounds =
        cx.builder
            .ins()
            .icmp(ir::condcodes::IntCC::UnsignedLessThan, index, inline_length);
    cx.builder.ins().brif(
        inline_in_bounds,
        inline_char_block,
        &[],
        out_of_bounds_block,
        &[],
    );

    cx.builder.switch_to_block(inline_char_block);
    cx.builder.seal_block(inline_char_block);
    let inline_latin1_char_block = cx.builder.create_block();
    let inline_ascii_char_block = cx.builder.create_block();
    let is_inline_latin1 = emit_is_inline_latin1_marker(cx.builder, object);
    cx.builder.ins().brif(
        is_inline_latin1,
        inline_latin1_char_block,
        &[],
        inline_ascii_char_block,
        &[],
    );

    cx.builder.switch_to_block(inline_ascii_char_block);
    cx.builder.seal_block(inline_ascii_char_block);
    let inline_unit = emit_extract_inline_ascii_unit(cx.builder, object, index);
    let inline_result = emit_inline_ascii_char_value(cx, inline_unit);
    define_value_boxed(cx.builder, cx.variables, dest, inline_result)?;
    cx.builder.ins().jump(merge_block, &[]);

    cx.builder.switch_to_block(inline_latin1_char_block);
    cx.builder.seal_block(inline_latin1_char_block);
    let inline_unit = emit_extract_inline_latin1_unit(cx.builder, object, index);
    let inline_result = emit_latin1_char_handle(cx, inline_unit, miss_block)?;
    define_value_boxed(cx.builder, cx.variables, dest, inline_result)?;
    cx.builder.ins().jump(merge_block, &[]);

    cx.builder.switch_to_block(dispatch_block);
    cx.builder.seal_block(dispatch_block);
    let tag_word = cx.builder.ins().ushr_imm_u(object, 32);
    let tag = cx.builder.ins().band_imm_u(
        tag_word,
        i64::try_from(value::TAG_MASK).expect("tag mask fits i64"),
    );
    let is_string = cx.builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::Equal,
        tag,
        i64::try_from(value::TAG_STRING).expect("string tag fits i64"),
    );
    let is_array = cx.builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::Equal,
        tag,
        i64::try_from(value::TAG_ARRAY).expect("array tag fits i64"),
    );
    cx.builder
        .ins()
        .brif(is_string, string_block, &[], array_dispatch_block, &[]);

    cx.builder.switch_to_block(array_dispatch_block);
    cx.builder.seal_block(array_dispatch_block);
    cx.builder
        .ins()
        .brif(is_array, array_block, &[], miss_block, &[]);

    cx.builder.switch_to_block(string_block);
    cx.builder.seal_block(string_block);
    let address = emit_string_address(cx, barrier_thunks, object, miss_block)?;
    let unit = emit_flat_string_code_unit(cx, address, index, miss_block, out_of_bounds_block);
    let result = emit_latin1_char_handle(cx, unit, miss_block)?;
    define_value_boxed(cx.builder, cx.variables, dest, result)?;
    cx.builder.ins().jump(merge_block, &[]);

    // 数组读取
    cx.builder.switch_to_block(array_block);
    cx.builder.seal_block(array_block);
    let address = emit_array_address(cx, barrier_thunks, object, miss_block)?;
    let header = cx
        .builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), address, 0);
    let kind = cx.builder.ins().ushr_imm_u(header, 40);
    let kind = cx.builder.ins().band_imm_u(kind, 0xff);
    let is_dict = cx.builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::Equal,
        kind,
        i64::from(wjsm_ir::constants::ARRAY_KIND_DICTIONARY),
    );
    let dict_check_block = cx.builder.create_block();
    cx.builder
        .ins()
        .brif(is_dict, miss_block, &[], dict_check_block, &[]);

    cx.builder.switch_to_block(dict_check_block);
    cx.builder.seal_block(dict_check_block);
    let shape = cx.builder.ins().load(
        types::I64,
        MemFlagsData::trusted(),
        address,
        i32::try_from(constants::HEAP_ARRAY_LENGTH_OFFSET).expect("length offset fits i32"),
    );
    let length = cx.builder.ins().band_imm_u(shape, i64::from(u32::MAX));
    let capacity = cx.builder.ins().ushr_imm_u(shape, 32);
    let index_u64 = index;
    let in_length =
        cx.builder
            .ins()
            .icmp(ir::condcodes::IntCC::UnsignedLessThan, index_u64, length);
    let in_capacity =
        cx.builder
            .ins()
            .icmp(ir::condcodes::IntCC::UnsignedLessThan, index_u64, capacity);
    let in_bounds = cx.builder.ins().band(in_length, in_capacity);

    let elem_read_block = cx.builder.create_block();
    cx.builder
        .ins()
        .brif(in_bounds, elem_read_block, &[], miss_block, &[]);

    cx.builder.switch_to_block(elem_read_block);
    cx.builder.seal_block(elem_read_block);
    let index_bytes = cx.builder.ins().ishl_imm_u(index_u64, 3);
    let elem_offset = cx
        .builder
        .ins()
        .iadd_imm_s(index_bytes, i64::from(constants::HEAP_OBJECT_HEADER_SIZE));
    let elem_addr = cx.builder.ins().iadd(address, elem_offset);
    let elem_val = cx
        .builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), elem_addr, 0);

    let hole_val = cx
        .builder
        .ins()
        .iconst(types::I64, value::encode_array_hole());
    let clean_elem = emit_strip_gc_color(cx.builder, elem_val);
    let is_hole = cx
        .builder
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, clean_elem, hole_val);
    let elem_hit_block = cx.builder.create_block();
    cx.builder
        .ins()
        .brif(is_hole, miss_block, &[], elem_hit_block, &[]);

    cx.builder.switch_to_block(elem_hit_block);
    cx.builder.seal_block(elem_hit_block);
    define_value_boxed(cx.builder, cx.variables, dest, clean_elem)?;
    cx.builder.ins().jump(merge_block, &[]);

    cx.builder.switch_to_block(out_of_bounds_block);
    cx.builder.seal_block(out_of_bounds_block);
    let undefined = cx
        .builder
        .ins()
        .iconst(types::I64, value::encode_undefined());
    define_value_boxed(cx.builder, cx.variables, dest, undefined)?;
    cx.builder.ins().jump(merge_block, &[]);

    cx.builder.switch_to_block(miss_block);
    cx.builder.seal_block(miss_block);
    if speculative {
        emit_deopt_to_generic(cx, cx.current_block, &[object_id, index_id])?;
        cx.builder.switch_to_block(merge_block);
        cx.builder.seal_block(merge_block);
        return Ok(());
    }
    if let Some(guard) = guard {
        let disabled = cx
            .builder
            .ins()
            .iconst(types::I64, value::encode_bool(false));
        define_value_boxed(cx.builder, cx.variables, guard, disabled)?;
    }
    let result = cx.call(
        NativeRuntimeOp::GetElem.id(),
        &[object, encoded_index],
        None,
    )?;
    define_value_boxed(cx.builder, cx.variables, dest, result)?;
    cx.builder.ins().jump(merge_block, &[]);

    cx.builder.switch_to_block(merge_block);
    cx.builder.seal_block(merge_block);
    Ok(())
}

/// packed 数组在索引处写入；仅处理非字典、索引在 capacity 内且值为非 boxed 数字的快路径。
pub(crate) fn lower_packed_array_store(
    cx: &mut LoweringCx<'_, '_>,
    barrier_thunks: &BarrierThunks,
    object: ir::Value,
    index: ir::Value,
    stored: ir::Value,
    dest: Option<ValueId>,
    miss_block: ir::Block,
    merge_block: ir::Block,
) -> Result<()> {
    let box_base = i64::from_ne_bytes(value::BOX_BASE.to_ne_bytes());
    let boxed_bits = cx.builder.ins().band_imm_s(stored, box_base);
    let is_heap_value =
        cx.builder
            .ins()
            .icmp_imm_s(ir::condcodes::IntCC::Equal, boxed_bits, box_base);
    let needs_host = cx.builder.create_block();
    let array_block = cx.builder.create_block();
    cx.builder
        .ins()
        .brif(is_heap_value, needs_host, &[], array_block, &[]);

    cx.builder.switch_to_block(needs_host);
    cx.builder.seal_block(needs_host);
    cx.builder.ins().jump(miss_block, &[]);

    cx.builder.switch_to_block(array_block);
    cx.builder.seal_block(array_block);
    let address = emit_array_address(cx, barrier_thunks, object, miss_block)?;
    let header = cx
        .builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), address, 0);
    let kind = cx.builder.ins().ushr_imm_u(header, 40);
    let kind = cx.builder.ins().band_imm_u(kind, 0xff);
    let is_dict = cx.builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::Equal,
        kind,
        i64::from(wjsm_ir::constants::ARRAY_KIND_DICTIONARY),
    );
    let dict_check_block = cx.builder.create_block();
    cx.builder
        .ins()
        .brif(is_dict, miss_block, &[], dict_check_block, &[]);

    cx.builder.switch_to_block(dict_check_block);
    cx.builder.seal_block(dict_check_block);
    let shape = cx.builder.ins().load(
        types::I64,
        MemFlagsData::trusted(),
        address,
        i32::try_from(constants::HEAP_ARRAY_LENGTH_OFFSET).expect("length offset fits i32"),
    );
    let length = cx.builder.ins().band_imm_u(shape, i64::from(u32::MAX));
    let capacity = cx.builder.ins().ushr_imm_u(shape, 32);
    let index_u64 = index;
    let in_capacity =
        cx.builder
            .ins()
            .icmp(ir::condcodes::IntCC::UnsignedLessThan, index_u64, capacity);
    let not_past_length = cx.builder.ins().icmp(
        ir::condcodes::IntCC::UnsignedLessThanOrEqual,
        index_u64,
        length,
    );
    let append = cx
        .builder
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, index_u64, length);
    let in_bounds = cx.builder.ins().band(in_capacity, not_past_length);
    let store_block = cx.builder.create_block();
    cx.builder
        .ins()
        .brif(in_bounds, store_block, &[], miss_block, &[]);

    cx.builder.switch_to_block(store_block);
    cx.builder.seal_block(store_block);
    let index_bytes = cx.builder.ins().ishl_imm_u(index_u64, 3);
    let elem_offset = cx
        .builder
        .ins()
        .iadd_imm_s(index_bytes, i64::from(constants::HEAP_OBJECT_HEADER_SIZE));
    let elem_addr = cx.builder.ins().iadd(address, elem_offset);
    cx.builder
        .ins()
        .store(MemFlagsData::trusted(), stored, elem_addr, 0);
    let after_store_block = cx.builder.create_block();
    let append_block = cx.builder.create_block();
    cx.builder
        .ins()
        .brif(append, append_block, &[], after_store_block, &[]);

    cx.builder.switch_to_block(after_store_block);
    cx.builder.seal_block(after_store_block);
    if let Some(dest) = dest {
        define_value_boxed(cx.builder, cx.variables, dest, stored)?;
    }
    cx.builder.ins().jump(merge_block, &[]);

    cx.builder.switch_to_block(append_block);
    cx.builder.seal_block(append_block);
    let new_length = cx.builder.ins().iadd_imm_u(index_u64, 1);
    let new_shape = cx.builder.ins().ishl_imm_u(capacity, 32);
    let new_shape = cx.builder.ins().bor(new_shape, new_length);
    cx.builder.ins().store(
        MemFlagsData::trusted(),
        new_shape,
        address,
        i32::try_from(constants::HEAP_ARRAY_LENGTH_OFFSET).expect("length offset fits i32"),
    );
    if let Some(dest) = dest {
        define_value_boxed(cx.builder, cx.variables, dest, stored)?;
    }
    cx.builder.ins().jump(merge_block, &[]);
    Ok(())
}

pub(crate) fn lower_set_elem(
    cx: &mut LoweringCx<'_, '_>,
    barrier_thunks: &BarrierThunks,
    dest: ValueId,
    object: ValueId,
    index: ValueId,
    stored: ValueId,
    strict: bool,
) -> Result<()> {
    let object_val = use_value_boxed(cx.builder, cx.variables, object)?;
    let encoded_index = use_value_boxed(cx.builder, cx.variables, index)?;
    let stored_val = use_value_boxed(cx.builder, cx.variables, stored)?;
    let (index_val, valid_index) = emit_nonnegative_integer_index(cx.builder, encoded_index);
    let index_block = cx.builder.create_block();
    let array_block = cx.builder.create_block();
    let miss_block = cx.builder.create_block();
    let merge_block = cx.builder.create_block();
    cx.builder
        .ins()
        .brif(valid_index, index_block, &[], miss_block, &[]);

    cx.builder.switch_to_block(index_block);
    cx.builder.seal_block(index_block);
    let tag_word = cx.builder.ins().ushr_imm_u(object_val, 32);
    let tag = cx.builder.ins().band_imm_u(
        tag_word,
        i64::try_from(value::TAG_MASK).expect("tag mask fits i64"),
    );
    let is_array = cx.builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::Equal,
        tag,
        i64::try_from(value::TAG_ARRAY).expect("array tag fits i64"),
    );
    cx.builder
        .ins()
        .brif(is_array, array_block, &[], miss_block, &[]);

    cx.builder.switch_to_block(array_block);
    cx.builder.seal_block(array_block);
    lower_packed_array_store(
        cx,
        barrier_thunks,
        object_val,
        index_val,
        stored_val,
        Some(dest),
        miss_block,
        merge_block,
    )?;

    cx.builder.switch_to_block(miss_block);
    cx.builder.seal_block(miss_block);
    let set_elem_op = if strict {
        NativeRuntimeOp::SetElemStrict
    } else {
        NativeRuntimeOp::SetElem
    };
    let result = cx.call(
        set_elem_op.id(),
        &[object_val, encoded_index, stored_val],
        None,
    )?;
    define_value_boxed(cx.builder, cx.variables, dest, result)?;
    cx.builder.ins().jump(merge_block, &[]);

    cx.builder.switch_to_block(merge_block);
    cx.builder.seal_block(merge_block);
    Ok(())
}

pub(crate) fn lower_array_push_inline(
    cx: &mut LoweringCx<'_, '_>,
    barrier_thunks: &BarrierThunks,
    object: ValueId,
    stored: ValueId,
) -> Result<()> {
    let object_val = use_value_boxed(cx.builder, cx.variables, object)?;
    let stored_val = use_value_boxed(cx.builder, cx.variables, stored)?;
    let miss_block = cx.builder.create_block();
    let array_block = cx.builder.create_block();
    let merge_block = cx.builder.create_block();
    let tag_word = cx.builder.ins().ushr_imm_u(object_val, 32);
    let tag = cx.builder.ins().band_imm_u(
        tag_word,
        i64::try_from(value::TAG_MASK).expect("tag mask fits i64"),
    );
    let is_array = cx.builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::Equal,
        tag,
        i64::try_from(value::TAG_ARRAY).expect("array tag fits i64"),
    );
    cx.builder
        .ins()
        .brif(is_array, array_block, &[], miss_block, &[]);

    cx.builder.switch_to_block(array_block);
    cx.builder.seal_block(array_block);
    let address = emit_array_address(cx, barrier_thunks, object_val, miss_block)?;
    let shape = cx.builder.ins().load(
        types::I64,
        MemFlagsData::trusted(),
        address,
        i32::try_from(constants::HEAP_ARRAY_LENGTH_OFFSET).expect("length offset fits i32"),
    );
    let length = cx.builder.ins().band_imm_u(shape, i64::from(u32::MAX));
    lower_packed_array_store(
        cx,
        barrier_thunks,
        object_val,
        length,
        stored_val,
        None,
        miss_block,
        merge_block,
    )?;

    cx.builder.switch_to_block(miss_block);
    cx.builder.seal_block(miss_block);
    let _ = cx.call(
        u32::from(Builtin::ArrayPush.wire_id()),
        &[object_val, stored_val],
        None,
    )?;
    cx.builder.ins().jump(merge_block, &[]);

    cx.builder.switch_to_block(merge_block);
    cx.builder.seal_block(merge_block);
    Ok(())
}
