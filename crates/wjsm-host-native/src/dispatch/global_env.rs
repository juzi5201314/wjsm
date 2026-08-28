//! 全局环境记录（ES §9.1.1.4）宿主实现：对象记录复用全局对象属性，
//! 声明式记录由本模块按 realm（全局对象句柄）维护持久词法绑定表。
//! 脚本模式主程序的 GlobalDeclarationInstantiation（§16.1.7）与
//! eval / vm / Function 的全局边界名字解析共用同一记录。

use std::collections::{HashMap, HashSet};

use wjsm_gc::HeapAccessV2Error;
use wjsm_ir::{Builtin, constants, value};
use wjsm_native_abi::{NativeRuntimeOp, NativeVmContext};

use super::errors::javascript_error;
use super::runtime::{self, fail_dispatch};
use crate::{NativeAgentState, PropertyKey};

const WRITABLE: u32 = constants::FLAG_WRITABLE as u32;
const ENUMERABLE: u32 = constants::FLAG_ENUMERABLE as u32;
const CONFIGURABLE: u32 = constants::FLAG_CONFIGURABLE as u32;

/// 全局声明式记录中的一个词法绑定（let / const / class）。
pub(crate) struct GlobalLexicalBinding {
    pub(crate) value: i64,
    pub(crate) initialized: bool,
    pub(crate) constant: bool,
}

/// 单个 realm 的全局环境记录：词法绑定表 + [[VarNames]]。
#[derive(Default)]
pub(crate) struct GlobalEnvRecord {
    pub(crate) lexical: HashMap<PropertyKey, GlobalLexicalBinding>,
    pub(crate) var_names: HashSet<PropertyKey>,
}

/// 全局词法绑定的读取结果（供 eval 边界解析复用）。
pub(crate) enum GlobalLexicalRead {
    Missing,
    Uninitialized,
    Value(i64),
}

/// 全局词法绑定的写入结果（供 eval 边界解析复用）。
pub(crate) enum GlobalLexicalWrite {
    Missing,
    Uninitialized,
    Constant,
    Written,
}

pub(super) fn dispatch_global_env(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    builtin: Builtin,
    args: &[i64],
) -> Option<i64> {
    Some(match builtin {
        Builtin::GlobalEnvCheck => check(ctx, state, args),
        Builtin::GlobalEnvDeclareVar => declare_var(ctx, state, args),
        Builtin::GlobalEnvDeclareFunc => declare_func(ctx, state, args),
        Builtin::GlobalEnvDeclareLex => declare_lex(ctx, state, args),
        Builtin::GlobalEnvInitLex => init_lex(ctx, state, args),
        Builtin::GlobalEnvGet => get_binding(ctx, state, args),
        Builtin::GlobalEnvSet => set_binding(ctx, state, args),
        Builtin::GlobalEnvDelete => delete_binding(ctx, state, args),
        _ => return None,
    })
}

/// 全局对象上恒为非可配置数据属性的受限名（HasRestrictedGlobalProperty）。
/// wjsm 的全局内建按需惰性物化，自有槽位不可靠，按规范名单静态判定。
fn is_restricted_global_name(state: &NativeAgentState, key: i64) -> bool {
    ["undefined", "NaN", "Infinity"]
        .iter()
        .any(|name| state.text_matches(key, name))
}

/// 读取指定 realm 的全局词法绑定（供本模块与 eval 边界解析复用）。
pub(crate) fn lexical_read(
    state: &NativeAgentState,
    global: i64,
    key: PropertyKey,
) -> GlobalLexicalRead {
    let Some(record) = state.global_env_records.get(&value::decode_handle(global)) else {
        return GlobalLexicalRead::Missing;
    };
    match record.lexical.get(&key) {
        None => GlobalLexicalRead::Missing,
        Some(binding) if !binding.initialized => GlobalLexicalRead::Uninitialized,
        Some(binding) => GlobalLexicalRead::Value(binding.value),
    }
}

