//! Mark-Sweep/G1 的 stop-the-world collector；只借用唯一 heap owner。

use parking_lot::Mutex;
use std::collections::{BTreeMap, BTreeSet, HashSet, VecDeque};
use std::error::Error;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use std::time::Instant;
use wjsm_ir::value;

use crate::zgc::director::{DirectorDecision, DirectorGeneration, GcDirector};
use crate::{
    CollectorHeapCapability, CycleKind, GcAlgorithmKind, GcSafepointAction, GcStats, GcTelemetry,
    GcTelemetrySnapshot, GrowableHeapMemory, HandleGeneration, HeapAccessV2Error, Nlab,
    RootSnapshot,
};

const G1_PROMOTION_AGE: u8 = 2;

/// 一次 production collection 的精确变更集合。
#[derive(Clone, Debug, Default)]
pub struct RuntimeGcReport {
    pub retired_handles: Vec<u32>,
    pub relocated_handles: Vec<u32>,
    pub promoted_handles: Vec<u32>,
    pub live_host_values: Vec<i64>,
    pub cleans_host_tables: bool,
    pub stats: GcStats,
}

pub struct StopTheWorldCollector {
    algorithm: StopTheWorldKind,
    g1_ages: BTreeMap<u32, u8>,
    director: Mutex<GcDirector>,
    next_epoch: AtomicU64,
    telemetry: GcTelemetry,
}

#[derive(Clone, Copy)]
enum StopTheWorldKind {
    MarkSweep,
    G1,
}

impl StopTheWorldCollector {
    pub fn new(algorithm: GcAlgorithmKind) -> Result<Self, StopTheWorldCollectorError> {
        let algorithm = match algorithm {
            GcAlgorithmKind::MarkSweep => StopTheWorldKind::MarkSweep,
            GcAlgorithmKind::G1 => StopTheWorldKind::G1,
            GcAlgorithmKind::Zgc => return Err(StopTheWorldCollectorError::UnsupportedAlgorithm),
        };
        Ok(Self {
            algorithm,
            g1_ages: BTreeMap::new(),
            director: Mutex::new(GcDirector::new()),
            next_epoch: AtomicU64::new(0),
            telemetry: GcTelemetry::default(),
        })
    }

    pub const fn algorithm(&self) -> GcAlgorithmKind {
        match self.algorithm {
            StopTheWorldKind::MarkSweep => GcAlgorithmKind::MarkSweep,
            StopTheWorldKind::G1 => GcAlgorithmKind::G1,
        }
    }

    pub fn observe_allocation(&self, bytes: u64, elapsed: Duration) {
        self.telemetry.record_allocation(bytes);
        self.director
            .lock()
            .observe_allocation(DirectorGeneration::Young, bytes, elapsed);
    }

    pub fn safepoint_action<M: GrowableHeapMemory>(
        &self,
        heap: CollectorHeapCapability<'_, M>,
    ) -> GcSafepointAction {
        let mut director = self.director.lock();
        director.update_space(heap.free_bytes(), 0);
        let (young_live, old_live) = heap
            .generation_bytes()
            .expect("managed page metadata must resolve every live handle");
        match director.evaluate(young_live, old_live) {
            DirectorDecision::StartYoung | DirectorDecision::StartOld => {
                let epoch = self.next_epoch.fetch_add(1, Ordering::SeqCst) + 1;
                GcSafepointAction::PublishRoots { epoch }
            }
            DirectorDecision::Idle | DirectorDecision::Continue => GcSafepointAction::Idle,
        }
    }

    pub fn collect<M: GrowableHeapMemory>(
        &mut self,
        heap: CollectorHeapCapability<'_, M>,
        snapshot: &RootSnapshot,
    ) -> Result<RuntimeGcReport, StopTheWorldCollectorError> {
        let started_at = Instant::now();
        let live_handles = heap.live_handles();
        let (reachable, live_host_values, marked_bytes) = trace_snapshot(&heap, snapshot)?;
        let mut report = RuntimeGcReport {
            live_host_values,
            cleans_host_tables: true,
            ..RuntimeGcReport::default()
        };

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
        heap.advance_epoch_and_reclaim()?;
        match self.algorithm {
            StopTheWorldKind::MarkSweep => {
                report.stats.cycle_kind = CycleKind::Full;
            }
            StopTheWorldKind::G1 => self.collect_g1(&heap, &reachable, &mut report)?,
        }
        heap.advance_epoch_and_reclaim()?;

        report.stats.marked = reachable
            .iter()
            .filter(|handle| live_handles.binary_search(handle).is_ok())
            .count();
        report.stats.mark_live_bytes = marked_bytes;
        report.stats.swept = report.retired_handles.len();
        report.stats.free_bytes_reusable = bytes_to_usize(heap.free_bytes())?;
        report.stats.heap_used_bytes = bytes_to_usize(heap.used_bytes())?;
        report.stats.elapsed = started_at.elapsed();
        report.stats.ensure_pause_from_elapsed();
        let generation = if self.director.lock().old_active() {
            DirectorGeneration::Old
        } else {
            DirectorGeneration::Young
        };
        let survival = if live_handles.is_empty() {
            0.0
        } else {
            report.stats.marked as f64 / live_handles.len() as f64
        };
        let pacing_relocated_bytes = match self.algorithm {
            StopTheWorldKind::MarkSweep => marked_bytes,
            StopTheWorldKind::G1 => report.stats.relocated_bytes as u64,
        };
        let mut director = self.director.lock();
        director.observe_relocate(generation, pacing_relocated_bytes, report.stats.elapsed);
        director.observe_mark(generation, marked_bytes, report.stats.elapsed);
        director.observe_survival(generation, survival);
        director.complete_cycle(generation, 0, report.stats.elapsed);
        self.telemetry
            .record_cycle(self.algorithm().as_str(), &report.stats);
        Ok(report)
    }

