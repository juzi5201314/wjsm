#![cfg(feature = "managed-heap-v2")]

use wasmparser::{Parser, Payload};
use wasmtime::{MemoryType, SharedMemory};
use wjsm_engine_config::EngineConfig;
use wjsm_runtime::{HeapAccessV2, SharedHeapMemory};

const HANDLE_REGION_BYTES: u64 = 32 * 1024 * 1024 * 1024;
const PAGE_BYTES: u64 = 64 * 1024;

fn heap_access() -> HeapAccessV2 {
    let engine = EngineConfig::artifact().build().unwrap();
    let min_pages = HANDLE_REGION_BYTES / PAGE_BYTES + 2;
    let max_pages = min_pages + 4;
    let memory = SharedMemory::new(
        &engine,
        MemoryType::builder()
            .memory64(true)
            .shared(true)
            .min(min_pages)
            .max(Some(max_pages))
            .build()
            .unwrap(),
    )
    .unwrap();
    HeapAccessV2::new(SharedHeapMemory::new(memory))
}

#[test]
fn v2_compiler_imports_memory64_array_host_abi() {
    let wasm =
        wjsm_runtime::compile_source("const array = ['value']; console.log(array);").unwrap();
    let imports = Parser::new(0)
        .parse_all(&wasm)
        .filter_map(Result::ok)
        .filter_map(|payload| match payload {
            Payload::ImportSection(section) => Some(section),
            _ => None,
        })
        .flat_map(|section| section.into_imports().filter_map(Result::ok))
        .map(|import| import.name.to_string())
        .collect::<Vec<_>>();

    assert!(imports.iter().any(|name| name == "__heap_memory"));
}

#[test]
fn host_heap_access_v2() {
    let wasm = wjsm_runtime::compile_source(
        "const key = ['answer'].join(''); const object = {}; object[key] = 42; const array = [7, 8]; const map = new Map(); map.set(key, object); const proxy = new Proxy(object, { get(target, property) { return target[property]; } }); console.log(map.has(key), map.get(key) === object, array[1], proxy[key]);",
    )
    .unwrap();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let (output, diagnostics) = runtime
        .block_on(wjsm_runtime::execute_with_writer(&wasm, Vec::new()))
        .unwrap();

    assert!(diagnostics.is_empty());
    assert_eq!(String::from_utf8(output).unwrap(), "true true 8 42\n");
}

#[test]
fn heap_access_v2_resolves_8byte_handle_and_updates_heap_relative_property_slot() {
    let access = heap_access();
    let handle = 41;
    let object = HANDLE_REGION_BYTES + PAGE_BYTES;

    access.publish_object(handle, object, u32::MAX, 2).unwrap();
    access.set_property(handle, 17, 123).unwrap();

    assert_eq!(access.resolve_handle(handle).unwrap(), object);
    assert_eq!(access.get_property(handle, 17).unwrap(), Some(123));
    assert_eq!(access.get_property(handle, 99).unwrap(), None);
}

#[test]
fn heap_access_v2_grows_object_property_capacity_without_losing_slots() {
    let access = heap_access();
    access.reserve_nlab(PAGE_BYTES).unwrap();
    let handle = 43;
    let object = HANDLE_REGION_BYTES + PAGE_BYTES;
    access.publish_object(handle, object, u32::MAX, 1).unwrap();

    access.set_property(handle, 17, 123).unwrap();
    access.set_property(handle, 18, 456).unwrap();

    assert_ne!(access.resolve_handle(handle).unwrap(), object);
    assert_eq!(access.get_property(handle, 17).unwrap(), Some(123));
    assert_eq!(access.get_property(handle, 18).unwrap(), Some(456));
}