/// 写入指定 realm 的全局词法绑定（SetMutableBinding 语义，不含对象记录回退）。
pub(crate) fn lexical_write(
    state: &mut NativeAgentState,
    global: i64,
    key: PropertyKey,
    stored: i64,
) -> GlobalLexicalWrite {
    let Some(binding) = state
        .global_env_records
        .get_mut(&value::decode_handle(global))
        .and_then(|record| record.lexical.get_mut(&key))
    else {
        return GlobalLexicalWrite::Missing;
    };
    if !binding.initialized {
        return GlobalLexicalWrite::Uninitialized;
    }
    if binding.constant {
        return GlobalLexicalWrite::Constant;
    }
    binding.value = stored;
    GlobalLexicalWrite::Written
}

/// 指定 realm 的全局词法绑定是否存在（HasBinding，供 eval 边界解析复用）。
pub(crate) fn lexical_has(state: &NativeAgentState, global: i64, key: PropertyKey) -> bool {
    state
        .global_env_records
        .get(&value::decode_handle(global))
        .is_some_and(|record| record.lexical.contains_key(&key))
}

/// 指定 realm 的 [[VarNames]] 是否包含该名（HasVarDeclaration）。
fn has_var_declaration(state: &NativeAgentState, global: i64, key: PropertyKey) -> bool {
    state
        .global_env_records
        .get(&value::decode_handle(global))
        .is_some_and(|record| record.var_names.contains(&key))
}

fn record_mut<'a>(state: &'a mut NativeAgentState, global: i64) -> &'a mut GlobalEnvRecord {
    state
        .global_env_records
        .entry(value::decode_handle(global))
        .or_default()
}

fn binding_name(state: &NativeAgentState, key: i64) -> String {
    state
        .string_owned(key)
        .and_then(|text| text.to_utf8())
        .unwrap_or_else(|| runtime::render_value(state, key))
}

fn redeclaration_error(ctx: &mut NativeVmContext, state: &mut NativeAgentState, key: i64) -> i64 {
    let name = binding_name(state, key);
    javascript_error(
        ctx,
        state,
        "SyntaxError",
        format!("Identifier '{name}' has already been declared"),
    )
}

/// GlobalDeclarationInstantiation（§16.1.7）步骤 1–6 的声明冲突预检。
/// kind=0：词法名（HasVarDeclaration / HasLexicalDeclaration /
/// HasRestrictedGlobalProperty）；kind=1：var / 函数名（HasLexicalDeclaration）。
fn check(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let [global, name, kind] = args else {
        return fail_dispatch(ctx);
    };
    let Some(key) = runtime::property_key(state, *name) else {
        return fail_dispatch(ctx);
    };
    let is_lexical = value::decode_f64(*kind) == 0.0;
    if lexical_has(state, *global, key) {
        return redeclaration_error(ctx, state, *name);
    }
    if is_lexical
        && (has_var_declaration(state, *global, key) || is_restricted_global_name(state, *name))
    {
        return redeclaration_error(ctx, state, *name);
    }
    value::encode_undefined()
}

/// 全局对象是否已持有该名（自有槽位或惰性内建）；用于 CreateGlobalVarBinding
/// 的 HasOwnProperty 判定——惰性内建视为既有属性，避免被 undefined 遮蔽。
fn global_has_own_or_lazy(
    state: &mut NativeAgentState,
    global: i64,
    name: i64,
    key: PropertyKey,
) -> bool {
    let own = value::is_object(global)
        && state
            .gc
            .heap()
            .get_property_slot(value::decode_handle(global), key)
            .ok()
            .flatten()
            .is_some();
    own || state.global_property(global, name).is_some()
}

/// 带 TLAB / OOM 重试的显式特性数据属性定义。
fn define_with_flags(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    global: i64,
    key: PropertyKey,
    stored: i64,
    flags: u32,
) -> Result<(), i64> {
    let handle = value::decode_handle(global);
    let define = |state: &mut NativeAgentState| {
        state
            .gc
            .heap()
            .define_data_property(handle, key, stored as u64, flags)
    };
    match define(state) {
        Ok(()) => Ok(()),
        Err(HeapAccessV2Error::NativeTlabNeedsMaterialization { .. }) => {
            state
                .gc
                .flush_native_tlab(ctx)
                .map_err(|_| fail_dispatch(ctx))?;
            define(state).map_err(|_| fail_dispatch(ctx))
        }
        Err(HeapAccessV2Error::HeapExhausted { .. }) => {
            state.collect_garbage(ctx).map_err(|_| fail_dispatch(ctx))?;
            define(state).map_err(|_| fail_dispatch(ctx))
        }
        Err(_) => Err(fail_dispatch(ctx)),
    }
}

