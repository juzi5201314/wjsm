//! `node:perf_hooks` 的集群级 HDR histogram backing。

use hdrhistogram::Histogram;
use std::fmt;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::Instant;

const EMPTY_MIN: u64 = i64::MAX as u64;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct HistogramStats {
    pub(crate) count: u64,
    pub(crate) min: u64,
    pub(crate) max: u64,
    pub(crate) mean: f64,
    pub(crate) stddev: f64,
    pub(crate) exceeds: u64,
}

struct RegistryMetrics {
    active: AtomicUsize,
}

struct HistogramBacking {
    histogram: Histogram<u64>,
    count: u64,
    exceeds: u64,
    previous: Option<Instant>,
    registry_metrics: Weak<RegistryMetrics>,
}

impl Drop for HistogramBacking {
    fn drop(&mut self) {
        if let Some(metrics) = self.registry_metrics.upgrade() {
            metrics.active.fetch_sub(1, Ordering::Relaxed);
        }
    }
}

/// 不可猜测的 Histogram backing capability；跨 Worker clone 直接共享该 Arc。
#[derive(Clone)]
pub struct HistogramCapability {
    backing: Arc<Mutex<HistogramBacking>>,
}

impl fmt::Debug for HistogramCapability {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HistogramCapability")
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug)]
pub(crate) struct HistogramWrapperEntry {
    pub(crate) capability: HistogramCapability,
    /// 0=只读 Histogram，1=Recordable，2=ELD interval。
    pub(crate) kind: u8,
}

/// 集群级 Histogram backing 工厂；registry 不持有 backing 的强引用。
pub(crate) struct PerfHooksHistogramRegistry {
    metrics: Arc<RegistryMetrics>,
}

impl PerfHooksHistogramRegistry {
    pub(crate) fn new() -> Self {
        Self {
            metrics: Arc::new(RegistryMetrics {
                active: AtomicUsize::new(0),
            }),
        }
    }

    pub(crate) fn create(
        &self,
        lowest: u64,
        highest: u64,
        figures: u8,
    ) -> Result<HistogramCapability, String> {
        if !(1..=5).contains(&figures) {
            return Err("histogram significant figures must be between 1 and 5".to_string());
        }
        let histogram = Histogram::new_with_bounds(lowest, highest, figures)
            .map_err(|error| format!("invalid histogram configuration: {error}"))?;
        self.metrics.active.fetch_add(1, Ordering::Relaxed);
        Ok(HistogramCapability {
            backing: Arc::new(Mutex::new(HistogramBacking {
                histogram,
                count: 0,
                exceeds: 0,
                previous: None,
                registry_metrics: Arc::downgrade(&self.metrics),
            })),
        })
    }

    pub(crate) fn record(
        &self,
        capability: &HistogramCapability,
        value: u64,
    ) -> Result<(), String> {
        let mut backing = Self::lock_backing(capability)?;
        Self::record_value(&mut backing, value)
    }

    /// 第一次调用仅建立基线；之后记录相邻调用之间的纳秒差。
    pub(crate) fn record_delta(
        &self,
        capability: &HistogramCapability,
        now: Instant,
    ) -> Result<(), String> {
        let mut backing = Self::lock_backing(capability)?;
        let Some(previous) = backing.previous else {
            backing.previous = Some(now);
            return Ok(());
        };
        let elapsed = now
            .checked_duration_since(previous)
            .ok_or_else(|| "recordDelta timestamp precedes its baseline".to_string())?;
        backing.previous = Some(now);
        let nanos = elapsed.as_nanos();
        if nanos > u128::from(u64::MAX) {
            backing.exceeds = backing.exceeds.saturating_add(1);
            return Ok(());
        }
        Self::record_value(&mut backing, nanos as u64)
    }

    pub(crate) fn clear_delta_baseline(
        &self,
        capability: &HistogramCapability,
    ) -> Result<(), String> {
        let mut backing = Self::lock_backing(capability)?;
        backing.previous = None;
        Ok(())
    }

    pub(crate) fn add(
        &self,
        destination: &HistogramCapability,
        source: &HistogramCapability,
    ) -> Result<(), String> {
        let source_snapshot = {
            let source = Self::lock_backing(source)?;
            (
                source.histogram.clone(),
                source.count,
                source.exceeds,
                source.previous,
            )
        };

        let mut destination = Self::lock_backing(destination)?;
        let mut merged = destination.histogram.clone();
        for recorded in source_snapshot.0.iter_recorded() {
            let value = recorded.value_iterated_to();
            if value > merged.high() {
                continue;
            }
            merged
                .record_n(value, recorded.count_at_value())
                .map_err(|error| format!("failed to add histogram: {error}"))?;
        }
        destination.histogram = merged;
        destination.count = destination.count.saturating_add(source_snapshot.1);
        destination.exceeds = destination.exceeds.saturating_add(source_snapshot.2);
        destination.previous = match (destination.previous, source_snapshot.3) {
            (Some(left), Some(right)) => Some(left.max(right)),
            (left, right) => left.or(right),
        };
        Ok(())
    }

