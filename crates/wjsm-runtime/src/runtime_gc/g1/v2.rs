//! memory64 V2 G1 policy。
//!
//! region identity 直接使用 `ManagedAllocator` page，evacuation 仅更新 atomic handle
//! entry；legacy `RegionSpace` 保留给 Task 15 前的 active default path。

mod collection;
mod types;

pub use types::{G1V2CollectionKind, G1V2Error, G1V2Generation, G1V2Report};

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use wjsm_ir::value;

use crate::heap::{
    Allocation, AllocatorError, HandleGeneration, HandleId, HandleTableV2, ManagedHeap,
    ManagedHeapLayout, Nlab, SharedHeapMemory,
};
use crate::runtime_gc::api::{CycleKind, GcStats};
use crate::runtime_gc::telemetry::{GcTelemetry, GcTelemetrySnapshot};
use crate::runtime_gc::{GcPacketKind, GcWorkPacket, GcWorkerPool, RootSnapshot};

use super::region::CARD_SIZE;
use super::rset::{G1RSet, SlotOwner};

const PROMOTION_AGE: u8 = 2;

#[derive(Clone)]
struct G1Object {
    allocation: Allocation,
    generation: G1V2Generation,
    age: u8,
    references: Vec<Option<HandleId>>,
}

struct CopyJob {
    destination: u64,
    payload: Box<[u8]>,
    result: Mutex<Option<Result<(), String>>>,
}

#[derive(Clone)]
struct RelocationPlan {
    handle: HandleId,
    source: Allocation,
    destination: Allocation,
    generation: G1V2Generation,
    promote: bool,
    age: u8,
}

pub struct G1V2 {
    heap: ManagedHeap<SharedHeapMemory>,
    handles: HandleTableV2,
    nlab: Mutex<Nlab>,
    objects: Mutex<BTreeMap<HandleId, G1Object>>,
    rset: Mutex<G1RSet>,
    copy_jobs: Arc<Mutex<BTreeMap<u32, Arc<CopyJob>>>>,
    workers: GcWorkerPool,
    telemetry: GcTelemetry,
}

impl G1V2 {
    pub fn new(
        memory: SharedHeapMemory,
        layout: ManagedHeapLayout,
        worker_count: usize,
    ) -> Result<Self, G1V2Error> {
        memory
            .grow_to(layout.object_heap_base())
            .map_err(G1V2Error::Memory)?;
        let copy_jobs = Arc::new(Mutex::new(BTreeMap::<u32, Arc<CopyJob>>::new()));
        let worker_jobs = Arc::clone(&copy_jobs);
        let worker_memory = memory.clone();
        let workers = GcWorkerPool::new(worker_count, 4096, move |_, packet| {
            if packet.kind() != GcPacketKind::RelocationRange {
                return;
            }
            let Some(job) = worker_jobs.lock().get(&(packet.start() as u32)).cloned() else {
                return;
            };
            let result = worker_memory
                .copy_from(crate::heap::HeapAddress::new(job.destination), &job.payload)
                .map_err(|error| error.to_string());
            *job.result.lock() = Some(result);
        })?;
        Ok(Self {
            heap: ManagedHeap::new(memory, layout.clone())?,
            handles: HandleTableV2::new(layout)?,
            nlab: Mutex::new(Nlab::new()),
            objects: Mutex::new(BTreeMap::new()),
            rset: Mutex::new(G1RSet::default()),
            copy_jobs,
            workers,
            telemetry: GcTelemetry::default(),
        })
    }

    pub fn telemetry_snapshot(&self) -> GcTelemetrySnapshot {
        self.telemetry.snapshot()
    }

    pub fn allocate_young(
        &self,
        bytes: u64,
        references: impl IntoIterator<Item = HandleId>,
    ) -> Result<HandleId, G1V2Error> {
        self.allocate(bytes, references, G1V2Generation::Eden)
    }

    pub fn allocate_old(
        &self,
        bytes: u64,
        references: impl IntoIterator<Item = HandleId>,
    ) -> Result<HandleId, G1V2Error> {
        self.allocate(bytes, references, G1V2Generation::Old)
    }

    pub fn address(&self, handle: HandleId) -> Option<u64> {
        self.handles.resolve(handle).map(|entry| entry.address())
    }

    pub fn generation(&self, handle: HandleId) -> Option<G1V2Generation> {
        self.objects
            .lock()
            .get(&handle)
            .map(|object| object.generation)
    }

    pub fn write_payload(&self, handle: HandleId, payload: &[u8]) -> Result<(), G1V2Error> {
        let object = self
            .objects
            .lock()
            .get(&handle)
            .cloned()
            .ok_or(G1V2Error::UnknownHandle(handle))?;
        if payload.len() > object.allocation.bytes() as usize {
            return Err(G1V2Error::Memory("payload exceeds object size".to_string()));
        }
        self.heap
            .memory()
            .copy_from(
                crate::heap::HeapAddress::new(object.allocation.object().offset()),
                payload,
            )
            .map_err(|error| G1V2Error::Memory(error.to_string()))
    }

