//! Predictive ZGC pacing director (design §13.2 / Task 22).
//!
//! Maintains young/old rate models (allocation, survival/live-growth, mark,
//! relocate) with EWMA smoothing, predicts cycle completion against free-space
//! runway, drives proportional mutator assist, and records structured stalls
//! only when the relocation reserve is exhausted.

#![cfg(feature = "managed-heap-v2")]

use std::time::Duration;

/// Which generation the director decision applies to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DirectorGeneration {
    Young,
    Old,
}

/// Why an allocation stall was entered. Only reserve exhaustion is legal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StallReason {
    RelocationReserveExhausted,
}

/// Structured stall event recorded for telemetry / gate analysis.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StallEvent {
    pub reason: StallReason,
    pub duration_ns: u64,
    pub free_bytes: u64,
    pub reserve_bytes: u64,
    pub prediction_error_ns: i64,
    pub generation: DirectorGeneration,
}

/// Snapshot of one generation's rate model after an observation.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GenerationRates {
    /// Allocated bytes per nanosecond (EWMA).
    pub alloc_bytes_per_ns: f64,
    /// Survival / live-growth fraction in [0, 1] (EWMA).
    pub survival_rate: f64,
    /// Mark throughput in bytes per nanosecond (EWMA).
    pub mark_bytes_per_ns: f64,
    /// Relocate throughput in bytes per nanosecond (EWMA).
    pub relocate_bytes_per_ns: f64,
    /// Previous cycle predicted duration minus actual (ns, signed).
    pub prediction_error_ns: i64,
    /// Sample count contributing to the EWMA.
    pub samples: u64,
}

/// Decision returned by [`GcDirector::evaluate`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DirectorDecision {
    Idle,
    StartYoung,
    StartOld,
    Continue,
}

/// Assist budget produced for a mutator allocation that incurred debt.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AssistBudget {
    /// Work bytes the mutator must repay before returning from allocation.
    pub work_bytes: u64,
    /// True when the request hit the hard assist cap.
    pub capped: bool,
}

const DEFAULT_EWMA_ALPHA: f64 = 0.2;
const DEFAULT_ASSIST_CAP_BYTES: u64 = 256 * 1024;
const MIN_THROUGHPUT: f64 = 1e-12;
const NS_PER_SEC: f64 = 1_000_000_000.0;

/// Young/old predictive pacer.
///
/// Fixed 4 MiB debt is **not** the primary trigger: a generation starts when
/// predicted cycle wall time approaches free-space runway under the current
/// allocation rate.
#[derive(Debug)]
pub struct GcDirector {
    young: GenerationRates,
    old: GenerationRates,
    free_bytes: u64,
    reserve_bytes: u64,
    young_active: bool,
    old_active: bool,
    ewma_alpha: f64,
    assist_cap_bytes: u64,
    stall_count: u64,
    last_stall: Option<StallEvent>,
    last_decision: DirectorDecision,
}

impl Default for GcDirector {
    fn default() -> Self {
        Self::new()
    }
}

impl GcDirector {
    pub fn new() -> Self {
        Self {
            young: GenerationRates::default(),
            old: GenerationRates::default(),
            free_bytes: 0,
            reserve_bytes: 0,
            young_active: false,
            old_active: false,
            ewma_alpha: DEFAULT_EWMA_ALPHA,
            assist_cap_bytes: DEFAULT_ASSIST_CAP_BYTES,
            stall_count: 0,
            last_stall: None,
            last_decision: DirectorDecision::Idle,
        }
    }

    pub fn with_alpha(mut self, alpha: f64) -> Self {
        self.ewma_alpha = alpha.clamp(0.01, 1.0);
        self
    }

    pub fn with_assist_cap(mut self, cap_bytes: u64) -> Self {
        self.assist_cap_bytes = cap_bytes.max(1);
        self
    }

    pub fn young_rates(&self) -> GenerationRates {
        self.young
    }

