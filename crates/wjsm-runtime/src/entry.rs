use wasmtime::*;

use crate::types::*;
use crate::host;

pub(crate) fn build_imports(store: &mut Store<RuntimeState>) -> Vec<Extern> {
    let mut tagged: Vec<(usize, Func)> = Vec::new();
    tagged.extend(host::console::create_host_functions(store));
    tagged.extend(host::float::create_host_functions(store));
    tagged.extend(host::iterator::create_host_functions(store));
    tagged.extend(host::operators::create_host_functions(store));
    tagged.extend(host::string_ops::create_host_functions(store));
    tagged.extend(host::equality::create_host_functions(store));
    tagged.extend(host::gc::create_host_functions(store));
    tagged.extend(host::timer::create_host_functions(store));
    tagged.extend(host::array::create_host_functions(store));
    tagged.extend(host::object::create_host_functions(store));
    tagged.extend(host::builtins::create_host_functions(store));
    tagged.extend(host::bigint::create_host_functions(store));
    tagged.extend(host::symbol::create_host_functions(store));
    tagged.extend(host::regexp::create_host_functions(store));
    tagged.extend(host::promise::create_host_functions(store));
    tagged.extend(host::async_mod::create_host_functions(store));

    tagged.sort_by_key(|(idx, _)| *idx);
    tagged.into_iter().map(|(_, f)| f.into()).collect()
}