    pub fn telemetry_snapshot(&self) -> GcTelemetrySnapshot {
        self.telemetry.snapshot()
    }

    pub fn reset_telemetry(&self) {
        self.telemetry.reset();
    }

    fn collect_g1<M: GrowableHeapMemory>(
        &mut self,
        heap: &CollectorHeapCapability<'_, M>,
        reachable: &HashSet<u32>,
        report: &mut RuntimeGcReport,
    ) -> Result<(), StopTheWorldCollectorError> {
        report.stats.cycle_kind = CycleKind::Young;
        let mut relocation_nlab = Nlab::new();
        for handle in heap
            .live_handles()
            .into_iter()
            .filter(|handle| reachable.contains(handle))
        {
            if heap.generation(handle) != Some(HandleGeneration::Young) {
                continue;
            }
            let bytes = heap.relocate(&mut relocation_nlab, handle)?;
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
            heap.advance_epoch_and_reclaim()?;
        }
        Ok(())
    }
}
fn trace_snapshot<M: GrowableHeapMemory>(
    heap: &CollectorHeapCapability<'_, M>,
    snapshot: &RootSnapshot,
) -> Result<(HashSet<u32>, Vec<i64>, u64), StopTheWorldCollectorError> {
    let mut pending = VecDeque::from(snapshot.roots().to_vec());
    let mut visited_edge_owners = BTreeSet::new();
    let mut live_host_values = BTreeSet::new();
    let mut reachable = HashSet::new();
    let mut activated = vec![false; snapshot.ephemerons().len()];
    let mut marked_bytes = 0_u64;

    loop {
        while let Some(encoded) = pending.pop_front() {
            let encoded = value::strip_gc_color(encoded);
            if !value::is_handle_backed_reference(encoded) {
                continue;
            }
            if visited_edge_owners.insert(encoded) {
                let edges = snapshot.strong_edges();
                let start = edges.partition_point(|edge| edge.owner < encoded);
                let end = edges.partition_point(|edge| edge.owner <= encoded);
                pending.extend(edges[start..end].iter().map(|edge| edge.target));
            }
            let is_heap = value::is_heap_reference(encoded)
                && heap.generation(value::decode_handle(encoded)).is_some();
            if is_heap {
                let handle = value::decode_handle(encoded);
                if !reachable.insert(handle) {
                    continue;
                }
                marked_bytes = marked_bytes.saturating_add(heap.object_size(handle)?);
                heap.scan_references(handle, |reference| {
                    if value::is_handle_backed_reference(reference) {
                        pending.push_back(reference);
                    }
                })?;
            } else {
                live_host_values.insert(encoded);
            }
        }

        let mut added = false;
        for (index, ephemeron) in snapshot.ephemerons().iter().enumerate() {
            if activated[index] {
                continue;
            }
            let is_live = |encoded: i64| {
                let encoded = value::strip_gc_color(encoded);
                if !value::is_handle_backed_reference(encoded) {
                    true
                } else if value::is_heap_reference(encoded)
                    && heap.generation(value::decode_handle(encoded)).is_some()
                {
                    reachable.contains(&value::decode_handle(encoded))
                } else {
                    live_host_values.contains(&encoded)
                }
            };
            if is_live(ephemeron.owner) && is_live(ephemeron.key) {
                activated[index] = true;
                pending.push_back(ephemeron.value);
                added = true;
            }
        }
        if !added {
            break;
        }
    }

    Ok((
        reachable,
        live_host_values.into_iter().collect(),
        marked_bytes,
    ))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StopTheWorldCollectorError {
    Heap(HeapAccessV2Error),
    HostAddressRange { bytes: u64 },
    UnsupportedAlgorithm,
}

impl fmt::Display for StopTheWorldCollectorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Heap(error) => error.fmt(formatter),
            Self::HostAddressRange { bytes } => {
                write!(formatter, "GC byte count {bytes} cannot fit host usize")
            }
            Self::UnsupportedAlgorithm => {
                formatter.write_str("stop-the-world collector does not support ZGC")
            }
        }
    }
}

impl Error for StopTheWorldCollectorError {}

impl From<HeapAccessV2Error> for StopTheWorldCollectorError {
    fn from(error: HeapAccessV2Error) -> Self {
        Self::Heap(error)
    }
}

fn bytes_to_usize(bytes: u64) -> Result<usize, StopTheWorldCollectorError> {
    usize::try_from(bytes).map_err(|_| StopTheWorldCollectorError::HostAddressRange { bytes })
}
