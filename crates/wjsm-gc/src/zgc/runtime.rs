//! 生产并发分代 ZGC 的唯一算法 owner。

use std::collections::{BTreeSet, VecDeque};
use std::error::Error;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use parking_lot::{Mutex, RwLock};
use wjsm_ir::value;

use crate::api::{CycleKind, GcStats};
use crate::heap::{GrowableHeapMemory, HandleGeneration, PageStats};
use crate::telemetry::{GcTelemetry, GcTelemetrySnapshot};
use crate::worker::{GcPacketKind, GcWorkPacket, GcWorkerPool, WorkerPoolError};
use crate::{HeapAccessV2, HeapAccessV2Error, RootSnapshot, RuntimeGcReport};

use super::{
    BarrierRecord, DirectorDecision, DirectorGeneration, GcDirector, HeapBarrier,
    RelocationDescriptor,
};

const ROOTS_PER_PACKET: usize = 64;
const OLD_PACKET_EPOCH_BIT: u64 = 1 << 63;
const OLD_MARK_PACKET_BUDGET: usize = 64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GcSafepointAction {
    Idle,
    PublishRoots { epoch: u64 },
    FlushBarriers,
    Assist { work_bytes: u64 },
    FinishCycle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MarkGeneration {
    Young,
    Old,
    Full,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RuntimePhase {
    Idle,
    AwaitRoots {
        epoch: u64,
        generation: MarkGeneration,
    },
    ConcurrentMark,
    ConcurrentSelectRelocationSet,
    ConcurrentRelocate,
    EpochReclaim,
    OldMarkReady,
}

struct RuntimeState {
    phase: RuntimePhase,
    generation: MarkGeneration,
    snapshot: Option<Arc<RootSnapshot>>,
    pending: VecDeque<i64>,
    live_host_values: BTreeSet<i64>,
    visited_edge_owners: BTreeSet<i64>,
    marked_handles: usize,
    bridge_handles: BTreeSet<u32>,
    marked_bytes: u64,
    started_at: Option<Instant>,
    worker_error: Option<String>,
    pages: Vec<PageStats>,
    selected_pages: usize,
    relocation_handles: Vec<u32>,
    relocation: Vec<Arc<RelocationDescriptor>>,
    retired_handles: Vec<u32>,
    promoted_handles: Vec<u32>,
    relocated_bytes: u64,
    freed_bytes: u64,
}

impl RuntimeState {
    fn new(packet_capacity: usize) -> Self {
        Self {
            phase: RuntimePhase::Idle,
            generation: MarkGeneration::Young,
            snapshot: None,
            pending: VecDeque::with_capacity(packet_capacity),
            live_host_values: BTreeSet::new(),
            visited_edge_owners: BTreeSet::new(),
            marked_handles: 0,
            marked_bytes: 0,
            started_at: None,
            bridge_handles: BTreeSet::new(),
            worker_error: None,
            pages: Vec::new(),
            selected_pages: 0,
            relocation_handles: Vec::with_capacity(packet_capacity),
            relocation: Vec::with_capacity(packet_capacity),
            retired_handles: Vec::with_capacity(packet_capacity),
            promoted_handles: Vec::with_capacity(packet_capacity),
            relocated_bytes: 0,
            freed_bytes: 0,
        }
    }
}

struct OldMarkState {
    active: bool,
    snapshot: Option<Arc<RootSnapshot>>,
    pending: VecDeque<i64>,
    live_host_values: BTreeSet<i64>,
    bridge_handles: BTreeSet<u32>,
    marked_handles: usize,
    marked_bytes: u64,
    started_at: Option<Instant>,
    worker_error: Option<String>,
}

impl OldMarkState {
    fn new(packet_capacity: usize) -> Self {
        Self {
            active: false,
            snapshot: None,
            pending: VecDeque::with_capacity(packet_capacity),
            live_host_values: BTreeSet::new(),
            bridge_handles: BTreeSet::new(),
            marked_handles: 0,
            marked_bytes: 0,
            started_at: None,
            worker_error: None,
        }
    }
}

struct RuntimeShared<M: GrowableHeapMemory> {
    heap: RwLock<Arc<HeapAccessV2<M>>>,
    state: Mutex<RuntimeState>,
    old: Mutex<OldMarkState>,
    remembered_slots: Mutex<BTreeSet<u64>>,
    remembered_owners: Mutex<BTreeSet<u32>>,
    main_inflight: AtomicU64,
    old_inflight: AtomicU64,
}
impl<M: GrowableHeapMemory> RuntimeShared<M> {
    fn process_packet(&self, worker_id: usize, packet: GcWorkPacket) {
        let old_packet = packet.epoch() & OLD_PACKET_EPOCH_BIT != 0;
        if let Err(error) = self.process_packet_inner(worker_id, packet) {
            if old_packet {
                self.old
                    .lock()
                    .worker_error
                    .get_or_insert_with(|| error.to_string());
            } else {
                self.state
                    .lock()
                    .worker_error
                    .get_or_insert_with(|| error.to_string());
            }
        }
        let inflight = if old_packet {
            &self.old_inflight
        } else {
            &self.main_inflight
        };
        inflight.fetch_sub(1, Ordering::SeqCst);
    }

    fn process_packet_inner(
        &self,
        worker_id: usize,
        packet: GcWorkPacket,
    ) -> Result<(), GenerationalZgcError> {
        if packet.epoch() & OLD_PACKET_EPOCH_BIT != 0 {
            if packet.kind() != GcPacketKind::BitmapWordRange {
                return Err(GenerationalZgcError::InvalidOldMarkPacket);
            }
            return self.drain_old_mark_queue();
        }
        match packet.kind() {
            GcPacketKind::RootRange => {
                let roots = {
                    let state = self.state.lock();
                    let snapshot = state
                        .snapshot
                        .as_ref()
                        .ok_or(GenerationalZgcError::MissingRootSnapshot)?;
                    let start = usize::try_from(packet.start())
                        .map_err(|_| GenerationalZgcError::Capacity("root packet start"))?;
                    let end = start
                        .checked_add(packet.len() as usize)
                        .ok_or(GenerationalZgcError::Capacity("root packet end"))?;
                    snapshot
                        .roots()
                        .get(start..end)
                        .ok_or(GenerationalZgcError::InvalidRootPacket)?
                        .to_vec()
                };
                self.state.lock().pending.extend(roots);
                self.drain_mark_queue()
            }
            GcPacketKind::PageRange => {
                let start = usize::try_from(packet.start())
                    .map_err(|_| GenerationalZgcError::Capacity("page packet start"))?;
                let end = start
                    .checked_add(packet.len() as usize)
                    .ok_or(GenerationalZgcError::Capacity("page packet end"))?;
                for index in start..end {
                    self.process_page(index)?;
                }
                Ok(())
            }
            GcPacketKind::RelocationRange => {
                let start = usize::try_from(packet.start())
                    .map_err(|_| GenerationalZgcError::Capacity("relocation packet start"))?;
                let end = start
                    .checked_add(packet.len() as usize)
                    .ok_or(GenerationalZgcError::Capacity("relocation packet end"))?;
                for index in start..end {
                    let descriptor = {
                        let state = self.state.lock();
                        Arc::clone(
                            state
                                .relocation
                                .get(index)
                                .ok_or(GenerationalZgcError::InvalidRelocationPacket)?,
                        )
                    };
                    self.heap
                        .read()
                        .relocate_descriptor(&descriptor, worker_id as u64)?;
                }
                Ok(())
            }
            GcPacketKind::BitmapWordRange => self.drain_mark_queue(),
        }
    }

    fn process_page(&self, index: usize) -> Result<(), GenerationalZgcError> {
        let (page, collection) = {
            let state = self.state.lock();
            (
                *state
                    .pages
                    .get(index)
                    .ok_or(GenerationalZgcError::InvalidPagePacket)?,
                state.generation,
            )
        };
        let heap = self.heap.read();
        let handles = heap.handles_in_page(page.page)?;
        let mut target_allocated = 0_u64;
        let mut target_live = 0_u64;
        let mut live = Vec::new();
        let mut dead = Vec::new();
        let mut exclusive = true;
        for handle in handles {
            let Some(generation) = heap.handle_generation(handle) else {
                continue;
            };
            if !collection.includes(generation) {
                exclusive = false;
                continue;
            }
            let bytes = heap.object_size(handle)?;
            target_allocated = target_allocated.saturating_add(bytes);
            if heap.is_marked_handle(handle, generation)? {
                target_live = target_live.saturating_add(bytes);
                live.push((handle, generation, bytes));
            } else {
                dead.push((handle, bytes));
            }
        }
        if target_allocated == 0 {
            return Ok(());
        }
        let dead_bytes = target_allocated.saturating_sub(target_live);
        let sparse =
            !page.dedicated && exclusive && dead_bytes.saturating_mul(4) >= target_allocated;
        let mut relocated = Vec::new();
        let mut promoted = Vec::new();
        for (handle, generation, _) in live {
            if generation == HandleGeneration::Young {
                let object = heap.resolve_handle(handle)?;
                let age = heap.object_age_at(object)?.saturating_add(1);
                heap.set_object_age(handle, age)?;
                if age >= 2 || page.dedicated || !sparse {
                    heap.promote_to_old(handle)?;
                    promoted.push(handle);
                    continue;
                }
            }
            if sparse {
                relocated.push(handle);
            }
        }
        {
            let mut old = self.old.lock();
            if old.active {
                old.pending.extend(
                    promoted
                        .iter()
                        .map(|handle| value::encode_object_handle(*handle)),
                );
            }
        }
        if !relocated.is_empty() {
            if let HeapBarrier::Zgc(barrier) = heap.barrier() {
                barrier.relocator().select_page(u64::from(page.page.get()));
            }
        }
        drop(heap);
        self.remembered_owners
            .lock()
            .extend(promoted.iter().copied());
        let mut state = self.state.lock();
        if !relocated.is_empty() {
            state.selected_pages = state.selected_pages.saturating_add(1);
            state.relocation_handles.extend(relocated);
        }
        state.promoted_handles.extend(promoted);
        state.freed_bytes = state
            .freed_bytes
            .saturating_add(dead.iter().map(|(_, bytes)| *bytes).sum::<u64>());
        state
            .retired_handles
            .extend(dead.into_iter().map(|(handle, _)| handle));
        Ok(())
    }

    fn drain_mark_queue(&self) -> Result<(), GenerationalZgcError> {
        loop {
            let encoded = self.state.lock().pending.pop_front();
            if let Some(encoded) = encoded {
                self.mark_encoded(encoded)?;
                continue;
            }
            if !self.activate_ephemerons()? {
                return Ok(());
            }
        }
    }
    fn enqueue_snapshot_edges(&self, encoded: i64) -> Result<(), GenerationalZgcError> {
        let snapshot = {
            let mut state = self.state.lock();
            if !state.visited_edge_owners.insert(encoded) {
                return Ok(());
            }
            if !value::is_object(encoded) && !value::is_array(encoded) {
                state.live_host_values.insert(encoded);
            }
            Arc::clone(
                state
                    .snapshot
                    .as_ref()
                    .ok_or(GenerationalZgcError::MissingRootSnapshot)?,
            )
        };
        let edges = snapshot.strong_edges();
        let start = edges.partition_point(|edge| edge.owner < encoded);
        let end = edges.partition_point(|edge| edge.owner <= encoded);
        self.state
            .lock()
            .pending
            .extend(edges[start..end].iter().map(|edge| edge.target));
        Ok(())
    }

    fn mark_encoded(&self, encoded: i64) -> Result<(), GenerationalZgcError> {
        let encoded = value::strip_gc_color(encoded);
        if !value::is_handle_backed_reference(encoded) {
            return Ok(());
        }
        let handle = value::decode_handle(encoded);
        self.enqueue_snapshot_edges(encoded)?;
        let heap = self.heap.read();
        let generation = self.state.lock().generation;
        if value::is_object(encoded) || value::is_array(encoded) {
            let Some(object_generation) = heap.handle_generation(handle) else {
                return Ok(());
            };
            if !generation.includes(object_generation) {
                if !self.state.lock().bridge_handles.insert(handle) {
                    return Ok(());
                }
                heap.scan_references(handle, |reference| {
                    if value::is_handle_backed_reference(reference) {
                        self.state.lock().pending.push_back(reference);
                    }
                })?;
                return Ok(());
            }
            if !heap.try_mark_handle(handle)? {
                return Ok(());
            }
            let bytes = heap.object_size(handle)?;
            {
                let mut state = self.state.lock();
                state.marked_handles = state.marked_handles.saturating_add(1);
                state.marked_bytes = state.marked_bytes.saturating_add(bytes);
            }
            heap.scan_references(handle, |reference| {
                if value::is_handle_backed_reference(reference) {
                    self.state.lock().pending.push_back(reference);
                }
            })?;
            return Ok(());
        }
        drop(heap);
        self.enqueue_snapshot_edges(encoded)
    }

    fn activate_ephemerons(&self) -> Result<bool, GenerationalZgcError> {
        let snapshot = {
            let state = self.state.lock();
            Arc::clone(
                state
                    .snapshot
                    .as_ref()
                    .ok_or(GenerationalZgcError::MissingRootSnapshot)?,
            )
        };
        let mut activated = Vec::new();
        for ephemeron in snapshot.ephemerons() {
            if self.value_is_live(ephemeron.owner)?
                && self.value_is_live(ephemeron.key)?
                && !self.value_is_live(ephemeron.value)?
            {
                activated.push(ephemeron.value);
            }
        }
        if activated.is_empty() {
            return Ok(false);
        }
        self.state.lock().pending.extend(activated);
        Ok(true)
    }

    fn value_is_live(&self, encoded: i64) -> Result<bool, GenerationalZgcError> {
        let encoded = value::strip_gc_color(encoded);
        if !value::is_handle_backed_reference(encoded) {
            return Ok(true);
        }
        if value::is_object(encoded) || value::is_array(encoded) {
            let heap = self.heap.read();
            let handle = value::decode_handle(encoded);

            let Some(generation) = heap.handle_generation(handle) else {
                return Ok(false);
            };
            return heap
                .is_marked_handle(handle, generation)
                .map_err(GenerationalZgcError::Heap);
        }
        Ok(self.state.lock().live_host_values.contains(&encoded))
    }
    fn route_assist_value(&self, heap: &HeapAccessV2<M>, encoded: i64) {
        let encoded = value::strip_gc_color(encoded);
        let main_generation = {
            let state = self.state.lock();
            (state.phase == RuntimePhase::ConcurrentMark).then_some(state.generation)
        };
        let old_active = self.old.lock().active;
        if value::is_object(encoded) || value::is_array(encoded) {
            if let Some(generation) = heap.handle_generation(value::decode_handle(encoded)) {
                if main_generation.is_some_and(|mark| mark.includes(generation)) {
                    self.state.lock().pending.push_back(encoded);
                }
                if old_active && generation == HandleGeneration::Old {
                    self.old.lock().pending.push_back(encoded);
                }
            }
        } else if value::is_handle_backed_reference(encoded) {
            if main_generation.is_some() {
                self.state.lock().pending.push_back(encoded);
            }
            if old_active {
                self.old.lock().pending.push_back(encoded);
            }
        }
    }

    fn ingest_barrier_record(
        &self,
        heap: &HeapAccessV2<M>,
        record: BarrierRecord,
    ) -> Result<(), GenerationalZgcError> {
        match record {
            BarrierRecord::Satb(encoded) | BarrierRecord::Mark(encoded) => {
                self.route_assist_value(heap, encoded);
            }
            BarrierRecord::RememberedSlot { slot_addr } => {
                self.remembered_slots.lock().insert(slot_addr);
                if let Ok(encoded) = heap.load_reference_slot(slot_addr) {
                    self.route_assist_value(heap, encoded);
                }
            }
            BarrierRecord::RememberedObject(handle) => {
                self.remembered_owners.lock().insert(handle.get());
                if heap.handle_generation(handle.get()).is_some() {
                    heap.scan_references(handle.get(), |encoded| {
                        self.route_assist_value(heap, encoded);
                    })?;
                }
            }
        }
        Ok(())
    }

    fn assist_barrier_record(&self, record: BarrierRecord, work_bytes: u64) -> bool {
        let result = (|| {
            let heap = self.heap.read();
            self.ingest_barrier_record(&heap, record)?;
            drop(heap);
            let budget = usize::try_from(work_bytes / 8).unwrap_or(usize::MAX).max(1);
            for _ in 0..budget {
                let main_encoded = self.state.lock().pending.pop_front();
                if let Some(encoded) = main_encoded {
                    self.mark_encoded(encoded)?;
                    continue;
                }
                let old_encoded = self.old.lock().pending.pop_front();
                if let Some(encoded) = old_encoded {
                    self.mark_old_encoded(encoded)?;
                    continue;
                }
                break;
            }
            Ok::<(), GenerationalZgcError>(())
        })();
        if let Err(error) = result {
            self.state
                .lock()
                .worker_error
                .get_or_insert_with(|| error.to_string());
            false
        } else {
            true
        }
    }

    fn drain_old_mark_queue(&self) -> Result<(), GenerationalZgcError> {
        for _ in 0..OLD_MARK_PACKET_BUDGET {
            let encoded = self.old.lock().pending.pop_front();
            if let Some(encoded) = encoded {
                self.mark_old_encoded(encoded)?;
                continue;
            }
            if !self.activate_old_ephemerons()? {
                break;
            }
        }
        Ok(())
    }

    fn mark_old_encoded(&self, encoded: i64) -> Result<(), GenerationalZgcError> {
        let encoded = value::strip_gc_color(encoded);
        if !value::is_handle_backed_reference(encoded) {
            return Ok(());
        }
        let handle = value::decode_handle(encoded);
        let heap = self.heap.read();
        if value::is_object(encoded) || value::is_array(encoded) {
            let Some(generation) = heap.handle_generation(handle) else {
                return Ok(());
            };
            if generation == HandleGeneration::Young {
                if !self.old.lock().bridge_handles.insert(handle) {
                    return Ok(());
                }
                heap.scan_references(handle, |reference| {
                    if value::is_handle_backed_reference(reference) {
                        self.old.lock().pending.push_back(reference);
                    }
                })?;
                return Ok(());
            }
            if !heap.try_mark_handle(handle)? {
                return Ok(());
            }
            let bytes = heap.object_size(handle)?;
            {
                let mut old = self.old.lock();
                old.marked_handles = old.marked_handles.saturating_add(1);
                old.marked_bytes = old.marked_bytes.saturating_add(bytes);
            }
            heap.scan_references(handle, |reference| {
                if value::is_handle_backed_reference(reference) {
                    self.old.lock().pending.push_back(reference);
                }
            })?;
            return Ok(());
        }
        drop(heap);

        let snapshot = {
            let mut old = self.old.lock();
            if !old.live_host_values.insert(encoded) {
                return Ok(());
            }
            Arc::clone(
                old.snapshot
                    .as_ref()
                    .ok_or(GenerationalZgcError::MissingRootSnapshot)?,
            )
        };
        let edges = snapshot.strong_edges();
        let start = edges.partition_point(|edge| edge.owner < encoded);
        let end = edges.partition_point(|edge| edge.owner <= encoded);
        self.old
            .lock()
            .pending
            .extend(edges[start..end].iter().map(|edge| edge.target));
        Ok(())
    }

    fn activate_old_ephemerons(&self) -> Result<bool, GenerationalZgcError> {
        let snapshot = {
            let old = self.old.lock();
            Arc::clone(
                old.snapshot
                    .as_ref()
                    .ok_or(GenerationalZgcError::MissingRootSnapshot)?,
            )
        };
        let mut activated = Vec::new();
        for ephemeron in snapshot.ephemerons() {
            if self.old_value_is_live(ephemeron.key)? && !self.old_value_is_live(ephemeron.value)? {
                activated.push(ephemeron.value);
            }
        }
        if activated.is_empty() {
            return Ok(false);
        }
        self.old.lock().pending.extend(activated);
        Ok(true)
    }

    fn old_value_is_live(&self, encoded: i64) -> Result<bool, GenerationalZgcError> {
        let encoded = value::strip_gc_color(encoded);
        if !value::is_handle_backed_reference(encoded) {
            return Ok(true);
        }
        if value::is_object(encoded) || value::is_array(encoded) {
            let heap = self.heap.read();
            let handle = value::decode_handle(encoded);
            return match heap.handle_generation(handle) {
                Some(HandleGeneration::Young) => {
                    Ok(self.old.lock().bridge_handles.contains(&handle))
                }
                Some(HandleGeneration::Old) => heap
                    .is_marked_handle(handle, HandleGeneration::Old)
                    .map_err(GenerationalZgcError::Heap),
                None => Ok(false),
            };
        }
        Ok(self.old.lock().live_host_values.contains(&encoded))
    }
}

impl MarkGeneration {
    fn includes(self, generation: HandleGeneration) -> bool {
        match self {
            Self::Young => generation == HandleGeneration::Young,
            Self::Old => generation == HandleGeneration::Old,
            Self::Full => true,
        }
    }

    fn cycle_kind(self) -> CycleKind {
        match self {
            Self::Young => CycleKind::Young,
            Self::Old => CycleKind::ZgcCycle,
            Self::Full => CycleKind::Full,
        }
    }
}

pub struct GenerationalZgc<M: GrowableHeapMemory + Clone + Send + Sync + 'static> {
    shared: Arc<RuntimeShared<M>>,
    workers: GcWorkerPool,
    director: Mutex<GcDirector>,
    telemetry: GcTelemetry,
    next_epoch: AtomicU64,
    shutdown: AtomicBool,
}

impl<M: GrowableHeapMemory + Clone + Send + Sync + 'static> GenerationalZgc<M> {
    pub fn new(
        heap: Arc<HeapAccessV2<M>>,
        worker_count: usize,
        packet_capacity: usize,
    ) -> Result<Self, GenerationalZgcError> {
        if !matches!(heap.barrier(), HeapBarrier::Zgc(_)) {
            return Err(GenerationalZgcError::BarrierDisabled);
        }
        let shared = Arc::new(RuntimeShared {
            heap: RwLock::new(heap),
            state: Mutex::new(RuntimeState::new(packet_capacity)),
            remembered_slots: Mutex::new(BTreeSet::new()),
            remembered_owners: Mutex::new(BTreeSet::new()),
            old: Mutex::new(OldMarkState::new(packet_capacity)),
            main_inflight: AtomicU64::new(0),
            old_inflight: AtomicU64::new(0),
        });
        if let HeapBarrier::Zgc(barrier) = shared.heap.read().barrier() {
            let weak = Arc::downgrade(&shared);
            barrier.install_assist(Arc::new(move |record, work_bytes| {
                weak.upgrade()
                    .is_some_and(|shared| shared.assist_barrier_record(record, work_bytes))
            }));
        }
        let worker_shared = Arc::clone(&shared);
        let workers =
            GcWorkerPool::new(worker_count, packet_capacity, move |worker_id, packet| {
                worker_shared.process_packet(worker_id, packet);
            })?;
        Ok(Self {
            shared,
            workers,
            director: Mutex::new(GcDirector::new()),
            telemetry: GcTelemetry::default(),
            next_epoch: AtomicU64::new(0),
            shutdown: AtomicBool::new(false),
        })
    }

    pub fn reset_heap(&self, heap: Arc<HeapAccessV2<M>>) -> Result<(), GenerationalZgcError> {
        if !matches!(heap.barrier(), HeapBarrier::Zgc(_)) {
            return Err(GenerationalZgcError::BarrierDisabled);
        }
        let idle = self.shared.state.lock().phase == RuntimePhase::Idle
            && !self.shared.old.lock().active
            && self.shared.main_inflight.load(Ordering::SeqCst) == 0
            && self.shared.old_inflight.load(Ordering::SeqCst) == 0;
        if !idle || !barrier_is_empty(&self.shared.heap.read()) {
            return Err(GenerationalZgcError::CollectorBusy);
        }
        *self.shared.heap.write() = heap;
        self.shared.remembered_slots.lock().clear();
        self.shared.remembered_owners.lock().clear();
        if let HeapBarrier::Zgc(barrier) = self.shared.heap.read().barrier() {
            let weak = Arc::downgrade(&self.shared);
            barrier.install_assist(Arc::new(move |record, work_bytes| {
                weak.upgrade()
                    .is_some_and(|shared| shared.assist_barrier_record(record, work_bytes))
            }));
        }
        Ok(())
    }

    pub fn cycle_active(&self) -> bool {
        self.shared.state.lock().phase != RuntimePhase::Idle
    }
    pub fn mark_black_allocation(&self, handle: u32) -> Result<(), HeapAccessV2Error> {
        let should_mark = {
            let state = self.shared.state.lock();
            matches!(
                state.phase,
                RuntimePhase::ConcurrentMark | RuntimePhase::ConcurrentSelectRelocationSet
            ) && state.generation.includes(HandleGeneration::Young)
        };
        if !should_mark {
            return Ok(());
        }
        let heap = self.shared.heap.read();
        if heap.try_mark_handle(handle)? {
            let bytes = heap.object_size(handle)?;
            let mut state = self.shared.state.lock();
            state.marked_handles = state.marked_handles.saturating_add(1);
            state.marked_bytes = state.marked_bytes.saturating_add(bytes);
        }
        Ok(())
    }
    pub fn record_host_write(&self, _owner: i64, old: Option<i64>, new: Option<i64>) {
        let phase = self.shared.state.lock().phase;
        if !matches!(
            phase,
            RuntimePhase::ConcurrentMark | RuntimePhase::ConcurrentSelectRelocationSet
        ) {
            return;
        }
        for encoded in old.into_iter().chain(new) {
            let encoded = value::strip_gc_color(encoded);
            if !value::is_handle_backed_reference(encoded) {
                continue;
            }
            if value::is_object(encoded) || value::is_array(encoded) {
                self.shared.state.lock().pending.push_back(encoded);
            } else {
                self.shared.state.lock().live_host_values.insert(encoded);
            }
        }
    }

    pub fn observe_allocation(&self, bytes: u64, elapsed: Duration) {
        self.telemetry.record_allocation(bytes);
        self.director
            .lock()
            .observe_allocation(DirectorGeneration::Young, bytes, elapsed);
    }

    pub fn safepoint_action(&self) -> GcSafepointAction {
        let phase = self.shared.state.lock().phase;
        match phase {
            RuntimePhase::Idle => {
                if self.shared.old.lock().active {
                    if !barrier_is_empty(&self.shared.heap.read()) {
                        return GcSafepointAction::FlushBarriers;
                    }
                    if self.old_mark_ready() {
                        self.shared.state.lock().phase = RuntimePhase::OldMarkReady;
                        return GcSafepointAction::FinishCycle;
                    }
                    if let Err(error) = self.continue_old_mark() {
                        self.shared
                            .old
                            .lock()
                            .worker_error
                            .get_or_insert_with(|| error.to_string());
                    }
                }
                let heap = self.shared.heap.read();
                let (young_live, old_live) = heap
                    .generation_bytes()
                    .expect("managed page metadata must resolve every live handle");
                let mut director = self.director.lock();
                director.update_space(heap.free_bytes(), 0);
                let generation = match director.evaluate(young_live, old_live) {
                    DirectorDecision::StartYoung => MarkGeneration::Young,
                    DirectorDecision::StartOld => MarkGeneration::Old,
                    DirectorDecision::Idle | DirectorDecision::Continue => {
                        return GcSafepointAction::Idle;
                    }
                };
                drop(director);
                drop(heap);
                let epoch = self.next_epoch.fetch_add(1, Ordering::SeqCst) + 1;
                self.shared.state.lock().phase = RuntimePhase::AwaitRoots { epoch, generation };
                GcSafepointAction::PublishRoots { epoch }
            }
            RuntimePhase::AwaitRoots { epoch, .. } => GcSafepointAction::PublishRoots { epoch },
            RuntimePhase::ConcurrentMark => {
                if !barrier_is_empty(&self.shared.heap.read()) {
                    GcSafepointAction::FlushBarriers
                } else if self.shared.main_inflight.load(Ordering::SeqCst) == 0
                    && self.shared.state.lock().pending.is_empty()
                {
                    GcSafepointAction::FinishCycle
                } else {
                    GcSafepointAction::Idle
                }
            }
            RuntimePhase::ConcurrentSelectRelocationSet | RuntimePhase::ConcurrentRelocate => {
                if self.shared.main_inflight.load(Ordering::SeqCst) == 0 {
                    GcSafepointAction::FinishCycle
                } else {
                    GcSafepointAction::Idle
                }
            }
            RuntimePhase::EpochReclaim | RuntimePhase::OldMarkReady => {
                GcSafepointAction::FinishCycle
            }
        }
    }
    pub fn at_safepoint(
        &self,
        roots: Option<RootSnapshot>,
    ) -> Result<Option<RuntimeGcReport>, GenerationalZgcError> {
        let phase = self.shared.state.lock().phase;
        match phase {
            RuntimePhase::Idle => {
                if self.shared.old.lock().active {
                    self.flush_barriers()?;
                    self.continue_old_mark()?;
                }
                Ok(None)
            }
            RuntimePhase::AwaitRoots { epoch, generation } => {
                let roots = roots.ok_or(GenerationalZgcError::MissingRootSnapshot)?;
                if roots.epoch() != epoch {
                    return Err(GenerationalZgcError::RootEpochMismatch {
                        expected: epoch,
                        actual: roots.epoch(),
                    });
                }
                if generation == MarkGeneration::Old {
                    self.begin_old_mark(roots)?;
                    self.shared.state.lock().phase = RuntimePhase::Idle;
                } else {
                    self.begin_mark(generation, roots)?;
                }
                Ok(None)
            }
            RuntimePhase::ConcurrentMark => {
                self.flush_barriers()?;
                if self.shared.main_inflight.load(Ordering::SeqCst) != 0 {
                    return Ok(None);
                }
                if !self.shared.state.lock().pending.is_empty() {
                    self.submit_pending_packet()?;
                    return Ok(None);
                }
                self.begin_relocation_selection()?;
                Ok(None)
            }
            RuntimePhase::ConcurrentSelectRelocationSet => {
                self.flush_barriers()?;
                if self.shared.main_inflight.load(Ordering::SeqCst) != 0 {
                    return Ok(None);
                }
                if !self.shared.state.lock().pending.is_empty() {
                    self.submit_pending_packet()?;
                    return Ok(None);
                }
                self.finalize_dead_candidates()?;
                self.begin_relocation()?;
                if self.shared.state.lock().phase == RuntimePhase::EpochReclaim {
                    self.finish_cycle().map(Some)
                } else {
                    Ok(None)
                }
            }
            RuntimePhase::ConcurrentRelocate => {
                if self.shared.main_inflight.load(Ordering::SeqCst) != 0 {
                    return Ok(None);
                }
                self.shared.state.lock().phase = RuntimePhase::EpochReclaim;
                self.finish_cycle().map(Some)
            }
            RuntimePhase::EpochReclaim => self.finish_cycle().map(Some),
            RuntimePhase::OldMarkReady => {
                self.finish_old_mark_to_selection()?;
                Ok(None)
            }
        }
    }

    pub fn collect_full(
        &self,
        roots: RootSnapshot,
    ) -> Result<RuntimeGcReport, GenerationalZgcError> {
        loop {
            let idle = self.shared.state.lock().phase == RuntimePhase::Idle
                && !self.shared.old.lock().active;
            if idle {
                break;
            }
            let roots = match self.safepoint_action() {
                GcSafepointAction::PublishRoots { epoch } => Some(roots.with_epoch(epoch)),
                GcSafepointAction::Idle
                | GcSafepointAction::FlushBarriers
                | GcSafepointAction::Assist { .. }
                | GcSafepointAction::FinishCycle => None,
            };
            let _ = self.at_safepoint(roots)?;
            std::thread::yield_now();
        }
        self.begin_mark(MarkGeneration::Full, roots)?;
        loop {
            self.workers.wait_for_idle();
            let phase = self.shared.state.lock().phase;
            match phase {
                RuntimePhase::ConcurrentMark => {
                    self.flush_barriers()?;
                    if !self.shared.state.lock().pending.is_empty()
                        || !barrier_is_empty(&self.shared.heap.read())
                    {
                        self.submit_pending_packet()?;
                    } else {
                        self.begin_relocation_selection()?;
                    }
                }
                RuntimePhase::ConcurrentSelectRelocationSet => self.begin_relocation()?,
                RuntimePhase::ConcurrentRelocate => {
                    self.shared.state.lock().phase = RuntimePhase::EpochReclaim;
                }
                RuntimePhase::EpochReclaim => return self.finish_cycle(),
                RuntimePhase::Idle
                | RuntimePhase::AwaitRoots { .. }
                | RuntimePhase::OldMarkReady => {
                    return Err(GenerationalZgcError::CollectorBusy);
                }
            }
        }
    }

    pub fn telemetry_snapshot(&self) -> GcTelemetrySnapshot {
        self.telemetry.snapshot()
    }

    pub fn reset_telemetry(&self) {
        self.telemetry.reset();
    }

    pub fn shutdown(&self) {
        if !self.shutdown.swap(true, Ordering::SeqCst) {
            self.workers.shutdown();
        }
    }

    fn begin_mark(
        &self,
        generation: MarkGeneration,
        roots: RootSnapshot,
    ) -> Result<(), GenerationalZgcError> {
        let heap = self.shared.heap.read();
        if generation.includes(HandleGeneration::Young) {
            heap.clear_marks(HandleGeneration::Young);
        }
        if generation.includes(HandleGeneration::Old) {
            heap.clear_marks(HandleGeneration::Old);
        }
        if let HeapBarrier::Zgc(barrier) = heap.barrier() {
            let epoch = match generation {
                MarkGeneration::Young => barrier.epoch().flip_young(),
                MarkGeneration::Old | MarkGeneration::Full => barrier.epoch().flip_old(),
            };
            barrier.set_epoch(epoch);
        }
        let mut remembered = Vec::new();
        if generation == MarkGeneration::Young {
            let mut stale_slots = Vec::new();
            {
                let slots = self.shared.remembered_slots.lock();
                for slot in slots.iter().copied() {
                    match heap.load_reference_slot(slot) {
                        Ok(encoded)
                            if value::is_handle_backed_reference(encoded)
                                && heap.handle_generation(value::decode_handle(encoded))
                                    == Some(HandleGeneration::Young) =>
                        {
                            remembered.push(encoded);
                        }
                        Ok(_) | Err(_) => stale_slots.push(slot),
                    }
                }
            }
            if !stale_slots.is_empty() {
                let mut slots = self.shared.remembered_slots.lock();
                for slot in stale_slots {
                    slots.remove(&slot);
                }
            }

            let owners: Vec<_> = self
                .shared
                .remembered_owners
                .lock()
                .iter()
                .copied()
                .collect();
            let mut stale_owners = Vec::new();
            for owner in owners {
                let mut has_young = false;
                if heap.handle_generation(owner) == Some(HandleGeneration::Old) {
                    heap.scan_references(owner, |encoded| {
                        if value::is_handle_backed_reference(encoded)
                            && heap.handle_generation(value::decode_handle(encoded))
                                == Some(HandleGeneration::Young)
                        {
                            has_young = true;
                            remembered.push(encoded);
                        }
                    })?;
                }
                if !has_young {
                    stale_owners.push(owner);
                }
            }
            if !stale_owners.is_empty() {
                let mut owners = self.shared.remembered_owners.lock();
                for owner in stale_owners {
                    owners.remove(&owner);
                }
            }
        }
        drop(heap);

        let roots = Arc::new(roots);
        let root_count = roots.roots().len();
        {
            let mut state = self.shared.state.lock();
            state.phase = RuntimePhase::ConcurrentMark;
            state.generation = generation;
            state.snapshot = Some(roots);
            state.pending.clear();
            state.pending.extend(remembered);
            state.live_host_values.clear();
            state.visited_edge_owners.clear();
            state.bridge_handles.clear();
            state.marked_handles = 0;
            state.marked_bytes = 0;
            state.started_at = Some(Instant::now());
            state.worker_error = None;
        }
        for start in (0..root_count).step_by(ROOTS_PER_PACKET) {
            let len = (root_count - start).min(ROOTS_PER_PACKET);
            self.submit_main_packet(GcWorkPacket::new(
                GcPacketKind::RootRange,
                start as u64,
                len as u32,
                self.next_epoch.load(Ordering::SeqCst),
            ))?;
        }
        if root_count == 0 && !self.shared.state.lock().pending.is_empty() {
            self.submit_pending_packet()?;
        }
        Ok(())
    }
    fn begin_old_mark(&self, roots: RootSnapshot) -> Result<(), GenerationalZgcError> {
        let heap = self.shared.heap.read();
        heap.clear_marks(HandleGeneration::Old);
        let HeapBarrier::Zgc(barrier) = heap.barrier() else {
            return Err(GenerationalZgcError::BarrierDisabled);
        };
        barrier.set_epoch(barrier.epoch().flip_old());
        drop(heap);

        let roots = Arc::new(roots);
        {
            let mut old = self.shared.old.lock();
            old.active = true;
            old.pending.clear();
            old.pending.extend(roots.roots().iter().copied());
            old.snapshot = Some(roots);
            old.live_host_values.clear();
            old.bridge_handles.clear();
            old.marked_handles = 0;
            old.marked_bytes = 0;
            old.started_at = Some(Instant::now());
            old.worker_error = None;
        }
        self.submit_old_mark_packet()
    }

    fn old_mark_ready(&self) -> bool {
        let old = self.shared.old.lock();
        old.active && old.pending.is_empty() && self.shared.old_inflight.load(Ordering::SeqCst) == 0
    }

    fn continue_old_mark(&self) -> Result<(), GenerationalZgcError> {
        let needs_packet = {
            let old = self.shared.old.lock();
            old.active
                && !old.pending.is_empty()
                && self.shared.old_inflight.load(Ordering::SeqCst) == 0
        };
        if needs_packet {
            self.submit_old_mark_packet()?;
        }
        Ok(())
    }

    fn finish_old_mark_to_selection(&self) -> Result<(), GenerationalZgcError> {
        let (snapshot, live_host_values, marked_handles, marked_bytes, started_at, worker_error) = {
            let mut old = self.shared.old.lock();
            old.active = false;
            (
                old.snapshot.take(),
                std::mem::take(&mut old.live_host_values),
                old.marked_handles,
                old.marked_bytes,
                old.started_at.take(),
                old.worker_error.take(),
            )
        };
        {
            let mut state = self.shared.state.lock();
            state.phase = RuntimePhase::ConcurrentMark;
            state.generation = MarkGeneration::Old;
            state.snapshot = snapshot;
            state.pending.clear();
            state.live_host_values = live_host_values;
            state.bridge_handles.clear();
            state.marked_handles = marked_handles;
            state.marked_bytes = marked_bytes;
            state.started_at = started_at;
            state.worker_error = worker_error;
        }
        self.begin_relocation_selection()
    }

    fn flush_barriers(&self) -> Result<(), GenerationalZgcError> {
        let heap = self.shared.heap.read();
        let HeapBarrier::Zgc(barrier) = heap.barrier() else {
            return Err(GenerationalZgcError::BarrierDisabled);
        };
        let main_generation = {
            let state = self.shared.state.lock();
            (state.phase == RuntimePhase::ConcurrentMark).then_some(state.generation)
        };
        let old_active = self.shared.old.lock().active;
        let mut main_pending = Vec::new();
        let mut old_pending = Vec::new();
        let mut remembered_owners = Vec::new();
        let mut route = |encoded: i64| {
            let encoded = value::strip_gc_color(encoded);
            if value::is_object(encoded) || value::is_array(encoded) {
                if let Some(generation) = heap.handle_generation(value::decode_handle(encoded)) {
                    if main_generation.is_some_and(|mark| mark.includes(generation)) {
                        main_pending.push(encoded);
                    }
                    if old_active && generation == HandleGeneration::Old {
                        old_pending.push(encoded);
                    }
                }
            } else if value::is_handle_backed_reference(encoded) {
                if main_generation.is_some() {
                    main_pending.push(encoded);
                }
                if old_active {
                    old_pending.push(encoded);
                }
            }
        };
        barrier.drain_records(|record| match record {
            BarrierRecord::Satb(encoded) | BarrierRecord::Mark(encoded) => route(encoded),
            BarrierRecord::RememberedSlot { slot_addr } => {
                self.shared.remembered_slots.lock().insert(slot_addr);
                if let Ok(encoded) = heap.load_reference_slot(slot_addr) {
                    route(encoded);
                }
            }
            BarrierRecord::RememberedObject(handle) => {
                self.shared.remembered_owners.lock().insert(handle.get());
                remembered_owners.push(handle.get());
            }
        });
        for owner in remembered_owners {
            if heap.handle_generation(owner).is_some() {
                heap.scan_references(owner, &mut route)?;
            }
        }
        drop(route);
        drop(heap);
        if !main_pending.is_empty() {
            self.shared.state.lock().pending.extend(main_pending);
            self.submit_pending_packet()?;
        }
        if !old_pending.is_empty() {
            self.shared.old.lock().pending.extend(old_pending);
            self.continue_old_mark()?;
        }
        Ok(())
    }

    fn submit_main_packet(&self, packet: GcWorkPacket) -> Result<(), GenerationalZgcError> {
        self.shared.main_inflight.fetch_add(1, Ordering::SeqCst);
        if let Err(error) = self.workers.submit(packet) {
            self.shared.main_inflight.fetch_sub(1, Ordering::SeqCst);
            return Err(error.into());
        }
        Ok(())
    }

    fn submit_old_mark_packet(&self) -> Result<(), GenerationalZgcError> {
        self.shared.old_inflight.fetch_add(1, Ordering::SeqCst);
        let packet = GcWorkPacket::new(
            GcPacketKind::BitmapWordRange,
            0,
            1,
            self.next_epoch.load(Ordering::SeqCst) | OLD_PACKET_EPOCH_BIT,
        );
        if let Err(error) = self.workers.submit(packet) {
            self.shared.old_inflight.fetch_sub(1, Ordering::SeqCst);
            return Err(error.into());
        }
        Ok(())
    }

    fn submit_pending_packet(&self) -> Result<(), GenerationalZgcError> {
        self.submit_main_packet(GcWorkPacket::new(
            GcPacketKind::BitmapWordRange,
            0,
            1,
            self.next_epoch.load(Ordering::SeqCst),
        ))?;
        Ok(())
    }

    fn begin_relocation_selection(&self) -> Result<(), GenerationalZgcError> {
        let heap = self.shared.heap.read();
        if let HeapBarrier::Zgc(barrier) = heap.barrier() {
            let epoch = match self.shared.state.lock().generation {
                MarkGeneration::Young => barrier.epoch().end_young(),
                MarkGeneration::Old | MarkGeneration::Full => barrier.epoch().end_old(),
            };
            barrier.set_epoch(epoch);
        }
        let pages = heap.page_stats();
        drop(heap);
        {
            let mut state = self.shared.state.lock();
            if let Some(error) = state.worker_error.take() {
                return Err(GenerationalZgcError::Worker(error));
            }
            state.phase = if pages.is_empty() {
                RuntimePhase::EpochReclaim
            } else {
                RuntimePhase::ConcurrentSelectRelocationSet
            };
            state.pages = pages;
            state.selected_pages = 0;
            state.relocation_handles.clear();
            state.relocation.clear();
            state.retired_handles.clear();
            state.promoted_handles.clear();
            state.relocated_bytes = 0;
            state.freed_bytes = 0;
        }
        let page_count = self.shared.state.lock().pages.len();
        for start in (0..page_count).step_by(ROOTS_PER_PACKET) {
            let len = (page_count - start).min(ROOTS_PER_PACKET);
            self.submit_main_packet(GcWorkPacket::new(
                GcPacketKind::PageRange,
                start as u64,
                len as u32,
                self.next_epoch.load(Ordering::SeqCst),
            ))?;
        }
        Ok(())
    }

    fn finalize_dead_candidates(&self) -> Result<(), GenerationalZgcError> {
        let candidates = self.shared.state.lock().retired_handles.clone();
        let heap = self.shared.heap.read();
        let mut retired = Vec::with_capacity(candidates.len());
        let mut freed_bytes = 0_u64;
        for handle in candidates {
            let Some(generation) = heap.handle_generation(handle) else {
                continue;
            };
            if heap.is_marked_handle(handle, generation)? {
                continue;
            }
            freed_bytes = freed_bytes.saturating_add(heap.retire_handle(handle)?);
            retired.push(handle);
        }
        drop(heap);
        let mut state = self.shared.state.lock();
        state.retired_handles = retired;
        state.freed_bytes = freed_bytes;
        Ok(())
    }

    fn begin_relocation(&self) -> Result<(), GenerationalZgcError> {
        let (handles, selected_pages, generation) = {
            let mut state = self.shared.state.lock();
            if let Some(error) = state.worker_error.take() {
                return Err(GenerationalZgcError::Worker(error));
            }
            (
                state.relocation_handles.clone(),
                state.selected_pages,
                state.generation,
            )
        };
        let heap = self.shared.heap.read();
        if handles.is_empty() || heap.free_pages() < selected_pages as u32 {
            drop(heap);
            let mut state = self.shared.state.lock();
            state.relocation_handles.clear();
            state.selected_pages = 0;
            state.phase = RuntimePhase::EpochReclaim;
            return Ok(());
        }
        let descriptors = heap.prepare_relocation(&handles, selected_pages as u32)?;
        let relocated_bytes = descriptors.iter().map(|descriptor| descriptor.size).sum();
        let HeapBarrier::Zgc(barrier) = heap.barrier() else {
            return Err(GenerationalZgcError::BarrierDisabled);
        };
        let relocation_generation = match generation {
            MarkGeneration::Young => HandleGeneration::Young,
            MarkGeneration::Old | MarkGeneration::Full => HandleGeneration::Old,
        };
        barrier
            .relocator()
            .pause_relocate_start(relocation_generation)
            .map_err(GenerationalZgcError::Relocation)?;
        barrier.publish_access_epoch();
        let relocation_count = descriptors.len();
        {
            let mut state = self.shared.state.lock();
            state.relocation = descriptors;
            state.relocated_bytes = relocated_bytes;
            state.phase = RuntimePhase::ConcurrentRelocate;
        }
        for start in (0..relocation_count).step_by(ROOTS_PER_PACKET) {
            let len = (relocation_count - start).min(ROOTS_PER_PACKET);
            self.submit_main_packet(GcWorkPacket::new(
                GcPacketKind::RelocationRange,
                start as u64,
                len as u32,
                self.next_epoch.load(Ordering::SeqCst),
            ))?;
        }
        Ok(())
    }

    fn finish_cycle(&self) -> Result<RuntimeGcReport, GenerationalZgcError> {
        let heap = self.shared.heap.read();
        heap.finish_relocation_epoch();
        let page_stats = heap.page_stats();
        let mut state = self.shared.state.lock();
        if let Some(error) = state.worker_error.take() {
            return Err(GenerationalZgcError::Worker(error));
        }
        let elapsed = state
            .started_at
            .take()
            .map_or(Duration::ZERO, |start| start.elapsed());
        let generation = state.generation;
        let mut report = RuntimeGcReport {
            retired_handles: state.retired_handles.clone(),
            relocated_handles: state.relocation_handles.clone(),
            promoted_handles: state.promoted_handles.clone(),
            live_host_values: state.live_host_values.iter().copied().collect(),
            cleans_host_tables: generation != MarkGeneration::Young,
            stats: GcStats {
                marked: state.marked_handles,
                swept: state.retired_handles.len(),
                freed_bytes: usize::try_from(state.freed_bytes).unwrap_or(usize::MAX),
                elapsed,
                heap_used_bytes: usize::try_from(heap.used_bytes()).unwrap_or(usize::MAX),
                cycle_kind: generation.cycle_kind(),
                relocated_bytes: usize::try_from(state.relocated_bytes).unwrap_or(usize::MAX),
                relocated_objects: state.relocation_handles.len(),
                committed_pages: page_stats.len(),
                free_bytes_reusable: usize::try_from(heap.free_bytes()).unwrap_or(usize::MAX),
                regions_total: page_stats.len(),
                regions_free: heap.free_pages() as usize,
                mark_live_bytes: state.marked_bytes,
                ..GcStats::default()
            },
        };
        if let HeapBarrier::Zgc(barrier) = heap.barrier() {
            let relocation = barrier.relocator().report();
            report.stats.pause_ns_max = relocation.pause_ns_max;
        }
        state.phase = RuntimePhase::Idle;
        state.snapshot = None;
        state.pending.clear();
        state.live_host_values.clear();
        state.visited_edge_owners.clear();
        state.pages.clear();
        state.relocation_handles.clear();
        state.relocation.clear();
        state.retired_handles.clear();
        state.promoted_handles.clear();
        drop(state);
        drop(heap);
        self.telemetry.record_cycle("zgc", &report.stats);
        let director_generation = match generation {
            MarkGeneration::Young => DirectorGeneration::Young,
            MarkGeneration::Old | MarkGeneration::Full => DirectorGeneration::Old,
        };
        let mut director = self.director.lock();
        director.complete_cycle(director_generation, 0, elapsed);
        self.telemetry.record_director(&director);
        Ok(report)
    }
}

