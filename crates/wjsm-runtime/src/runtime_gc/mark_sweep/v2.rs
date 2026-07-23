//! memory64 V2 mark-sweep policy。
//!
//! 该策略只依赖 V2 allocator、atomic handle table 与显式 root snapshot；不会读取
//! main-memory `obj_table`。active full collect 经 `active_v2` 调度本策略的语义对等路径。

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::sync::Arc;

use parking_lot::Mutex;

use crate::heap::{
    Allocation, AllocatorError, EpochParticipant, HandleGeneration, HandleId, HandleTableError,
    HandleTableV2, ManagedHeap, ManagedHeapLayout, Nlab, SharedHeapMemory,
};
use crate::runtime_gc::RootSnapshot;

/// 一次 V2 mark-sweep 的可观测结果。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MarkSweepV2Report {
    pub marked: usize,
    pub retired: usize,
    pub reclaimed_handles: usize,
    pub reclaimed_dedicated_pages: u32,
}

/// 一次 allocation fast path 或 OOM full collection 的结果。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MarkSweepV2Allocation {
    pub handle: HandleId,
    pub full_collection: Option<MarkSweepV2Report>,
}

/// V2 mark-sweep policy 的错误边界。
#[derive(Debug)]
pub enum MarkSweepV2Error {
    Allocation(AllocatorError),
    Handle(HandleTableError),
    MemoryGrow(String),
}

impl fmt::Display for MarkSweepV2Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Allocation(error) => write!(formatter, "V2 allocation failed: {error}"),
            Self::Handle(error) => write!(formatter, "V2 handle operation failed: {error}"),
            Self::MemoryGrow(error) => write!(formatter, "unable to grow V2 shared heap: {error}"),
        }
    }
}

impl Error for MarkSweepV2Error {}

impl From<AllocatorError> for MarkSweepV2Error {
    fn from(error: AllocatorError) -> Self {
        Self::Allocation(error)
    }
}

impl From<HandleTableError> for MarkSweepV2Error {
    fn from(error: HandleTableError) -> Self {
        Self::Handle(error)
    }
}

#[derive(Clone)]
struct TrackedObject {
    allocation: Allocation,
    references: Arc<[HandleId]>,
}

/// 使用 `ManagedHeap` 和 `HandleTableV2` 的 stop-the-world mark-sweep baseline。
///
/// collector 只消费 immutable handle-only `RootSnapshot`。mark phase 将可达对象写入
/// allocator object-map bitmap；sweep phase 先 retire handle、执行 side-table cleanup，
/// 再回收 whole dead dedicated page。共享 NLAB page 中的 dead object 保持 retired，供
/// 后续 compaction policy 回收。
pub struct MarkSweepV2 {
    heap: ManagedHeap<SharedHeapMemory>,
    handles: HandleTableV2,
    nlab: Mutex<Nlab>,
    objects: Mutex<BTreeMap<HandleId, TrackedObject>>,
}

impl MarkSweepV2 {
    pub fn new(
        memory: SharedHeapMemory,
        layout: ManagedHeapLayout,
    ) -> Result<Self, MarkSweepV2Error> {
        memory
            .grow_to(layout.object_heap_base())
            .map_err(MarkSweepV2Error::MemoryGrow)?;
        Ok(Self {
            heap: ManagedHeap::new(memory, layout.clone())?,
            handles: HandleTableV2::new(layout)?,
            nlab: Mutex::new(Nlab::new()),
            objects: Mutex::new(BTreeMap::new()),
        })
    }

    pub fn allocate(
        &self,
        bytes: u64,
        references: impl IntoIterator<Item = HandleId>,
    ) -> Result<HandleId, MarkSweepV2Error> {
        self.allocate_tracked(bytes, references.into_iter().collect())
    }

