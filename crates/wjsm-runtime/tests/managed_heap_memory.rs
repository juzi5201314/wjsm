use std::sync::{Arc, Barrier};
use std::thread;
use wasmtime::{MemoryType, SharedMemory};
use wjsm_engine_config::EngineConfig;
use wjsm_runtime::{HeapAddress, HeapMemoryError, NativeHeapMemory, SharedHeapMemory};

#[test]
fn native_memory_checks_alignment_bounds_and_u64_addresses() {
    let base = 1_u64 << 40;
    let memory = NativeHeapMemory::with_base(base, 64);
    let word = HeapAddress::new(base + 8);

    memory.store_word(word, 0x0102_0304_0506_0708).unwrap();
    assert_eq!(memory.load_word(word).unwrap(), 0x0102_0304_0506_0708);
    assert_eq!(
        memory.load_word(HeapAddress::new(base + 1)),
        Err(HeapMemoryError::UnalignedWord { address: base + 1 })
    );
    assert!(matches!(
        memory.load_word(HeapAddress::new(base + 64)),
        Err(HeapMemoryError::OutOfBounds { .. })
    ));
}

#[test]
fn native_memory_publishes_words_seqcst_across_threads() {
    let memory = NativeHeapMemory::new(64);
    let producer = memory.clone();
    let barrier = Arc::new(Barrier::new(2));
    let producer_barrier = Arc::clone(&barrier);
    let thread = thread::spawn(move || {
        producer.store_word(HeapAddress::new(0), 99).unwrap();
        producer_barrier.wait();
    });

    barrier.wait();
    assert_eq!(memory.load_word(HeapAddress::new(0)).unwrap(), 99);
    thread.join().unwrap();
}

#[test]
fn native_memory_copies_only_checked_unpublished_byte_ranges() {
    let memory = NativeHeapMemory::new(64);
    memory
        .copy_from(HeapAddress::new(3), &[1, 2, 3, 4])
        .unwrap();
    assert_eq!(
        memory.copy_to(HeapAddress::new(3), 4).unwrap(),
        vec![1, 2, 3, 4]
    );
    assert!(matches!(
        memory.copy_from(HeapAddress::new(63), &[1, 2]),
        Err(HeapMemoryError::OutOfBounds { .. })
    ));
}

#[test]
fn shared_memory_uses_memory64_checked_atomic_words() {
    let engine = EngineConfig::artifact().build().unwrap();
    let ty = MemoryType::builder()
        .memory64(true)
        .shared(true)
        .min(1)
        .max(Some(1))
        .build()
        .unwrap();
    let memory = SharedMemory::new(&engine, ty).unwrap();
    let heap = SharedHeapMemory::new(memory);

    heap.store_word(HeapAddress::new(16), 0xfeed_face).unwrap();
    assert_eq!(heap.load_word(HeapAddress::new(16)).unwrap(), 0xfeed_face);
}