    pub(crate) fn reset(&self, capability: &HistogramCapability) -> Result<(), String> {
        let mut backing = Self::lock_backing(capability)?;
        backing.histogram.reset();
        backing.count = 0;
        backing.exceeds = 0;
        backing.previous = None;
        Ok(())
    }

    pub(crate) fn stats(&self, capability: &HistogramCapability) -> Result<HistogramStats, String> {
        let backing = Self::lock_backing(capability)?;
        let empty = backing.histogram.is_empty();
        Ok(HistogramStats {
            count: backing.count,
            min: if empty {
                EMPTY_MIN
            } else {
                backing.histogram.min()
            },
            max: if empty { 0 } else { backing.histogram.max() },
            mean: if empty {
                f64::NAN
            } else {
                backing.histogram.mean()
            },
            stddev: if empty {
                f64::NAN
            } else {
                backing.histogram.stdev()
            },
            exceeds: backing.exceeds,
        })
    }

    pub(crate) fn percentile(
        &self,
        capability: &HistogramCapability,
        percentile: f64,
    ) -> Result<u64, String> {
        if !percentile.is_finite() || percentile <= 0.0 || percentile > 100.0 {
            return Err("histogram percentile must be greater than 0 and at most 100".to_string());
        }
        let backing = Self::lock_backing(capability)?;
        Ok(backing.histogram.value_at_percentile(percentile))
    }

    pub(crate) fn percentiles(
        &self,
        capability: &HistogramCapability,
    ) -> Result<Vec<(f64, u64)>, String> {
        let backing = Self::lock_backing(capability)?;
        if backing.histogram.is_empty() {
            return Ok(vec![(100.0, 0)]);
        }
        Ok(backing
            .histogram
            .iter_quantiles(1)
            .map(|value| {
                (
                    value.quantile_iterated_to() * 100.0,
                    value.value_iterated_to(),
                )
            })
            .collect())
    }

    pub(crate) fn active_count(&self) -> usize {
        self.metrics.active.load(Ordering::Relaxed)
    }

    pub(crate) fn is_empty(&self) -> Result<bool, String> {
        Ok(self.active_count() == 0)
    }

    fn record_value(backing: &mut HistogramBacking, value: u64) -> Result<(), String> {
        if value > backing.histogram.high() {
            backing.exceeds = backing.exceeds.saturating_add(1);
            return Ok(());
        }
        backing
            .histogram
            .record(value)
            .map_err(|error| format!("failed to record histogram value: {error}"))?;
        backing.count = backing.count.saturating_add(1);
        Ok(())
    }