    pub fn allocate_or_collect(
        &self,
        bytes: u64,
        references: impl IntoIterator<Item = HandleId>,
        roots: &RootSnapshot,
        mut cleanup_side_tables: impl FnMut(HandleId),
    ) -> Result<MarkSweepV2Allocation, MarkSweepV2Error> {
        let references = references.into_iter().collect::<Arc<[_]>>();
        match self.allocate_tracked(bytes, Arc::clone(&references)) {
            Ok(handle) => Ok(MarkSweepV2Allocation {
                handle,
                full_collection: None,
            }),
            Err(MarkSweepV2Error::Allocation(AllocatorError::OutOfPages { .. })) => {
                let report = self.collect(roots, &mut cleanup_side_tables)?;
                let handle = self.allocate_tracked(bytes, references)?;
                Ok(MarkSweepV2Allocation {
                    handle,
                    full_collection: Some(report),
                })
            }
            Err(error) => Err(error),
        }
    }

    pub fn resolve(&self, handle: HandleId) -> Option<u64> {
        self.handles.resolve(handle).map(|entry| entry.address())
    }

    pub fn is_marked(&self, handle: HandleId) -> bool {
        self.objects.lock().get(&handle).is_some_and(|object| {
            self.heap
                .allocator()
                .is_marked_current(object.allocation.object())
                .unwrap_or(false)
        })
    }

    pub fn register_epoch_participant(&self) -> EpochParticipant {
        self.handles.register_participant()
    }

    pub fn collect(
        &self,
        roots: &RootSnapshot,
        mut cleanup_side_tables: impl FnMut(HandleId),
    ) -> Result<MarkSweepV2Report, MarkSweepV2Error> {
        self.heap.allocator().clear_current_marks();
        let roots = roots
            .handles()
            .iter()
            .copied()
            .map(HandleId::new)
            .collect::<BTreeSet<_>>();
        let objects = self.objects.lock();
        let marked = mark_reachable(&objects, roots);
        for handle in &marked {
            if let Some(object) = objects.get(handle) {
                self.heap
                    .allocator()
                    .mark_current(object.allocation.object())?;
            }
        }
        let dead = objects
            .keys()
            .filter(|handle| !marked.contains(handle))
            .copied()
            .collect::<BTreeSet<_>>();
        drop(objects);

        let mut report = MarkSweepV2Report {
            marked: marked.len(),
            ..MarkSweepV2Report::default()
        };
        let mut reclaimed_pages = BTreeSet::new();
        let mut objects = self.objects.lock();
        for handle in dead {
            let object = objects
                .remove(&handle)
                .expect("mark-sweep dead handle disappeared during stop-the-world sweep");
            self.handles.retire(handle)?;
            cleanup_side_tables(handle);
            report.retired += 1;
            if object.allocation.is_dedicated() {
                self.heap
                    .allocator()
                    .release_dedicated(&object.allocation)?;
                reclaimed_pages.insert(object.allocation.page());
            }
        }
        drop(objects);
        report.reclaimed_dedicated_pages = reclaimed_pages.len() as u32;
        self.handles.advance_epoch();
        report.reclaimed_handles = self.handles.reclaim_quarantine();
        Ok(report)
    }

    fn allocate_tracked(
        &self,
        bytes: u64,
        references: Arc<[HandleId]>,
    ) -> Result<HandleId, MarkSweepV2Error> {
        let allocation = self.heap.allocate(&mut self.nlab.lock(), bytes)?;
        let end = allocation.object().offset() + allocation.bytes();
        self.heap
            .memory()
            .grow_to(end)
            .map_err(MarkSweepV2Error::MemoryGrow)?;
        let handle = self.handles.allocate_handle()?;
        self.handles.publish(
            handle,
            allocation.object().offset(),
            HandleGeneration::Young,
        )?;
        self.objects.lock().insert(
            handle,
            TrackedObject {
                allocation,
                references,
            },
        );
        Ok(handle)
    }
}

fn mark_reachable(
    objects: &BTreeMap<HandleId, TrackedObject>,
    roots: BTreeSet<HandleId>,
) -> BTreeSet<HandleId> {
    let mut marked = BTreeSet::new();
    let mut pending = roots;
    while let Some(handle) = pending.pop_first() {
        let Some(object) = objects.get(&handle) else {
            continue;
        };
        if !marked.insert(handle) {
            continue;
        }
        pending.extend(
            object
                .references
                .iter()
                .copied()
                .filter(|child| !marked.contains(child)),
        );
    }
    marked
}