    pub fn read_payload(&self, handle: HandleId, len: usize) -> Result<Vec<u8>, G1V2Error> {
        let object = self
            .objects
            .lock()
            .get(&handle)
            .cloned()
            .ok_or(G1V2Error::UnknownHandle(handle))?;
        self.heap
            .memory()
            .copy_to(
                crate::heap::HeapAddress::new(object.allocation.object().offset()),
                len.min(object.allocation.bytes() as usize) as u64,
            )
            .map_err(|error| G1V2Error::Memory(error.to_string()))
    }

    pub fn record_reference_write(
        &self,
        owner: HandleId,
        slot: usize,
        target: Option<HandleId>,
    ) -> Result<(), G1V2Error> {
        let mut objects = self.objects.lock();
        let target_kind = target
            .and_then(|handle| objects.get(&handle))
            .map(|object| object.generation.region_kind());
        let object = objects
            .get_mut(&owner)
            .ok_or(G1V2Error::UnknownHandle(owner))?;
        if slot >= object.references.len() {
            object.references.resize(slot + 1, None);
        }
        let old = std::mem::replace(&mut object.references[slot], target);
        let slot_address = object.allocation.object().offset() + slot as u64 * 8;
        let card = self.card_index(slot_address);
        self.rset.lock().record_write(
            slot_address as usize,
            old.map_or_else(value::encode_undefined, |handle| {
                value::encode_object_handle(handle.get())
            }),
            target.map_or_else(value::encode_undefined, |handle| {
                value::encode_object_handle(handle.get())
            }),
            SlotOwner {
                region_idx: object.allocation.page().get() as usize,
                kind: object.generation.region_kind(),
            },
            card,
            target_kind.map(|kind| SlotOwner {
                region_idx: 0,
                kind,
            }),
        );
        Ok(())
    }

    fn allocate(
        &self,
        bytes: u64,
        references: impl IntoIterator<Item = HandleId>,
        requested_generation: G1V2Generation,
    ) -> Result<HandleId, G1V2Error> {
        let allocation = self.heap.allocate(&mut self.nlab.lock(), bytes)?;
        self.grow_for(&allocation)?;
        let generation = if allocation.pages().len() > 1 {
            G1V2Generation::Humongous
        } else {
            requested_generation
        };
        let handle = self.handles.allocate_handle()?;
        self.handles.publish(
            handle,
            allocation.object().offset(),
            if generation.is_young() {
                HandleGeneration::Young
            } else {
                HandleGeneration::Old
            },
        )?;
        self.objects.lock().insert(
            handle,
            G1Object {
                allocation,
                generation,
                age: 0,
                references: references.into_iter().map(Some).collect(),
            },
        );
        Ok(handle)
    }

    fn evacuate_young(
        &self,
        handles: &[HandleId],
        report: &mut G1V2Report,
    ) -> Result<(), G1V2Error> {
        let mut plans = Vec::new();
        let mut objects = self.objects.lock();
        for handle in handles {
            let object = objects
                .get_mut(handle)
                .expect("young evacuation handle disappeared");
            let age = object.age.saturating_add(1);
            let generation = if age >= PROMOTION_AGE {
                G1V2Generation::Old
            } else {
                G1V2Generation::Survivor
            };
            match self
                .heap
                .allocate(&mut self.nlab.lock(), object.allocation.bytes())
            {
                Ok(destination) => {
                    self.grow_for(&destination)?;
                    plans.push(RelocationPlan {
                        handle: *handle,
                        source: object.allocation.clone(),
                        destination,
                        generation,
                        promote: generation == G1V2Generation::Old,
                        age,
                    });
                }
                Err(AllocatorError::OutOfPages { .. }) => {
                    self.handles.promote(*handle)?;
                    object.generation = G1V2Generation::Old;
                    object.age = PROMOTION_AGE;
                    report.promoted += 1;
                    report.promoted_bytes = report
                        .promoted_bytes
                        .saturating_add(object.allocation.bytes());
                    report.promotion_failed = true;
                }
                Err(error) => return Err(error.into()),
            }
        }
        drop(objects);
        self.execute_relocations(plans, report)
    }

    fn evacuate_old(&self, handles: &[HandleId], report: &mut G1V2Report) -> Result<(), G1V2Error> {
        let objects = self.objects.lock();
        let mut plans = Vec::new();
        for handle in handles {
            let object = objects
                .get(handle)
                .expect("mixed evacuation handle disappeared");
            let destination = match self
                .heap
                .allocate(&mut self.nlab.lock(), object.allocation.bytes())
            {
                Ok(destination) => destination,
                Err(AllocatorError::OutOfPages { .. }) => continue,
                Err(error) => return Err(error.into()),
            };
            self.grow_for(&destination)?;
            plans.push(RelocationPlan {
                handle: *handle,
                source: object.allocation.clone(),
                destination,
                generation: object.generation,
                promote: false,
                age: object.age,
            });
        }
        drop(objects);
        self.execute_relocations(plans, report)
    }