/// CreateGlobalVarBinding（§9.1.1.4.17）：属性缺失且全局对象可扩展时定义
/// {undefined, writable, enumerable, configurable=args[2]}，并计入 [[VarNames]]。
fn declare_var(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let [global, name, configurable] = args else {
        return fail_dispatch(ctx);
    };
    let Some(key) = runtime::property_key(state, *name) else {
        return fail_dispatch(ctx);
    };
    if !global_has_own_or_lazy(state, *global, *name, key) {
        if state
            .non_extensible_objects
            .contains(&value::decode_handle(*global))
        {
            let text = binding_name(state, *name);
            return javascript_error(
                ctx,
                state,
                "TypeError",
                format!("Cannot define property {text}, object is not extensible"),
            );
        }
        let mut flags = WRITABLE | ENUMERABLE;
        if value::is_bool(*configurable) && value::decode_bool(*configurable) {
            flags |= CONFIGURABLE;
        }
        if let Err(exception) =
            define_with_flags(ctx, state, *global, key, value::encode_undefined(), flags)
        {
            return exception;
        }
    }
    record_mut(state, *global).var_names.insert(key);
    value::encode_undefined()
}

/// CreateGlobalFunctionBinding（§9.1.1.4.18）：既有属性可配置（或缺失）时按
/// {value, writable, enumerable, configurable=false} 重定义，否则仅更新值；
/// 并计入 [[VarNames]]。属性缺失且全局对象不可扩展时抛 TypeError
/// （DefinePropertyOrThrow）。
fn declare_func(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let [global, name, stored] = args else {
        return fail_dispatch(ctx);
    };
    let Some(key) = runtime::property_key(state, *name) else {
        return fail_dispatch(ctx);
    };
    let existing = value::is_object(*global)
        .then(|| {
            state
                .gc
                .heap()
                .get_property_slot(value::decode_handle(*global), key)
                .ok()
                .flatten()
        })
        .flatten();
    if existing.is_none()
        && !global_has_own_or_lazy(state, *global, *name, key)
        && state
            .non_extensible_objects
            .contains(&value::decode_handle(*global))
    {
        let text = binding_name(state, *name);
        return javascript_error(
            ctx,
            state,
            "TypeError",
            format!("Cannot define property {text}, object is not extensible"),
        );
    }
    let flags = match existing {
        Some(property) if property.flags & CONFIGURABLE == 0 => {
            property.flags & !(constants::FLAG_IS_ACCESSOR as u32)
        }
        _ => WRITABLE | ENUMERABLE,
    };
    if let Err(exception) = define_with_flags(ctx, state, *global, key, *stored, flags) {
        return exception;
    }
    record_mut(state, *global).var_names.insert(key);
    value::encode_undefined()
}

/// 全局声明式记录 CreateMutableBinding / CreateImmutableBinding：
/// 创建未初始化（TDZ）词法绑定。
fn declare_lex(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let [global, name, is_const] = args else {
        return fail_dispatch(ctx);
    };
    let Some(key) = runtime::property_key(state, *name) else {
        return fail_dispatch(ctx);
    };
    let constant = value::is_bool(*is_const) && value::decode_bool(*is_const);
    record_mut(state, *global).lexical.insert(
        key,
        GlobalLexicalBinding {
            value: value::encode_uninitialized(),
            initialized: false,
            constant,
        },
    );
    value::encode_undefined()
}

