//! 生产 runtime 的三种 collector 调度；所有算法只借用唯一 heap owner。

use std::collections::{BTreeMap, HashSet};
use std::error::Error;
use std::fmt;
use std::time::Instant;

use crate::{
    CollectorHeapCapability, CycleKind, GcAlgorithmKind, GcStats, GcTelemetry, GcTelemetrySnapshot,
    GrowableHeapMemory, HandleGeneration, HeapAccessV2Error,
};

const G1_PROMOTION_AGE: u8 = 2;

/// 一次 production collection 的精确变更集合。
#[derive(Clone, Debug, Default)]
pub struct RuntimeGcReport {
    pub retired_handles: Vec<u32>,
    pub relocated_handles: Vec<u32>,
    pub promoted_handles: Vec<u32>,
    pub stats: GcStats,
}

/// 运行时 collector 状态；不持有 heap、memory 或 handle table。
pub struct RuntimeCollector {
    algorithm: GcAlgorithmKind,
    g1_ages: BTreeMap<u32, u8>,
    telemetry: GcTelemetry,
}

impl RuntimeCollector {
    pub fn new(algorithm: GcAlgorithmKind) -> Self {
        Self {
            algorithm,
            g1_ages: BTreeMap::new(),
            telemetry: GcTelemetry::default(),
        }
    }

    pub const fn algorithm(&self) -> GcAlgorithmKind {
        self.algorithm
    }

    pub fn collect<M: GrowableHeapMemory>(
        &mut self,
        heap: CollectorHeapCapability<'_, M>,
        reachable: &HashSet<u32>,
    ) -> Result<RuntimeGcReport, RuntimeCollectorError> {
        let started_at = Instant::now();
        let live_handles = heap.live_handles();
        let mut report = RuntimeGcReport::default();

        for handle in live_handles
            .iter()
            .copied()
            .filter(|handle| !reachable.contains(handle))
        {
            let bytes = heap.retire(handle)?;
            report.stats.freed_bytes = report
                .stats
                .freed_bytes
                .saturating_add(bytes_to_usize(bytes)?);
            report.retired_handles.push(handle);
            self.g1_ages.remove(&handle);
        }

        // STW owner 没有活动 object guard；先发布死亡对象回收，再执行 moving collector。
        heap.advance_epoch_and_reclaim();
        match self.algorithm {
            GcAlgorithmKind::MarkSweep => {
                report.stats.cycle_kind = CycleKind::Full;
            }
            GcAlgorithmKind::G1 => self.collect_g1(&heap, reachable, &mut report)?,
            GcAlgorithmKind::Zgc => self.collect_zgc(&heap, reachable, &mut report)?,
        }
        heap.advance_epoch_and_reclaim();

        report.stats.marked = reachable
            .iter()
            .filter(|handle| live_handles.binary_search(handle).is_ok())
            .count();
        report.stats.swept = report.retired_handles.len();
        report.stats.free_bytes_reusable = bytes_to_usize(heap.free_bytes())?;
        report.stats.heap_used_bytes = bytes_to_usize(heap.used_bytes())?;
        report.stats.elapsed = started_at.elapsed();
        report.stats.ensure_pause_from_elapsed();
        self.telemetry
            .record_cycle(self.algorithm.as_str(), &report.stats);
        Ok(report)
    }

    pub fn telemetry_snapshot(&self) -> GcTelemetrySnapshot {
        self.telemetry.snapshot()
    }

    fn collect_g1<M: GrowableHeapMemory>(
        &mut self,
        heap: &CollectorHeapCapability<'_, M>,
        reachable: &HashSet<u32>,
        report: &mut RuntimeGcReport,
    ) -> Result<(), RuntimeCollectorError> {
        report.stats.cycle_kind = CycleKind::Young;
        for handle in heap
            .live_handles()
            .into_iter()
            .filter(|handle| reachable.contains(handle))
        {
            if heap.generation(handle) != Some(HandleGeneration::Young) {
                continue;
            }
            let bytes = heap.relocate(handle)?;
            report.stats.relocated_bytes = report
                .stats
                .relocated_bytes
                .saturating_add(bytes_to_usize(bytes)?);
            report.stats.relocated_objects = report.stats.relocated_objects.saturating_add(1);
            report.relocated_handles.push(handle);

            let age = self.g1_ages.entry(handle).or_default();
            *age = age.saturating_add(1);
            if *age >= G1_PROMOTION_AGE {
                heap.promote(handle)?;
                self.g1_ages.remove(&handle);
                report.promoted_handles.push(handle);
            }
            // 逐对象完成 grace period，避免 evacuation 需要第二份完整 live heap。
            heap.advance_epoch_and_reclaim();
        }
        Ok(())
    }

    fn collect_zgc<M: GrowableHeapMemory>(
        &mut self,
        heap: &CollectorHeapCapability<'_, M>,
        reachable: &HashSet<u32>,
        report: &mut RuntimeGcReport,
    ) -> Result<(), RuntimeCollectorError> {
        report.stats.cycle_kind = CycleKind::ZgcCycle;
        for handle in heap
            .live_handles()
            .into_iter()
            .filter(|handle| reachable.contains(handle))
        {
            let bytes = heap.relocate(handle)?;
            report.stats.relocated_bytes = report
                .stats
                .relocated_bytes
                .saturating_add(bytes_to_usize(bytes)?);
            report.stats.relocated_objects = report.stats.relocated_objects.saturating_add(1);
            report.relocated_handles.push(handle);
            heap.advance_epoch_and_reclaim();
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RuntimeCollectorError {
    Heap(HeapAccessV2Error),
    HostAddressRange { bytes: u64 },
}

impl fmt::Display for RuntimeCollectorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Heap(error) => error.fmt(formatter),
            Self::HostAddressRange { bytes } => {
                write!(formatter, "GC byte count {bytes} cannot fit host usize")
            }
        }
    }
}

impl Error for RuntimeCollectorError {}

impl From<HeapAccessV2Error> for RuntimeCollectorError {
    fn from(error: HeapAccessV2Error) -> Self {
        Self::Heap(error)
    }
}

fn bytes_to_usize(bytes: u64) -> Result<usize, RuntimeCollectorError> {
    usize::try_from(bytes).map_err(|_| RuntimeCollectorError::HostAddressRange { bytes })
}
