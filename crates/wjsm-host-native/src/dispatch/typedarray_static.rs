//! %TypedArray% 抽象构造器的行为面：本体 Call / Construct（§23.2.1，一律
//! TypeError）、静态 from（§23.2.2.1）/ of（§23.2.2.2），以及共享原型的
//! @@toStringTag 访问器 getter（§23.2.3.38）。
//!
//! 这些成员安装在 %TypedArray% 上，11 种具体构造器经静态原型链继承；
//! `this` 决定实际构造目标（TypedArrayCreateFromConstructor，§23.2.4.2），
//! 内在构造器 receiver 合流缺省直建快路径。

use wjsm_ir::value;
use wjsm_native_abi::NativeVmContext;

use super::runtime::{
    fail_dispatch, get_property, is_constructor_value, is_truthy, iterator_done,
    iterator_from_method, iterator_value, render_value, to_number, type_error,
};
use super::typedarray::{TypedArrayKind, constructor_kind, render_getter_receiver};
use crate::{NativeAgentState, NativeCallableKind};

/// %TypedArray% 本体被 Call / Construct（§23.2.1 步骤 1）：一律 TypeError，
/// 文案对齐 V8；`class X extends %TypedArray%` 的 super() 同样落此。
pub(crate) fn abstract_construct(ctx: &mut NativeVmContext, state: &mut NativeAgentState) -> i64 {
    type_error(
        ctx,
        state,
        "Abstract class TypedArray not directly constructable",
    )
}

/// get %TypedArray%.prototype [ %Symbol.toStringTag% ]（§23.2.3.38）：
/// this 无 [[TypedArrayName]] 槽（基元 / 原型对象 / DataView 等）返回
/// undefined 而非抛错，否则返回元素类型名字符串。
pub(crate) fn to_string_tag(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    this_value: i64,
) -> i64 {
    if !value::is_js_object(this_value) {
        return value::encode_undefined();
    }
    let Some(array) = state.typed_arrays.get(&value::decode_handle(this_value)) else {
        return value::encode_undefined();
    };
    let name = kind_name(array.kind);
    state
        .intern_text(name.into(), value::TAG_STRING)
        .unwrap_or_else(|| fail_dispatch(ctx))
}

/// [[TypedArrayName]]（Table 71）：元素类型 → 构造器名。
fn kind_name(kind: TypedArrayKind) -> &'static str {
    match kind {
        TypedArrayKind::Int8 => "Int8Array",
        TypedArrayKind::Uint8 => "Uint8Array",
        TypedArrayKind::Uint8Clamped => "Uint8ClampedArray",
        TypedArrayKind::Int16 => "Int16Array",
        TypedArrayKind::Uint16 => "Uint16Array",
        TypedArrayKind::Int32 => "Int32Array",
        TypedArrayKind::Uint32 => "Uint32Array",
        TypedArrayKind::Float32 => "Float32Array",
        TypedArrayKind::Float64 => "Float64Array",
        TypedArrayKind::BigInt64 => "BigInt64Array",
        TypedArrayKind::BigUint64 => "BigUint64Array",
    }
}

/// %TypedArray%.from(source, mapfn, thisArg)（§23.2.2.1）。
pub(crate) fn static_from(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    this_value: i64,
    args: &[i64],
) -> i64 {
    // 步骤 1–2：C = this，IsConstructor 检查。
    let Some(target) = constructor_target(state, this_value) else {
        return not_a_constructor(ctx, state, this_value);
    };
    // 步骤 3–4：mapfn undefined 表示不映射；否则必须 callable。
    let map = args
        .get(1)
        .copied()
        .filter(|mapfn| !value::is_undefined(*mapfn));
    if let Some(mapfn) = map
        && !value::is_callable(mapfn)
    {
        let rendered = render_non_callable(state, mapfn);
        return type_error(ctx, state, &format!("{rendered} is not a function"));
    }
    let this_arg = args.get(2).copied().unwrap_or_else(value::encode_undefined);
    let source = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    // 步骤 5：usingIterator = GetMethod(source, @@iterator)。GetV 对 null /
    // undefined 先抛（文案对齐 V8 的 not-iterable 报错）。
    if value::is_null(source) || value::is_undefined(source) {
        let rendered = if value::is_null(source) {
            "object null"
        } else {
            "undefined"
        };
        return type_error(
            ctx,
            state,
            &format!("{rendered} is not iterable (cannot read property Symbol(Symbol.iterator))"),
        );
    }
    let initial_temp_roots = state.temporary_roots.len();
    state.temporary_roots.push(source);
    state.temporary_roots.push(this_arg);
    if let Some(mapfn) = map {
        state.temporary_roots.push(mapfn);
    }
    let result = from_rooted(ctx, state, target, source, map, this_arg);
    state.temporary_roots.truncate(initial_temp_roots);
    result
}

