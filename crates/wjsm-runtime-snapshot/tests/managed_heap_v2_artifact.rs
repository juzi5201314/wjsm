#![cfg(all(feature = "embedded", feature = "managed-heap-v2"))]

use wjsm_engine_config::{EngineConfig, compatibility_fingerprint};
use wjsm_runtime_snapshot::EMBEDDED_MANAGED_HEAP_V2_ARTIFACT_ABI;
use wjsm_runtime_support::abi::managed_heap_v2_support_abi_hash;
use wjsm_snapshot_format::decode_managed_heap_v2_artifact_abi;

#[test]
fn embedded_managed_heap_v2_artifact_abi_matches_canonical_engine_and_support() {
    let engine = EngineConfig::artifact().build().expect("artifact engine");
    let engine_fingerprint = compatibility_fingerprint(&engine);
    let support_abi_hash = managed_heap_v2_support_abi_hash();
    let bytes = EMBEDDED_MANAGED_HEAP_V2_ARTIFACT_ABI.expect("embedded V2 artifact ABI");

    let artifact = decode_managed_heap_v2_artifact_abi(bytes, engine_fingerprint, support_abi_hash)
        .expect("decode embedded V2 artifact ABI");

    assert_eq!(artifact.engine_fingerprint, engine_fingerprint);
    assert_eq!(artifact.support_abi_hash, support_abi_hash);
}
