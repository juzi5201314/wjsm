use anyhow::{Context, Result};
use wjsm_snapshot_format::{
    ManagedHeapV2ArtifactAbi, decode_managed_heap_v2_artifact_abi,
    encode_managed_heap_v2_artifact_abi,
};

pub(crate) fn build_artifact_abi_bytes() -> Result<Vec<u8>> {
    let engine = wjsm_engine_config::EngineConfig::artifact().build()?;
    let engine_fingerprint = wjsm_engine_config::compatibility_fingerprint(&engine);
    let support_abi_hash = wjsm_runtime_support::abi::managed_heap_v2_support_abi_hash();
    let artifact = ManagedHeapV2ArtifactAbi {
        engine_fingerprint,
        support_abi_hash,
    };
    let bytes = encode_managed_heap_v2_artifact_abi(artifact);
    decode_managed_heap_v2_artifact_abi(&bytes, engine_fingerprint, support_abi_hash)
        .context("managed heap V2 artifact ABI self-validation failed")?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_snapshot_v2_artifact_abi_self_validates() {
        let bytes = build_artifact_abi_bytes().expect("build V2 artifact ABI");
        let engine = wjsm_engine_config::EngineConfig::artifact()
            .build()
            .expect("artifact engine");
        let engine_fingerprint = wjsm_engine_config::compatibility_fingerprint(&engine);
        let support_abi_hash = wjsm_runtime_support::abi::managed_heap_v2_support_abi_hash();
        let artifact =
            decode_managed_heap_v2_artifact_abi(&bytes, engine_fingerprint, support_abi_hash)
                .expect("decode V2 artifact ABI");

        assert_eq!(artifact.engine_fingerprint, engine_fingerprint);
        assert_eq!(artifact.support_abi_hash, support_abi_hash);
    }
}
