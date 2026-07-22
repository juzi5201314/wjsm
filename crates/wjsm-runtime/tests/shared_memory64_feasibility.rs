use anyhow::Result;
use std::mem::{align_of, size_of};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;
use wasmtime::{Linker, MemoryType, Module, SharedMemory, Store};
use wjsm_engine_config::{EngineConfig, RuntimeEngineOptions, compatibility_fingerprint};

const WASM_PAGE_SIZE: u64 = 64 * 1024;
const HANDLE_REGION_SIZE: u64 = 32 * 1024 * 1024 * 1024;
const OBJECT_HEAP_BASE: u64 = HANDLE_REGION_SIZE + WASM_PAGE_SIZE;
const HEAP_PAGES: u64 = OBJECT_HEAP_BASE / WASM_PAGE_SIZE + 1;
const INCREMENTS_PER_SIDE: u64 = 10_000;

#[test]
fn shared_memory64_support_cwasm_is_feasible() -> Result<()> {
    const { assert!(OBJECT_HEAP_BASE > HANDLE_REGION_SIZE) };

    let build_engine = EngineConfig::artifact().build()?;
    let runtime_engine = EngineConfig::runtime(RuntimeEngineOptions::default()).build()?;
    assert_eq!(
        compatibility_fingerprint(&build_engine),
        compatibility_fingerprint(&runtime_engine)
    );

    let support_wat = support_module_wat();
    let support_cwasm = build_engine.precompile_module(support_wat.as_bytes())?;
    // SAFETY: `support_cwasm` 是上一行由可信 build engine 直接生成且未被修改的
    // precompile 输出；runtime engine 的 compatibility fingerprint 已在上方验证一致。
    let support_module = unsafe { Module::deserialize(&runtime_engine, &support_cwasm) }?;
    let user_module = Module::new(&runtime_engine, user_module_wat())?;
    let heap_memory = shared_memory64(&runtime_engine)?;

    let mut store = Store::new(&runtime_engine, ());
    // artifact profile 固定 epoch interruption；未设置 deadline 会立即 interrupt。
    store.set_epoch_deadline(u64::MAX);
    let mut linker = Linker::new(&runtime_engine);
    linker.define(&mut store, "env", "__heap_memory", heap_memory.clone())?;
    let user = linker.instantiate(&mut store, &user_module)?;
    let support = linker.instantiate(&mut store, &support_module)?;

    let main_memory = user
        .get_memory(&mut store, "memory")
        .expect("main memory32");
    assert!(!main_memory.ty(&store).is_64());
    assert!(heap_memory.ty().is_64());
    assert!(heap_memory.ty().is_shared());

    run_concurrent_atomic_access(&mut store, &user, &support, heap_memory)?;
    Ok(())
}

fn shared_memory64(engine: &wasmtime::Engine) -> Result<SharedMemory> {
    let ty = MemoryType::builder()
        .memory64(true)
        .shared(true)
        .min(HEAP_PAGES)
        .max(Some(HEAP_PAGES))
        .build()?;
    Ok(SharedMemory::new(engine, ty)?)
}

fn user_module_wat() -> String {
    format!(
        r#"(module
            (import "env" "__heap_memory" (memory $heap i64 {HEAP_PAGES} {HEAP_PAGES} shared))
            (memory $main 1)
            (export "memory" (memory $main))
            (func (export "increment") (param $address i64) (result i64)
                (i64.atomic.rmw.add $heap
                    (local.get $address)
                    (i64.const 1))))"#
    )
}

fn support_module_wat() -> String {
    format!(
        r#"(module
            (import "env" "__heap_memory" (memory $heap i64 {HEAP_PAGES} {HEAP_PAGES} shared))
            (func (export "load") (param $address i64) (result i64)
                (i64.atomic.load $heap (local.get $address))))"#
    )
}

fn run_concurrent_atomic_access(
    store: &mut Store<()>,
    user: &wasmtime::Instance,
    support: &wasmtime::Instance,
    heap_memory: SharedMemory,
) -> Result<()> {
    let increment = user.get_typed_func::<i64, i64>(&mut *store, "increment")?;
    let load = support.get_typed_func::<i64, i64>(&mut *store, "load")?;
    let start = Arc::new(Barrier::new(2));
    let host_start = Arc::clone(&start);
    let host_memory = heap_memory.clone();
    let host = thread::spawn(move || {
        let data = host_memory.data();
        let offset = usize::try_from(OBJECT_HEAP_BASE).expect("memory64 host needs 64-bit usize");
        let end = offset
            .checked_add(size_of::<u64>())
            .expect("object heap word range must not overflow");
        assert!(end <= data.len());
        let base = data.as_ptr().cast::<u8>();
        // SAFETY: `offset..end` 已验证完整落在 `data` 的同一 allocation 内。
        let word_ptr = unsafe { base.add(offset).cast_mut().cast::<u64>() };
        assert_eq!((word_ptr as usize) % align_of::<AtomicU64>(), 0);
        // SAFETY: 上方验证了范围与 AtomicU64 对齐；`host_memory` 在引用使用期间
        // 保持 Wasmtime shared-memory mapping 存活且基址稳定，底层字节由 UnsafeCell
        // 暴露。该 word 的全部竞争访问均为 host AtomicU64 或 Wasm i64 atomic 指令。
        let word = unsafe { AtomicU64::from_ptr(word_ptr) };
        host_start.wait();
        for _ in 0..INCREMENTS_PER_SIDE {
            word.fetch_add(1, Ordering::SeqCst);
        }
    });

    start.wait();
    for _ in 0..INCREMENTS_PER_SIDE {
        increment.call(&mut *store, OBJECT_HEAP_BASE as i64)?;
    }
    host.join().expect("host atomic worker must finish");

    let observed = load.call(&mut *store, OBJECT_HEAP_BASE as i64)? as u64;
    assert_eq!(observed, INCREMENTS_PER_SIDE * 2);
    Ok(())
}