#[test]
fn heap_access_v2_publishes_and_updates_array_elements() {
    let access = heap_access();
    let handle = 42;
    let object = HANDLE_REGION_BYTES + PAGE_BYTES;

    access.publish_array(handle, object, u32::MAX, 3).unwrap();
    access.set_element(handle, 0, 7).unwrap();
    access.set_element(handle, 1, 8).unwrap();

    assert_eq!(access.push_element(handle, 9).unwrap(), 3);
    assert_eq!(access.get_element(handle, 0).unwrap(), Some(7));
    assert_eq!(access.get_element(handle, 1).unwrap(), Some(8));
    assert_eq!(access.get_element(handle, 2).unwrap(), Some(9));
}

#[test]
fn v2_runtime_executes_runtime_string_computed_property_access() {
    let wasm = wjsm_runtime::compile_source(
        "const key = 'answer'; const object = {}; object[key] = 42; console.log(object[key]);",
    )
    .unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (output, diagnostics) = runtime
        .block_on(wjsm_runtime::execute_with_writer(&wasm, Vec::new()))
        .unwrap();

    assert!(diagnostics.is_empty());
    assert_eq!(String::from_utf8(output).unwrap(), "42\n");
}

#[test]
fn v2_runtime_distinguishes_static_property_keys() {
    let wasm = wjsm_runtime::compile_source(
        "const object = {}; object.first = 1; object.second = 2; console.log(object.first, object.second);",
    )
    .unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (output, diagnostics) = runtime
        .block_on(wjsm_runtime::execute_with_writer(&wasm, Vec::new()))
        .unwrap();

    assert!(diagnostics.is_empty());
    assert_eq!(String::from_utf8(output).unwrap(), "1 2\n");
}

#[test]
fn v2_runtime_distinguishes_private_capture_property_keys() {
    let wasm = wjsm_runtime::compile_source(
        "const object = {}; object['$1.$private_function#1_0'] = 1; object['$1.$private_function#1_1'] = 2; console.log(object['$1.$private_function#1_0'], object['$1.$private_function#1_1']);",
    )
    .unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (output, diagnostics) = runtime
        .block_on(wjsm_runtime::execute_with_writer(&wasm, Vec::new()))
        .unwrap();

    assert!(diagnostics.is_empty());
    assert_eq!(String::from_utf8(output).unwrap(), "1 2\n");
}

#[test]
fn v2_runtime_distinguishes_private_member_function_keys() {
    let wasm = wjsm_runtime::compile_source(
        "const first = () => 1; const second = () => 2; const object = {}; object['#first@0'] = first; object['#second@0'] = second; console.log(object['#first@0'](), object['#second@0']());",
    )
    .unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (output, diagnostics) = runtime
        .block_on(wjsm_runtime::execute_with_writer(&wasm, Vec::new()))
        .unwrap();

    assert!(diagnostics.is_empty());
    assert_eq!(String::from_utf8(output).unwrap(), "1 2\n");
}

#[test]
fn v2_runtime_distinguishes_function_property_values() {
    let wasm = wjsm_runtime::compile_source(
        "const object = { first() { return 1; }, second() { return 2; } }; console.log(object.first(), object.second());",
    )
    .unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (output, diagnostics) = runtime
        .block_on(wjsm_runtime::execute_with_writer(&wasm, Vec::new()))
        .unwrap();

    assert!(diagnostics.is_empty());
    assert_eq!(String::from_utf8(output).unwrap(), "1 2\n");
}
#[test]
fn v2_runtime_executes_dynamic_string_computed_property_access() {
    let wasm = wjsm_runtime::compile_source(
        "const key = ['answer'].join(''); const object = {}; object[key] = 42; console.log(object[key]);",
    )
    .unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (output, diagnostics) = runtime
        .block_on(wjsm_runtime::execute_with_writer(&wasm, Vec::new()))
        .unwrap();

    assert!(diagnostics.is_empty());
    assert_eq!(String::from_utf8(output).unwrap(), "42\n");
}

#[test]
fn v2_runtime_executes_map_methods_without_memory32_reverse_lookup() {
    let wasm = wjsm_runtime::compile_source(
        "const map = new Map(); console.log(map.set('value', 42) === map, map.has('value'), map.get('value'));"
    )
    .unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (output, diagnostics) = runtime
        .block_on(wjsm_runtime::execute_with_writer(&wasm, Vec::new()))
        .unwrap();

    assert!(diagnostics.is_empty());
    assert_eq!(String::from_utf8(output).unwrap(), "true true 42\n");
}