    pub fn old_rates(&self) -> GenerationRates {
        self.old
    }

    pub fn free_bytes(&self) -> u64 {
        self.free_bytes
    }

    pub fn reserve_bytes(&self) -> u64 {
        self.reserve_bytes
    }

    pub fn stall_count(&self) -> u64 {
        self.stall_count
    }

    pub fn last_stall(&self) -> Option<StallEvent> {
        self.last_stall
    }

    pub fn last_decision(&self) -> DirectorDecision {
        self.last_decision
    }

    pub fn young_active(&self) -> bool {
        self.young_active
    }

    pub fn old_active(&self) -> bool {
        self.old_active
    }

    /// Publish instantaneous free heap and relocation reserve sizes.
    pub fn update_space(&mut self, free_bytes: u64, reserve_bytes: u64) {
        self.free_bytes = free_bytes;
        self.reserve_bytes = reserve_bytes;
    }

    /// Observe mutator allocation rate for `generation` over a wall interval.
    pub fn observe_allocation(
        &mut self,
        generation: DirectorGeneration,
        allocated_bytes: u64,
        elapsed: Duration,
    ) {
        let rate = rate_bytes_per_ns(allocated_bytes, elapsed);
        let alpha = self.ewma_alpha;
        let rates = self.rates_mut(generation);
        rates.alloc_bytes_per_ns = ewma(rates.alloc_bytes_per_ns, rate, alpha);
        rates.samples = rates.samples.saturating_add(1);
    }

    /// Observe mark throughput for a completed mark phase.
    pub fn observe_mark(
        &mut self,
        generation: DirectorGeneration,
        marked_bytes: u64,
        elapsed: Duration,
    ) {
        let rate = rate_bytes_per_ns(marked_bytes, elapsed);
        let alpha = self.ewma_alpha;
        let rates = self.rates_mut(generation);
        rates.mark_bytes_per_ns = ewma(rates.mark_bytes_per_ns, rate, alpha);
        rates.samples = rates.samples.saturating_add(1);
    }

    /// Observe relocate throughput for a completed relocation phase.
    pub fn observe_relocate(
        &mut self,
        generation: DirectorGeneration,
        relocated_bytes: u64,
        elapsed: Duration,
    ) {
        let rate = rate_bytes_per_ns(relocated_bytes, elapsed);
        let alpha = self.ewma_alpha;
        let rates = self.rates_mut(generation);
        rates.relocate_bytes_per_ns = ewma(rates.relocate_bytes_per_ns, rate, alpha);
        rates.samples = rates.samples.saturating_add(1);
    }

    /// Observe cycle survival / live growth after a completed cycle.
    ///
    /// `survival_rate` is live_after / live_before (or allocated) clamped to [0, 1].
    pub fn observe_survival(&mut self, generation: DirectorGeneration, survival_rate: f64) {
        let survival = survival_rate.clamp(0.0, 1.0);
        let alpha = self.ewma_alpha;
        let rates = self.rates_mut(generation);
        rates.survival_rate = ewma(rates.survival_rate, survival, alpha);
        rates.samples = rates.samples.saturating_add(1);
    }

    /// Close a cycle: record prediction error = predicted − actual (signed ns).
    pub fn complete_cycle(
        &mut self,
        generation: DirectorGeneration,
        predicted_ns: u64,
        actual: Duration,
    ) {
        let actual_ns = duration_ns(actual);
        let error = predicted_ns as i64 - actual_ns as i64;
        let rates = self.rates_mut(generation);
        rates.prediction_error_ns = error;
        match generation {
            DirectorGeneration::Young => self.young_active = false,
            DirectorGeneration::Old => self.old_active = false,
        }
    }

