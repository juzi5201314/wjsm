//! eval 桥接 builtin 的宿主实现：直接/间接 eval、作用域绑定读写。

use wjsm_ir::{Builtin, value};
use wjsm_native_abi::{NativeRuntimeOp, NativeVmContext};

use super::errors::javascript_error;
use super::global_env;
use super::modules;
use super::node_vm;
use super::runtime::{self, fail_dispatch};
use super::with_env;
use crate::NativeAgentState;

pub(super) fn dispatch_eval(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    builtin: Builtin,
    args: &[i64],
) -> Option<i64> {
    Some(match builtin {
        Builtin::EvalIndirect => eval_indirect(ctx, state, args),
        Builtin::Eval => eval_dynamic(ctx, state, args),
        Builtin::EvalGetBinding => eval_get_binding(ctx, state, args),
        Builtin::EvalSetBinding => eval_set_binding(ctx, state, args),
        Builtin::EvalHasBinding => {
            let [environment, key] = args else {
                return Some(fail_dispatch(ctx));
            };
            match eval_binding_exists(ctx, state, *environment, *key) {
                Ok(exists) => value::encode_bool(exists),
                Err(exception) => exception,
            }
        }
        Builtin::EvalDeleteBinding => {
            let [environment, key] = args else {
                return Some(fail_dispatch(ctx));
            };
            eval_delete_binding(ctx, state, *environment, *key)
        }
        Builtin::EvalSuperBase => {
            let [environment] = args else {
                return Some(fail_dispatch(ctx));
            };
            let record = effective_scope_record(state, *environment);
            modules::scope_record_super_base(state, record).unwrap_or_else(value::encode_undefined)
        }
        Builtin::EvalWithBase => {
            let [environment, key] = args else {
                return Some(fail_dispatch(ctx));
            };
            let record = effective_scope_record(state, *environment);
            match resolve_with_layers(ctx, state, record, *key) {
                WithLayerResolution::Object(object) => object,
                WithLayerResolution::Static => value::encode_undefined(),
                WithLayerResolution::Abrupt(exception) => exception,
            }
        }
        _ => return None,
    })
}

/// eval 绑定 builtin 的环境实参归约：direct eval 主函数直传调用方
/// ScopeRecord；嵌套闭包传入的是 env 对象链（原型链根接记录，见语义层
/// eval 桥物化），沿链归约到记录后环境链才能穿过嵌套函数到调用方
/// （GetIdentifierReference §9.4.2）。链上无记录时保持原值（平面语义）。
fn effective_scope_record(state: &NativeAgentState, environment: i64) -> i64 {
    modules::resolve_scope_record(state, environment).unwrap_or(environment)
}

/// 名字经 ScopeRecord with 层链（由内到外）的解析结果。
enum WithLayerResolution {
    /// 命中某层 with 对象环境记录：读写以该对象为基座。
    Object(i64),
    /// 被内侧静态绑定遮蔽或全链未命中：回退平面静态绑定 / outer。
    Static,
    /// has 探测（proxy trap / `@@unscopables` getter）抛出。
    Abrupt(i64),
}

/// 按 GetIdentifierReference（§9.4.2）的层序在静态绑定与 with 对象之间路由：
/// 每层先看是否被内侧静态绑定遮蔽，再做对象 HasBinding 探测，命中即短路。
fn resolve_with_layers(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    environment: i64,
    key: i64,
) -> WithLayerResolution {
    if !modules::scope_record_has_with_layers(state, environment) {
        return WithLayerResolution::Static;
    }
    for (object, shadowed) in modules::scope_record_with_layers_for(state, environment, key) {
        if shadowed {
            return WithLayerResolution::Static;
        }
        match with_env::with_has_binding(ctx, state, object, key) {
            Ok(true) => return WithLayerResolution::Object(object),
            Ok(false) => {}
            Err(exception) => return WithLayerResolution::Abrupt(exception),
        }
    }
    WithLayerResolution::Static
}

/// 间接 eval：源码在新建的全局作用域记录中执行。
fn eval_indirect(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let [code] = args else {
        return fail_dispatch(ctx);
    };
    let global = node_vm::current_context(state);
    if !node_vm::strings_enabled(state, global) {
        return modules::named_error_object(
            state,
            "EvalError",
            "Code generation from strings disallowed for this context".into(),
        )
        .and_then(|error| state.create_exception(error))
        .unwrap_or_else(|| fail_dispatch(ctx));
    }
    let Some(source) = state.string_to_utf8(*code) else {
        return *code;
    };
    let Some(environment) = modules::create_scope_record_with_outer(state, global) else {
        return fail_dispatch(ctx);
    };
    let result =
        modules::execute_eval_script(ctx, state, &source, environment, global, "eval:indirect");
    modules::destroy_scope_record(state, environment);
    eval_execution_result(ctx, state, result)
}