#[test]
fn v2_runtime_executes_set_methods_without_memory32_reverse_lookup() {
    let wasm = wjsm_runtime::compile_source(
        "const set = new Set(); set.add(42); console.log(set.has(42), set.delete(42), set.has(42));",
    )
    .unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (output, diagnostics) = runtime
        .block_on(wjsm_runtime::execute_with_writer(&wasm, Vec::new()))
        .unwrap();

    assert!(diagnostics.is_empty());
    assert_eq!(String::from_utf8(output).unwrap(), "true true false\n");
}

#[test]
fn v2_runtime_executes_collection_size_accessors_without_memory32_reverse_lookup() {
    let wasm = wjsm_runtime::compile_source(
        "const map = new Map(); map.set('value', 42); const set = new Set(); set.add(42); console.log(map.size, set.size);"
    )
    .unwrap();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let (output, diagnostics) = runtime
        .block_on(wjsm_runtime::execute_with_writer(&wasm, Vec::new()))
        .unwrap();

    assert!(diagnostics.is_empty());
    assert_eq!(String::from_utf8(output).unwrap(), "1 1\n");
}

#[test]
fn v2_runtime_executes_collection_for_each_without_memory32_reverse_lookup() {
    let wasm = wjsm_runtime::compile_source(
        "const map = new Map([['first', 2], ['second', 3]]); const set = new Set([5, 7]); let total = 0; map.forEach(value => { total += value; }); set.forEach(value => { total += value; }); console.log(total);",
    )
    .unwrap();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let (output, diagnostics) = runtime
        .block_on(wjsm_runtime::execute_with_writer(&wasm, Vec::new()))
        .unwrap();

    assert!(diagnostics.is_empty());
    assert_eq!(String::from_utf8(output).unwrap(), "17\n");
}

#[test]
fn v2_runtime_preserves_closure_lexical_mutation() {
    let wasm = wjsm_runtime::compile_source(
        "let total = 0; (() => { total += 2; })(); console.log(total);",
    )
    .unwrap();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let (output, diagnostics) = runtime
        .block_on(wjsm_runtime::execute_with_writer(&wasm, Vec::new()))
        .unwrap();

    assert!(diagnostics.is_empty());
    assert_eq!(String::from_utf8(output).unwrap(), "2\n");
}

#[test]
fn v2_runtime_distinguishes_multiple_closure_captures() {
    let wasm = wjsm_runtime::compile_source(
        "const first_value = 1; const second_value = 2; const first = () => first_value; const second = () => second_value; console.log(first(), second());",
    )
    .unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (output, diagnostics) = runtime
        .block_on(wjsm_runtime::execute_with_writer(&wasm, Vec::new()))
        .unwrap();

    assert!(diagnostics.is_empty());
    assert_eq!(String::from_utf8(output).unwrap(), "1 2\n");
}

#[test]
fn v2_runtime_distinguishes_captured_function_values() {
    let wasm = wjsm_runtime::compile_source(
        "const first = () => 1; const second = () => 2; const invoke = () => first() * 10 + second(); console.log(invoke());",
    )
    .unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (output, diagnostics) = runtime
        .block_on(wjsm_runtime::execute_with_writer(&wasm, Vec::new()))
        .unwrap();

    assert!(diagnostics.is_empty());
    assert_eq!(String::from_utf8(output).unwrap(), "12\n");
}

#[test]
fn v2_runtime_constructs_collections_from_iterables_without_memory32_reverse_lookup() {
    let wasm = wjsm_runtime::compile_source(
        "const map = new Map([['value', 42]]); const set = new Set([42]); console.log(map.get('value'), set.has(42));",
    )
    .unwrap();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let (output, diagnostics) = runtime
        .block_on(wjsm_runtime::execute_with_writer(&wasm, Vec::new()))
        .unwrap();

    assert!(diagnostics.is_empty());
    assert_eq!(String::from_utf8(output).unwrap(), "42 true\n");
}

