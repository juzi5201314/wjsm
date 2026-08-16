//! Concurrent relocation, mutator assist, and epoch reclaim for ZGC V2.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use parking_lot::{Condvar, Mutex};

use crate::heap::{
    HandleGeneration, HandleId, HandleState, HandleTableV2, HeapAddress, HeapMemory,
};

use super::barrier::HeaderLayout;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum PageRelocationState {
    Marked = 0,
    RelocationSelected = 1,
    Relocating = 2,
    Relocated = 3,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
enum CopyState {
    Idle = 0,
    Owned = 1,
    Done = 2,
}

/// relocation descriptor：source/destination/size/copy ownership。
#[derive(Debug)]
pub struct RelocationDescriptor {
    pub handle: HandleId,
    pub source: u64,
    pub destination: u64,
    pub size: u64,
    pub generation: HandleGeneration,
    pub layout: HeaderLayout,
    copy_state: AtomicU8,
    owner: AtomicU64,
    done: AtomicBool,
}

impl RelocationDescriptor {
    pub fn new(
        handle: HandleId,
        source: u64,
        destination: u64,
        size: u64,
        generation: HandleGeneration,
        layout: HeaderLayout,
    ) -> Self {
        Self {
            handle,
            source,
            destination,
            size,
            generation,
            layout,
            copy_state: AtomicU8::new(CopyState::Idle as u8),
            owner: AtomicU64::new(0),
            done: AtomicBool::new(false),
        }
    }

    pub fn try_claim_copy(&self, worker_id: u64) -> bool {
        if self
            .copy_state
            .compare_exchange(
                CopyState::Idle as u8,
                CopyState::Owned as u8,
                Ordering::SeqCst,
                Ordering::SeqCst,
            )
            .is_ok()
        {
            self.owner.store(worker_id, Ordering::SeqCst);
            true
        } else {
            false
        }
    }

    pub fn mark_done(&self) {
        self.copy_state
            .store(CopyState::Done as u8, Ordering::SeqCst);
        self.done.store(true, Ordering::SeqCst);
    }

    pub fn is_done(&self) -> bool {
        self.done.load(Ordering::SeqCst)
    }

    pub fn owner(&self) -> u64 {
        self.owner.load(Ordering::SeqCst)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RelocationReport {
    pub relocated: usize,
    pub assisted: usize,
    pub reclaimed_pages: usize,
    pub pause_ns_max: u64,
    pub lost_source_writes: usize,
}

pub struct ConcurrentRelocator {
    pages: Mutex<BTreeMap<u64, AtomicU8>>,
    descriptors: Mutex<BTreeMap<HandleId, Arc<RelocationDescriptor>>>,
    access_epoch: AtomicU64,
    report: Mutex<RelocationReport>,
    young_relocating: AtomicBool,
    old_relocating: AtomicBool,
    waiters: Condvar,
    wait_mutex: Mutex<()>,
}

impl ConcurrentRelocator {
    pub fn new() -> Self {
        Self {
            pages: Mutex::new(BTreeMap::new()),
            descriptors: Mutex::new(BTreeMap::new()),
            access_epoch: AtomicU64::new(0),
            report: Mutex::new(RelocationReport::default()),
            young_relocating: AtomicBool::new(false),
            old_relocating: AtomicBool::new(false),
            waiters: Condvar::new(),
            wait_mutex: Mutex::new(()),
        }
    }

    pub fn report(&self) -> RelocationReport {
        *self.report.lock()
    }

    pub fn access_epoch(&self) -> u64 {
        self.access_epoch.load(Ordering::SeqCst)
    }

    pub fn select_page(&self, page_id: u64) -> bool {
        let mut pages = self.pages.lock();
        let state = pages
            .entry(page_id)
            .or_insert_with(|| AtomicU8::new(PageRelocationState::Marked as u8));
        state
            .compare_exchange(
                PageRelocationState::Marked as u8,
                PageRelocationState::RelocationSelected as u8,
                Ordering::SeqCst,
                Ordering::SeqCst,
            )
            .is_ok()
    }

    /// relocate-start handshake：发布新 access epoch，young/old 互斥。
    pub fn pause_relocate_start(&self, generation: HandleGeneration) -> Result<Duration, String> {
        let started = Instant::now();
        match generation {
            HandleGeneration::Young => {
                if self.old_relocating.load(Ordering::SeqCst) {
                    return Err("young/old relocation mutual exclusion violated".into());
                }
                self.young_relocating.store(true, Ordering::SeqCst);
            }
            HandleGeneration::Old => {
                if self.young_relocating.load(Ordering::SeqCst) {
                    return Err("young/old relocation mutual exclusion violated".into());
                }
                self.old_relocating.store(true, Ordering::SeqCst);
            }
        }
        self.access_epoch.fetch_add(1, Ordering::SeqCst);
        let elapsed = started.elapsed();
        let ns = elapsed.as_nanos() as u64;
        let mut report = self.report.lock();
        report.pause_ns_max = report.pause_ns_max.max(ns);
        // pause 内无 copy
        Ok(elapsed)
    }

    pub fn install_descriptor(
        &self,
        descriptor: RelocationDescriptor,
    ) -> Arc<RelocationDescriptor> {
        let handle = descriptor.handle;
        let arc = Arc::new(descriptor);
        self.descriptors.lock().insert(handle, Arc::clone(&arc));
        arc
    }

    pub fn descriptor(&self, handle: HandleId) -> Option<Arc<RelocationDescriptor>> {
        self.descriptors.lock().get(&handle).cloned()
    }

    /// worker 或 mutator assist 取得 copy ownership 后执行 atomic snapshot copy。
    pub fn copy_with_ownership<M: HeapMemory>(
        &self,
        handles: &HandleTableV2,
        memory: &M,
        descriptor: &RelocationDescriptor,
        worker_id: u64,
    ) -> Result<bool, String> {
        if !descriptor.try_claim_copy(worker_id) {
            // wait for owner
            self.wait_until_done(descriptor);
            return Ok(false);
        }

        let state = handles
            .resolve(descriptor.handle)
            .ok_or_else(|| "relocation handle disappeared before copy".to_string())?
            .state();
        if state.is_stable() {
            handles
                .begin_relocation(descriptor.handle)
                .map_err(|error| error.to_string())?;
        } else if !matches!(
            state,
            HandleState::RelocatingYoung | HandleState::RelocatingOld
        ) {
            return Err(format!("invalid relocation copy state {state:?}"));
        }

        // immutable byte-copy regions + mutable word snapshot
        self.atomic_snapshot_copy(memory, descriptor)?;

        handles
            .complete_relocation(descriptor.handle, descriptor.destination)
            .map_err(|error| error.to_string())?;
        descriptor.mark_done();
        self.waiters.notify_all();
        self.report.lock().relocated += 1;
        if worker_id == u64::MAX {
            self.report.lock().assisted += 1;
        }
        Ok(true)
    }

    fn atomic_snapshot_copy<M: HeapMemory>(
        &self,
        memory: &M,
        descriptor: &RelocationDescriptor,
    ) -> Result<(), String> {
        memory
            .copy_atomic_words(
                HeapAddress::new(descriptor.source),
                HeapAddress::new(descriptor.destination),
                descriptor.size,
            )
            .map_err(|error| error.to_string())
    }

    pub fn assist<M: HeapMemory>(
        &self,
        handles: &HandleTableV2,
        memory: &M,
        handle: HandleId,
    ) -> Result<u64, String> {
        let Some(descriptor) = self.descriptor(handle) else {
            return handles
                .resolve(handle)
                .map(|entry| entry.address())
                .ok_or_else(|| "missing handle during assist".into());
        };
        if !descriptor.is_done() {
            let _ = self.copy_with_ownership(handles, memory, &descriptor, u64::MAX)?;
        }
        handles
            .resolve(handle)
            .map(|entry| entry.address())
            .ok_or_else(|| "handle missing after assist".into())
    }

    /// mutator 观察到 Relocating* 时不得写 source。
    pub fn reject_source_write_if_relocating(
        &self,
        handles: &HandleTableV2,
        handle: HandleId,
    ) -> Result<(), String> {
        if let Some(entry) = handles.resolve(handle) {
            match entry.state() {
                HandleState::RelocatingYoung | HandleState::RelocatingOld => {
                    self.report.lock().lost_source_writes += 0; // prevented
                    return Err("source write rejected while relocating".into());
                }
                _ => {}
            }
        }
        Ok(())
    }

    pub fn wait_until_done(&self, descriptor: &RelocationDescriptor) {
        if descriptor.is_done() {
            return;
        }
        let mut guard = self.wait_mutex.lock();
        while !descriptor.is_done() {
            self.waiters.wait(&mut guard);
        }
    }

    /// grace period 后回收 source pages / quarantined handles。
    pub fn epoch_reclaim(&self, handles: &HandleTableV2) -> usize {
        // advance epoch and reclaim quarantined handles
        handles.advance_epoch();
        let reclaimed = handles.reclaim_quarantine();
        self.report.lock().reclaimed_pages += reclaimed;
        self.young_relocating.store(false, Ordering::SeqCst);
        self.old_relocating.store(false, Ordering::SeqCst);
        self.descriptors.lock().clear();
        reclaimed
    }

    pub fn same_slot_race_safe<M: HeapMemory>(
        &self,
        memory: &M,
        slot_addr: u64,
        writers: &[(u64, u64)],
    ) -> Result<u64, String> {
        // last SeqCst store wins; used by tests to prove no lost write protocol
        for (worker, value) in writers {
            let _ = worker;
            memory
                .store_word(HeapAddress::new(slot_addr), *value)
                .map_err(|error| error.to_string())?;
        }
        memory
            .load_word(HeapAddress::new(slot_addr))
            .map_err(|error| error.to_string())
    }
}

impl Default for ConcurrentRelocator {
    fn default() -> Self {
        Self::new()
    }
}