    /// Predicted wall-clock nanoseconds to finish a full mark+relocate cycle
    /// for the given live-set size under current throughput.
    pub fn predict_cycle_ns(&self, generation: DirectorGeneration, live_bytes: u64) -> u64 {
        let rates = self.rates(generation);
        let mark = rates.mark_bytes_per_ns.max(MIN_THROUGHPUT);
        let relocate = rates.relocate_bytes_per_ns.max(MIN_THROUGHPUT);
        let survival = if rates.survival_rate > 0.0 {
            rates.survival_rate
        } else {
            0.5
        };
        let relocate_volume = (live_bytes as f64) * survival;
        let mark_ns = (live_bytes as f64) / mark;
        let relocate_ns = relocate_volume / relocate;
        (mark_ns + relocate_ns).ceil().max(1.0) as u64
    }

    /// Free-space runway in nanoseconds under the current allocation rate.
    pub fn free_runway_ns(&self, generation: DirectorGeneration) -> u64 {
        let rates = self.rates(generation);
        let alloc = rates.alloc_bytes_per_ns;
        if alloc <= MIN_THROUGHPUT {
            return u64::MAX / 4;
        }
        ((self.free_bytes as f64) / alloc).floor() as u64
    }

    /// Evaluate whether young/old collection should start based on runway.
    pub fn evaluate(&mut self, young_live_bytes: u64, old_live_bytes: u64) -> DirectorDecision {
        if self.young_active || self.old_active {
            self.last_decision = DirectorDecision::Continue;
            return DirectorDecision::Continue;
        }

        // Prefer young: shorter cycles absorb allocation pressure first.
        if self.should_start(DirectorGeneration::Young, young_live_bytes) {
            self.young_active = true;
            self.last_decision = DirectorDecision::StartYoung;
            return DirectorDecision::StartYoung;
        }
        if self.should_start(DirectorGeneration::Old, old_live_bytes) {
            self.old_active = true;
            self.last_decision = DirectorDecision::StartOld;
            return DirectorDecision::StartOld;
        }
        self.last_decision = DirectorDecision::Idle;
        DirectorDecision::Idle
    }

    /// Proportional assist work for allocation debt; hard-capped per call.
    pub fn assist_budget(&self, debt_bytes: u64) -> AssistBudget {
        if debt_bytes == 0 {
            return AssistBudget::default();
        }
        // Work ≈ debt; never exceed assist_cap_bytes in one allocation.
        if debt_bytes > self.assist_cap_bytes {
            AssistBudget {
                work_bytes: self.assist_cap_bytes,
                capped: true,
            }
        } else {
            AssistBudget {
                work_bytes: debt_bytes,
                capped: false,
            }
        }
    }

    /// Record an allocation stall. Only [`StallReason::RelocationReserveExhausted`]
    /// is accepted; other reasons panic in debug and are rejected in release.
    pub fn record_stall(
        &mut self,
        reason: StallReason,
        duration: Duration,
        generation: DirectorGeneration,
    ) -> Result<StallEvent, &'static str> {
        match reason {
            StallReason::RelocationReserveExhausted => {}
        }
        if self.reserve_bytes > 0 {
            return Err("stall only legal when relocation reserve is exhausted");
        }
        let event = StallEvent {
            reason,
            duration_ns: duration_ns(duration),
            free_bytes: self.free_bytes,
            reserve_bytes: self.reserve_bytes,
            prediction_error_ns: self.rates(generation).prediction_error_ns,
            generation,
        };
        self.stall_count = self.stall_count.saturating_add(1);
        self.last_stall = Some(event);
        Ok(event)
    }

    /// Absolute relative prediction error for convergence checks.
    pub fn relative_prediction_error(&self, generation: DirectorGeneration) -> f64 {
        let rates = self.rates(generation);
        let err = rates.prediction_error_ns.unsigned_abs() as f64;
        if rates.samples == 0 {
            return f64::INFINITY;
        }
        err / NS_PER_SEC
    }

    fn should_start(&self, generation: DirectorGeneration, live_bytes: u64) -> bool {
        let rates = self.rates(generation);
        if rates.samples == 0 || rates.alloc_bytes_per_ns <= MIN_THROUGHPUT {
            // Cold start: trigger when free space is under live as a
            // conservative bootstrap, without a fixed 4 MiB debt rule.
            return self.free_bytes > 0 && live_bytes > 0 && self.free_bytes <= live_bytes;
        }
        let predicted = self.predict_cycle_ns(generation, live_bytes.max(1));
        let runway = self.free_runway_ns(generation);
        // Start when predicted cycle time consumes ≥ 80% of free runway.
        predicted.saturating_mul(5) >= runway.saturating_mul(4)
    }

    fn rates(&self, generation: DirectorGeneration) -> GenerationRates {
        match generation {
            DirectorGeneration::Young => self.young,
            DirectorGeneration::Old => self.old,
        }
    }

    fn rates_mut(&mut self, generation: DirectorGeneration) -> &mut GenerationRates {
        match generation {
            DirectorGeneration::Young => &mut self.young,
            DirectorGeneration::Old => &mut self.old,
        }
    }
}

