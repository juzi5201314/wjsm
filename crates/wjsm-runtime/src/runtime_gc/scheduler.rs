//! GC safepoint 调度器（spec §12/T0.5）。
//!
//! 当前阶段只接管单步预算与完整周期后的 heap-goal 触发目标；P1 再把
//! `trigger_bytes` 暴露给新的 WASM globals。

use std::time::Duration;

use super::zgc::director::{AssistBudget, DirectorDecision, GcDirector};

const DEFAULT_PAUSE_TARGET: Duration = Duration::from_millis(4);
const DEFAULT_GC_PERCENT: usize = 100;
const DEFAULT_TRIGGER_BYTES: usize = 256 * 1024;
const MIN_STEP_WORK_BYTES: usize = 64 * 1024;

/// 根据 pause target 和 heap-goal 估算驱动增量 GC 的轻量调度器。
pub struct GcScheduler {
    /// 单次 safepoint 期望停顿上限。
    #[allow(dead_code)]
    pub pause_target: Duration,
    /// Go-style heap goal 百分比；100 表示目标约为 live 的 2 倍。
    pub gc_percent: usize,
    /// 下一轮触发阈值（字节）。T0.5 先由 host 维护，P1 暴露给 WASM。
    pub trigger_bytes: usize,
    /// 单次 safepoint 的基础工作量，按 pause feedback 在范围内自适应。
    #[allow(dead_code)]
    step_work_bytes: usize,
    /// mutator 分配跑赢 GC 时累计的待偿还工作量；T0.5 仅纳入预算上限。
    alloc_debt_bytes: usize,
    /// Task 22 predictive young/old director (feature-gated).
    #[allow(dead_code)]
    director: GcDirector,
}

impl Default for GcScheduler {
    fn default() -> Self {
        Self {
            pause_target: DEFAULT_PAUSE_TARGET,
            gc_percent: DEFAULT_GC_PERCENT,
            trigger_bytes: DEFAULT_TRIGGER_BYTES,
            step_work_bytes: MIN_STEP_WORK_BYTES,
            alloc_debt_bytes: 0,
            director: GcDirector::new(),
        }
    }
}

impl GcScheduler {
    /// 完整周期结束后更新下一轮触发阈值。
    pub fn after_cycle(
        &mut self,
        live_bytes: usize,
        root_scan_bytes_estimate: usize,
        heap_limit: usize,
    ) {
        let growth_bytes = live_bytes.saturating_mul(self.gc_percent) / 100;
        let heap_goal = live_bytes
            .saturating_add(growth_bytes.max(root_scan_bytes_estimate))
            .max(1);
        self.trigger_bytes = heap_goal.min(heap_limit.max(1)).max(1);
    }

    /// mutator 分配产生 debt 时累计；director assist 使用同一计数。
    #[allow(dead_code)]
    pub fn add_alloc_debt(&mut self, bytes: usize) {
        self.alloc_debt_bytes = self.alloc_debt_bytes.saturating_add(bytes);
    }

    #[allow(dead_code)]
    pub fn alloc_debt_bytes(&self) -> usize {
        self.alloc_debt_bytes
    }

    #[allow(dead_code)]
    pub fn repay_alloc_debt(&mut self, bytes: usize) {
        self.alloc_debt_bytes = self.alloc_debt_bytes.saturating_sub(bytes);
    }

    #[allow(dead_code)]
    pub fn director(&self) -> &GcDirector {
        &self.director
    }

    #[allow(dead_code)]
    pub fn director_mut(&mut self) -> &mut GcDirector {
        &mut self.director
    }

    /// 结合 director runway 评估是否应启动 young/old cycle。
    #[allow(dead_code)]
    pub fn evaluate_director(
        &mut self,
        free_bytes: u64,
        reserve_bytes: u64,
        young_live_bytes: u64,
        old_live_bytes: u64,
    ) -> DirectorDecision {
        self.director.update_space(free_bytes, reserve_bytes);
        self.director.evaluate(young_live_bytes, old_live_bytes)
    }

    /// 按 debt 比例给出 assist 预算，并硬上限截断。
    #[allow(dead_code)]
    pub fn assist_for_debt(&self) -> AssistBudget {
        self.director
            .assist_budget(u64::try_from(self.alloc_debt_bytes).unwrap_or(u64::MAX))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn after_cycle_keeps_trigger_non_zero_and_heap_limited() {
        let mut scheduler = GcScheduler::default();

        scheduler.after_cycle(256 * 1024, 64 * 1024, 512 * 1024);
        assert_eq!(scheduler.trigger_bytes, 512 * 1024);

        scheduler.after_cycle(0, 0, 512 * 1024);
        assert!(scheduler.trigger_bytes > 0);
        assert!(scheduler.trigger_bytes <= 512 * 1024);

        scheduler.after_cycle(usize::MAX, usize::MAX, 128 * 1024);
        assert_eq!(scheduler.trigger_bytes, 128 * 1024);
    }
}
