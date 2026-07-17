#![cfg(feature = "managed-heap-v2")]

use wjsm_runtime::{GcPacketKind, GcWorkPacket, HeapAddress, NativeHeapMemory};

#[test]
fn native_heap_memory_keeps_word_and_byte_boundaries_safe() {
    let memory = NativeHeapMemory::with_base(0x1_0000_0000, 128);
    let word = HeapAddress::new(0x1_0000_0010);

    memory.store_word(word, 0x1122_3344_5566_7788).unwrap();
    assert_eq!(memory.load_word(word).unwrap(), 0x1122_3344_5566_7788);
    memory
        .copy_from(HeapAddress::new(0x1_0000_0021), &[9, 8, 7])
        .unwrap();
    assert_eq!(
        memory.copy_to(HeapAddress::new(0x1_0000_0021), 3).unwrap(),
        [9, 8, 7]
    );
}

#[test]
fn gc_work_packet_is_a_copyable_value_without_worker_deque_execution() {
    let packet = GcWorkPacket::new(GcPacketKind::RelocationRange, 0x1_0000_0000, 4, 9);
    let copy = packet;

    assert_eq!(copy.kind(), GcPacketKind::RelocationRange);
    assert_eq!(copy.start(), 0x1_0000_0000);
    assert_eq!(copy.len(), 4);
    assert_eq!(copy.epoch(), 9);
}
