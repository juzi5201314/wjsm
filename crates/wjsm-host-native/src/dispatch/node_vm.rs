use std::collections::HashMap;

use wjsm_ir::value;
use wjsm_native_abi::NativeVmContext;

use super::{fail_dispatch, modules, object};
use crate::{NativeAgentState, NativeCallableKind};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum NodeVmCallable {
    CompileFunction,
    CreateContext,
    IsContext,
    RunInContext,
    RunInNewContext,
    RunInThisContext,
    ScriptRunInContext,
    ScriptRunInNewContext,
    ScriptRunInThisContext,
}

#[derive(Default)]
pub(crate) struct NodeVmState {
    bridge: Option<i64>,
    contexts: HashMap<u32, VmContextOptions>,
    active_contexts: Vec<i64>,
    deadlines: Vec<std::time::Instant>,
    next_script_id: u64,
}

#[derive(Clone, Copy)]
struct VmContextOptions {
    after_evaluate: bool,
    strings_enabled: bool,
    array_prototype: i64,
    array_constructor: i64,
}
pub(crate) fn is_context(state: &NativeAgentState, value: i64) -> bool {
    state
        .node_vm
        .contexts
        .contains_key(&value::decode_handle(value))
}
pub(crate) fn current_context(state: &NativeAgentState) -> i64 {
    state
        .node_vm
        .active_contexts
        .last()
        .copied()
        .or(state.global_object)
        .unwrap_or_else(value::encode_undefined)
}
pub(crate) fn array_constructor_for_context(state: &NativeAgentState, context: i64) -> Option<i64> {
    state
        .node_vm
        .contexts
        .get(&value::decode_handle(context))
        .map(|options| options.array_constructor)
}

pub(crate) fn array_prototype_for_handle(state: &NativeAgentState, context: u32) -> Option<i64> {
    state
        .node_vm
        .contexts
        .get(&context)
        .map(|options| options.array_prototype)
}

fn create_array_intrinsics(state: &mut NativeAgentState, context: u32) -> Option<(i64, i64)> {
    state.ensure_intrinsic_prototypes().ok()?;
    let object_prototype = state.object_prototype?;
    let array_prototype = state.allocate_array_values(&[]).ok()?;
    state
        .gc
        .heap()
        .set_prototype(
            value::decode_handle(array_prototype),
            value::decode_handle(object_prototype),
        )
        .ok()?;
    let array_constructor =
        state.native_callable(NativeCallableKind::RealmArrayConstructor(context))?;
    let prototype_key = state.intern_property_string("prototype".into())?;
    state
        .callable_properties
        .insert((array_constructor, prototype_key), array_prototype);
    state.callable_property_flags.insert(
        (array_constructor, prototype_key),
        crate::FUNCTION_PROTOTYPE_FLAGS,
    );
    Some((array_prototype, array_constructor))
}
impl NodeVmState {
    pub(crate) fn current_deadline(&self) -> Option<std::time::Instant> {
        self.deadlines.last().copied()
    }
}

pub(crate) fn ensure_bridge(state: &mut NativeAgentState) -> Option<i64> {
    if let Some(bridge) = state.node_vm.bridge {
        return Some(bridge);
    }
    let bridge = state.allocate_object(9, false).ok()?;
    for (name, method) in [
        ("compileFunction", NodeVmCallable::CompileFunction),
        ("createContext", NodeVmCallable::CreateContext),
        ("isContext", NodeVmCallable::IsContext),
        ("runInContext", NodeVmCallable::RunInContext),
        ("runInNewContext", NodeVmCallable::RunInNewContext),
        ("runInThisContext", NodeVmCallable::RunInThisContext),
        ("scriptRunInContext", NodeVmCallable::ScriptRunInContext),
        (
            "scriptRunInNewContext",
            NodeVmCallable::ScriptRunInNewContext,
        ),
        (
            "scriptRunInThisContext",
            NodeVmCallable::ScriptRunInThisContext,
        ),
    ] {
        let callable = state.native_callable(NativeCallableKind::NodeVm(method))?;
        modules::set_named_property(state, bridge, name, callable).ok()?;
    }
    state.node_vm.bridge = Some(bridge);
    Some(bridge)
}

