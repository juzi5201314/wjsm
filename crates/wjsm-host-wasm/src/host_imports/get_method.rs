use crate::exec_context_impl::WasmExecContext;
use crate::RuntimeState;
use wasmtime::Caller;
pub(crate) fn get_method_by_name_id(caller: &mut Caller<'_, RuntimeState>, obj: i64, name_id: u32) -> Result<Option<i64>, i64> {
    let mut ctx = WasmExecContext::new(caller);
    wjsm_builtins::get_method::get_method_by_name_id(&mut ctx, obj, name_id)
}
pub(crate) fn get_by_name_id_sync(caller: &mut Caller<'_, RuntimeState>, obj: i64, name_id: u32) -> i64 {
    let mut ctx = WasmExecContext::new(caller);
    wjsm_builtins::get_method::get_by_name_id(&mut ctx, obj, name_id)
}
pub(crate) fn invoke_getter_sync(caller: &mut Caller<'_, RuntimeState>, getter: i64, receiver: i64) -> i64 {
    let mut ctx = WasmExecContext::new(caller);
    wjsm_builtins::get_method::invoke_getter(&mut ctx, getter, receiver)
}
pub(crate) fn read_object_property_by_name_id_proto_walk(caller: &mut Caller<'_, RuntimeState>, obj_ptr: usize, name_id: u32) -> Option<i64> {
    let mut ctx = WasmExecContext::new(caller);
    wjsm_builtins::get_method::read_object_property_by_name_id_proto_walk(&mut ctx, obj_ptr, name_id)
}