/// 直接 eval：源码在传入的环境记录中执行。
fn eval_dynamic(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let [code, environment] = args else {
        return fail_dispatch(ctx);
    };
    let global = node_vm::current_context(state);
    if !node_vm::strings_enabled(state, global) {
        return modules::named_error_object(
            state,
            "EvalError",
            "Code generation from strings disallowed for this context".into(),
        )
        .and_then(|error| state.create_exception(error))
        .unwrap_or_else(|| fail_dispatch(ctx));
    }
    let Some(source) = state.string_to_utf8(*code) else {
        return *code;
    };
    let result =
        modules::execute_eval_script(ctx, state, &source, *environment, global, "eval:dynamic");
    eval_execution_result(ctx, state, result)
}

fn eval_get_binding(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let [environment, key] = args else {
        return fail_dispatch(ctx);
    };
    let mut environment = effective_scope_record(state, *environment);
    let innermost = environment;
    if state.text_matches(*key, "__wjsm_new_target")
        && let Some(new_target) = modules::scope_record_new_target(state, environment)
    {
        return new_target;
    }
    // 记录可成链（嵌套 direct eval：内层记录 outer 接外层桥环境）：逐记录按
    // with 层 → 静态绑定的层序解析，未命中沿 outer 归约到下一记录
    // （GetIdentifierReference 的 outer 递归，§9.4.2）。
    let outer = loop {
        // with 层先于静态绑定按层序路由：声明于 with 体外侧的名字可被对象环境拦截。
        match resolve_with_layers(ctx, state, environment, *key) {
            WithLayerResolution::Object(object) => {
                let Ok(result) = runtime::get_property(ctx, state, object, *key) else {
                    return fail_dispatch(ctx);
                };
                return result;
            }
            WithLayerResolution::Abrupt(exception) => return exception,
            WithLayerResolution::Static => {}
        }
        match modules::scope_record_get(state, environment, *key) {
            modules::ScopeBindingRead::Value(result) => return result,
            modules::ScopeBindingRead::Uninitialized => {
                let name = eval_binding_name(state, *key);
                // 派生构造器 this TDZ（`$this` 仅在 super() 前持哨兵）：文案
                // 对齐 V8 的 super 提示；其余词法绑定用通用 TDZ 文案。
                let message = if name == "$this" {
                    "Must call super constructor in derived class before accessing 'this' \
                     or returning from derived constructor"
                        .to_string()
                } else {
                    format!("Cannot access '{name}' before initialization")
                };
                return javascript_error(ctx, state, "ReferenceError", message);
            }
            modules::ScopeBindingRead::Missing => {}
        }
        let outer = modules::scope_record_outer(state, environment).unwrap_or(environment);
        match modules::resolve_scope_record(state, outer) {
            Some(next) if next != environment => environment = next,
            _ => break outer,
        }
    };
    // 全局环境声明式记录（脚本级 let/const/class）先于全局对象属性命中（§9.1.1.4.1）。
    if let Some(env_key) = runtime::property_key(state, *key) {
        match global_env::lexical_read(state, outer, env_key) {
            global_env::GlobalLexicalRead::Value(result) => return result,
            global_env::GlobalLexicalRead::Uninitialized => {
                let name = eval_binding_name(state, *key);
                return javascript_error(
                    ctx,
                    state,
                    "ReferenceError",
                    format!("Cannot access '{name}' before initialization"),
                );
            }
            global_env::GlobalLexicalRead::Missing => {}
        }
    }
    let Ok(result) = runtime::get_property(ctx, state, outer, *key) else {
        return fail_dispatch(ctx);
    };
    if !value::is_undefined(result) {
        return result;
    }
    match eval_binding_exists(ctx, state, innermost, *key) {
        Ok(true) => result,
        Ok(false) => javascript_error(
            ctx,
            state,
            "ReferenceError",
            format!("{} is not defined", eval_binding_name(state, *key)),
        ),
        Err(exception) => exception,
    }
}

