use wjsm_ir::{Builtin, constants, value};
use wjsm_native_abi::NativeVmContext;

use super::fail_dispatch;
use crate::{NativeAgentState, NativeCallableKind, PropertyKey};
use wjsm_host::RuntimeString;

const DATA_FLAGS: u32 =
    (constants::FLAG_CONFIGURABLE | constants::FLAG_ENUMERABLE | constants::FLAG_WRITABLE) as u32;
const HIDDEN_DATA_FLAGS: u32 = (constants::FLAG_CONFIGURABLE | constants::FLAG_WRITABLE) as u32;

/// mapped arguments 的 [[ParameterMap]] 侧表条目（ES §10.4.4.7）。
///
/// 映射期间形参绑定真值就是 arguments 对象的自有索引属性，`bindings[i]`
/// 只在 `mapped[i]` 解除后持有形参绑定的独立快照；两个向量长度都等于
/// 形参个数，超出实参个数的索引从创建起就不在 map 中（绑定槽起始为
/// undefined，对应缺省形参值）。
pub(crate) struct NativeMappedArguments {
    pub(crate) mapped: Vec<bool>,
    pub(crate) bindings: Vec<i64>,
}

pub(super) fn dispatch_arguments(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    builtin: Builtin,
    args: &[i64],
) -> Option<i64> {
    let mapped = match builtin {
        Builtin::CreateMappedArgumentsObject => true,
        Builtin::CreateUnmappedArgumentsObject => false,
        Builtin::MappedArgumentsBindingRead => return Some(binding_read(ctx, state, args)),
        Builtin::MappedArgumentsBindingWrite => return Some(binding_write(ctx, state, args)),
        _ => return None,
    };
    Some(create(ctx, state, mapped, args))
}

pub(super) fn create(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    mapped: bool,
    args: &[i64],
) -> i64 {
    let Some(source) = args
        .first()
        .copied()
        .filter(|source| value::is_array(*source))
    else {
        return fail_dispatch(ctx);
    };
    let source_handle = value::decode_handle(source);
    let Ok(length) = state.gc.heap().array_length(source_handle) else {
        return fail_dispatch(ctx);
    };
    // 固定布局一次分配到位：索引属性 length 个 + "length" + @@iterator +
    // callee（mapped 为数据属性占 1 槽；unmapped 为 accessor 占 getter/setter
    // 2 槽）。容量不足会触发 shape 扩容 relocate，而本对象可能仍在 native
    // TLAB 中未物化，扩容将以 NativeTlabNeedsMaterialization 失败。
    let extra_slots = if mapped { 3 } else { 4 };
    let Ok(arguments) =
        state.allocate_object_with_gc_retry(ctx, length.saturating_add(extra_slots), false)
    else {
        return fail_dispatch(ctx);
    };
    let handle = value::decode_handle(arguments);
    if state
        .gc
        .heap()
        .set_object_type(handle, wjsm_ir::HEAP_TYPE_ARGUMENTS)
        .is_err()
    {
        return fail_dispatch(ctx);
    }
    for index in 0..length {
        let stored = state
            .gc
            .heap()
            .get_element(source_handle, index)
            .ok()
            .flatten()
            .map(|stored| stored as i64)
            .unwrap_or_else(value::encode_undefined);
        let Some(key) = state.intern_property_string(RuntimeString::from(index.to_string())) else {
            return fail_dispatch(ctx);
        };
        if state
            .gc
            .heap()
            .define_data_property(handle, key, stored as u64, DATA_FLAGS)
            .is_err()
        {
            return fail_dispatch(ctx);
        }
    }
    if !define_named(
        state,
        handle,
        "length",
        value::encode_f64(f64::from(length)),
        HIDDEN_DATA_FLAGS,
    ) {
        return fail_dispatch(ctx);
    }
    // @@iterator 初值为 %Array.prototype.values%（§10.4.4.6）：与数组的
    // values / @@iterator 同一函数身份，CreateArrayIterator 对 receiver 通用。
    let Some(iterator) = state.native_callable(NativeCallableKind::ArrayIterator(
        crate::NativeIteratorKind::Values,
    )) else {
        return fail_dispatch(ctx);
    };
    let iterator_key = PropertyKey::symbol(wjsm_ir::wk_symbol::ITERATOR);

    if state
        .gc
        .heap()
        .define_data_property(handle, iterator_key, iterator as u64, HIDDEN_DATA_FLAGS)
        .is_err()
    {
        return fail_dispatch(ctx);
    }
    if mapped {
        // §10.2.1.1 步骤 8：callee 恒为自有数据属性
        // `{[[Writable]]: true, [[Enumerable]]: false, [[Configurable]]: true}`。
        // 取值即 §10.2.11 步骤 22 传给 CreateMappedArgumentsObject 的 func——
        // 本次调用的函数对象。物化 arguments 的函数含 CollectRestArgs，
        // direct_call 直调优化已排除它们，每次进入都经 prepare_call 压
        // activation，因此栈顶 activation 记录的被调值恒为当前帧的用户可见
        // 函数（具名声明/表达式/generator 与 async wrapper 一致；bound/
        // proxy 转发在内层 prepare_call 已解包为真实目标）。
        let callee = state
            .activations
            .last()
            .map_or_else(value::encode_undefined, |activation| activation.callee);
        if !define_named(state, handle, "callee", callee, HIDDEN_DATA_FLAGS) {
            return fail_dispatch(ctx);
        }
        // [[ParameterMap]] 侧表：语义层仅在形参别名重定向生效（简单参数列表、
        // 非 direct-eval 函数）时传非零形参个数；传 0 表示保持普通对象行为。
        let param_count = args
            .get(1)
            .copied()
            .filter(|count| value::is_f64(*count))
            .map(value::decode_f64)
            .filter(|count| count.fract() == 0.0 && *count >= 0.0)
            .map(|count| count as usize)
            .unwrap_or(0);
        if param_count > 0 {
            let argc = length as usize;
            let entry = NativeMappedArguments {
                mapped: (0..param_count).map(|index| index < argc).collect(),
                bindings: vec![value::encode_undefined(); param_count],
            };
            state.mapped_arguments.insert(handle, entry);
        }
    } else {
        let Some(thrower) = state.native_callable(NativeCallableKind::ArgumentsStrictCallee) else {
            return fail_dispatch(ctx);
        };
        let Some(key) = state.intern_property_string("callee".into()) else {
            return fail_dispatch(ctx);
        };
        if state
            .gc
            .heap()
            .define_accessor_property_with_flags(handle, key, thrower as u64, thrower as u64, 0)
            .is_err()
        {
            return fail_dispatch(ctx);
        }
    }
    arguments
}

