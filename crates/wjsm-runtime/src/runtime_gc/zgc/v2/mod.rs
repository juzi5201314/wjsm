mod cycle;
mod types;

pub use types::{ZgcV2Error, ZgcV2Phase, ZgcV2Report, ZgcV2StepOutcome};

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::Instant;

use parking_lot::Mutex;

use crate::heap::{
    Allocation, HandleGeneration, HandleId, HandleTableV2, HeapAddress, ManagedHeap,
    ManagedHeapLayout, Nlab, SharedHeapMemory,
};
use crate::runtime_gc::RootSnapshot;
use crate::runtime_gc::telemetry::{GcTelemetry, GcTelemetrySnapshot};

#[derive(Clone)]
struct ZgcObject {
    allocation: Allocation,
    references: Vec<Option<HandleId>>,
}

struct ZgcV2State {
    phase: ZgcV2Phase,
    pending_mark: VecDeque<HandleId>,
    marked: BTreeSet<HandleId>,
    pending_relocation: VecDeque<HandleId>,
    report: ZgcV2Report,
    started_at: Option<Instant>,
}

impl Default for ZgcV2State {
    fn default() -> Self {
        Self {
            phase: ZgcV2Phase::Idle,
            pending_mark: VecDeque::new(),
            marked: BTreeSet::new(),
            pending_relocation: VecDeque::new(),
            report: ZgcV2Report::default(),
            started_at: None,
        }
    }
}

pub struct ZgcV2 {
    heap: ManagedHeap<SharedHeapMemory>,
    handles: HandleTableV2,
    nlab: Mutex<Nlab>,
    objects: Mutex<BTreeMap<HandleId, ZgcObject>>,
    state: Mutex<ZgcV2State>,
    telemetry: GcTelemetry,
}

impl ZgcV2 {
    pub fn new(memory: SharedHeapMemory, layout: ManagedHeapLayout) -> Result<Self, ZgcV2Error> {
        memory
            .grow_to(layout.object_heap_base())
            .map_err(ZgcV2Error::Memory)?;
        let heap = ManagedHeap::new(memory, layout.clone())?;
        let handles = HandleTableV2::new(layout)?;
        Ok(Self {
            heap,
            handles,
            nlab: Mutex::new(Nlab::new()),
            objects: Mutex::new(BTreeMap::new()),
            state: Mutex::new(ZgcV2State::default()),
            telemetry: GcTelemetry::default(),
        })
    }

    pub fn allocate(
        &self,
        bytes: u64,
        references: impl IntoIterator<Item = HandleId>,
    ) -> Result<HandleId, ZgcV2Error> {
        let allocation = self.heap.allocate(&mut self.nlab.lock(), bytes)?;
        if let Err(error) = self.grow_for(&allocation) {
            self.discard_allocation(&allocation);
            return Err(error);
        }
        let handle = match self.handles.allocate_handle() {
            Ok(handle) => handle,
            Err(error) => {
                self.discard_allocation(&allocation);
                return Err(error.into());
            }
        };
        if let Err(error) = self.handles.publish(
            handle,
            allocation.object().offset(),
            HandleGeneration::Young,
        ) {
            self.discard_allocation(&allocation);
            return Err(error.into());
        }
        self.objects.lock().insert(
            handle,
            ZgcObject {
                allocation,
                references: references.into_iter().map(Some).collect(),
            },
        );
        Ok(handle)
    }

    pub fn address(&self, handle: HandleId) -> Option<u64> {
        self.handles.resolve(handle).map(|entry| entry.address())
    }

    pub fn phase(&self) -> ZgcV2Phase {
        self.state.lock().phase
    }

    pub fn is_marked(&self, handle: HandleId) -> Result<bool, ZgcV2Error> {
        let object = self
            .objects
            .lock()
            .get(&handle)
            .cloned()
            .ok_or(ZgcV2Error::UnknownHandle(handle))?;
        self.heap
            .allocator()
            .is_marked_current(object.allocation.object())
            .map_err(Into::into)
    }

    pub fn telemetry_snapshot(&self) -> GcTelemetrySnapshot {
        self.telemetry.snapshot()
    }

