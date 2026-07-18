use wjsm_snapshot_format::{
    ManagedHeapV2ArtifactAbi, ManagedHeapV2Generation, ManagedHeapV2Handle, ManagedHeapV2Layout,
    ManagedHeapV2Page, ManagedHeapV2Snapshot, SNAPSHOT_FORMAT_VERSION, SNAPSHOT_MAGIC,
    decode_managed_heap_v2_artifact_abi, decode_managed_heap_v2_snapshot,
    encode_managed_heap_v2_artifact_abi, encode_managed_heap_v2_snapshot,
};

const ENGINE_FINGERPRINT: u64 = 0x4d48_5632_454e_4749;
const SUPPORT_ABI_HASH: u64 = 0x5355_5050_4f52_5456;

fn snapshot() -> ManagedHeapV2Snapshot {
    let object_heap_base = 0x80_0000_0000;
    ManagedHeapV2Snapshot {
        engine_fingerprint: ENGINE_FINGERPRINT,
        layout: ManagedHeapV2Layout {
            object_heap_base,
            object_heap_end: object_heap_base + 2 * 64 * 1024,
            page_bytes: 64 * 1024,
        },
        pages: vec![ManagedHeapV2Page {
            page_id: 0,
            range_start: object_heap_base,
            range_end: object_heap_base + 64 * 1024,
            object_count: 2,
            current_marked: 1,
            previous_marked: 2,
        }],
        handles: vec![
            ManagedHeapV2Handle {
                handle: 1,
                raw_entry: 0x7ff8_0000_0000_0040,
                generation: ManagedHeapV2Generation::Young,
            },
            ManagedHeapV2Handle {
                handle: 7,
                raw_entry: 0x7ff8_0000_0001_0040,
                generation: ManagedHeapV2Generation::Old,
            },
        ],
    }
}

#[test]
fn managed_heap_v2_snapshot_roundtrips_page_and_atomic_handle_metadata() {
    let snapshot = snapshot();

    let bytes = encode_managed_heap_v2_snapshot(&snapshot).unwrap();
    let decoded = decode_managed_heap_v2_snapshot(&bytes, ENGINE_FINGERPRINT).unwrap();

    assert_eq!(decoded, snapshot);
    assert_ne!(&bytes[..SNAPSHOT_MAGIC.len()], &SNAPSHOT_MAGIC);
    assert_eq!(SNAPSHOT_FORMAT_VERSION, 9);
}

#[test]
fn managed_heap_v2_snapshot_rejects_v1_magic_and_engine_fingerprint() {
    assert!(decode_managed_heap_v2_snapshot(&SNAPSHOT_MAGIC, ENGINE_FINGERPRINT).is_err());

    let bytes = encode_managed_heap_v2_snapshot(&snapshot()).unwrap();
    assert!(decode_managed_heap_v2_snapshot(&bytes, ENGINE_FINGERPRINT + 1).is_err());
}

#[test]
fn managed_heap_v2_artifact_abi_binds_engine_and_support_contracts() {
    let artifact = ManagedHeapV2ArtifactAbi {
        engine_fingerprint: ENGINE_FINGERPRINT,
        support_abi_hash: SUPPORT_ABI_HASH,
    };

    let bytes = encode_managed_heap_v2_artifact_abi(artifact);

    assert_eq!(
        decode_managed_heap_v2_artifact_abi(&bytes, ENGINE_FINGERPRINT, SUPPORT_ABI_HASH).unwrap(),
        artifact
    );
    assert!(
        decode_managed_heap_v2_artifact_abi(&bytes, ENGINE_FINGERPRINT, SUPPORT_ABI_HASH + 1)
            .is_err()
    );
}
