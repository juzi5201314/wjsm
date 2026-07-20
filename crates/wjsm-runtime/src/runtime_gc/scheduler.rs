//! GC safepoint 调度器（spec §12/T0.5）。
//!
//! 当前阶段只接管单步预算与完整周期后的 heap-goal 触发目标；P1 再把
//! `trigger_bytes` 暴露给新的 WASM globals。

use std::time::{Duration, Instant};

use super::api::{StepBudget, StepOutcome};
#[cfg(feature = "managed-heap-v2")]
use super::zgc::director::{AssistBudget, DirectorDecision, GcDirector};


const DEFAULT_PAUSE_TARGET: Duration = Duration::from_millis(4);
const DEFAULT_GC_PERCENT: usize = 100;
const DEFAULT_TRIGGER_BYTES: usize = 256 * 1024;
const MIN_STEP_WORK_BYTES: usize = 64 * 1024;
const MAX_STEP_WORK_BYTES: usize = 8 * 1024 * 1024;

/// 根据 pause target 和 heap-goal 估算驱动增量 GC 的轻量调度器。
pub struct GcScheduler {
    /// 单次 safepoint 期望停顿上限。
    pub pause_target: Duration,
    /// Go-style heap goal 百分比；100 表示目标约为 live 的 2 倍。
    pub gc_percent: usize,
    /// 下一轮触发阈值（字节）。T0.5 先由 host 维护，P1 暴露给 WASM。
    pub trigger_bytes: usize,
    /// 单次 safepoint 的基础工作量，按 pause feedback 在范围内自适应。
    step_work_bytes: usize,
    /// mutator 分配跑赢 GC 时累计的待偿还工作量；T0.5 仅纳入预算上限。
    alloc_debt_bytes: usize,
    /// Task 22 predictive young/old director (feature-gated).
    #[cfg(feature = "managed-heap-v2")]
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
            #[cfg(feature = "managed-heap-v2")]
            director: GcDirector::new(),
        }
    }
}

impl GcScheduler {
    /// 为下一次 safepoint 构造工作量与 wall-clock deadline。
    pub fn budget(&self) -> StepBudget {
        StepBudget {
            work_bytes: self
                .step_work_bytes
                .saturating_add(self.alloc_debt_bytes)
                .clamp(MIN_STEP_WORK_BYTES, MAX_STEP_WORK_BYTES),
            deadline: Instant::now() + self.pause_target,
        }
    }

    /// 根据刚完成的 safepoint 反馈调整下一步工作量。
    pub fn after_step(&mut self, outcome: &StepOutcome, elapsed: Duration) {
        let current = self
            .step_work_bytes
            .clamp(MIN_STEP_WORK_BYTES, MAX_STEP_WORK_BYTES);
        self.step_work_bytes = current;

        if elapsed > self.pause_target {
            self.step_work_bytes = (current / 2).max(MIN_STEP_WORK_BYTES);
            return;
        }

        if elapsed < self.pause_target
            && matches!(
                outcome,
                StepOutcome::Progress { .. } | StepOutcome::CycleComplete
            )
        {
            self.step_work_bytes = current
                .saturating_mul(2)
                .clamp(MIN_STEP_WORK_BYTES, MAX_STEP_WORK_BYTES);
        }
    }

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

    #[cfg(feature = "managed-heap-v2")]
    #[allow(dead_code)]
    pub fn director(&self) -> &GcDirector {
        &self.director
    }

    #[cfg(feature = "managed-heap-v2")]
    #[allow(dead_code)]
    pub fn director_mut(&mut self) -> &mut GcDirector {
        &mut self.director
    }

    /// 结合 director runway 评估是否应启动 young/old cycle。
    #[cfg(feature = "managed-heap-v2")]
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
    #[cfg(feature = "managed-heap-v2")]
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
    fn budget_uses_defaults_and_deadline() {
        let scheduler = GcScheduler::default();

        assert_eq!(scheduler.pause_target, Duration::from_millis(4));
        assert_eq!(scheduler.gc_percent, 100);
        assert_eq!(scheduler.trigger_bytes, 256 * 1024);
        assert_eq!(scheduler.step_work_bytes, MIN_STEP_WORK_BYTES);

        let before = Instant::now();
        let budget = scheduler.budget();
        assert_eq!(budget.work_bytes, MIN_STEP_WORK_BYTES);
        let until_deadline = budget.deadline.duration_since(before);
        assert!(until_deadline >= scheduler.pause_target);
        assert!(until_deadline < scheduler.pause_target + Duration::from_secs(1));
    }

    #[test]
    fn pause_target_feedback_converges_with_clamps() {
        let mut scheduler = GcScheduler {
            step_work_bytes: 4 * 1024 * 1024,
            ..GcScheduler::default()
        };

        for _ in 0..8 {
            scheduler.after_step(
                &StepOutcome::Progress {
                    remaining_estimate: 1,
                },
                Duration::from_millis(8),
            );
        }
        assert_eq!(scheduler.step_work_bytes, MIN_STEP_WORK_BYTES);

        for _ in 0..8 {
            scheduler.after_step(
                &StepOutcome::Progress {
                    remaining_estimate: 1,
                },
                Duration::from_millis(1),
            );
        }
        assert_eq!(scheduler.step_work_bytes, MAX_STEP_WORK_BYTES);

        scheduler.after_step(&StepOutcome::Idle, Duration::from_millis(1));
        assert_eq!(scheduler.step_work_bytes, MAX_STEP_WORK_BYTES);

        scheduler.after_step(&StepOutcome::CycleComplete, Duration::from_millis(8));
        assert_eq!(scheduler.step_work_bytes, 4 * 1024 * 1024);
    }

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
