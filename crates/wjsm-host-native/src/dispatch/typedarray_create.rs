//! TypedArray 创建协议：AllocateTypedArray 的 newTarget 原型解析
//! （§23.2.5.1 / §10.1.13 OrdinaryCreateFromConstructor）与
//! TypedArraySpeciesCreate（§23.2.4.1）/ TypedArrayCreateFromConstructor
//! （§23.2.4.2）的构造器解析 + 结果校验。
//!
//! @@species 访问器安装在 %TypedArray% 抽象构造器上，11 种具体构造器经
//! 静态原型链继承（§23.2.2.4）；SpeciesConstructor 的 defaultConstructor
//! 仍是各元素类型的内在构造器（Table 71）。

use wjsm_ir::value;
use wjsm_native_abi::NativeVmContext;

use super::runtime::{
    construct_value, fail_dispatch, get_property, is_constructor_value, type_error,
};
use super::typedarray::{TypedArrayKind, constructor_builtin, render_getter_receiver};
use crate::NativeAgentState;

/// newTarget 的实例原型槽：undefined → None 沿用缺省内在原型；`prototype`
/// 非对象 → None 回退缺省（§10.1.13 步骤 3）；读取（Proxy trap / 再入
/// getter）的异常原样传播。
pub(super) fn instance_prototype_slot(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    new_target: i64,
) -> Result<Option<u32>, i64> {
    if value::is_undefined(new_target) {
        return Ok(None);
    }
    let Some(key) = state.intern_text("prototype".into(), value::TAG_STRING) else {
        return Err(fail_dispatch(ctx));
    };
    let prototype = get_property(ctx, state, new_target, key).map_err(|()| fail_dispatch(ctx))?;
    if value::is_exception(prototype) {
        return Err(prototype);
    }
    if !(value::is_js_object(prototype) || value::is_regexp(prototype)) {
        return Ok(None);
    }
    Ok(super::runtime::encode_proto_slot(prototype))
}

/// SpeciesConstructor(O, defaultConstructor)（§7.3.22）的解析结果。
pub(super) enum SpeciesDecision {
    /// 缺省内在构造器（constructor 为 undefined、species 为 undefined/null、
    /// 或 species 即本元素类型的内在构造器本体）：走既有快路径创建。
    Default,
    /// 自定义 species 构造器：以规范实参列表执行 Construct。
    Construct(i64),
}

/// SpeciesConstructor(exemplar, %<kind>Array%)：constructor / @@species 的
/// getter（含 Proxy trap）异常原样传播；TypeError 文案对齐 V8。
pub(super) fn species_constructor(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    receiver: i64,
    kind: TypedArrayKind,
) -> Result<SpeciesDecision, i64> {
    // 步骤 1：C = Get(O, "constructor")。
    let Some(constructor_key) = state.intern_text("constructor".into(), value::TAG_STRING) else {
        return Err(fail_dispatch(ctx));
    };
    let candidate =
        get_property(ctx, state, receiver, constructor_key).map_err(|()| fail_dispatch(ctx))?;
    if value::is_exception(candidate) {
        return Err(candidate);
    }
    // 步骤 2：undefined → defaultConstructor。
    if value::is_undefined(candidate) {
        return Ok(SpeciesDecision::Default);
    }
    // 步骤 3：C 非对象 → TypeError。
    if !(value::is_js_object(candidate)
        || value::is_array(candidate)
        || value::is_callable(candidate)
        || value::is_proxy(candidate)
        || value::is_regexp(candidate))
    {
        return Err(type_error(
            ctx,
            state,
            "The .constructor property is not an object",
        ));
    }
    // 步骤 4：S = Get(C, @@species)。species getter 可再入用户代码触发 GC，
    // 读取期间 candidate 锚根。
    let initial_temp_roots = state.temporary_roots.len();
    state.temporary_roots.push(candidate);
    let species_key = value::encode_handle(value::TAG_SYMBOL, wjsm_ir::wk_symbol::SPECIES);
    let species = get_property(ctx, state, candidate, species_key);
    state.temporary_roots.truncate(initial_temp_roots);
    let species = species.map_err(|()| fail_dispatch(ctx))?;
    if value::is_exception(species) {
        return Err(species);
    }
    // 步骤 5：undefined / null → defaultConstructor。
    if value::is_undefined(species) || value::is_null(species) {
        return Ok(SpeciesDecision::Default);
    }
    // 步骤 6–7：非构造器 → TypeError。
    if !is_constructor_value(state, species) {
        return Err(type_error(
            ctx,
            state,
            "object.constructor[Symbol.species] is not a constructor",
        ));
    }
    // S 为本元素类型的内在构造器本体（缺省 species 或用户显式指回）：
    // Construct(default, args) 与快路径缺省创建等价，合流缺省路径。
    if state.native_callable_kind(species)
        == Some(crate::NativeCallableKind::Builtin(
            constructor_builtin(kind),
            false,
        ))
    {
        return Ok(SpeciesDecision::Default);
    }
    Ok(SpeciesDecision::Construct(species))
}

/// TypedArrayCreateFromConstructor(constructor, argumentList)（§23.2.4.2）
/// + TypedArraySpeciesCreate 步骤 5 的 [[ContentType]] 检查。`min_length`
/// 为 argumentList 是单个 Number 时的最小长度门槛（§23.2.4.1 步骤 3）。
pub(super) fn species_create(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    exemplar_kind: TypedArrayKind,
    constructor: i64,
    args: &[i64],
    method: &str,
    min_length: Option<usize>,
) -> Result<i64, i64> {
    create_from_constructor(
        ctx,
        state,
        constructor,
        args,
        &format!("%TypedArray%.prototype.{method}"),
        min_length,
        Some(exemplar_kind),
    )
}

/// TypedArrayCreateFromConstructor(constructor, argumentList)（§23.2.4.2）：
/// Construct + ValidateTypedArray + 单数值实参的最小长度门槛。`content_kind`
/// 为 TypedArraySpeciesCreate 步骤 5 的 [[ContentType]] 检查（from / of 无
/// exemplar，不检查）；`method` 为品牌检查失败文案中的方法路径
/// （"%TypedArray%.prototype.slice" / "%TypedArray%.from" 等）。
pub(super) fn create_from_constructor(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    constructor: i64,
    args: &[i64],
    method: &str,
    min_length: Option<usize>,
    content_kind: Option<TypedArrayKind>,
) -> Result<i64, i64> {
    let result = construct_value(ctx, state, constructor, args, constructor);
    if value::is_exception(result) {
        return Err(result);
    }
    // ValidateTypedArray：结果无 [[TypedArrayName]] → TypeError（V8 文案）。
    let created = if value::is_js_object(result) {
        state.typed_arrays.get(&value::decode_handle(result))
    } else {
        None
    };
    let Some(created) = created else {
        let rendered = render_getter_receiver(state, Some(result));
        let message = format!("Method {method} called on incompatible receiver {rendered}");
        return Err(type_error(ctx, state, &message));
    };
    let created_kind = created.kind;
    let created_length = created.length;
    if min_length.is_some_and(|min| created_length < min) {
        return Err(type_error(
            ctx,
            state,
            "Derived TypedArray constructor created an array which was too small",
        ));
    }
    if content_kind.is_some_and(|kind| created_kind.is_bigint() != kind.is_bigint()) {
        return Err(type_error(
            ctx,
            state,
            "Cannot mix BigInt and other types, use explicit conversions",
        ));
    }
    Ok(result)
}