/// from 的主体（source / mapfn / thisArg 已锚根）。
fn from_rooted(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    target: TargetConstructor,
    source: i64,
    map: Option<i64>,
    this_arg: i64,
) -> i64 {
    let iterator_key = value::encode_handle(value::TAG_SYMBOL, wjsm_ir::wk_symbol::ITERATOR);
    let method = match get_property(ctx, state, source, iterator_key) {
        Ok(method) => method,
        Err(()) => return fail_dispatch(ctx),
    };
    if value::is_exception(method) {
        return method;
    }
    if value::is_undefined(method) || value::is_null(method) {
        return from_array_like(ctx, state, target, source, map, this_arg);
    }
    // GetMethod 步骤 3：@@iterator 存在但不可调用 → TypeError（V8 文案）。
    if !value::is_callable(method) {
        return type_error(
            ctx,
            state,
            "%TypedArray%.from requires that the property of the first argument, \
             items[Symbol.iterator], when exists, be a function",
        );
    }
    // 步骤 6a–6b：IteratorToList 先于目标构造（构造器可再入用户代码）。
    let values = match iterator_to_list(ctx, state, source, method) {
        Ok(values) => values,
        Err(exception) => return exception,
    };
    // 步骤 6c–6e：targetObj = TypedArrayCreateFromConstructor(C, «len»)，
    // 再逐元素 map + Set。
    let created = match create_target(ctx, state, target, values.len(), "from") {
        Ok(created) => created,
        Err(exception) => return exception,
    };
    state.temporary_roots.push(created);
    for (index, stored) in values.into_iter().enumerate() {
        if let Err(exception) = map_and_write(ctx, state, created, index, stored, map, this_arg) {
            return exception;
        }
    }
    created
}

/// 步骤 7–13：array-like 路径。len 求值后先构造目标，再逐索引 Get + map +
/// Set（§23.2.2.1 步骤 9–12，Get 顺序对用户 getter 可观察）。
fn from_array_like(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    target: TargetConstructor,
    source: i64,
    map: Option<i64>,
    this_arg: i64,
) -> i64 {
    let length = match array_like_length(ctx, state, source) {
        Ok(length) => length,
        Err(exception) => return exception,
    };
    let created = match create_target(ctx, state, target, length, "from") {
        Ok(created) => created,
        Err(exception) => return exception,
    };
    state.temporary_roots.push(created);
    for index in 0..length {
        let Some(key) = state.intern_text(index.to_string(), value::TAG_STRING) else {
            return fail_dispatch(ctx);
        };
        let stored = match get_property(ctx, state, source, key) {
            Ok(stored) => stored,
            Err(()) => return fail_dispatch(ctx),
        };
        if value::is_exception(stored) {
            return stored;
        }
        if let Err(exception) = map_and_write(ctx, state, created, index, stored, map, this_arg) {
            return exception;
        }
    }
    created
}

/// %TypedArray%.of(...items)（§23.2.2.2）。
pub(crate) fn static_of(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    this_value: i64,
    args: &[i64],
) -> i64 {
    // 步骤 2–3：C = this，IsConstructor 检查。
    let Some(target) = constructor_target(state, this_value) else {
        return not_a_constructor(ctx, state, this_value);
    };
    // 步骤 4–6：newObj = TypedArrayCreateFromConstructor(C, «len»)，逐个 Set。
    let created = match create_target(ctx, state, target, args.len(), "of") {
        Ok(created) => created,
        Err(exception) => return exception,
    };
    let initial_temp_roots = state.temporary_roots.len();
    state.temporary_roots.push(created);
    for (index, stored) in args.iter().copied().enumerate() {
        if let Err(exception) = write_element(ctx, state, created, index, stored) {
            state.temporary_roots.truncate(initial_temp_roots);
            return exception;
        }
    }
    state.temporary_roots.truncate(initial_temp_roots);
    created
}

/// from / of 的构造目标：内在构造器 receiver 合流缺省直建快路径，其余
/// 构造器值走 Construct + ValidateTypedArray。
enum TargetConstructor {
    Intrinsic(TypedArrayKind),
    Value(i64),
}

fn constructor_target(state: &NativeAgentState, this_value: i64) -> Option<TargetConstructor> {
    if let Some(NativeCallableKind::Builtin(builtin, false)) =
        state.native_callable_kind(this_value)
        && let Some(kind) = constructor_kind(builtin)
    {
        return Some(TargetConstructor::Intrinsic(kind));
    }
    is_constructor_value(state, this_value).then_some(TargetConstructor::Value(this_value))
}

/// IsConstructor(C) 为假（§23.2.2.1 / §23.2.2.2 步骤 2）：TypeError，
/// receiver 渲染对齐 V8（undefined / #<Object> / 基元按值）。
fn not_a_constructor(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    this_value: i64,
) -> i64 {
    let rendered = render_getter_receiver(state, Some(this_value));
    type_error(ctx, state, &format!("{rendered} is not a constructor"))
}

