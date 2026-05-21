// Import 321: get_builtin_global (kept temporarily for semantic layer compat)
{
    let get_builtin_global_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, name_val: i64| -> i64 {
            let name = read_runtime_string(&mut caller, name_val);
            let mut native_callables = caller.data().native_callables.lock().unwrap();
            let idx = native_callables.len() as u32;
            match name.as_str() {
                "Array" => native_callables.push(NativeCallable::ArrayConstructor),
                "Object" => native_callables.push(NativeCallable::ObjectConstructor),
                "Function" => native_callables.push(NativeCallable::FunctionConstructor),
                "String" => native_callables.push(NativeCallable::StringConstructor),
                "Boolean" => native_callables.push(NativeCallable::BooleanConstructor),
                "Number" => native_callables.push(NativeCallable::NumberConstructor),
                "Symbol" => native_callables.push(NativeCallable::SymbolConstructor),
                "BigInt" => native_callables.push(NativeCallable::BigIntConstructor),
                "RegExp" => native_callables.push(NativeCallable::RegExpConstructor),
                "Error" => native_callables.push(NativeCallable::ErrorConstructor),
                "TypeError" => native_callables.push(NativeCallable::TypeErrorConstructor),
                "RangeError" => native_callables.push(NativeCallable::RangeErrorConstructor),
                "SyntaxError" => native_callables.push(NativeCallable::SyntaxErrorConstructor),
                "ReferenceError" => native_callables.push(NativeCallable::ReferenceErrorConstructor),
                "URIError" => native_callables.push(NativeCallable::URIErrorConstructor),
                "EvalError" => native_callables.push(NativeCallable::EvalErrorConstructor),
                "AggregateError" => native_callables.push(NativeCallable::AggregateErrorConstructor),
                "Map" => native_callables.push(NativeCallable::MapConstructor),
                "Set" => native_callables.push(NativeCallable::SetConstructor),
                "WeakMap" => native_callables.push(NativeCallable::WeakMapConstructor),
                "WeakSet" => native_callables.push(NativeCallable::WeakSetConstructor),
                "WeakRef" => native_callables.push(NativeCallable::WeakRefConstructor),
                "FinalizationRegistry" => native_callables.push(NativeCallable::FinalizationRegistryConstructor),
                "Date" => native_callables.push(NativeCallable::DateConstructorGlobal),
                "Promise" => native_callables.push(NativeCallable::PromiseConstructor),
                "ArrayBuffer" => native_callables.push(NativeCallable::ArrayBufferConstructorGlobal),
                "DataView" => native_callables.push(NativeCallable::DataViewConstructorGlobal),
                "Proxy" => native_callables.push(NativeCallable::ProxyConstructor),
                "gc" => native_callables.push(NativeCallable::GcCollect),
                _ => return value::encode_undefined(),
            }
            value::encode_native_callable_idx(idx)
        },
    );
    get_builtin_global_fn.into()
}
