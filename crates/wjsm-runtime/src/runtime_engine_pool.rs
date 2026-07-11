//! 进程内 Wasmtime Engine 池与共享 EpochController。
//!
//! 同一 `EngineConfigKey` 复用同一个 `Engine` + 一个 lazy ticker；
//! Store / Linker / RuntimeState 仍每次新建。epoch ticker 仅在有
//! 活跃 `vm` timeout（armed > 0）时每 1ms `increment_epoch()`。

use crate::RuntimeCompiler;
use anyhow::Result;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, LazyLock, Mutex};
use std::time::Duration;
use wasmtime::{Config, Engine, OptLevel, Strategy, UpdateDeadline};

/// Engine 池键：决定 wasmtime Config 的全部可区分维度。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct EngineConfigKey {
    pub compiler: RuntimeCompiler,
    pub opt_level: OptLevelKey,
    pub use_epoch_async_yield: bool,
    pub memory_reservation: Option<u64>,
    pub guest_debug: bool,
}

/// 与 `WJSM_OPT_LEVEL` 对齐的可哈希优化等级。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum OptLevelKey {
    Default,
    None,
    SpeedAndSize,
}

impl OptLevelKey {
    pub(crate) fn from_env() -> Self {
        match std::env::var("WJSM_OPT_LEVEL").as_deref() {
            Ok("none") => Self::None,
            Ok("speed_and_size") => Self::SpeedAndSize,
            _ => Self::Default,
        }
    }
}

/// 每个 Engine 一份：armed 引用计数 + lazy ticker 线程。
pub(crate) struct EpochController {
    engine: Engine,
    armed: AtomicUsize,
    state: Mutex<EpochTickerState>,
    cv: Condvar,
}

struct EpochTickerState {
    ticker_started: bool,
}

/// 池中取出的 Engine 句柄；`engine` 可 clone，`epoch` 共享。
#[derive(Clone)]
pub(crate) struct PooledEngine {
    pub engine: Engine,
    pub epoch: Arc<EpochController>,
}

static ENGINE_POOL: LazyLock<Mutex<HashMap<EngineConfigKey, PooledEngine>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// 解析编译器：显式 `RuntimeOptions.compiler` 优先，否则读 `WJSM_COMPILER`，默认 Cranelift。
pub(crate) fn resolve_compiler(explicit: Option<RuntimeCompiler>) -> RuntimeCompiler {
    if let Some(compiler) = explicit {
        return compiler;
    }
    match std::env::var("WJSM_COMPILER").as_deref() {
        Ok("winch") | Ok("Winch") | Ok("WINCH") => RuntimeCompiler::Winch,
        _ => RuntimeCompiler::Cranelift,
    }
}

/// 从 options / 环境 / guest_debug 构造池键。
pub(crate) fn engine_config_key(
    compiler: Option<RuntimeCompiler>,
    use_epoch_async_yield: bool,
    memory_reservation: Option<u64>,
    guest_debug: bool,
) -> EngineConfigKey {
    // guest_debug 与 Winch 不兼容：强制 Cranelift。
    let compiler = if guest_debug {
        RuntimeCompiler::Cranelift
    } else {
        resolve_compiler(compiler)
    };
    EngineConfigKey {
        compiler,
        opt_level: OptLevelKey::from_env(),
        use_epoch_async_yield,
        memory_reservation,
        guest_debug,
    }
}

/// 获取（或首次创建）与 key 匹配的 Engine。
pub(crate) fn acquire_engine(key: EngineConfigKey) -> Result<PooledEngine> {
    let mut pool = ENGINE_POOL
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(existing) = pool.get(&key) {
        return Ok(existing.clone());
    }
    let config = build_engine_config(&key);
    let engine = Engine::new(&config)
        .map_err(|e| anyhow::anyhow!("Failed to create async engine: {:?}", e))?;
    let epoch = EpochController::new(engine.clone());
    let pooled = PooledEngine {
        engine,
        epoch,
    };
    pool.insert(key, pooled.clone());
    Ok(pooled)
}

/// 冷路径 / 启动快照构建：不经池，直接 `Engine::new`。
#[allow(dead_code)]
pub(crate) fn create_cold_engine(key: EngineConfigKey) -> Result<Engine> {
    let config = build_engine_config(&key);
    Engine::new(&config).map_err(|e| anyhow::anyhow!("Failed to create async engine: {:?}", e))
}

pub(crate) fn build_engine_config(key: &EngineConfigKey) -> Config {
    let mut config = Config::new();
    if key.guest_debug {
        config.guest_debug(true);
    } else if key.compiler == RuntimeCompiler::Winch {
        config.strategy(Strategy::Winch);
    }
    match key.opt_level {
        OptLevelKey::None => {
            config.cranelift_opt_level(OptLevel::None);
        }
        OptLevelKey::SpeedAndSize => {
            config.cranelift_opt_level(OptLevel::SpeedAndSize);
        }
        OptLevelKey::Default => {}
    }
    if key.use_epoch_async_yield {
        config.epoch_interruption(true);
    }
    if let Some(bytes) = key.memory_reservation {
        config.memory_reservation(bytes);
        config.memory_reservation_for_growth(bytes.clamp(1 << 20, 64 << 20));
        config.memory_guard_size(64 << 10);
        config.guard_before_linear_memory(false);
    }
    config.wasm_backtrace_max_frames(std::num::NonZero::new(50));
    config.generate_address_map(true);
    config.wasm_bulk_memory(true);
    config
}