#[test]
fn v2_runtime_executes_array_iterator_without_memory32_reverse_lookup() {
    let wasm = wjsm_runtime::compile_source(
        "const iterator = ['value', 42].values(); const first = iterator.next(); const second = iterator.next(); console.log(first.value, first.done, second.value, second.done);",
    )
    .unwrap();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let (output, diagnostics) = runtime
        .block_on(wjsm_runtime::execute_with_writer(&wasm, Vec::new()))
        .unwrap();

    assert!(diagnostics.is_empty());
    assert_eq!(String::from_utf8(output).unwrap(), "value false 42 false\n");
}

#[test]
fn v2_runtime_grows_array_through_push_without_memory32_reverse_lookup() {
    let wasm = wjsm_runtime::compile_source(
        "const array = []; console.log(array.push(1), array.length, array[0]);",
    )
    .unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (output, diagnostics) = runtime
        .block_on(wjsm_runtime::execute_with_writer(&wasm, Vec::new()))
        .unwrap();

    assert!(diagnostics.is_empty());
    assert_eq!(String::from_utf8(output).unwrap(), "1 1 1\n");
}

#[test]
fn v2_runtime_executes_process_env_proxy_traps_without_memory32_reverse_lookup() {
    let wasm = wjsm_runtime::compile_source(
        "process.env.B = '3'; console.log('B' in process.env, process.env.B); const keys = Object.keys(process.env); const sorted = keys.sort(); console.log(sorted === keys, typeof sorted.join, sorted.join(','));",
    )
    .unwrap();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let options = wjsm_runtime::RuntimeOptions {
        env: vec![("B".to_string(), "2".to_string())],
        ..wjsm_runtime::RuntimeOptions::default()
    };
    let (output, diagnostics) = runtime
        .block_on(wjsm_runtime::execute_with_writer_with_options(
            &wasm,
            Vec::new(),
            options,
        ))
        .unwrap();

    assert!(diagnostics.is_empty());
    assert_eq!(
        String::from_utf8(output).unwrap(),
        "true 2\ntrue function B\n"
    );
}

#[test]
fn v2_runtime_publishes_array_symbol_iterator_without_memory32_reverse_lookup() {
    let wasm =
        wjsm_runtime::compile_source("console.log(typeof ['value'][Symbol.iterator]);").unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (output, diagnostics) = runtime
        .block_on(wjsm_runtime::execute_with_writer(&wasm, Vec::new()))
        .unwrap();

    assert!(diagnostics.is_empty());
    assert_eq!(String::from_utf8(output).unwrap(), "function\n");
}

#[test]
fn v2_runtime_publishes_array_values_method_without_memory32_reverse_lookup() {
    let wasm = wjsm_runtime::compile_source("console.log(typeof ['value'].values);").unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (output, diagnostics) = runtime
        .block_on(wjsm_runtime::execute_with_writer(&wasm, Vec::new()))
        .unwrap();

    assert!(diagnostics.is_empty());
    assert_eq!(String::from_utf8(output).unwrap(), "function\n");
}

#[test]
fn v2_runtime_resolves_native_callable_prototype_properties() {
    let wasm = wjsm_runtime::compile_source(
        "function f() { console.log(arguments[Symbol.iterator] === Array.prototype.values); } f(1, 2);",
    )
    .unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (output, diagnostics) = runtime
        .block_on(wjsm_runtime::execute_with_writer(&wasm, Vec::new()))
        .unwrap();

    assert!(diagnostics.is_empty());
    assert_eq!(String::from_utf8(output).unwrap(), "true\n");
}