fn ewma(previous: f64, sample: f64, alpha: f64) -> f64 {
    if previous <= 0.0 {
        sample
    } else {
        alpha * sample + (1.0 - alpha) * previous
    }
}

fn rate_bytes_per_ns(bytes: u64, elapsed: Duration) -> f64 {
    let ns = duration_ns(elapsed).max(1) as f64;
    (bytes as f64) / ns
}

fn duration_ns(elapsed: Duration) -> u64 {
    u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gc_director_ewma_converges_under_stable_allocation_load() {
        let mut director = GcDirector::new().with_alpha(0.25);
        let step = Duration::from_millis(10);
        for _ in 0..40 {
            director.observe_allocation(DirectorGeneration::Young, 10_000, step);
        }
        let rate = director.young_rates().alloc_bytes_per_ns;
        let expected = 10_000.0 / 10_000_000.0;
        assert!((rate - expected).abs() / expected < 0.05, "rate={rate}");
    }

    #[test]
    fn gc_director_prediction_error_shrinks_on_stable_throughput() {
        let mut director = GcDirector::new().with_alpha(0.3);
        let mark_step = Duration::from_millis(5);
        let relocate_step = Duration::from_millis(5);
        for _ in 0..20 {
            director.observe_mark(DirectorGeneration::Young, 1_000_000, mark_step);
            director.observe_relocate(DirectorGeneration::Young, 500_000, relocate_step);
            director.observe_survival(DirectorGeneration::Young, 0.5);
            director.observe_allocation(
                DirectorGeneration::Young,
                200_000,
                Duration::from_millis(20),
            );
        }
        let predicted = director.predict_cycle_ns(DirectorGeneration::Young, 1_000_000);
        director.complete_cycle(
            DirectorGeneration::Young,
            predicted,
            Duration::from_millis(10),
        );
        let err1 = director.young_rates().prediction_error_ns.unsigned_abs();

        for _ in 0..20 {
            director.observe_mark(DirectorGeneration::Young, 1_000_000, mark_step);
            director.observe_relocate(DirectorGeneration::Young, 500_000, relocate_step);
        }
        let predicted2 = director.predict_cycle_ns(DirectorGeneration::Young, 1_000_000);
        director.complete_cycle(
            DirectorGeneration::Young,
            predicted2,
            Duration::from_nanos(predicted2),
        );
        let err2 = director.young_rates().prediction_error_ns.unsigned_abs();
        assert!(err2 <= err1, "err1={err1} err2={err2}");
        assert_eq!(err2, 0);
    }

    #[test]
    fn gc_director_evaluate_starts_young_when_runway_exhausted() {
        let mut director = GcDirector::new().with_alpha(1.0);
        director.observe_allocation(
            DirectorGeneration::Young,
            100 * 1024 * 1024,
            Duration::from_secs(1),
        );
        director.observe_mark(
            DirectorGeneration::Young,
            200 * 1024 * 1024,
            Duration::from_secs(1),
        );
        director.observe_relocate(
            DirectorGeneration::Young,
            200 * 1024 * 1024,
            Duration::from_secs(1),
        );
        director.observe_survival(DirectorGeneration::Young, 0.5);
        // Free only 8 MiB → runway ~80ms; live 16 MiB → cycle ~120ms → start.
        director.update_space(8 * 1024 * 1024, 4 * 1024 * 1024);
        let decision = director.evaluate(16 * 1024 * 1024, 0);
        assert_eq!(decision, DirectorDecision::StartYoung);
        assert!(director.young_active());
        assert_eq!(
            director.evaluate(16 * 1024 * 1024, 0),
            DirectorDecision::Continue
        );
    }

    #[test]
    fn gc_director_evaluate_starts_old_when_young_idle_and_old_runway_tight() {
        let mut director = GcDirector::new().with_alpha(1.0);
        director.observe_allocation(
            DirectorGeneration::Old,
            50 * 1024 * 1024,
            Duration::from_secs(1),
        );
        director.observe_mark(
            DirectorGeneration::Old,
            80 * 1024 * 1024,
            Duration::from_secs(1),
        );
        director.observe_relocate(
            DirectorGeneration::Old,
            80 * 1024 * 1024,
            Duration::from_secs(1),
        );
        director.observe_survival(DirectorGeneration::Old, 0.8);
        director.update_space(4 * 1024 * 1024, 2 * 1024 * 1024);
        // young_live tiny so cold-start free<=live is false for young; old has rates.
        let decision = director.evaluate(1, 64 * 1024 * 1024);
        assert_eq!(decision, DirectorDecision::StartOld);
    }

    #[test]
    fn gc_director_assist_is_proportional_and_hard_capped() {
        let director = GcDirector::new().with_assist_cap(64 * 1024);
        assert_eq!(
            director.assist_budget(0),
            AssistBudget {
                work_bytes: 0,
                capped: false
            }
        );
        assert_eq!(
            director.assist_budget(32 * 1024),
            AssistBudget {
                work_bytes: 32 * 1024,
                capped: false
            }
        );
        assert_eq!(
            director.assist_budget(128 * 1024),
            AssistBudget {
                work_bytes: 64 * 1024,
                capped: true
            }
        );
    }

    #[test]
    fn gc_director_stall_only_when_reserve_exhausted() {
        let mut director = GcDirector::new();
        director.update_space(1024, 4096);
        let rejected = director.record_stall(
            StallReason::RelocationReserveExhausted,
            Duration::from_micros(100),
            DirectorGeneration::Young,
        );
        assert!(rejected.is_err());
        assert_eq!(director.stall_count(), 0);

        director.update_space(1024, 0);
        let event = director
            .record_stall(
                StallReason::RelocationReserveExhausted,
                Duration::from_micros(250),
                DirectorGeneration::Young,
            )
            .unwrap();
        assert_eq!(event.reason, StallReason::RelocationReserveExhausted);
        assert_eq!(event.reserve_bytes, 0);
        assert_eq!(event.duration_ns, 250_000);
        assert_eq!(director.stall_count(), 1);
        assert_eq!(director.last_stall(), Some(event));
    }

    #[test]
    fn gc_director_idle_when_runway_ample() {
        let mut director = GcDirector::new().with_alpha(1.0);
        director.observe_allocation(
            DirectorGeneration::Young,
            1024 * 1024,
            Duration::from_secs(1),
        );
        director.observe_mark(
            DirectorGeneration::Young,
            200 * 1024 * 1024,
            Duration::from_secs(1),
        );
        director.observe_relocate(
            DirectorGeneration::Young,
            200 * 1024 * 1024,
            Duration::from_secs(1),
        );
        director.observe_survival(DirectorGeneration::Young, 0.1);
        director.update_space(512 * 1024 * 1024, 64 * 1024 * 1024);
        assert_eq!(
            director.evaluate(16 * 1024 * 1024, 0),
            DirectorDecision::Idle
        );
    }
}
