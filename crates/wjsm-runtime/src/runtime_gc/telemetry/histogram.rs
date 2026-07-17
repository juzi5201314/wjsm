use hdrhistogram::Histogram;

use super::HistogramSnapshot;

pub(super) struct PauseHistogram {
    values: Histogram<u64>,
    min_ns: Option<u64>,
    max_ns: u64,
}

impl Default for PauseHistogram {
    fn default() -> Self {
        Self {
            values: Histogram::new(3).expect("3-digit HDR histogram configuration is valid"),
            min_ns: None,
            max_ns: 0,
        }
    }
}

impl PauseHistogram {
    pub(super) fn record(&mut self, value_ns: u64) {
        self.min_ns = Some(self.min_ns.map_or(value_ns, |min| min.min(value_ns)));
        self.max_ns = self.max_ns.max(value_ns);
        self.values
            .record(value_ns.max(1))
            .expect("u64 nanosecond pause fits HDR histogram");
    }

    pub(super) fn snapshot(&self) -> HistogramSnapshot {
        if self.values.is_empty() {
            return HistogramSnapshot::default();
        }
        HistogramSnapshot {
            count: self.values.len(),
            min_ns: self
                .min_ns
                .expect("non-empty histogram has a recorded minimum"),
            max_ns: self.max_ns,
            p50_ns: self.values.value_at_quantile(0.50),
            p95_ns: self.values.value_at_quantile(0.95),
            p99_ns: self.values.value_at_quantile(0.99),
            p999_ns: self.values.value_at_quantile(0.999),
        }
    }
}
