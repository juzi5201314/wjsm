use anyhow::Result;
use wasmtime::{Caller, Func, Linker, Store};

use crate::exec_context_impl::WasmExecContext;
use crate::*;

/// 原始字符串属性读取 — gc 与 host import 共用。
pub(crate) fn primitive_string_get_property_impl(
    caller: &mut Caller<'_, RuntimeState>,
    receiver: i64,
    name_id: u32,
) -> i64 {
    let mut ctx = WasmExecContext::new(caller);
    wjsm_builtins::string_methods::primitive_string_get_property(&mut ctx, receiver, name_id)
}

pub(crate) fn define_string_methods(
    linker: &mut Linker<RuntimeState>,
    mut store: &mut Store<RuntimeState>,
) -> Result<()> {
    let primitive_string_get_property_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64, name_id: i32| -> i64 {
            primitive_string_get_property_impl(&mut caller, receiver, name_id as u32)
        },
    );
    linker.define(
        &mut store,
        "env",
        "primitive_string_get_property",
        primitive_string_get_property_fn,
    )?;

    macro_rules! def {
        ($name:expr, $body:expr) => {{
            let f = Func::wrap(&mut store, $body);
            linker.define(&mut store, "env", $name, f)?;
        }};
    }

    def!("string_at", |mut caller: Caller<'_, RuntimeState>,
                       receiver: i64,
                       index: i64|
     -> i64 {
        let mut ctx = WasmExecContext::new(&mut caller);
        wjsm_builtins::string_methods::string_at(&mut ctx, receiver, index)
    });
    def!("string_char_at", |mut caller: Caller<'_, RuntimeState>,
                            receiver: i64,
                            pos: i64|
     -> i64 {
        let mut ctx = WasmExecContext::new(&mut caller);
        wjsm_builtins::string_methods::string_char_at(&mut ctx, receiver, pos)
    });
    def!("string_char_code_at", |mut caller: Caller<
        '_,
        RuntimeState,
    >,
                                 receiver: i64,
                                 pos: i64|
     -> i64 {
        let mut ctx = WasmExecContext::new(&mut caller);
        wjsm_builtins::string_methods::string_char_code_at(&mut ctx, receiver, pos)
    });
    def!("string_code_point_at", |mut caller: Caller<
        '_,
        RuntimeState,
    >,
                                  receiver: i64,
                                  pos: i64|
     -> i64 {
        let mut ctx = WasmExecContext::new(&mut caller);
        wjsm_builtins::string_methods::string_code_point_at(&mut ctx, receiver, pos)
    });
    def!("string_proto_concat", |mut caller: Caller<
        '_,
        RuntimeState,
    >,
                                 _env: i64,
                                 this_val: i64,
                                 args_base: i32,
                                 args_count: i32|
     -> i64 {
        let mut ctx = WasmExecContext::new(&mut caller);
        wjsm_builtins::string_methods::string_proto_concat(
            &mut ctx, this_val, args_base, args_count,
        )
    });
    def!("string_ends_with", |mut caller: Caller<
        '_,
        RuntimeState,
    >,
                              receiver: i64,
                              search: i64,
                              end_pos: i64|
     -> i64 {
        let mut ctx = WasmExecContext::new(&mut caller);
        wjsm_builtins::string_methods::string_ends_with(&mut ctx, receiver, search, end_pos)
    });
    def!("string_includes", |mut caller: Caller<'_, RuntimeState>,
                             receiver: i64,
                             search: i64,
                             pos: i64|
     -> i64 {
        let mut ctx = WasmExecContext::new(&mut caller);
        wjsm_builtins::string_methods::string_includes(&mut ctx, receiver, search, pos)
    });
    def!("string_index_of", |mut caller: Caller<'_, RuntimeState>,
                             receiver: i64,
                             search: i64,
                             pos: i64|
     -> i64 {
        let mut ctx = WasmExecContext::new(&mut caller);
        wjsm_builtins::string_methods::string_index_of(&mut ctx, receiver, search, pos)
    });
    def!("string_last_index_of", |mut caller: Caller<
        '_,
        RuntimeState,
    >,
                                  receiver: i64,
                                  search: i64,
                                  pos: i64|
     -> i64 {
        let mut ctx = WasmExecContext::new(&mut caller);
        wjsm_builtins::string_methods::string_last_index_of(&mut ctx, receiver, search, pos)
    });
    def!("string_pad_end", |mut caller: Caller<'_, RuntimeState>,
                            receiver: i64,
                            target_len: i64,
                            pad_str_val: i64|
     -> i64 {
        let mut ctx = WasmExecContext::new(&mut caller);
        wjsm_builtins::string_methods::string_pad_end(&mut ctx, receiver, target_len, pad_str_val)
    });
    def!("string_pad_start", |mut caller: Caller<
        '_,
        RuntimeState,
    >,
                              receiver: i64,
                              target_len: i64,
                              pad_str_val: i64|
     -> i64 {
        let mut ctx = WasmExecContext::new(&mut caller);
        wjsm_builtins::string_methods::string_pad_start(&mut ctx, receiver, target_len, pad_str_val)
    });
    def!("string_repeat", |mut caller: Caller<'_, RuntimeState>,
                           receiver: i64,
                           count: i64|
     -> i64 {
        let mut ctx = WasmExecContext::new(&mut caller);
        wjsm_builtins::string_methods::string_repeat(&mut ctx, receiver, count)
    });
    def!("string_replace_all", |mut caller: Caller<
        '_,
        RuntimeState,
    >,
                                receiver: i64,
                                search: i64,
                                replace: i64|
     -> i64 {
        let mut ctx = WasmExecContext::new(&mut caller);
        wjsm_builtins::string_methods::string_replace_all(&mut ctx, receiver, search, replace)
    });
    def!("string_slice", |mut caller: Caller<'_, RuntimeState>,
                          receiver: i64,
                          start: i64,
                          end: i64|
     -> i64 {
        let mut ctx = WasmExecContext::new(&mut caller);
        wjsm_builtins::string_methods::string_slice(&mut ctx, receiver, start, end)
    });
    def!("string_starts_with", |mut caller: Caller<
        '_,
        RuntimeState,
    >,
                                receiver: i64,
                                search: i64,
                                pos: i64|
     -> i64 {
        let mut ctx = WasmExecContext::new(&mut caller);
        wjsm_builtins::string_methods::string_starts_with(&mut ctx, receiver, search, pos)
    });
    def!("string_substring", |mut caller: Caller<
        '_,
        RuntimeState,
    >,
                              receiver: i64,
                              start: i64,
                              end: i64|
     -> i64 {
        let mut ctx = WasmExecContext::new(&mut caller);
        wjsm_builtins::string_methods::string_substring(&mut ctx, receiver, start, end)
    });
    def!("string_normalize", |mut caller: Caller<
        '_,
        RuntimeState,
    >,
                              receiver: i64,
                              form_val: i64,
                              unused: i64|
     -> i64 {
        let mut ctx = WasmExecContext::new(&mut caller);
        wjsm_builtins::string_methods::string_normalize(&mut ctx, receiver, form_val, unused)
    });
    def!("string_to_lower_case", |mut caller: Caller<
        '_,
        RuntimeState,
    >,
                                  receiver: i64|
     -> i64 {
        let mut ctx = WasmExecContext::new(&mut caller);
        wjsm_builtins::string_methods::string_to_lower_case(&mut ctx, receiver)
    });
    def!("string_to_upper_case", |mut caller: Caller<
        '_,
        RuntimeState,
    >,
                                  receiver: i64|
     -> i64 {
        let mut ctx = WasmExecContext::new(&mut caller);
        wjsm_builtins::string_methods::string_to_upper_case(&mut ctx, receiver)
    });
    def!("string_trim", |mut caller: Caller<'_, RuntimeState>,
                         receiver: i64|
     -> i64 {
        let mut ctx = WasmExecContext::new(&mut caller);
        wjsm_builtins::string_methods::string_trim(&mut ctx, receiver)
    });
    def!("string_trim_end", |mut caller: Caller<'_, RuntimeState>,
                             receiver: i64|
     -> i64 {
        let mut ctx = WasmExecContext::new(&mut caller);
        wjsm_builtins::string_methods::string_trim_end(&mut ctx, receiver)
    });
    def!("string_trim_start", |mut caller: Caller<
        '_,
        RuntimeState,
    >,
                               receiver: i64|
     -> i64 {
        let mut ctx = WasmExecContext::new(&mut caller);
        wjsm_builtins::string_methods::string_trim_start(&mut ctx, receiver)
    });
    def!("string_to_string", |mut caller: Caller<
        '_,
        RuntimeState,
    >,
                              receiver: i64|
     -> i64 {
        let mut ctx = WasmExecContext::new(&mut caller);
        wjsm_builtins::string_methods::string_to_string(&mut ctx, receiver)
    });
    def!("string_value_of", |mut caller: Caller<'_, RuntimeState>,
                             receiver: i64|
     -> i64 {
        let mut ctx = WasmExecContext::new(&mut caller);
        wjsm_builtins::string_methods::string_value_of(&mut ctx, receiver)
    });
    def!("string_iterator", |mut caller: Caller<'_, RuntimeState>,
                             receiver: i64|
     -> i64 {
        let mut ctx = WasmExecContext::new(&mut caller);
        wjsm_builtins::string_methods::string_iterator(&mut ctx, receiver)
    });
    def!("string_from_char_code", |mut caller: Caller<
        '_,
        RuntimeState,
    >,
                                   _env: i64,
                                   _this: i64,
                                   args_base: i32,
                                   args_count: i32|
     -> i64 {
        let mut ctx = WasmExecContext::new(&mut caller);
        wjsm_builtins::string_methods::string_from_char_code(&mut ctx, args_base, args_count)
    });
    def!("string_from_code_point", |mut caller: Caller<
        '_,
        RuntimeState,
    >,
                                    _env: i64,
                                    _this: i64,
                                    args_base: i32,
                                    args_count: i32|
     -> i64 {
        let mut ctx = WasmExecContext::new(&mut caller);
        wjsm_builtins::string_methods::string_from_code_point(&mut ctx, args_base, args_count)
    });

    Ok(())
}