    pub fn write_payload(&self, handle: HandleId, payload: &[u8]) -> Result<(), ZgcV2Error> {
        let object = self
            .objects
            .lock()
            .get(&handle)
            .cloned()
            .ok_or(ZgcV2Error::UnknownHandle(handle))?;
        if payload.len() > object.allocation.bytes() as usize {
            return Err(ZgcV2Error::Memory(
                "payload exceeds object size".to_string(),
            ));
        }
        self.heap
            .memory()
            .copy_from(
                HeapAddress::new(object.allocation.object().offset()),
                payload,
            )
            .map_err(|error| ZgcV2Error::Memory(error.to_string()))
    }

    pub fn read_payload(&self, handle: HandleId, len: usize) -> Result<Vec<u8>, ZgcV2Error> {
        let object = self
            .objects
            .lock()
            .get(&handle)
            .cloned()
            .ok_or(ZgcV2Error::UnknownHandle(handle))?;
        self.heap
            .memory()
            .copy_to(
                HeapAddress::new(object.allocation.object().offset()),
                len.min(object.allocation.bytes() as usize) as u64,
            )
            .map_err(|error| ZgcV2Error::Memory(error.to_string()))
    }

    pub fn write_reference(
        &self,
        owner: HandleId,
        slot: usize,
        target: Option<HandleId>,
    ) -> Result<(), ZgcV2Error> {
        if self.phase() == ZgcV2Phase::Relocate {
            return Err(ZgcV2Error::RelocationInProgress);
        }
        let old = {
            let mut objects = self.objects.lock();
            if let Some(target) = target
                && !objects.contains_key(&target)
            {
                return Err(ZgcV2Error::UnknownHandle(target));
            }
            let object = objects
                .get_mut(&owner)
                .ok_or(ZgcV2Error::UnknownHandle(owner))?;
            if slot >= object.references.len() {
                object.references.resize(slot + 1, None);
            }
            std::mem::replace(&mut object.references[slot], target)
        };
        if let Some(old) = old
            && self.phase() == ZgcV2Phase::Mark
        {
            self.state.lock().pending_mark.push_back(old);
        }
        Ok(())
    }

    pub fn safepoint_step(
        &self,
        roots: &RootSnapshot,
        work_budget: usize,
        cleanup: impl FnMut(HandleId),
    ) -> Result<ZgcV2StepOutcome, ZgcV2Error> {
        let phase = self.start_cycle(roots)?;
        match phase {
            ZgcV2Phase::Mark => self.mark_step(work_budget, cleanup),
            ZgcV2Phase::Relocate => self.relocate_step(work_budget, cleanup),
            ZgcV2Phase::Idle => unreachable!("ZGC V2 step starts a cycle before dispatch"),
        }
    }

    fn start_cycle(&self, roots: &RootSnapshot) -> Result<ZgcV2Phase, ZgcV2Error> {
        let mut state = self.state.lock();
        if state.phase != ZgcV2Phase::Idle {
            return Ok(state.phase);
        }
        self.heap.allocator().clear_current_marks();
        state.phase = ZgcV2Phase::Mark;
        state.pending_mark = roots.handles().iter().copied().map(HandleId::new).collect();
        state.marked.clear();
        state.pending_relocation.clear();
        state.report = ZgcV2Report::default();
        state.started_at = Some(Instant::now());
        Ok(state.phase)
    }

    fn grow_for(&self, allocation: &Allocation) -> Result<(), ZgcV2Error> {
        self.heap
            .memory()
            .grow_to(allocation.object().offset() + allocation.bytes())
            .map_err(ZgcV2Error::Memory)
    }

    fn release_allocation(
        &self,
        allocation: &Allocation,
        report: &mut ZgcV2Report,
    ) -> Result<(), ZgcV2Error> {
        if allocation.is_dedicated() {
            self.heap.allocator().release_dedicated(allocation)?;
            report.reclaimed_pages += allocation.pages().len();
            return Ok(());
        }
        self.heap
            .allocator()
            .forget_object(allocation.object(), allocation.bytes())?;
        if self
            .heap
            .allocator()
            .release_empty_page(allocation.page())?
        {
            report.reclaimed_pages += 1;
        }
        Ok(())
    }

    fn discard_allocation(&self, allocation: &Allocation) {
        if allocation.is_dedicated() {
            let _ = self.heap.allocator().release_dedicated(allocation);
            return;
        }
        let allocator = self.heap.allocator();
        let _ = allocator.forget_object(allocation.object(), allocation.bytes());
        let _ = allocator.release_empty_page(allocation.page());
    }
}
