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
use wjsm_ir::value;
use wjsm_native_abi::{
    NATIVE_ALLOCATION_FAST_ARRAY, NATIVE_ALLOCATION_FAST_HOST, NATIVE_ALLOCATION_FAST_OBJECT,
    NATIVE_BARRIER_MARKING_MASK, NativeBarrierState, NativeVmContext,
};

const ZGC_BARRIER_RING_CAPACITY: usize = 4_096;
const NATIVE_TLAB_HANDLE_COUNT: u32 = 4_096;
const ZGC_PACKET_CAPACITY: usize = 4_096;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct NativeAllocationDiagnostics {
    pub tlab_fast_allocations: u64,
    pub tlab_fast_bytes: u64,
    pub tlab_refills: u64,
    pub tlab_flushes: u64,
    pub slow_allocations: u64,
    pub inline_string_constructions: u64,
    pub inline_property_keys: u64,
}

struct NativeAllocationCounters {
    enabled: bool,
    tlab_fast_allocations: Cell<u64>,
    tlab_fast_bytes: Cell<u64>,
    tlab_refills: Cell<u64>,
    tlab_flushes: Cell<u64>,
    slow_allocations: Cell<u64>,
    inline_string_constructions: Cell<u64>,
    inline_property_keys: Cell<u64>,
}

impl NativeAllocationCounters {
    const fn new(enabled: bool) -> Self {
        Self {
            enabled,
            tlab_fast_allocations: Cell::new(0),
            tlab_fast_bytes: Cell::new(0),
            tlab_refills: Cell::new(0),
            tlab_flushes: Cell::new(0),
            slow_allocations: Cell::new(0),
            inline_string_constructions: Cell::new(0),
            inline_property_keys: Cell::new(0),
        }
    }

    fn record(&self, counter: &Cell<u64>, amount: u64) {
        if self.enabled {
            counter.set(counter.get().saturating_add(amount));
        }
    }

    fn record_tlab_refill(&self) {
        self.record(&self.tlab_refills, 1);
    }

    fn record_slow_allocation(&self) {
        self.record(&self.slow_allocations, 1);
    }

    fn snapshot(&self) -> NativeAllocationDiagnostics {
        NativeAllocationDiagnostics {
            tlab_fast_allocations: self.tlab_fast_allocations.get(),
            tlab_fast_bytes: self.tlab_fast_bytes.get(),
            tlab_refills: self.tlab_refills.get(),
            tlab_flushes: self.tlab_flushes.get(),
            slow_allocations: self.slow_allocations.get(),
            inline_string_constructions: self.inline_string_constructions.get(),
            inline_property_keys: self.inline_property_keys.get(),
        }
    }
}

pub(super) struct NativeGc {
    collector: NativeCollector,
    heap: Arc<HeapAccessV2<NativeHeapMemory>>,
    nlab: RefCell<Nlab>,
    native_tlab: RefCell<Option<NativeTlabWindow>>,
    mutator: MutatorContext,
    pacing_poll_requested: Cell<bool>,
    barrier_state: Box<NativeBarrierState>,
    allocation_prototypes_ready: Cell<bool>,
    allocation_diagnostics: NativeAllocationCounters,
}
struct NativeTlabWindow {
    reservation: wjsm_gc::NativeTlabReservation,
    top: u64,
    next_handle: u32,
}

impl NativeTlabWindow {
    fn new(reservation: wjsm_gc::NativeTlabReservation) -> Self {
        Self {
            top: reservation.object_start(),
            next_handle: reservation.handle_start(),
            reservation,
        }
    }
}

enum NativeCollector {
    StopTheWorld(StopTheWorldCollector),
    Zgc(GenerationalZgc<NativeHeapMemory>),
}

