//! Native runtime 的唯一 heap、mutator 与 collector 生命周期 owner。

use std::cell::{Cell, RefCell};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use thiserror::Error;
use wjsm_gc::{
    GcAlgorithmKind, GcRuntimeV2, GcSafepointAction, GcTelemetrySnapshot, GenerationalZgc,
    GenerationalZgcError, HandleTableV2, HeapAccessV2, HeapAccessV2Error, HeapBarrier,
    ManagedHeapLayout, MutatorContext, NativeHeapMemory, Nlab, PAGE_GRANULE_BYTES, RootSnapshot,
    RuntimeGcReport, StopTheWorldCollector, StopTheWorldCollectorError, ZgcBarrierSet,
};
use wjsm_native_abi::{NATIVE_BARRIER_MARKING_MASK, NativeBarrierState, NativeVmContext};

const ZGC_BARRIER_RING_CAPACITY: usize = 4_096;
const ZGC_PACKET_CAPACITY: usize = 4_096;

pub(super) struct NativeGc {
    collector: NativeCollector,
    heap: Arc<HeapAccessV2<NativeHeapMemory>>,
    nlab: RefCell<Nlab>,
    mutator: MutatorContext,
    pacing_poll_requested: Cell<bool>,
    barrier_state: Box<NativeBarrierState>,
}

enum NativeCollector {
    StopTheWorld(StopTheWorldCollector),
    Zgc(GenerationalZgc<NativeHeapMemory>),
}

impl NativeGc {
    pub(super) fn new(
        algorithm: GcAlgorithmKind,
        max_heap_size: u64,
    ) -> Result<Self, NativeGcError> {
        let heap = Self::fresh_heap(algorithm, max_heap_size)?;
        let collector = match algorithm {
            GcAlgorithmKind::MarkSweep | GcAlgorithmKind::G1 => {
                NativeCollector::StopTheWorld(StopTheWorldCollector::new(algorithm)?)
            }
            GcAlgorithmKind::Zgc => NativeCollector::Zgc(GenerationalZgc::new(
                Arc::clone(&heap),
                worker_count(),
                ZGC_PACKET_CAPACITY,
            )?),
        };
        let control = GcRuntimeV2::new();
        let mutator = control.register_mutator();
        Ok(Self {
            collector,
            heap,
            nlab: RefCell::new(Nlab::new()),
            mutator,
            pacing_poll_requested: Cell::new(false),
            barrier_state: Box::default(),
        })
    }

    pub(super) fn fresh_heap(
        algorithm: GcAlgorithmKind,
        max_heap_size: u64,
    ) -> Result<Arc<HeapAccessV2<NativeHeapMemory>>, NativeGcError> {
        let layout = Arc::new(
            ManagedHeapLayout::new(max_heap_size, 64 * 1024)
                .map_err(HeapAccessV2Error::HandleTable)?,
        );
        let memory = NativeHeapMemory::for_layout(&layout)
            .map_err(|error| NativeGcError::NativeMemory(error.to_string()))?;
        let handles = Arc::new(
            HandleTableV2::new(layout.as_ref().clone()).map_err(HeapAccessV2Error::HandleTable)?,
        );
        let barrier = match algorithm {
            GcAlgorithmKind::Zgc => HeapBarrier::Zgc(Arc::new(ZgcBarrierSet::new(
                Arc::clone(&handles),
                memory.clone(),
                ZGC_BARRIER_RING_CAPACITY,
            ))),
            GcAlgorithmKind::MarkSweep | GcAlgorithmKind::G1 => HeapBarrier::Disabled,
        };
        Ok(Arc::new(HeapAccessV2::with_handles(
            memory, layout, handles, barrier,
        )?))
    }

    pub(super) fn heap(&self) -> &HeapAccessV2<NativeHeapMemory> {
        &self.heap
    }

    pub(super) fn allocate(&self, bytes: u64) -> Result<u64, HeapAccessV2Error> {
        let mut nlab = self.nlab.borrow_mut();
        let refills = nlab.refills();
        let allocation = self.heap.allocate(&mut nlab, bytes)?;
        let needs_poll = allocation.is_dedicated() || nlab.refills() != refills;
        drop(nlab);
        if needs_poll {
            self.pacing_poll_requested.set(true);
            let observed = if allocation.is_dedicated() {
                allocation.bytes()
            } else {
                PAGE_GRANULE_BYTES
            };
            match &self.collector {
                NativeCollector::StopTheWorld(collector) => {
                    collector.observe_allocation(observed, Duration::from_micros(10));
                }
                NativeCollector::Zgc(collector) => {
                    collector.observe_allocation(observed, Duration::from_micros(10));
                }
            }
        }
        Ok(allocation.object().offset())
    }
    pub(super) fn mark_black_allocation(&self, handle: u32) -> Result<(), HeapAccessV2Error> {
        match &self.collector {
            NativeCollector::StopTheWorld(_) => Ok(()),
            NativeCollector::Zgc(collector) => collector.mark_black_allocation(handle),
        }
    }
    pub(super) fn record_host_write(&self, owner: i64, old: Option<i64>, new: Option<i64>) {
        if self.barrier_state.phase.load(Ordering::Acquire) & NATIVE_BARRIER_MARKING_MASK == 0 {
            return;
        }
        if let NativeCollector::Zgc(collector) = &self.collector {
            collector.record_host_write(owner, old, new);
        }
    }