pub(crate) fn strict_callee_error(ctx: &mut NativeVmContext, state: &mut NativeAgentState) -> i64 {
    super::modules::named_error_object(
        state,
        "TypeError",
        "'callee' and 'caller' properties are not defined".into(),
    )
    .and_then(|error| state.create_exception(error))
    .unwrap_or_else(|| fail_dispatch(ctx))
}

fn define_named(
    state: &mut NativeAgentState,
    object: u32,
    name: &str,
    stored: i64,
    flags: u32,
) -> bool {
    let Some(key) = state.intern_property_string(name.into()) else {
        return false;
    };
    state
        .gc
        .heap()
        .define_data_property(object, key, stored as u64, flags)
        .is_ok()
}

/// 解析 MappedArgumentsBindingRead/Write 的公共实参：(对象句柄, 形参索引)。
/// 语义层只对确证存在侧表条目的函数发射这两个 builtin，解析失败即不变量破坏。
fn binding_args(state: &NativeAgentState, args: &[i64]) -> Option<(u32, usize)> {
    let object = args.first().copied().filter(|v| value::is_js_object(*v))?;
    let index = args
        .get(1)
        .copied()
        .filter(|v| value::is_f64(*v))
        .map(value::decode_f64)
        .filter(|v| v.fract() == 0.0 && *v >= 0.0)
        .map(|v| v as usize)?;
    let handle = value::decode_handle(object);
    let entry = state.mapped_arguments.get(&handle)?;
    (index < entry.bindings.len()).then_some((handle, index))
}

/// 形参绑定读取：映射期间读 arguments 自有索引属性（该属性即绑定真值，
/// 映射中的索引恒为可写数据属性），解除映射后读侧表绑定槽。
fn binding_read(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some((handle, index)) = binding_args(state, args) else {
        return fail_dispatch(ctx);
    };
    let entry = &state.mapped_arguments[&handle];
    if !entry.mapped[index] {
        return entry.bindings[index];
    }
    let Some(key) = state.intern_property_string(RuntimeString::from(index.to_string())) else {
        return fail_dispatch(ctx);
    };
    match state.gc.heap().get_property_slot(handle, key) {
        Ok(Some(slot)) => slot.value as i64,
        _ => fail_dispatch(ctx),
    }
}