/// TypedArrayCreate(C, «len»)：内在构造器直建；其余 Construct +
/// ValidateTypedArray + 最小长度门槛（§23.2.4.2 步骤 3）。
fn create_target(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    target: TargetConstructor,
    length: usize,
    method: &str,
) -> Result<i64, i64> {
    let length_value = value::encode_f64(length as f64);
    match target {
        TargetConstructor::Intrinsic(kind) => {
            let created = super::typedarray::construct_default(ctx, state, &[length_value], kind);
            if value::is_exception(created) {
                Err(created)
            } else {
                Ok(created)
            }
        }
        TargetConstructor::Value(constructor) => super::typedarray_create::create_from_constructor(
            ctx,
            state,
            constructor,
            &[length_value],
            &format!("%TypedArray%.{method}"),
            Some(length),
            None,
        ),
    }
}

/// mapfn 存在时先 Call(mapfn, thisArg, «kValue, 𝔽(k)») 再写入（§23.2.2.1
/// 步骤 6e-ii / 12b–12c）。
fn map_and_write(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    created: i64,
    index: usize,
    stored: i64,
    map: Option<i64>,
    this_arg: i64,
) -> Result<(), i64> {
    let mapped = if let Some(mapfn) = map {
        let mapped = state
            .invoke_callable(
                ctx,
                mapfn,
                this_arg,
                &[stored, value::encode_f64(index as f64)],
            )
            .unwrap_or_else(|| fail_dispatch(ctx));
        if value::is_exception(mapped) {
            return Err(mapped);
        }
        mapped
    } else {
        stored
    };
    write_element(ctx, state, created, index, mapped)
}

/// Set(target, 𝔽(k), value, true) 的 IntegerIndexedElementSet 语义
/// （§10.4.5.16），与索引赋值路径同一口径：越界写静默成功，BigInt 内容
/// 类型不匹配抛 TypeError。
fn write_element(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    target: i64,
    index: usize,
    stored: i64,
) -> Result<(), i64> {
    let Some(array) = state.typed_arrays.get(&value::decode_handle(target)) else {
        return Err(fail_dispatch(ctx));
    };
    if index >= array.length {
        return Ok(());
    }
    if array.kind.is_bigint() != value::is_bigint(stored) {
        return Err(type_error(ctx, state, "Cannot convert value to a BigInt"));
    }
    super::typedarray::set_element(state, target, index, stored)
        .map(|_| ())
        .ok_or_else(|| fail_dispatch(ctx))
}

/// IteratorToList(GetIteratorFromMethod(source, method))（§23.2.2.1.1）：
/// 收集的值逐个锚根，迭代器异常原样传播。
fn iterator_to_list(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    source: i64,
    method: i64,
) -> Result<Vec<i64>, i64> {
    let iterator = iterator_from_method(ctx, state, source, method);
    if value::is_exception(iterator) {
        return Err(iterator);
    }
    let mut values = Vec::new();
    loop {
        let done = iterator_done(ctx, state, &[iterator]);
        if value::is_exception(done) {
            return Err(done);
        }
        if is_truthy(state, done) {
            break;
        }
        let stored = iterator_value(ctx, state, &[iterator], true);
        if value::is_exception(stored) {
            return Err(stored);
        }
        state.temporary_roots.push(stored);
        values.push(stored);
    }
    Ok(values)
}

/// LengthOfArrayLike(source)（§7.3.19）：数组直读长度，其余 Get("length")
/// 后 ToLength 截断。
fn array_like_length(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    source: i64,
) -> Result<usize, i64> {
    if value::is_array(source) {
        return state
            .gc
            .heap()
            .array_length(value::decode_handle(source))
            .map(|length| length as usize)
            .map_err(|_| fail_dispatch(ctx));
    }
    let Some(key) = state.intern_text("length".into(), value::TAG_STRING) else {
        return Err(fail_dispatch(ctx));
    };
    let stored = get_property(ctx, state, source, key).map_err(|()| fail_dispatch(ctx))?;
    if value::is_exception(stored) {
        return Err(stored);
    }
    let number = to_number(state, stored).unwrap_or(0.0);
    Ok(if !number.is_finite() || number <= 0.0 {
        0
    } else {
        number.trunc().min(f64::from(u32::MAX)) as usize
    })
}

/// V8 kCalledNonCallable 的渲染近似：基元带 typeof 前缀（字符串加引号），
/// null 为 "object null"，对象一律 "object"。
fn render_non_callable(state: &NativeAgentState, encoded: i64) -> String {
    if value::is_null(encoded) {
        return "object null".into();
    }
    if value::is_string(encoded) {
        return format!("string \"{}\"", render_value(state, encoded));
    }
    if value::is_f64(encoded) {
        return format!("number {}", render_value(state, encoded));
    }
    if value::is_bool(encoded) {
        return format!("boolean {}", render_value(state, encoded));
    }
    if value::is_bigint(encoded) {
        return format!("bigint {}", render_value(state, encoded));
    }
    if value::is_object(encoded) || value::is_array(encoded) || value::is_proxy(encoded) {
        return "object".into();
    }
    render_getter_receiver(state, Some(encoded))
}