    pub(super) fn take_pacing_poll_request(&self) -> bool {
        self.pacing_poll_requested.replace(false)
    }

    pub(super) fn take_safepoint_poll_request(&self) -> bool {
        let pacing_requested = self.take_pacing_poll_request();
        match &self.collector {
            NativeCollector::StopTheWorld(_) => pacing_requested,
            NativeCollector::Zgc(collector) => pacing_requested || collector.cycle_active(),
        }
    }

    pub(super) fn reset_nlab(&self) {
        self.nlab.borrow_mut().reset();
        self.heap.reset_nlab();
        self.pacing_poll_requested.set(false);
    }

    pub(super) fn reset_heap(
        &mut self,
        heap: Arc<HeapAccessV2<NativeHeapMemory>>,
    ) -> Result<(), NativeGcError> {
        if let NativeCollector::Zgc(collector) = &self.collector {
            collector.reset_heap(Arc::clone(&heap))?;
        }
        self.heap = heap;
        self.reset_nlab();
        self.sync_barrier_state();
        Ok(())
    }

    pub(super) fn safepoint_action(&self) -> GcSafepointAction {
        match &self.collector {
            NativeCollector::StopTheWorld(collector) => {
                collector.safepoint_action(self.heap.collector_capability())
            }
            NativeCollector::Zgc(collector) => collector.safepoint_action(),
        }
    }
    pub(super) fn cycle_active(&self) -> bool {
        match &self.collector {
            NativeCollector::StopTheWorld(_) => false,
            NativeCollector::Zgc(collector) => collector.cycle_active(),
        }
    }

    pub(super) fn at_safepoint(
        &mut self,
        snapshot: Option<RootSnapshot>,
    ) -> Result<Option<RuntimeGcReport>, NativeGcError> {
        if snapshot.is_some() {
            self.reset_nlab();
        }
        let report = match &mut self.collector {
            NativeCollector::StopTheWorld(collector) => snapshot
                .as_ref()
                .map(|snapshot| collector.collect(self.heap.collector_capability(), snapshot))
                .transpose()?,
            NativeCollector::Zgc(collector) => collector.at_safepoint(snapshot)?,
        };
        if report.is_some() {
            self.reset_nlab();
        }
        self.sync_barrier_state();
        Ok(report)
    }

    pub(super) fn collect_full(
        &mut self,
        snapshot: RootSnapshot,
    ) -> Result<RuntimeGcReport, NativeGcError> {
        self.reset_nlab();
        let report = match &mut self.collector {
            NativeCollector::StopTheWorld(collector) => {
                collector.collect(self.heap.collector_capability(), &snapshot)?
            }
            NativeCollector::Zgc(collector) => collector.collect_full(snapshot)?,
        };
        self.reset_nlab();
        self.sync_barrier_state();
        Ok(report)
    }

    pub(super) fn telemetry_snapshot(&self) -> GcTelemetrySnapshot {
        match &self.collector {
            NativeCollector::StopTheWorld(collector) => collector.telemetry_snapshot(),
            NativeCollector::Zgc(collector) => collector.telemetry_snapshot(),
        }
    }
    pub(super) fn reset_telemetry(&self) {
        match &self.collector {
            NativeCollector::StopTheWorld(collector) => collector.reset_telemetry(),
            NativeCollector::Zgc(collector) => collector.reset_telemetry(),
        }
    }

    pub(super) fn bind_context(&self, context: &mut NativeVmContext) {
        self.sync_barrier_state();
        context.handle_table_base = self.heap.handle_table_base();
        context.heap_object_delta = self.heap.object_address_delta();
        context.gc_state = std::ptr::from_ref(self).cast_mut().cast();
        context.allocation_state = std::ptr::from_ref(&self.nlab).cast_mut().cast();
        context.barrier_state = std::ptr::from_ref(self.barrier_state.as_ref())
            .cast_mut()
            .cast();
        let _ = self.mutator.participant_id();
    }

    fn sync_barrier_state(&self) {
        let (phase, access_epoch) = match self.heap.barrier() {
            HeapBarrier::Disabled => (0, 0),
            HeapBarrier::Zgc(barrier) => (barrier.epoch().pack(), barrier.access_epoch()),
        };
        self.barrier_state.phase.store(phase, Ordering::Release);
        self.barrier_state
            .access_epoch
            .store(access_epoch, Ordering::Release);
    }
}

fn worker_count() -> usize {
    1
}

#[derive(Debug, Error)]
pub enum NativeGcError {
    #[error(transparent)]
    Heap(#[from] HeapAccessV2Error),
    #[error(transparent)]
    StopTheWorld(#[from] StopTheWorldCollectorError),
    #[error(transparent)]
    Zgc(#[from] GenerationalZgcError),
    #[error("native heap mapping failed: {0}")]
    NativeMemory(String),
}