impl NativeGc {
    pub(super) fn new(
        algorithm: GcAlgorithmKind,
        max_heap_size: u64,
        allocation_diagnostics_enabled: bool,
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
            native_tlab: RefCell::new(None),
            mutator,
            pacing_poll_requested: Cell::new(false),
            barrier_state: Box::default(),
            allocation_prototypes_ready: Cell::new(false),
            allocation_diagnostics: NativeAllocationCounters::new(allocation_diagnostics_enabled),
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
        self.allocation_diagnostics.record_slow_allocation();
        if nlab.refills() != refills {
            self.allocation_diagnostics.record_tlab_refill();
        }
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
    pub(super) fn reserve_native_tlab(&self) -> Result<(), NativeGcError> {
        if self.native_tlab.borrow().is_some() {
            return Ok(());
        }
        if self.native_allocation_allowed() {
            let reservation = self.heap.reserve_native_tlab(NATIVE_TLAB_HANDLE_COUNT)?;
            self.native_tlab
                .replace(Some(NativeTlabWindow::new(reservation)));
            self.allocation_diagnostics.record_tlab_refill();
        }
        Ok(())
    }

    pub(super) fn flush_native_tlab(&self, context: &NativeVmContext) -> Result<(), NativeGcError> {
        let mut native_tlab = self.native_tlab.borrow_mut();
        let Some(window) = native_tlab.as_mut() else {
            return Ok(());
        };
        if context.bump_limit != window.reservation.object_limit()
            || context.bump_handle_limit != window.reservation.handle_limit()
        {
            return Err(NativeGcError::Invariant(
                "native TLAB vmctx limits do not match owner reservation".into(),
            ));
        }
        let previous_top = window.reservation.materialized_top();
        let previous_handles = window.reservation.materialized_handles();
        if context.bump_ptr == previous_top && context.bump_handle_cursor == previous_handles {
            window.top = context.bump_ptr;
            window.next_handle = context.bump_handle_cursor;
            return Ok(());
        }
        window.top = context.bump_ptr;
        window.next_handle = context.bump_handle_cursor;
        let allocated_bytes = window.top.saturating_sub(previous_top);
        let allocated_handles = window.next_handle.saturating_sub(previous_handles);
        self.heap.materialize_native_tlab(
            &mut window.reservation,
            window.top,
            window.next_handle,
        )?;
        if allocated_handles != 0 {
            self.allocation_diagnostics.record(
                &self.allocation_diagnostics.tlab_fast_allocations,
                u64::from(allocated_handles),
            );
            self.allocation_diagnostics.record(
                &self.allocation_diagnostics.tlab_fast_bytes,
                allocated_bytes,
            );
        }
        if window.top != previous_top {
            self.allocation_diagnostics
                .record(&self.allocation_diagnostics.tlab_flushes, 1);
        }
        Ok(())
    }
    /// 在宿主即将解引用 generated 数组或执行不透明 builtin 前登记 TLAB 对象。
    /// 普通对象属性写入使用四槽初始容量，避免每次写入都查询尚未物化的 page metadata。
    pub(super) fn operation_requires_native_tlab_flush(
        &self,
        context: &NativeVmContext,
        operation: Option<wjsm_native_abi::NativeRuntimeOp>,
        builtin: bool,
        args: &[i64],
    ) -> Result<bool, NativeGcError> {
        let Some(encoded) = args
            .first()
            .copied()
            .filter(|value| value::is_object(*value) || value::is_array(*value))
        else {
            return Ok(false);
        };
        let must_materialize = value::is_array(encoded)
            || builtin
            || matches!(
                operation,
                Some(wjsm_native_abi::NativeRuntimeOp::ObjectSpread)
            );
        if !must_materialize {
            return Ok(false);
        }
        let native_tlab = self.native_tlab.borrow();
        let Some(window) = native_tlab.as_ref() else {
            return Ok(false);
        };
        if context.bump_limit != window.reservation.object_limit()
            || context.bump_handle_limit != window.reservation.handle_limit()
            || context.bump_ptr < window.top
            || context.bump_handle_cursor < window.next_handle
            || context.bump_ptr > context.bump_limit
            || context.bump_handle_cursor > context.bump_handle_limit
        {
            return Err(NativeGcError::Invariant(
                "native TLAB vmctx cursor is outside owner reservation".into(),
            ));
        }
        let first = window.reservation.materialized_handles();
        let limit = context.bump_handle_cursor;
        debug_assert!(first <= window.next_handle);
        if !(first..limit).contains(&value::decode_handle(encoded)) {
            return Ok(false);
        }
        Ok(true)
    }

    pub(super) fn publish_native_tlab_object(
        &self,
        context: &NativeVmContext,
        handle: u32,
        object: u64,
        prototype: u32,
        array: bool,
        capacity: u32,
    ) -> Result<(), NativeGcError> {
        let native_tlab = self.native_tlab.borrow();
        let Some(window) = native_tlab.as_ref() else {
            return Err(NativeGcError::Invariant(
                "native TLAB is unavailable for fast allocation".into(),
            ));
        };
        if context.bump_limit != window.reservation.object_limit()
            || context.bump_handle_limit != window.reservation.handle_limit()
        {
            return Err(NativeGcError::Invariant(
                "native TLAB vmctx reservation is stale".into(),
            ));
        }
        self.heap.publish_native_tlab_object(
            &window.reservation,
            handle,
            object,
            prototype,
            array,
            capacity,
        )?;
        Ok(())
    }
    pub(super) fn adopt_native_tlab_cursor(
        &self,
        context: &NativeVmContext,
    ) -> Result<(), NativeGcError> {
        let mut native_tlab = self.native_tlab.borrow_mut();
        let Some(window) = native_tlab.as_mut() else {
            return Ok(());
        };
        if context.bump_limit != window.reservation.object_limit()
            || context.bump_handle_limit != window.reservation.handle_limit()
            || context.bump_ptr < window.top
            || context.bump_handle_cursor < window.next_handle
            || context.bump_ptr > context.bump_limit
            || context.bump_handle_cursor > context.bump_handle_limit
        {
            return Err(NativeGcError::Invariant(
                "native TLAB vmctx cursor is outside owner reservation".into(),
            ));
        }
        window.top = context.bump_ptr;
        window.next_handle = context.bump_handle_cursor;
        Ok(())
    }

    pub(super) fn commit_native_tlab_cursor(&self, context: &NativeVmContext) {
        if let Some(window) = self.native_tlab.borrow_mut().as_mut() {
            window.top = context.bump_ptr;
            window.next_handle = context.bump_handle_cursor;
        }
    }

    pub(super) fn allocation_diagnostics_slow_allocation(&self) {
        self.allocation_diagnostics.record_slow_allocation();
    }
    pub(super) fn record_inline_string(&self) {
        self.allocation_diagnostics
            .record(&self.allocation_diagnostics.inline_string_constructions, 1);
    }

    pub(super) fn record_inline_property_key(&self) {
        self.allocation_diagnostics
            .record(&self.allocation_diagnostics.inline_property_keys, 1);
    }
    pub(super) fn reset_native_tlab(&self) {
        if let Some(window) = self.native_tlab.replace(None) {
            let _ = self.heap.release_native_tlab_if_empty(&window.reservation);
        }
    }

    pub(super) fn bind_context(
        &self,
        context: &mut NativeVmContext,
        object_prototype: Option<u32>,
        array_prototype: Option<u32>,
    ) -> Result<(), NativeGcError> {
        self.sync_barrier_state();
        context.handle_table_base = self.heap.handle_table_base();
        context.heap_object_delta = self.heap.object_address_delta();
        context.gc_state = std::ptr::from_ref(self).cast_mut().cast();
        context.allocation_state = std::ptr::from_ref(&self.nlab).cast_mut().cast();
        context.barrier_state = std::ptr::from_ref(self.barrier_state.as_ref())
            .cast_mut()
            .cast();
        context.object_prototype_handle = object_prototype.unwrap_or(u32::MAX);
        context.array_prototype_handle = array_prototype.unwrap_or(u32::MAX);
        self.allocation_prototypes_ready
            .set(object_prototype.is_some() && array_prototype.is_some());
        self.reserve_native_tlab()?;
        self.sync_native_tlab_context(context);
        let _ = self.mutator.participant_id();
        Ok(())
    }

    pub(super) fn sync_native_tlab_context(&self, context: &mut NativeVmContext) {
        let native_tlab = self.native_tlab.borrow();
        let Some(window) = native_tlab.as_ref() else {
            context.bump_ptr = 0;
            context.bump_limit = 0;
            context.bump_handle_cursor = 0;
            context.bump_handle_limit = 0;
            context.allocation_fast_flags = 0;
            context.allocation_small_limit = 0;
            return;
        };
        context.bump_ptr = window.top;
        context.bump_limit = window.reservation.object_limit();
        context.bump_handle_cursor = window.next_handle;
        context.bump_handle_limit = window.reservation.handle_limit();
        context.allocation_small_limit = window.reservation.small_object_limit();
        let mut flags = self.native_allocation_flags();
        if context.object_prototype_handle == u32::MAX {
            flags &= !NATIVE_ALLOCATION_FAST_OBJECT;
        }
        if context.array_prototype_handle == u32::MAX {
            flags &= !NATIVE_ALLOCATION_FAST_ARRAY;
        }
        context.allocation_fast_flags = flags;
    }

    /// Dispatcher 或 GC 完成后，在当前 window 已耗尽时建立下一页和 handle 区间。
    pub(super) fn native_tlab_needs_refill(&self, context: &NativeVmContext) -> bool {
        self.native_tlab.borrow().as_ref().is_none_or(|window| {
            context.bump_ptr >= window.reservation.object_limit()
                || context.bump_handle_cursor >= window.reservation.handle_limit()
        })
    }

    /// Dispatcher 或 GC 完成后，在当前 window 已耗尽时建立下一页和 handle 区间。
    pub(super) fn refill_native_tlab_if_exhausted(
        &self,
        context: &mut NativeVmContext,
    ) -> Result<(), NativeGcError> {
        if !self.native_tlab_needs_refill(context) {
            self.sync_native_tlab_context(context);
            return Ok(());
        }
        self.flush_native_tlab(context)?;
        if self.native_allocation_allowed() {
            self.pacing_poll_requested.set(true);
            match &self.collector {
                NativeCollector::StopTheWorld(collector) => {
                    collector.observe_allocation(PAGE_GRANULE_BYTES, Duration::from_micros(10));
                }
                NativeCollector::Zgc(collector) => {
                    collector.observe_allocation(PAGE_GRANULE_BYTES, Duration::from_micros(10));
                }
            }
            self.reset_native_tlab();
            self.reserve_native_tlab()?;
        }
        self.sync_native_tlab_context(context);
        Ok(())
    }

    pub(super) fn should_collect_before_native_tlab_refill(&self) -> bool {
        matches!(self.collector, NativeCollector::Zgc(_))
            && self.native_tlab.borrow().is_some()
            && self.heap.free_pages() <= 8
    }

    fn native_allocation_allowed(&self) -> bool {
        self.barrier_state.phase.load(Ordering::Acquire) & NATIVE_BARRIER_MARKING_MASK == 0
    }
    fn native_allocation_flags(&self) -> u32 {
        if !self.allocation_prototypes_ready.get() || !self.native_allocation_allowed() {
            return 0;
        }
        NATIVE_ALLOCATION_FAST_HOST | NATIVE_ALLOCATION_FAST_OBJECT | NATIVE_ALLOCATION_FAST_ARRAY
    }

    pub(super) fn host_fast_allocation_allowed(&self) -> bool {
        self.native_allocation_allowed()
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
        self.allocation_prototypes_ready.set(false);
        self.reset_native_tlab();
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
        context: &mut NativeVmContext,
        action: GcSafepointAction,
        snapshot: Option<RootSnapshot>,
    ) -> Result<Option<RuntimeGcReport>, NativeGcError> {
        let requires_flush = !matches!(action, GcSafepointAction::Idle);
        if requires_flush {
            self.flush_native_tlab(context)?;
        }
        let requires_reset = snapshot.is_some() || matches!(action, GcSafepointAction::FinishCycle);
        if requires_reset {
            self.reset_native_tlab();
            if snapshot.is_some() {
                self.reset_nlab();
            }
        }
        let report = match &mut self.collector {
            NativeCollector::StopTheWorld(collector) => snapshot
                .as_ref()
                .map(|snapshot| collector.collect(self.heap.collector_capability(), snapshot))
                .transpose()?,
            NativeCollector::Zgc(collector) => collector.at_safepoint(snapshot)?,
        };
        self.sync_barrier_state();
        if requires_reset {
            self.reserve_native_tlab()?;
            self.sync_native_tlab_context(context);
        }
        Ok(report)
    }

    pub(super) fn collect_full(
        &mut self,
        context: &mut NativeVmContext,
        snapshot: RootSnapshot,
    ) -> Result<RuntimeGcReport, NativeGcError> {
        self.flush_native_tlab(context)?;
        self.reset_native_tlab();
        self.reset_nlab();
        let report = match &mut self.collector {
            NativeCollector::StopTheWorld(collector) => {
                collector.collect(self.heap.collector_capability(), &snapshot)?
            }
            NativeCollector::Zgc(collector) => collector.collect_full(snapshot)?,
        };
        self.reset_nlab();
        self.sync_barrier_state();
        self.reserve_native_tlab()?;
        self.sync_native_tlab_context(context);
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

    pub(super) fn allocation_diagnostics(&self) -> NativeAllocationDiagnostics {
        self.allocation_diagnostics.snapshot()
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
    #[error("native GC invariant violated: {0}")]
    Invariant(String),
}