#[test]
fn v2_runtime_preserves_private_field_method_and_accessor_protocol() {
    let wasm = wjsm_runtime::compile_source(
        "let setter_calls = 0; class Counter { #value = 1; #method() { return this.#value; } get #accessor() { return this.#value + 100; } set #accessor(value) { setter_calls = setter_calls + 1; this.#value = value; } readMethod() { return this.#method(); } readAccessor() { return this.#accessor; } writeAccessor(value) { this.#accessor = value; } } const counter = new Counter(); console.log(counter.readMethod(), counter.readAccessor()); counter.writeAccessor(42); console.log(counter.readMethod(), counter.readAccessor(), setter_calls);",
    )
    .unwrap();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let (output, diagnostics) = runtime
        .block_on(wjsm_runtime::execute_with_writer(&wasm, Vec::new()))
        .unwrap();

    assert!(diagnostics.is_empty());
    assert_eq!(String::from_utf8(output).unwrap(), "1 101\n42 142 1\n");
}

#[test]
fn v2_runtime_distinguishes_private_method_callbacks() {
    let wasm = wjsm_runtime::compile_source(
        "class Counter { #first() { return 1; } #second() { return 2; } first() { return this.#first(); } second() { return this.#second(); } } const counter = new Counter(); console.log(counter.first(), counter.second());",
    )
    .unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (output, diagnostics) = runtime
        .block_on(wjsm_runtime::execute_with_writer(&wasm, Vec::new()))
        .unwrap();

    assert!(diagnostics.is_empty());
    assert_eq!(String::from_utf8(output).unwrap(), "1 2\n");
}

#[test]
fn v2_runtime_distinguishes_private_field_values() {
    let wasm = wjsm_runtime::compile_source(
        "class Counter { #first = 1; #second = 2; first() { return this.#first; } second() { return this.#second; } } const counter = new Counter(); console.log(counter.first(), counter.second());",
    )
    .unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (output, diagnostics) = runtime
        .block_on(wjsm_runtime::execute_with_writer(&wasm, Vec::new()))
        .unwrap();

    assert!(diagnostics.is_empty());
    assert_eq!(String::from_utf8(output).unwrap(), "1 2\n");
}

#[test]
fn v2_runtime_invokes_private_accessors() {
    let wasm = wjsm_runtime::compile_source(
        "class Counter { get #accessor() { return 101; } set #accessor(value) { this.called = true; this.publicValue = value; } get() { return this.#accessor; } set(value) { this.#accessor = value; } } const counter = new Counter(); console.log(counter.get()); counter.set(42); console.log(counter.called, counter.publicValue);"
    )
    .unwrap();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let (output, diagnostics) = runtime
        .block_on(wjsm_runtime::execute_with_writer(&wasm, Vec::new()))
        .unwrap();

    assert!(diagnostics.is_empty());
    assert_eq!(String::from_utf8(output).unwrap(), "101\ntrue 42\n");
}

#[test]
fn v2_runtime_executes_proxy_property_access_without_memory32_reverse_lookup() {
    let wasm = wjsm_runtime::compile_source(
        "const target = { answer: 42 }; const proxy = new Proxy(target, {}); console.log(proxy.answer);",
    )
    .unwrap();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let (output, diagnostics) = runtime
        .block_on(wjsm_runtime::execute_with_writer(&wasm, Vec::new()))
        .unwrap();

    assert!(diagnostics.is_empty());
    assert_eq!(String::from_utf8(output).unwrap(), "42\n");
}
#[test]
fn v2_runtime_executes_proxy_get_trap_without_memory32_reverse_lookup() {
    let wasm = wjsm_runtime::compile_source(
        "const target = { answer: 42 }; const proxy = new Proxy(target, { get(target, key) { return key === 'answer' ? target.answer + 1 : undefined; } }); console.log(proxy.answer);",
    )
    .unwrap();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let (output, diagnostics) = runtime
        .block_on(wjsm_runtime::execute_with_writer(&wasm, Vec::new()))
        .unwrap();

    assert!(diagnostics.is_empty());
    assert_eq!(String::from_utf8(output).unwrap(), "43\n");
}