pub(crate) fn call(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    callable: NodeVmCallable,
    args: &[i64],
) -> i64 {
    match callable {
        NodeVmCallable::CompileFunction => compile_function(ctx, state, args),
        NodeVmCallable::CreateContext => create_context(ctx, state, args),
        NodeVmCallable::IsContext => value::encode_bool(
            args.first()
                .is_some_and(|context| is_context(state, *context)),
        ),
        NodeVmCallable::RunInContext | NodeVmCallable::ScriptRunInContext => {
            run(ctx, state, args, RunTarget::Context)
        }
        NodeVmCallable::RunInNewContext | NodeVmCallable::ScriptRunInNewContext => {
            run(ctx, state, args, RunTarget::NewContext)
        }
        NodeVmCallable::RunInThisContext | NodeVmCallable::ScriptRunInThisContext => {
            run(ctx, state, args, RunTarget::ThisContext)
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum RunTarget {
    Context,
    NewContext,
    ThisContext,
}

fn create_context(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let sandbox = match args.first().copied() {
        Some(sandbox) if value::is_js_object(sandbox) => sandbox,
        Some(sandbox) if !value::is_undefined(sandbox) => {
            return type_error(ctx, state, "The sandbox argument must be an object");
        }
        _ => match state.allocate_object(4, false) {
            Ok(sandbox) => sandbox,
            Err(_) => return fail_dispatch(ctx),
        },
    };
    let options = args.get(1).copied().unwrap_or_else(value::encode_undefined);
    let after_evaluate = modules::named_property(state, options, "microtaskMode")
        .and_then(|mode| state.string_owned(mode))
        .and_then(|text| text.to_utf8())
        .is_some_and(|mode| mode == "afterEvaluate");
    let strings_enabled = context_strings_enabled(state, options);
    let context = value::decode_handle(sandbox);
    let Some((array_prototype, array_constructor)) = state
        .node_vm
        .contexts
        .get(&context)
        .map(|options| (options.array_prototype, options.array_constructor))
        .or_else(|| create_array_intrinsics(state, context))
    else {
        return fail_dispatch(ctx);
    };
    state.node_vm.contexts.insert(
        context,
        VmContextOptions {
            after_evaluate,
            strings_enabled,
            array_prototype,
            array_constructor,
        },
    );
    sandbox
}

fn run(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
    target: RunTarget,
) -> i64 {
    let Some(source) = args
        .first()
        .and_then(|source| state.string_owned(*source))
        .and_then(|text| text.to_utf8())
    else {
        return type_error(ctx, state, "The code argument must be a string");
    };
    let global = match target {
        RunTarget::Context => {
            let Some(global) = args
                .get(1)
                .copied()
                .filter(|global| is_context(state, *global))
            else {
                return type_error(ctx, state, "The contextifiedObject must be a vm context");
            };
            global
        }
        RunTarget::NewContext => {
            let sandbox_args = args.get(1).copied().into_iter().collect::<Vec<_>>();
            let global = create_context(ctx, state, &sandbox_args);
            if value::is_exception(global) {
                return global;
            }
            global
        }
        RunTarget::ThisContext => match state.global_object {
            Some(global) => global,
            None => return fail_dispatch(ctx),
        },
    };
    if !strings_enabled(state, global) && contains_string_codegen(&source) {
        return eval_error(ctx, state);
    }
    let logical_url = next_url(state, "run");
    let options = match target {
        RunTarget::Context | RunTarget::NewContext => args.get(2).copied(),
        RunTarget::ThisContext => args.get(1).copied(),
    };
    let timeout = timeout_deadline(ctx, state, options);
    let has_deadline = timeout.is_some();
    if let Some(deadline) = timeout {
        state.node_vm.deadlines.push(deadline);
    }
    let previous_array_prototype = state.array_prototype;
    let previous_array_prototype_handle = ctx.array_prototype_handle;
    if target != RunTarget::ThisContext
        && let Some(prototype) = array_prototype_for_handle(state, value::decode_handle(global))
    {
        state.array_prototype = Some(prototype);
        ctx.array_prototype_handle = value::decode_handle(prototype);
    }
    let result = (|| {
        state.node_vm.active_contexts.push(global);
        let execution = modules::execute_vm_script(ctx, state, &source, global, &logical_url);
        state.node_vm.active_contexts.pop();
        if has_deadline {
            state.node_vm.deadlines.pop();
        }
        let result = match execution {
            Ok(result) => result,
            Err(modules::VmExecutionError::Host(
                wjsm_native_abi::PendingExceptionKind::Terminated,
            )) => return timeout_error(ctx, state),
            Err(error) => return vm_error(ctx, state, error),
        };
        if target != RunTarget::ThisContext
            && state
                .node_vm
                .contexts
                .get(&value::decode_handle(global))
                .is_some_and(|options| options.after_evaluate)
        {
            let drained = super::promise::drain_microtasks(ctx, state);
            if value::is_exception(drained) {
                return drained;
            }
        }
        result
    })();
    state.array_prototype = previous_array_prototype;
    ctx.array_prototype_handle = previous_array_prototype_handle;
    result
}

fn compile_function(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(body) = args
        .first()
        .and_then(|body| state.string_owned(*body))
        .and_then(|text| text.to_utf8())
    else {
        return type_error(ctx, state, "The code argument must be a string");
    };
    let params = match args.get(1).copied() {
        Some(params) if value::is_array(params) => match string_array(state, params) {
            Some(params) => params,
            None => return type_error(ctx, state, "params must be an array of strings"),
        },
        Some(params) if !value::is_undefined(params) => {
            return type_error(ctx, state, "params must be an array of strings");
        }
        _ => Vec::new(),
    };
    let options = args.get(2).copied().unwrap_or_else(value::encode_undefined);
    let Some(mut global) = state.global_object else {
        return fail_dispatch(ctx);
    };
    if !value::is_undefined(options)
        && let Some(parsing_context) = modules::named_property(state, options, "parsingContext")
        && !value::is_undefined(parsing_context)
    {
        if !state
            .node_vm
            .contexts
            .contains_key(&value::decode_handle(parsing_context))
        {
            return type_error(ctx, state, "options.parsingContext must be a vm context");
        }
        global = parsing_context;
    }
    if !value::is_undefined(options)
        && let Some(extensions) = modules::named_property(state, options, "contextExtensions")
        && value::is_array(extensions)
    {
        let Ok(overlay) = state.allocate_object(4, false) else {
            return fail_dispatch(ctx);
        };
        if state
            .gc
            .heap()
            .set_prototype(value::decode_handle(overlay), value::decode_handle(global))
            .is_err()
            || !copy_extensions(state, overlay, extensions)
        {
            return fail_dispatch(ctx);
        }
        global = overlay;
    }
    let logical_url = next_url(state, "function");
    match modules::compile_vm_function(ctx, state, &body, &params, global, &logical_url) {
        Ok(function) => function,
        Err(error) => vm_error(ctx, state, error),
    }
}
fn contains_string_codegen(source: &str) -> bool {
    source.contains("eval(") || source.contains("Function(")
}

fn eval_error(ctx: &mut NativeVmContext, state: &mut NativeAgentState) -> i64 {
    modules::named_error_object(
        state,
        "EvalError",
        "Code generation from strings disallowed for this context".into(),
    )
    .and_then(|error| state.create_exception(error))
    .unwrap_or_else(|| fail_dispatch(ctx))
}
fn timeout_deadline(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    options: Option<i64>,
) -> Option<std::time::Instant> {
    let options = options.filter(|options| !value::is_undefined(*options))?;
    let timeout = modules::named_property(state, options, "timeout")?;
    if !value::is_f64(timeout) {
        let _ = type_error(ctx, state, "options.timeout must be a number");
        return None;
    }
    let timeout = value::decode_f64(timeout);
    if !timeout.is_finite() || timeout < 1.0 {
        let _ = type_error(ctx, state, "options.timeout must be >= 1");
        return None;
    }
    Some(std::time::Instant::now() + std::time::Duration::from_secs_f64(timeout / 1000.0))
}

fn timeout_error(ctx: &mut NativeVmContext, state: &mut NativeAgentState) -> i64 {
    modules::named_error_object(state, "Error", "Script execution timed out".into())
        .and_then(|error| state.create_exception(error))
        .unwrap_or_else(|| fail_dispatch(ctx))
}
fn string_array(state: &NativeAgentState, array: i64) -> Option<Vec<String>> {
    let handle = value::decode_handle(array);
    let length = state.gc.heap().array_length(handle).ok()?;
    (0..length)
        .map(|index| {
            state
                .gc
                .heap()
                .get_element(handle, index)
                .ok()
                .flatten()
                .map(|entry| entry as i64)
                .and_then(|entry| state.string_owned(entry))
                .and_then(|text| text.to_utf8())
        })
        .collect()
}
fn context_strings_enabled(state: &mut NativeAgentState, options: i64) -> bool {
    for name in ["codeGeneration", "contextCodeGeneration"] {
        let Some(policy) = modules::named_property(state, options, name) else {
            continue;
        };
        if modules::named_property(state, policy, "strings")
            .is_some_and(|enabled| value::is_bool(enabled) && !value::decode_bool(enabled))
        {
            return false;
        }
    }
    true
}

pub(crate) fn strings_enabled(state: &NativeAgentState, context: i64) -> bool {
    state
        .node_vm
        .contexts
        .get(&value::decode_handle(context))
        .is_none_or(|options| options.strings_enabled)
}

fn copy_extensions(state: &mut NativeAgentState, target: i64, extensions: i64) -> bool {
    let handle = value::decode_handle(extensions);
    let Ok(length) = state.gc.heap().array_length(handle) else {
        return false;
    };
    for index in 0..length {
        let Some(extension) = state
            .gc
            .heap()
            .get_element(handle, index)
            .ok()
            .flatten()
            .map(|entry| entry as i64)
        else {
            return false;
        };
        let Some(properties) = object::own_keys(state, extension, true) else {
            return false;
        };
        for (key, stored) in properties {
            let Some(key) = super::runtime::property_key(state, key) else {
                return false;
            };
            if state
                .gc
                .heap()
                .set_property(value::decode_handle(target), key, stored as u64)
                .is_err()
            {
                return false;
            }
        }
    }
    true
}

pub(crate) fn next_url(state: &mut NativeAgentState, kind: &str) -> String {
    let id = state.node_vm.next_script_id;
    state.node_vm.next_script_id = id.saturating_add(1);
    format!("vm:{kind}:{id}")
}

fn vm_error(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    error: modules::VmExecutionError,
) -> i64 {
    if let modules::VmExecutionError::JavaScript(exception) = error {
        return exception;
    }
    let name = if matches!(error, modules::VmExecutionError::Compile(_)) {
        "SyntaxError"
    } else {
        "Error"
    };
    modules::named_error_object(state, name, error.to_string())
        .and_then(|error| state.create_exception(error))
        .unwrap_or_else(|| fail_dispatch(ctx))
}

fn type_error(ctx: &mut NativeVmContext, state: &mut NativeAgentState, message: &str) -> i64 {
    modules::named_error_object(state, "TypeError", message.into())
        .and_then(|error| state.create_exception(error))
        .unwrap_or_else(|| fail_dispatch(ctx))
}