impl EpochController {
    fn new(engine: Engine) -> Arc<Self> {
        Arc::new(Self {
            engine,
            armed: AtomicUsize::new(0),
            state: Mutex::new(EpochTickerState {
                ticker_started: false,
            }),
            cv: Condvar::new(),
        })
    }

    /// None → Some 时调用：armed++，必要时唤醒 ticker。
    pub fn arm(self: &Arc<Self>) {
        let prev = self.armed.fetch_add(1, Ordering::SeqCst);
        if prev == 0 {
            self.ensure_ticker();
            self.cv.notify_one();
        }
    }

    /// Some → None 时调用：armed--；到 0 时 ticker 自行阻塞。
    pub fn disarm(&self) {
        let prev = self.armed.fetch_sub(1, Ordering::SeqCst);
        debug_assert!(
            prev > 0,
            "EpochController::disarm with armed==0"
        );
    }

    fn ensure_ticker(self: &Arc<Self>) {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if st.ticker_started {
            return;
        }
        st.ticker_started = true;
        let this = Arc::clone(self);
        std::thread::Builder::new()
            .name("wjsm-epoch-ticker".into())
            .spawn(move || this.ticker_loop())
            .expect("spawn epoch ticker");
    }

    fn ticker_loop(&self) {
        loop {
            {
                let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
                while self.armed.load(Ordering::SeqCst) == 0 {
                    st = self
                        .cv
                        .wait(st)
                        .unwrap_or_else(|e| e.into_inner());
                }
            }
            while self.armed.load(Ordering::SeqCst) > 0 {
                self.engine.increment_epoch();
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }
}

/// 安装 per-Store epoch 回调：deadline 到期 Interrupt，否则 Yield(1)。
pub(crate) fn install_epoch_deadline_callback(store: &mut wasmtime::Store<crate::RuntimeState>) {
    store.set_epoch_deadline(1);
    store.epoch_deadline_callback(|store_ctx| {
        let expired = store_ctx
            .data()
            .vm_deadline
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some_and(|d| std::time::Instant::now() >= d);
        if expired {
            Ok(UpdateDeadline::Interrupt)
        } else {
            Ok(UpdateDeadline::Yield(1))
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RuntimeCompiler;

    #[test]
    fn engine_pool_same_key_shares_engine_identity() {
        let key = EngineConfigKey {
            compiler: RuntimeCompiler::Winch,
            opt_level: OptLevelKey::Default,
            use_epoch_async_yield: true,
            memory_reservation: None,
            guest_debug: false,
        };
        let a = acquire_engine(key).expect("acquire a");
        let b = acquire_engine(key).expect("acquire b");
        assert!(
            Engine::same(&a.engine, &b.engine),
            "same key must return same Engine identity"
        );
        assert!(Arc::ptr_eq(&a.epoch, &b.epoch));
    }

    #[test]
    fn engine_pool_compiler_key_isolation() {
        let winch = EngineConfigKey {
            compiler: RuntimeCompiler::Winch,
            opt_level: OptLevelKey::Default,
            use_epoch_async_yield: true,
            memory_reservation: None,
            guest_debug: false,
        };
        let cranelift = EngineConfigKey {
            compiler: RuntimeCompiler::Cranelift,
            opt_level: OptLevelKey::Default,
            use_epoch_async_yield: true,
            memory_reservation: None,
            guest_debug: false,
        };
        let a = acquire_engine(winch).expect("winch");
        let b = acquire_engine(cranelift).expect("cranelift");
        assert!(
            !Engine::same(&a.engine, &b.engine),
            "different compilers must not share Engine"
        );
    }

    #[test]
    fn engine_pool_guest_debug_key_isolation() {
        let normal = EngineConfigKey {
            compiler: RuntimeCompiler::Cranelift,
            opt_level: OptLevelKey::Default,
            use_epoch_async_yield: true,
            memory_reservation: None,
            guest_debug: false,
        };
        let debug = EngineConfigKey {
            compiler: RuntimeCompiler::Cranelift,
            opt_level: OptLevelKey::Default,
            use_epoch_async_yield: true,
            memory_reservation: None,
            guest_debug: true,
        };
        let a = acquire_engine(normal).expect("normal");
        let b = acquire_engine(debug).expect("debug");
        assert!(!Engine::same(&a.engine, &b.engine));
    }

    #[test]
    fn engine_pool_memory_reservation_key_isolation() {
        let none = EngineConfigKey {
            compiler: RuntimeCompiler::Cranelift,
            opt_level: OptLevelKey::Default,
            use_epoch_async_yield: true,
            memory_reservation: None,
            guest_debug: false,
        };
        let reserved = EngineConfigKey {
            compiler: RuntimeCompiler::Cranelift,
            opt_level: OptLevelKey::Default,
            use_epoch_async_yield: true,
            memory_reservation: Some(1 << 20),
            guest_debug: false,
        };
        let a = acquire_engine(none).expect("none");
        let b = acquire_engine(reserved).expect("reserved");
        assert!(!Engine::same(&a.engine, &b.engine));
    }

    #[test]
    fn resolve_compiler_explicit_overrides_env() {
        // 显式值优先；不依赖环境。
        assert_eq!(
            resolve_compiler(Some(RuntimeCompiler::Winch)),
            RuntimeCompiler::Winch
        );
        assert_eq!(
            resolve_compiler(Some(RuntimeCompiler::Cranelift)),
            RuntimeCompiler::Cranelift
        );
    }

    #[test]
    fn engine_config_key_guest_debug_forces_cranelift() {
        let key = engine_config_key(Some(RuntimeCompiler::Winch), true, None, true);
        assert_eq!(key.compiler, RuntimeCompiler::Cranelift);
        assert!(key.guest_debug);
    }
}