fn eval_set_binding(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let [environment, key, stored] = args else {
        return fail_dispatch(ctx);
    };
    let mut environment = effective_scope_record(state, *environment);
    // 写点有效严格性 = 最内层记录（eval 体自身）的 strict 位：绑定命中链上
    // 外层记录 / with 层 / 全局时仍按写点严格性裁决（PutValue 的 S 参数）。
    let site_strict = modules::scope_record_is_strict(state, environment);
    // 记录链逐层解析（层序同 eval_get_binding），未命中落到最外层记录的
    // outer（realm 全局）。
    let outer_env = loop {
        // with 层命中：PutValue → 对象 [[Set]]。
        match resolve_with_layers(ctx, state, environment, *key) {
            WithLayerResolution::Object(object) => {
                let operation = if site_strict {
                    NativeRuntimeOp::SetPropStrict
                } else {
                    NativeRuntimeOp::SetProp
                };
                return runtime::dispatch_runtime(
                    ctx,
                    state,
                    operation,
                    &[object, *key, *stored],
                    None,
                );
            }
            WithLayerResolution::Abrupt(exception) => return exception,
            WithLayerResolution::Static => {}
        }
        match modules::scope_record_set(state, environment, *key, *stored) {
            modules::ScopeBindingWrite::Updated => return *stored,
            modules::ScopeBindingWrite::Constant => {
                return javascript_error(
                    ctx,
                    state,
                    "TypeError",
                    format!(
                        "assignment to constant `{}`",
                        eval_binding_name(state, *key)
                    ),
                );
            }
            // 非严格不可变绑定（具名函数表达式自身名字，S=false）：写入按
            // eval 体有效严格性分流——严格 TypeError、非严格静默忽略
            // （赋值表达式值仍为 RHS）。
            modules::ScopeBindingWrite::SloppyImmutable => {
                if site_strict {
                    return javascript_error(
                        ctx,
                        state,
                        "TypeError",
                        format!(
                            "assignment to constant `{}`",
                            eval_binding_name(state, *key)
                        ),
                    );
                }
                return *stored;
            }
            modules::ScopeBindingWrite::Missing => {}
        }
        let outer = modules::scope_record_outer(state, environment).unwrap_or(environment);
        match modules::resolve_scope_record(state, outer) {
            Some(next) if next != environment => environment = next,
            _ => break outer,
        }
    };
    // 全局环境声明式记录命中：SetMutableBinding（TDZ / const 检查）先于对象记录。
    if let Some(env_key) = runtime::property_key(state, *key) {
        match global_env::lexical_write(state, outer_env, env_key, *stored) {
            global_env::GlobalLexicalWrite::Written => return *stored,
            global_env::GlobalLexicalWrite::Uninitialized => {
                let name = eval_binding_name(state, *key);
                return javascript_error(
                    ctx,
                    state,
                    "ReferenceError",
                    format!("Cannot access '{name}' before initialization"),
                );
            }
            global_env::GlobalLexicalWrite::Constant => {
                return javascript_error(
                    ctx,
                    state,
                    "TypeError",
                    "Assignment to constant variable.".to_string(),
                );
            }
            global_env::GlobalLexicalWrite::Missing => {}
        }
    }
    if site_strict {
        // 严格 eval 写未入快照的名字：全局对象记录持有该属性（含惰性内建）
        // 时按 [[Set]]（strict）写入；确实缺失才抛 ReferenceError（§9.1.1.1.5）。
        let exists = runtime::property_key(state, *key).is_some_and(|env_key| {
            global_env::global_has_own_or_lazy(state, outer_env, *key, env_key)
        }) || match runtime::has_property(ctx, state, outer_env, *key) {
            Ok(present) => present,
            Err(exception) => return exception,
        };
        if !exists {
            return javascript_error(
                ctx,
                state,
                "ReferenceError",
                format!("{} is not defined", eval_binding_name(state, *key)),
            );
        }
        return runtime::dispatch_runtime(
            ctx,
            state,
            NativeRuntimeOp::SetPropStrict,
            &[outer_env, *key, *stored],
            None,
        );
    }
    runtime::dispatch_runtime(
        ctx,
        state,
        NativeRuntimeOp::SetProp,
        &[outer_env, *key, *stored],
        None,
    )
}

fn eval_binding_exists(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    environment: i64,
    key: i64,
) -> Result<bool, i64> {
    let mut environment = effective_scope_record(state, environment);
    if state.text_matches(key, "__wjsm_new_target")
        && modules::scope_record_new_target(state, environment).is_some()
    {
        return Ok(true);
    }
    // 记录链逐层探测（层序同 eval_get_binding）。
    let outer = loop {
        match resolve_with_layers(ctx, state, environment, key) {
            WithLayerResolution::Object(_) => return Ok(true),
            WithLayerResolution::Abrupt(exception) => return Err(exception),
            WithLayerResolution::Static => {}
        }
        if modules::scope_record_contains(state, environment, key) {
            return Ok(true);
        }
        let outer = modules::scope_record_outer(state, environment).unwrap_or(environment);
        match modules::resolve_scope_record(state, outer) {
            Some(next) if next != environment => environment = next,
            _ => break outer,
        }
    };
    if let Some(env_key) = runtime::property_key(state, key)
        && global_env::lexical_has(state, outer, env_key)
    {
        return Ok(true);
    }
    match runtime::get_property(ctx, state, outer, key) {
        Ok(property) if !value::is_undefined(property) => Ok(true),
        Ok(_) => runtime::has_property(ctx, state, outer, key),
        Err(()) => Ok(false),
    }
}