/// 形参绑定写入：映射期间写 arguments 自有索引属性（保持既有特性；映射
/// 存续保证该属性是可写数据属性），解除映射后写侧表绑定槽。回传写入值。
fn binding_write(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some((handle, index)) = binding_args(state, args) else {
        return fail_dispatch(ctx);
    };
    let Some(stored) = args.get(2).copied() else {
        return fail_dispatch(ctx);
    };
    let entry = state
        .mapped_arguments
        .get_mut(&handle)
        .expect("binding_args 已确认条目存在");
    if !entry.mapped[index] {
        let old = entry.bindings[index];
        entry.bindings[index] = stored;
        state
            .gc
            .record_host_write(value::encode_object_handle(handle), Some(old), Some(stored));
        return stored;
    }
    let Some(key) = state.intern_property_string(RuntimeString::from(index.to_string())) else {
        return fail_dispatch(ctx);
    };
    let flags = match state.gc.heap().get_property_slot(handle, key) {
        Ok(Some(slot)) => slot.flags,
        _ => return fail_dispatch(ctx),
    };
    if state
        .gc
        .heap()
        .define_data_property(handle, key, stored as u64, flags)
        .is_err()
    {
        return fail_dispatch(ctx);
    }
    stored
}

/// 返回 `key` 命中的仍在映射中的形参索引（对象无侧表条目时为 None）。
///
/// 创建时的映射键是十进制索引字符串的驻留形态，这里按同一驻留正向比对；
/// defineProperty/delete/freeze 是冷路径，线性扫描形参个数无碍。
pub(crate) fn live_mapped_index(
    state: &mut NativeAgentState,
    handle: u32,
    key: PropertyKey,
) -> Option<usize> {
    let live: Vec<usize> = state
        .mapped_arguments
        .get(&handle)?
        .mapped
        .iter()
        .enumerate()
        .filter_map(|(index, mapped)| mapped.then_some(index))
        .collect();
    live.into_iter().find(|index| {
        state
            .intern_property_string(RuntimeString::from(index.to_string()))
            .is_some_and(|candidate| candidate == key)
    })
}

/// 读取仍在映射中的索引属性当前值（即形参绑定的当前真值）。
fn mapped_slot_value(state: &mut NativeAgentState, handle: u32, index: usize) -> Option<i64> {
    let key = state.intern_property_string(RuntimeString::from(index.to_string()))?;
    state
        .gc
        .heap()
        .get_property_slot(handle, key)
        .ok()
        .flatten()
        .map(|slot| slot.value as i64)
}

/// 解除单个索引的映射（map.[[Delete]]），把 `snapshot` 快照为绑定槽的
/// 独立值；此后该形参绑定与对象属性各自演化。
pub(crate) fn unmap_index(state: &mut NativeAgentState, handle: u32, index: usize, snapshot: i64) {
    let Some(entry) = state.mapped_arguments.get_mut(&handle) else {
        return;
    };
    let old = entry.bindings[index];
    entry.mapped[index] = false;
    entry.bindings[index] = snapshot;
    state.gc.record_host_write(
        value::encode_object_handle(handle),
        Some(old),
        Some(snapshot),
    );
}

/// [[DefineOwnProperty]] 成功后的 map 收尾（ES §10.4.4.2 步骤 7）：访问器
/// 描述符解除映射（绑定保留 define 前的属性值 `previous`）；数据描述符带
/// writable:false 也解除映射，但绑定取 define 后的属性值（步骤 7.b.i 先把
/// [[Value]] 写进绑定，7.b.ii 再删 map 条目）。其余情形映射存续——属性值
/// 更新对绑定天然可见（映射期间两者同储）。
pub(crate) fn after_define_own_property(
    state: &mut NativeAgentState,
    handle: u32,
    index: usize,
    previous: i64,
    is_accessor: bool,
    writable_false: bool,
) {
    if is_accessor {
        unmap_index(state, handle, index, previous);
        return;
    }
    if writable_false {
        let snapshot = mapped_slot_value(state, handle, index).unwrap_or(previous);
        unmap_index(state, handle, index, snapshot);
    }
}

/// [[Delete]] 成功后的 map 收尾（ES §10.4.4.4）：绑定保留删除前的属性值。
pub(crate) fn after_delete_property(
    state: &mut NativeAgentState,
    handle: u32,
    index: usize,
    previous: i64,
) {
    unmap_index(state, handle, index, previous);
}

/// Object.freeze 的 map 收尾：freeze 对每个数据属性应用 writable:false 的
/// [[DefineOwnProperty]]，按步骤 7.b.ii 逐一解除映射，绑定快照当前属性值。
/// seal 只收紧 configurable，映射存续，不走此路径。
pub(crate) fn unmap_all_for_freeze(state: &mut NativeAgentState, handle: u32) {
    let Some(entry) = state.mapped_arguments.get(&handle) else {
        return;
    };
    let live: Vec<usize> = entry
        .mapped
        .iter()
        .enumerate()
        .filter_map(|(index, mapped)| mapped.then_some(index))
        .collect();
    for index in live {
        let snapshot =
            mapped_slot_value(state, handle, index).unwrap_or_else(value::encode_undefined);
        unmap_index(state, handle, index, snapshot);
    }
}