    fn execute_relocations(
        &self,
        plans: Vec<RelocationPlan>,
        report: &mut G1V2Report,
    ) -> Result<(), G1V2Error> {
        for plan in &plans {
            let payload = self
                .heap
                .memory()
                .copy_to(
                    crate::heap::HeapAddress::new(plan.source.object().offset()),
                    plan.source.bytes(),
                )
                .map_err(|error| G1V2Error::Memory(error.to_string()))?;
            self.copy_jobs.lock().insert(
                plan.handle.get(),
                Arc::new(CopyJob {
                    destination: plan.destination.object().offset(),
                    payload: payload.into_boxed_slice(),
                    result: Mutex::new(None),
                }),
            );
            self.workers.submit(GcWorkPacket::new(
                GcPacketKind::RelocationRange,
                u64::from(plan.handle.get()),
                u32::try_from(plan.source.bytes()).unwrap_or(u32::MAX),
                0,
            ))?;
        }
        self.workers.wait_for_idle();
        let mut objects = self.objects.lock();
        for plan in plans {
            let job = self
                .copy_jobs
                .lock()
                .remove(&plan.handle.get())
                .expect("completed G1 copy job disappeared");
            job.result
                .lock()
                .take()
                .expect("G1 copy worker did not publish a result")
                .map_err(G1V2Error::Memory)?;
            self.handles.begin_relocation(plan.handle)?;
            self.handles
                .complete_relocation(plan.handle, plan.destination.object().offset())?;
            if plan.promote {
                self.handles.promote(plan.handle)?;
                report.promoted += 1;
                report.promoted_bytes = report.promoted_bytes.saturating_add(plan.source.bytes());
            }
            self.release_allocation(&plan.source, report)?;
            let object = objects
                .get_mut(&plan.handle)
                .expect("relocated G1 object disappeared");
            object.allocation = plan.destination;
            object.generation = plan.generation;
            object.age = plan.age;
            report.evacuated += 1;
            report.relocated_bytes = report.relocated_bytes.saturating_add(plan.source.bytes());
        }
        Ok(())
    }

    fn retire_objects(
        &self,
        handles: &[HandleId],
        report: &mut G1V2Report,
        cleanup: &mut impl FnMut(HandleId),
    ) -> Result<(), G1V2Error> {
        let mut objects = self.objects.lock();
        for handle in handles {
            let object = objects
                .remove(handle)
                .expect("G1 dead object disappeared during stop-the-world sweep");
            self.handles.retire(*handle)?;
            report.reclaimed_bytes = report
                .reclaimed_bytes
                .saturating_add(object.allocation.bytes());
            cleanup(*handle);
            self.release_allocation(&object.allocation, report)?;
            report.retired += 1;
        }
        self.handles.advance_epoch();
        self.handles.reclaim_quarantine();
        Ok(())
    }

    fn release_allocation(
        &self,
        allocation: &Allocation,
        report: &mut G1V2Report,
    ) -> Result<(), G1V2Error> {
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

    fn grow_for(&self, allocation: &Allocation) -> Result<(), G1V2Error> {
        self.heap
            .memory()
            .grow_to(allocation.object().offset() + allocation.bytes())
            .map_err(G1V2Error::Memory)
    }
    fn record_collection(&self, report: &G1V2Report, elapsed: Duration) {
        let rset = self.rset.lock().stats_snapshot();
        let cycle_kind = match report.kind.expect("G1 V2 collection report has a kind") {
            G1V2CollectionKind::Young => CycleKind::Young,
            G1V2CollectionKind::Mixed => CycleKind::Mixed,
            G1V2CollectionKind::Full => CycleKind::Full,
        };
        let mut stats = GcStats {
            marked: report.marked,
            swept: report.retired,
            freed_bytes: usize::try_from(report.reclaimed_bytes)
                .expect("48-bit managed heap byte count fits usize"),
            elapsed,
            cycle_kind,
            relocated_bytes: usize::try_from(report.relocated_bytes)
                .expect("48-bit managed heap byte count fits usize"),
            relocated_objects: report.evacuated,
            rset_cards: rset.dirty_cards,
            rset_precise_slots: rset.precise_slots,
            ..GcStats::default()
        };
        stats.record_pause(elapsed);
        self.telemetry.record_cycle("g1-v2", &stats);
    }

    fn card_index(&self, address: u64) -> usize {
        ((address - self.heap.allocator().layout().object_heap_base()) / CARD_SIZE as u64) as usize
    }
}
