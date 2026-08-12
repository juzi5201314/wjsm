use wjsm_ir::{Builtin, value};
use wjsm_native_abi::NativeVmContext;

use super::runtime::{create_iterator_result, fail_dispatch, type_error};
use crate::NativeAgentState;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GeneratorStatus {
    SuspendedStart,
    SuspendedYield,
    Executing,
    Completed,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct NativeGenerator {
    continuation: i64,
    status: GeneratorStatus,
}

pub(super) fn dispatch_generator(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    builtin: Builtin,
    args: &[i64],
) -> Option<i64> {
    Some(match builtin {
        Builtin::GeneratorStart => start(ctx, state, args),
        Builtin::GeneratorNext => next(ctx, state, args),
        Builtin::GeneratorReturn => return_(ctx, state, args),
        Builtin::GeneratorThrow => throw(ctx, state, args),
        _ => return None,
    })
}

pub(crate) fn method(state: &NativeAgentState, receiver: i64, key: &str) -> Option<Builtin> {
    is_generator(state, receiver).then(|| match key {
        "next" => Some(Builtin::GeneratorNext),
        "return" => Some(Builtin::GeneratorReturn),
        "throw" => Some(Builtin::GeneratorThrow),
        _ => None,
    })?
}

pub(crate) fn is_generator(state: &NativeAgentState, receiver: i64) -> bool {
    value::is_object(receiver)
        && state
            .generators
            .contains_key(&value::decode_object_handle(receiver))
}

fn start(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(continuation) = args.first().copied() else {
        return fail_dispatch(ctx);
    };
    let continuation_handle = value::decode_handle(continuation);
    let Some(record) = state.continuations.get_mut(&continuation_handle) else {
        return fail_dispatch(ctx);
    };
    let Some([state_slot, completion_slot, ..]) = record.vars.get_mut(..) else {
        return fail_dispatch(ctx);
    };
    *state_slot = value::encode_f64(0.0);
    *completion_slot = value::encode_f64(0.0);
    let Ok(generator) = state.allocate_object(0, false) else {
        return fail_dispatch(ctx);
    };
    state.generators.insert(
        value::decode_object_handle(generator),
        NativeGenerator {
            continuation,
            status: GeneratorStatus::SuspendedStart,
        },
    );
    generator
}

fn next(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(generator) = generator_argument(ctx, state, args) else {
        return type_error(
            ctx,
            state,
            "Generator.prototype.next called on incompatible receiver",
        );
    };
    let resumed = args.get(1).copied().unwrap_or_else(value::encode_undefined);
    match state.generators[&generator].status {
        GeneratorStatus::Executing => suspend_with(ctx, state, generator, resumed, false),
        GeneratorStatus::Completed => {
            create_iterator_result(ctx, state, value::encode_undefined(), true)
        }
        GeneratorStatus::SuspendedStart | GeneratorStatus::SuspendedYield => {
            resume(ctx, state, generator, resumed, 0.0)
        }
    }
}

fn return_(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(generator) = generator_argument(ctx, state, args) else {
        return type_error(
            ctx,
            state,
            "Generator.prototype.return called on incompatible receiver",
        );
    };
    let returned = args.get(1).copied().unwrap_or_else(value::encode_undefined);
    match state.generators[&generator].status {
        GeneratorStatus::Executing => suspend_with(ctx, state, generator, returned, true),
        GeneratorStatus::SuspendedYield => resume(ctx, state, generator, returned, 2.0),
        GeneratorStatus::SuspendedStart | GeneratorStatus::Completed => {
            state
                .generators
                .get_mut(&generator)
                .expect("checked")
                .status = GeneratorStatus::Completed;
            create_iterator_result(ctx, state, returned, true)
        }
    }
}

fn throw(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(generator) = generator_argument(ctx, state, args) else {
        return type_error(
            ctx,
            state,
            "Generator.prototype.throw called on incompatible receiver",
        );
    };
    let thrown = args.get(1).copied().unwrap_or_else(value::encode_undefined);
    match state.generators[&generator].status {
        GeneratorStatus::Executing => complete_abrupt(ctx, state, generator, thrown),
        GeneratorStatus::SuspendedYield => resume(ctx, state, generator, thrown, 1.0),
        GeneratorStatus::SuspendedStart | GeneratorStatus::Completed => {
            complete_abrupt(ctx, state, generator, thrown)
        }
    }
}

fn generator_argument(
    _ctx: &mut NativeVmContext,
    state: &NativeAgentState,
    args: &[i64],
) -> Option<u32> {
    let receiver = args.first().copied()?;
    is_generator(state, receiver).then(|| value::decode_object_handle(receiver))
}

fn suspend_with(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    generator: u32,
    result: i64,
    done: bool,
) -> i64 {
    state
        .generators
        .get_mut(&generator)
        .expect("checked")
        .status = if done {
        GeneratorStatus::Completed
    } else {
        GeneratorStatus::SuspendedYield
    };
    create_iterator_result(ctx, state, result, done)
}

fn resume(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    generator: u32,
    resumed: i64,
    completion: f64,
) -> i64 {
    let continuation = state.generators[&generator].continuation;
    let continuation_handle = value::decode_handle(continuation);
    let Some(record) = state.continuations.get_mut(&continuation_handle) else {
        return fail_dispatch(ctx);
    };
    let Some(completion_slot) = record.vars.get_mut(1) else {
        return fail_dispatch(ctx);
    };
    *completion_slot = value::encode_f64(completion);
    let function = record.function;
    state
        .generators
        .get_mut(&generator)
        .expect("checked")
        .status = GeneratorStatus::Executing;
    let result = state
        .invoke_callable_with_environment(ctx, function, continuation, resumed, &[])
        .unwrap_or_else(|| fail_dispatch(ctx));
    if value::is_exception(result) {
        state
            .generators
            .get_mut(&generator)
            .expect("checked")
            .status = GeneratorStatus::Completed;
    } else if state.generators[&generator].status == GeneratorStatus::Executing {
        return fail_dispatch(ctx);
    }
    result
}

fn complete_abrupt(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    generator: u32,
    thrown: i64,
) -> i64 {
    state
        .generators
        .get_mut(&generator)
        .expect("checked")
        .status = GeneratorStatus::Completed;
    state
        .create_exception(thrown)
        .unwrap_or_else(|| fail_dispatch(ctx))
}