#[test]
fn v2_runtime_executes_proxy_set_trap_without_memory32_reverse_lookup() {
    let wasm = wjsm_runtime::compile_source(
        "const target = { answer: 0 }; const proxy = new Proxy(target, { set(target, key, value) { target[key] = value + 1; return true; } }); proxy.answer = 42; console.log(target.answer);",
    )
    .unwrap();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let (output, diagnostics) = runtime
        .block_on(wjsm_runtime::execute_with_writer(&wasm, Vec::new()))
        .unwrap();

    assert!(diagnostics.is_empty());
    assert_eq!(String::from_utf8(output).unwrap(), "43\n");
}

#[test]
fn v2_runtime_executes_collection_values_without_memory32_reverse_lookup() {
    let wasm = wjsm_runtime::compile_source(
        "const map = new Map(); const value = { answer: 42 }; map.set('value', value); console.log(map.get('value').answer);",
    )
    .unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (output, diagnostics) = runtime
        .block_on(wjsm_runtime::execute_with_writer(&wasm, Vec::new()))
        .unwrap();

    assert!(diagnostics.is_empty());
    assert_eq!(String::from_utf8(output).unwrap(), "42\n");
}
#[test]
fn v2_runtime_executes_object_and_array_access_without_memory32_reverse_lookup() {
    let wasm = wjsm_runtime::compile_source(
        "const object = { answer: 42 }; const array = [7, 8]; console.log(object.answer, array[1]);",
    )
    .unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (output, diagnostics) = runtime
        .block_on(wjsm_runtime::execute_with_writer(&wasm, Vec::new()))
        .unwrap();

    assert!(diagnostics.is_empty());
    assert_eq!(String::from_utf8(output).unwrap(), "42 8\n");
}

#[test]
fn v2_runtime_assigns_toobject_wrapper_into_target_preserves_indexed_properties() {
    let wasm = wjsm_runtime::compile_source(
        "const out = Object.assign({}, \"ab\"); console.log(Object.keys(out).length, out[0], out[1]);",
    )
    .unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (output, diagnostics) = runtime
        .block_on(wjsm_runtime::execute_with_writer(&wasm, Vec::new()))
        .unwrap();

    assert!(diagnostics.is_empty());
    assert_eq!(String::from_utf8(output).unwrap(), "2 a b\n");
}

#[test]
fn v2_runtime_assigns_string_source_keys_preserve_distinct_names() {
    let wasm = wjsm_runtime::compile_source(
        "const out = Object.assign({x:1}, \"abc\"); console.log(Object.keys(out).length, out[0], out[1], out[2]);",
    )
    .unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (output, diagnostics) = runtime
        .block_on(wjsm_runtime::execute_with_writer(&wasm, Vec::new()))
        .unwrap();

    assert!(diagnostics.is_empty());
    assert_eq!(String::from_utf8(output).unwrap(), "4 a b c\n");
}

#[test]
fn v2_set_property_with_distinct_runtime_keys_does_not_collapse_into_one_slot() {
    // 白盒模拟 V2 to_object("abc") 写 3 个索引 + 1 个 length；
    // 用不同 encode_runtime_string_name_id 值作 key（与 intern 表等价形态）。
    let access = heap_access();
    access.reserve_nlab(PAGE_BYTES).unwrap();
    let handle = 47;
    let object = HANDLE_REGION_BYTES + PAGE_BYTES;
    access.publish_object(handle, object, u32::MAX, 4).unwrap();

    const K0: u32 = 0x4000_0000;
    const K1: u32 = 0x4000_0001;
    const K2: u32 = 0x4000_0002;

    access.set_property(handle, K0, 11).unwrap();
    access.set_property(handle, K1, 22).unwrap();
    access.set_property(handle, K2, 33).unwrap();

    let slots = access.own_property_slots(handle).unwrap();
    assert_eq!(slots.len(), 3, "expected 3 distinct slots, got {slots:?}");

    assert_eq!(access.get_property(handle, K0).unwrap(), Some(11));
    assert_eq!(access.get_property(handle, K1).unwrap(), Some(22));
    assert_eq!(access.get_property(handle, K2).unwrap(), Some(33));
}
