//! WJSM 唯一 Wasmtime engine 配置 owner。
//!
//! 所有 profile 固定开启 threads / shared-memory / memory64 / multi-memory /
//! bulk-memory，并保持现有 backtrace / address-map 不变量。`Config` 构造与
//! mutation 只允许出现在本 crate。

use anyhow::Result;
use std::hash::{Hash, Hasher};
use std::num::NonZeroUsize;
use wasmtime::{Config, Engine, OptLevel, Strategy};

/// 编译器后端，与 runtime 的 `RuntimeCompiler` 语义对齐。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CompilerStrategy {
    Cranelift,
    Winch,
}

/// Cranelift 优化等级，与 `WJSM_OPT_LEVEL` 语义对齐。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum CraneliftOptLevel {
    #[default]
    Default,
    None,
    SpeedAndSize,
}

/// 运行时可变选项；canonical artifact profile 不暴露这些开关。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RuntimeEngineOptions {
    pub compiler: CompilerStrategy,
    pub opt_level: CraneliftOptLevel,
    pub epoch_interruption: bool,
    pub memory_reservation: Option<u64>,
    pub guest_debug: bool,
}

impl Default for RuntimeEngineOptions {
    fn default() -> Self {
        Self {
            compiler: CompilerStrategy::Cranelift,
            opt_level: CraneliftOptLevel::Default,
            epoch_interruption: true,
            memory_reservation: None,
            guest_debug: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum ProfileKind {
    /// support cwasm / 可行性测试使用的 canonical artifact。
    Artifact,
    Runtime(RuntimeEngineOptions),
}

/// 唯一 engine 配置 owner。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EngineConfig {
    kind: ProfileKind,
}

impl EngineConfig {
    /// Canonical artifact engine：固定 Cranelift + epoch interruption。
    pub fn artifact() -> Self {
        Self {
            kind: ProfileKind::Artifact,
        }
    }

    /// 运行时 engine：保留 compiler / opt / epoch / memory reservation / guest-debug。
    pub fn runtime(mut options: RuntimeEngineOptions) -> Self {
        // guest_debug 与 Winch 不兼容：强制 Cranelift。
        if options.guest_debug {
            options.compiler = CompilerStrategy::Cranelift;
        }
        Self {
            kind: ProfileKind::Runtime(options),
        }
    }

    /// 构造与本 profile 兼容的 `Engine`。
    pub fn build(self) -> Result<Engine> {
        self.validate()?;
        let config = self.into_config();
        Engine::new(&config).map_err(|e| anyhow::anyhow!("create Wasmtime engine: {e}"))
    }

    fn validate(self) -> Result<()> {
        #[cfg(target_arch = "aarch64")]
        if matches!(
            self.kind,
            ProfileKind::Runtime(RuntimeEngineOptions {
                compiler: CompilerStrategy::Winch,
                ..
            })
        ) {
            anyhow::bail!(
                "Winch on AArch64 cannot provide the required WebAssembly threads capability"
            );
        }
        Ok(())
    }

    fn into_config(self) -> Config {
        let mut config = Config::new();
        apply_fixed_features(&mut config);
        match self.kind {
            ProfileKind::Artifact => {
                config.strategy(Strategy::Cranelift);
                config.epoch_interruption(true);
            }
            ProfileKind::Runtime(options) => {
                apply_runtime_options(&mut config, options);
            }
        }
        config
    }
}

/// 基于 `Engine::precompile_compatibility_hash` 的稳定 u64 fingerprint。
pub fn compatibility_fingerprint(engine: &Engine) -> u64 {
    let mut hasher = StableHasher::new();
    engine.precompile_compatibility_hash().hash(&mut hasher);
    hasher.finish()
}


fn apply_fixed_features(config: &mut Config) {
    config.wasm_threads(true);
    config.shared_memory(true);
    config.wasm_memory64(true);
    config.wasm_multi_memory(true);
    config.wasm_bulk_memory(true);
    config.wasm_backtrace_max_frames(NonZeroUsize::new(50));
    config.generate_address_map(true);
}

fn apply_runtime_options(config: &mut Config, options: RuntimeEngineOptions) {
    if options.guest_debug {
        config.guest_debug(true);
    }
    match options.compiler {
        CompilerStrategy::Cranelift => config.strategy(Strategy::Cranelift),
        CompilerStrategy::Winch => config.strategy(Strategy::Winch),
    };
    match options.opt_level {
        CraneliftOptLevel::None => {
            config.cranelift_opt_level(OptLevel::None);
        }
        CraneliftOptLevel::SpeedAndSize => {
            config.cranelift_opt_level(OptLevel::SpeedAndSize);
        }
        CraneliftOptLevel::Default => {}
    }
    if options.epoch_interruption {
        config.epoch_interruption(true);
    }
    if let Some(bytes) = options.memory_reservation {
        config.memory_reservation(bytes);
        config.memory_reservation_for_growth(bytes.clamp(1 << 20, 64 << 20));
        config.memory_guard_size(64 << 10);
        config.guard_before_linear_memory(false);
    }
}

/// 固定种子的 FNV-1a 64-bit hasher；fingerprint 不得依赖进程随机 seed。
struct StableHasher {
    state: u64,
}

impl StableHasher {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x100_0000_01b3;

    fn new() -> Self {
        Self {
            state: Self::OFFSET,
        }
    }
}

impl Hasher for StableHasher {
    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.state ^= u64::from(byte);
            self.state = self.state.wrapping_mul(Self::PRIME);
        }
    }

    fn finish(&self) -> u64 {
        self.state
    }
}