/// direct/indirect eval 自由名的 DeleteBinding（§13.5.1.2 步骤 3–6）。
/// 层序与 `eval_binding_exists` 一致：with 对象环境记录命中即按 [[Delete]]
/// 裁决（§9.1.1.2.7）；调用方声明式绑定（scope record 快照，含 arguments）
/// 不可删除返回 false（§9.1.1.1.8）；全局词法绑定与受限全局名返回 false；
/// 其余交由全局对象属性 [[Delete]]（可配置属性删除返回 true，缺失名即
/// 不可解析引用亦 true）。嵌套闭包传入 env 对象链时先归约到链根记录，
/// delete 与读取走同一条到调用方的环境链；链上无记录的历史平面形态按
/// 链上命中即声明式绑定（false，绝不从 env 对象删属性）、未命中回退全局。
/// delete 标识符在严格代码是 early error，本 builtin 只会从 sloppy 站点发射。
fn eval_delete_binding(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    environment: i64,
    key: i64,
) -> i64 {
    let mut environment = effective_scope_record(state, environment);
    let global = if modules::is_scope_record(state, environment) {
        // 记录链逐层裁决（层序同 eval_binding_exists）。
        loop {
            match resolve_with_layers(ctx, state, environment, key) {
                WithLayerResolution::Object(base) => {
                    return runtime::delete_property_operator(ctx, state, base, key, false);
                }
                WithLayerResolution::Abrupt(exception) => return exception,
                WithLayerResolution::Static => {}
            }
            if modules::scope_record_contains(state, environment, key)
                || (state.text_matches(key, "arguments")
                    && modules::scope_record_has_arguments(state, environment))
            {
                return value::encode_bool(false);
            }
            let outer = modules::scope_record_outer(state, environment).unwrap_or(environment);
            match modules::resolve_scope_record(state, outer) {
                Some(next) if next != environment => environment = next,
                _ => break outer,
            }
        }
    } else {
        match runtime::has_property(ctx, state, environment, key) {
            Ok(true) => return value::encode_bool(false),
            Ok(false) => {}
            Err(exception) => return exception,
        }
        let Some(global) = state.global_object else {
            return value::encode_bool(true);
        };
        global
    };
    if let Some(env_key) = runtime::property_key(state, key)
        && global_env::lexical_has(state, global, env_key)
    {
        return value::encode_bool(false);
    }
    if global_env::is_restricted_global_name(state, key) {
        return value::encode_bool(false);
    }
    runtime::delete_property_operator(ctx, state, global, key, false)
}

fn eval_binding_name(state: &NativeAgentState, key: i64) -> String {
    state
        .string_owned(key)
        .and_then(|text| text.to_utf8())
        .unwrap_or_else(|| runtime::render_value(state, key))
}

pub(crate) fn eval_execution_result(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    result: Result<i64, modules::VmExecutionError>,
) -> i64 {
    match result {
        Ok(result) => result,
        Err(modules::VmExecutionError::JavaScript(exception)) => exception,
        Err(modules::VmExecutionError::Compile(error)) => {
            if let Some(wjsm_semantic::LoweringError::Diagnostic(diagnostic)) =
                error.downcast_ref::<wjsm_semantic::LoweringError>()
            {
                if diagnostic.message.contains("cannot redeclare identifier") {
                    let identifier = diagnostic.message.split('`').nth(1).unwrap_or("<unknown>");
                    return javascript_error(
                        ctx,
                        state,
                        "SyntaxError",
                        format!("cannot redeclare identifier `{identifier}` in eval"),
                    );
                }
                if diagnostic
                    .message
                    .contains("cannot reassign a const-declared variable")
                {
                    let identifier = diagnostic.message.split('`').nth(1).unwrap_or("<unknown>");
                    return javascript_error(
                        ctx,
                        state,
                        "TypeError",
                        format!("assignment to constant `{identifier}`"),
                    );
                }
            }
            javascript_error(ctx, state, "SyntaxError", error.to_string())
        }
        Err(error) => javascript_error(ctx, state, "Error", error.to_string()),
    }
}
