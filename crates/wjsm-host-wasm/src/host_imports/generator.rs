use anyhow::Result;
use wasmtime::Store;
use wasmtime::{Caller, Func, Linker};

use crate::*;

pub(crate) fn define_generator(
    linker: &mut Linker<RuntimeState>,
    mut store: &mut Store<RuntimeState>,
) -> Result<()> {
    let generator_start_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, continuation: i64| -> i64 {
            let generator = alloc_object(&mut caller, 4);
            let generator_proto = caller.data().generator_prototype;
            if !value::is_undefined(generator_proto) {
                let env = WasmEnv::from_caller(&mut caller).expect("WasmEnv");
                crate::runtime_heap::set_object_proto_header(
                    &mut caller,
                    &env,
                    generator,
                    generator_proto,
                );
            }
            if !value::is_object(generator) {
                return value::encode_undefined();
            }
            let next =
                create_generator_method(caller.data(), generator, GeneratorCompletionType::Next);
            let ret =
                create_generator_method(caller.data(), generator, GeneratorCompletionType::Return);
            let throw =
                create_generator_method(caller.data(), generator, GeneratorCompletionType::Throw);
            let iterator_identity = create_generator_identity(caller.data(), generator);
            let _ = define_host_data_property_from_caller(&mut caller, generator, "next", next);
            let _ = define_host_data_property_from_caller(&mut caller, generator, "return", ret);
            let _ = define_host_data_property_from_caller(&mut caller, generator, "throw", throw);
            let _ = define_host_data_property_by_name_id_with_flags(
                &mut caller,
                generator,
                encode_symbol_name_id(wjsm_ir::wk_symbol::ITERATOR),
                iterator_identity,
                constants::FLAG_CONFIGURABLE | constants::FLAG_WRITABLE,
            );
            init_generator_entry(caller.data(), generator, continuation)
        },
    );
    linker.define(&mut store, "env", "generator_start", generator_start_fn)?;

    let generator_next_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, generator: i64, value: i64| -> i64 {
            generator_yield_from_caller(&mut caller, generator, value)
        },
    );
    linker.define(&mut store, "env", "generator_next", generator_next_fn)?;

    let generator_return_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, generator: i64, value: i64| -> i64 {
            generator_return_from_caller(&mut caller, generator, value)
        },
    );
    linker.define(&mut store, "env", "generator_return", generator_return_fn)?;

    let generator_throw_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, generator: i64, value: i64| -> i64 {
            generator_throw_from_caller(&mut caller, generator, value)
        },
    );
    linker.define(&mut store, "env", "generator_throw", generator_throw_fn)?;

    Ok(())
}
