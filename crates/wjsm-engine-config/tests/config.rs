use anyhow::Result;
#[cfg(target_arch = "aarch64")]
use wjsm_engine_config::CompilerStrategy;
use wjsm_engine_config::{EngineConfig, RuntimeEngineOptions, compatibility_fingerprint};

#[test]
fn artifact_engine_fingerprint_is_stable() -> Result<()> {
    let first = EngineConfig::artifact().build()?;
    let second = EngineConfig::artifact().build()?;

    let first_fingerprint = compatibility_fingerprint(&first);
    assert_ne!(first_fingerprint, 0);
    assert_eq!(first_fingerprint, compatibility_fingerprint(&second));
    Ok(())
}

#[test]
fn default_runtime_engine_matches_artifact_fingerprint() -> Result<()> {
    let artifact = EngineConfig::artifact().build()?;
    let runtime = EngineConfig::runtime(RuntimeEngineOptions::default()).build()?;

    assert_eq!(
        compatibility_fingerprint(&artifact),
        compatibility_fingerprint(&runtime)
    );
    Ok(())
}

#[cfg(target_arch = "aarch64")]
#[test]
fn winch_rejects_missing_threads_capability_on_aarch64() {
    let options = RuntimeEngineOptions {
        compiler: CompilerStrategy::Winch,
        ..RuntimeEngineOptions::default()
    };

    let error = EngineConfig::runtime(options)
        .build()
        .expect_err("AArch64 Winch cannot satisfy the threads contract");
    assert!(error.to_string().contains("required WebAssembly threads"));
}