impl<M: GrowableHeapMemory + Clone + Send + Sync + 'static> Drop for GenerationalZgc<M> {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn barrier_is_empty<M: GrowableHeapMemory>(heap: &HeapAccessV2<M>) -> bool {
    match heap.barrier() {
        HeapBarrier::Disabled => true,
        HeapBarrier::Zgc(barrier) => barrier.records().is_empty(),
    }
}

#[derive(Debug)]
pub enum GenerationalZgcError {
    BarrierDisabled,
    Capacity(&'static str),
    CollectorBusy,
    Heap(HeapAccessV2Error),
    InvalidRootPacket,
    InvalidPagePacket,
    InvalidOldMarkPacket,
    InvalidRelocationPacket,
    MissingRootSnapshot,
    RootEpochMismatch { expected: u64, actual: u64 },
    Relocation(String),
    Worker(String),
    WorkerPool(WorkerPoolError),
}

impl fmt::Display for GenerationalZgcError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BarrierDisabled => {
                formatter.write_str("GenerationalZgc requires a ZGC heap barrier")
            }
            Self::Capacity(context) => write!(formatter, "{context} exceeds host capacity"),
            Self::CollectorBusy => formatter.write_str("GenerationalZgc is not idle"),
            Self::Heap(error) => error.fmt(formatter),
            Self::InvalidRootPacket => {
                formatter.write_str("GC root packet is outside the snapshot")
            }
            Self::InvalidPagePacket => {
                formatter.write_str("GC page packet is outside the page snapshot")
            }
            Self::InvalidOldMarkPacket => {
                formatter.write_str("old mark worker received a non-mark packet")
            }
            Self::InvalidRelocationPacket => {
                formatter.write_str("GC relocation packet is outside the descriptor slab")
            }
            Self::MissingRootSnapshot => formatter.write_str("GC root snapshot is missing"),
            Self::RootEpochMismatch { expected, actual } => {
                write!(
                    formatter,
                    "GC root epoch mismatch: expected {expected}, got {actual}"
                )
            }
            Self::Relocation(error) => write!(formatter, "GC relocation failed: {error}"),
            Self::Worker(error) => write!(formatter, "GC worker failed: {error}"),
            Self::WorkerPool(error) => error.fmt(formatter),
        }
    }
}

impl Error for GenerationalZgcError {}

impl From<HeapAccessV2Error> for GenerationalZgcError {
    fn from(error: HeapAccessV2Error) -> Self {
        Self::Heap(error)
    }
}

impl From<WorkerPoolError> for GenerationalZgcError {
    fn from(error: WorkerPoolError) -> Self {
        Self::WorkerPool(error)
    }
}