    fn lock_backing(
        capability: &HistogramCapability,
    ) -> Result<MutexGuard<'_, HistogramBacking>, String> {
        capability
            .backing
            .lock()
            .map_err(|_| "perf_hooks histogram backing lock poisoned".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::{EMPTY_MIN, PerfHooksHistogramRegistry};
    use std::time::{Duration, Instant};

    fn new_registry() -> PerfHooksHistogramRegistry {
        PerfHooksHistogramRegistry::new()
    }

    #[test]
    fn empty_histogram_uses_node_sentinels() {
        let registry = new_registry();
        assert!(registry.is_empty().expect("inspect registry"));
        let capability = registry.create(1, 10_000, 3).expect("create histogram");
        assert!(!registry.is_empty().expect("inspect registry"));

        let stats = registry.stats(&capability).expect("read stats");
        assert_eq!(stats.count, 0);
        assert_eq!(stats.min, EMPTY_MIN);
        assert_eq!(stats.max, 0);
        assert!(stats.mean.is_nan());
        assert!(stats.stddev.is_nan());
        assert_eq!(stats.exceeds, 0);
        assert_eq!(
            registry.percentiles(&capability).expect("read percentiles"),
            vec![(100.0, 0)]
        );
        drop(capability);
        assert!(registry.is_empty().expect("inspect registry"));
    }

    #[test]
    fn cloned_capability_keeps_backing_alive_until_last_owner_drops() {
        let registry = new_registry();
        let capability = registry.create(1, 10_000, 3).expect("create histogram");
        let clone = capability.clone();

        drop(capability);
        assert_eq!(registry.active_count(), 1);
        registry.record(&clone, 42).expect("record through clone");

        drop(clone);
        assert!(registry.is_empty().expect("inspect registry"));
    }

    #[test]
    fn percentile_uses_hdr_quantization() {
        let registry = new_registry();
        let capability = registry.create(1, 100_000, 3).expect("create histogram");
        for value in [1, 2, 3, 4, 5, 10, 100] {
            registry.record(&capability, value).expect("record value");
        }

        assert_eq!(registry.percentile(&capability, 50.0).expect("p50"), 4);
        assert_eq!(registry.percentile(&capability, 75.0).expect("p75"), 10);
        assert_eq!(registry.percentile(&capability, 100.0).expect("p100"), 100);
        let percentiles = registry.percentiles(&capability).expect("read percentiles");
        assert_eq!(percentiles.first(), Some(&(0.0, 1)));
        assert_eq!(percentiles.last(), Some(&(100.0, 100)));
    }

    #[test]
    fn values_above_highest_only_increment_exceeds() {
        let registry = new_registry();
        let capability = registry.create(1, 100, 3).expect("create histogram");

        registry
            .record(&capability, 100)
            .expect("record in-range value");
        registry
            .record(&capability, 101)
            .expect("record overflow value");

        let stats = registry.stats(&capability).expect("read stats");
        assert_eq!(stats.count, 1);
        assert_eq!(stats.max, 100);
        assert_eq!(stats.exceeds, 1);
    }

    #[test]
    fn add_merges_compatible_histograms_and_reset_clears_state() {
        let registry = new_registry();
        let destination = registry.create(1, 1_000, 3).expect("create destination");
        let source = registry.create(1, 1_000, 3).expect("create source");
        registry
            .record(&destination, 10)
            .expect("record destination");
        registry.record(&source, 20).expect("record source");
        registry
            .record(&source, 2_000)
            .expect("record source overflow");

        registry.add(&destination, &source).expect("add histograms");
        let stats = registry.stats(&destination).expect("read merged stats");
        assert_eq!(stats.count, 2);
        assert_eq!(stats.min, 10);
        assert_eq!(stats.max, 20);
        assert_eq!(stats.exceeds, 1);

        registry.reset(&destination).expect("reset histogram");
        let stats = registry.stats(&destination).expect("read reset stats");
        assert_eq!(stats.count, 0);
        assert_eq!(stats.min, EMPTY_MIN);
        assert_eq!(stats.exceeds, 0);
    }

    #[test]
    fn add_preserves_logical_count_when_source_buckets_do_not_fit() {
        let registry = new_registry();
        let destination = registry.create(1, 100, 3).expect("create destination");
        let source = registry.create(1, 1_000, 3).expect("create source");
        registry.record(&source, 500).expect("record source");

        registry
            .add(&destination, &source)
            .expect("add incompatible histogram");
        let stats = registry.stats(&destination).expect("read stats");
        assert_eq!(stats.count, 1);
        assert_eq!(stats.max, 0);
        assert_eq!(stats.exceeds, 0);
    }

    #[test]
    fn record_delta_uses_first_call_as_baseline() {
        let registry = new_registry();
        let capability = registry.create(1, 10_000_000, 3).expect("create histogram");
        let start = Instant::now();

        registry
            .record_delta(&capability, start)
            .expect("set baseline");
        assert_eq!(registry.stats(&capability).expect("read stats").count, 0);
        registry
            .record_delta(&capability, start + Duration::from_micros(250))
            .expect("record delta");

        let stats = registry.stats(&capability).expect("read stats");
        assert_eq!(stats.count, 1);
        assert_eq!(stats.min, 249_984);
        assert_eq!(stats.max, 250_111);
    }

    #[test]
    fn clearing_delta_baseline_excludes_disabled_time() {
        let registry = new_registry();
        let capability = registry
            .create(1, 20_000_000_000, 3)
            .expect("create histogram");
        let start = Instant::now();

        registry
            .record_delta(&capability, start)
            .expect("set initial baseline");
        registry
            .record_delta(&capability, start + Duration::from_millis(1))
            .expect("record initial delta");
        registry
            .clear_delta_baseline(&capability)
            .expect("clear baseline");
        registry
            .record_delta(&capability, start + Duration::from_secs(10))
            .expect("set re-enabled baseline");

        let stats = registry.stats(&capability).expect("read stats");
        assert_eq!(stats.count, 1);
        assert!(stats.max < 2_000_000);
    }

    #[test]
    fn poisoned_backing_returns_error() {
        let registry = new_registry();
        let capability = registry.create(1, 100, 3).expect("create histogram");
        let backing = capability.backing.clone();
        let _ = std::panic::catch_unwind(|| {
            let _guard = backing.lock().expect("lock backing");
            panic!("poison backing");
        });
        assert!(
            registry
                .stats(&capability)
                .expect_err("poisoned backing")
                .contains("poisoned")
        );
    }
}