/// 全局声明式记录 InitializeBinding：写入初值并解除 TDZ。
fn init_lex(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let [global, name, stored] = args else {
        return fail_dispatch(ctx);
    };
    let Some(key) = runtime::property_key(state, *name) else {
        return fail_dispatch(ctx);
    };
    let Some(binding) = state
        .global_env_records
        .get_mut(&value::decode_handle(*global))
        .and_then(|record| record.lexical.get_mut(&key))
    else {
        return fail_dispatch(ctx);
    };
    binding.value = *stored;
    binding.initialized = true;
    value::encode_undefined()
}

fn tdz_error(ctx: &mut NativeVmContext, state: &mut NativeAgentState, name: i64) -> i64 {
    let text = binding_name(state, name);
    javascript_error(
        ctx,
        state,
        "ReferenceError",
        format!("Cannot access '{text}' before initialization"),
    )
}

fn not_defined_error(ctx: &mut NativeVmContext, state: &mut NativeAgentState, name: i64) -> i64 {
    let text = binding_name(state, name);
    javascript_error(
        ctx,
        state,
        "ReferenceError",
        format!("{text} is not defined"),
    )
}

/// 全局环境 ResolveBinding + GetValue：声明式记录 →（TDZ 检查）→ 全局对象
/// 属性；均未命中时按 flags bit0（typeof 容忍）返回 undefined 或抛
/// ReferenceError。
fn get_binding(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let [global, name, flags] = args else {
        return fail_dispatch(ctx);
    };
    let Some(key) = runtime::property_key(state, *name) else {
        return fail_dispatch(ctx);
    };
    match lexical_read(state, *global, key) {
        GlobalLexicalRead::Value(stored) => return stored,
        GlobalLexicalRead::Uninitialized => return tdz_error(ctx, state, *name),
        GlobalLexicalRead::Missing => {}
    }
    let Ok(result) = runtime::get_property(ctx, state, *global, *name) else {
        return fail_dispatch(ctx);
    };
    if !value::is_undefined(result) || runtime::has_property(state, *global, *name) {
        return result;
    }
    let typeof_tolerant = value::decode_f64(*flags) as i64 & 1 != 0;
    if typeof_tolerant {
        return value::encode_undefined();
    }
    not_defined_error(ctx, state, *name)
}

/// 全局环境 SetMutableBinding / PutValue：声明式记录命中检查 TDZ 与 const；
/// 否则按对象记录 [[Set]]（strict 未解析名抛 ReferenceError，sloppy 创建
/// 隐式全局）。
fn set_binding(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let [global, name, stored, strict] = args else {
        return fail_dispatch(ctx);
    };
    let Some(key) = runtime::property_key(state, *name) else {
        return fail_dispatch(ctx);
    };
    match lexical_write(state, *global, key, *stored) {
        GlobalLexicalWrite::Written => return *stored,
        GlobalLexicalWrite::Uninitialized => return tdz_error(ctx, state, *name),
        GlobalLexicalWrite::Constant => {
            return javascript_error(
                ctx,
                state,
                "TypeError",
                "Assignment to constant variable.".to_string(),
            );
        }
        GlobalLexicalWrite::Missing => {}
    }
    let is_strict = value::is_bool(*strict) && value::decode_bool(*strict);
    if is_strict {
        let exists = global_has_own_or_lazy(state, *global, *name, key)
            || runtime::has_property(state, *global, *name);
        if !exists {
            return not_defined_error(ctx, state, *name);
        }
    }
    let operation = if is_strict {
        NativeRuntimeOp::SetPropStrict
    } else {
        NativeRuntimeOp::SetProp
    };
    runtime::dispatch_runtime(ctx, state, operation, &[*global, *name, *stored], None)
}

/// 全局环境 DeleteBinding：词法绑定与受限全局属性不可删除（false）；
/// 其余按全局对象 [[Delete]]。
fn delete_binding(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let [global, name] = args else {
        return fail_dispatch(ctx);
    };
    let Some(key) = runtime::property_key(state, *name) else {
        return fail_dispatch(ctx);
    };
    if lexical_has(state, *global, key) || is_restricted_global_name(state, *name) {
        return value::encode_bool(false);
    }
    match runtime::delete_property(state, *global, *name) {
        Ok(deleted) => value::encode_bool(deleted),
        Err(()) => fail_dispatch(ctx),
    }
}
