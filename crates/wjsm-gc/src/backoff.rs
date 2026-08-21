//! 协作式退避：GC 周期推进方的等待原语。
//!
//! [`Backoff`] 用于"等 GC worker 干活"的循环（`collect_full`、宿主
//! `drain_gc_cycle`）。裸 `std::thread::yield_now()` 在 CPU 超订（nextest
//! 每核一进程 + rayon 全局池）时会让 worker 线程饿死——mutator 以 yield
//! 风暴占满时间片，worker 抢不到 CPU，周期推进被无限拉长，测试触发 30s
//! 超时。这里改为指数退避：先短暂自旋（低延迟路径），随后升级为
//! `thread::sleep` 让出整颗核心，保证 worker 必然被调度。
//!
//! 上限 1ms：GC packet 处理是毫秒级操作，1ms 轮询间隔对 mutator 吞吐的
//! 影响可忽略，同时把最坏情况下的空转功耗压到近零。

use std::hint;
use std::thread;
use std::time::Duration;

/// 单次退避等待的最大睡眠时长。
const BACKOFF_MAX_SLEEP: Duration = Duration::from_millis(1);

/// 自旋阶段的重试次数；超过后进入睡眠阶段。
const SPIN_RETRIES: u32 = 16;

/// 协作式退避状态机：`spin_left` 用尽后每次睡眠时长翻倍直至上限。
#[derive(Clone, Copy, Debug)]
pub struct Backoff {
    spin_left: u32,
    sleep: Duration,
}

impl Backoff {
    pub fn new() -> Self {
        Self {
            spin_left: SPIN_RETRIES,
            sleep: Duration::from_nanos(200),
        }
    }

    /// 执行一轮等待：先 `spin_loop` 提示 CPU 进入低功耗暂停态，
    /// 自旋预算耗尽后指数升级为真正的线程睡眠。
    pub fn wait(&mut self) {
        if self.spin_left > 0 {
            self.spin_left -= 1;
            hint::spin_loop();
            return;
        }
        thread::sleep(self.sleep);
        self.sleep = (self.sleep * 2).min(BACKOFF_MAX_SLEEP);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_sleep_saturates_at_limit() {
        let mut backoff = Backoff::new();
        for _ in 0..SPIN_RETRIES {
            backoff.wait();
        }
        // 自旋耗尽后进入睡眠阶段，多次翻倍必须收敛到上限。
        for _ in 0..40 {
            backoff.wait();
        }
        assert_eq!(backoff.sleep, BACKOFF_MAX_SLEEP);
    }
}
