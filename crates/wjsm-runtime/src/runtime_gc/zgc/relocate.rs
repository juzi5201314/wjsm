//! ZGC relocation set 与搬迁 owner。

use std::time::Instant;

use wjsm_ir::constants;

use crate::runtime_gc::api::{CycleKind, GcContext, GcStats, Handle, StepBudget};
use crate::runtime_gc::context::object_size_from_memory;
use crate::runtime_gc::heap_governance;

use super::color::{ZColor, ZEntry};
use super::mark::{read_entry, write_entry};
use super::page::{ZPAGE_SIZE, ZPageSpace};

#[derive(Debug)]
pub(super) enum RelocateStep {
    Idle,
    Progress { remaining_estimate: usize },
    Complete { stats: GcStats },
}

#[derive(Debug, Default)]
pub(super) struct ZRelocateState {
    active: bool,
    started_at: Option<Instant>,
    allocator: RelocationAllocator,
    scratch: Vec<u8>,
    candidates: Vec<RelocationCandidate>,
    cursor: usize,
    cycle_result: RelocateResult,
}

#[derive(Debug, Default)]
struct RelocationAllocator {
    current: Option<BumpPage>,
}

#[derive(Debug)]
struct BumpPage {
    cursor: usize,
    end: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RelocationCandidate {
    handle: Handle,
    slot: usize,
    ptr: usize,
    size: usize,
}

#[derive(Debug, Default)]
struct RelocateResult {
    relocated_objects: usize,
    relocated_bytes: usize,
    released_pages: usize,
}

impl RelocateResult {
    fn add(&mut self, other: Self) {
        self.relocated_objects = self
            .relocated_objects
            .saturating_add(other.relocated_objects);
        self.relocated_bytes = self.relocated_bytes.saturating_add(other.relocated_bytes);
        self.released_pages = self.released_pages.saturating_add(other.released_pages);
    }
}

impl ZRelocateState {
    pub(super) fn new() -> Self {
        Self::default()
    }

    pub(super) fn is_active(&self) -> bool {
        self.active
    }

    #[cfg(test)]
    pub(super) fn start_cycle(&mut self, pages: &mut ZPageSpace, copy_budget: usize) -> bool {
        self.start_cycle_excluding(pages, copy_budget, None)
    }

    pub(super) fn start_cycle_excluding(
        &mut self,
        pages: &mut ZPageSpace,
        copy_budget: usize,
        excluded_page: Option<usize>,
    ) -> bool {
        self.active = false;
        self.started_at = None;
        self.allocator.reset();
        self.candidates.clear();
        self.cursor = 0;
        self.cycle_result = RelocateResult::default();
        pages.clear_relocation_set();

        let selected = pages.select_relocation_set_excluding(copy_budget, excluded_page);
        for &idx in &selected {
            pages.mark_relocation_set(idx);
        }
        self.active = !selected.is_empty();
        if self.active {
            self.started_at = Some(Instant::now());
        } else {
            pages.clear_relocation_set();
        }
        self.active
    }

    pub(super) fn drain_incremental(
        &mut self,
        ctx: &mut GcContext<'_>,
        pages: &mut ZPageSpace,
        budget: StepBudget,
    ) -> RelocateStep {
        if !self.active {
            return RelocateStep::Idle;
        }

        let mut copied = 0usize;
        if self.candidates.is_empty() {
            self.candidates = collect_relocation_candidates(ctx, pages);
            self.active = !self.candidates.is_empty();
            if !self.active {
                return self.finish(ctx, pages);
            }
        }
        let budget_bytes = budget.work_bytes.max(1);
        loop {
            if copied >= budget_bytes || Instant::now() >= budget.deadline {
                return RelocateStep::Progress {
                    remaining_estimate: self.remaining_candidate_bytes(),
                };
            }
            let Some(candidate) = self.next_relocation_candidate(ctx, pages) else {
                return self.finish(ctx, pages);
            };
            let Some(relocated) = relocate_candidate(
                ctx,
                pages,
                &mut self.allocator,
                &mut self.scratch,
                candidate,
            ) else {
                remap_remaining_rs_entries(ctx, pages);
                return self.finish(ctx, pages);
            };
            copied = copied.saturating_add(candidate.size.max(1));
            self.cycle_result.add(relocated);
        }
    }

    fn remaining_candidate_bytes(&self) -> usize {
        self.candidates[self.cursor.min(self.candidates.len())..]
            .iter()
            .map(|candidate| candidate.size)
            .sum()
    }

    fn next_relocation_candidate(
        &mut self,
        ctx: &mut GcContext<'_>,
        pages: &ZPageSpace,
    ) -> Option<RelocationCandidate> {
        while let Some(candidate) = self.candidates.get(self.cursor).copied() {
            self.cursor += 1;
            let Some((slot, entry)) = read_entry(ctx, candidate.handle) else {
                continue;
            };
            if slot != candidate.slot || entry.is_empty() || entry.ptr() as usize != candidate.ptr {
                continue;
            }
            if pages.addr_in_relocation_set(candidate.ptr) {
                return Some(candidate);
            }
        }
        None
    }

    pub(super) fn relocate_or_remap_handle(
        &mut self,
        ctx: &mut GcContext<'_>,
        pages: &mut ZPageSpace,
        h: Handle,
    ) -> u32 {
        let Some((slot, entry)) = read_entry(ctx, h) else {
            return 0;
        };
        if entry.is_empty() {
            return 0;
        }
        let ptr = entry.ptr() as usize;
        if self.active && pages.addr_in_relocation_set(ptr) {
            let size = ctx.with_memory(|data| object_size_from_memory(data, ptr));
            let Some(size) = size else {
                debug_assert!(false, "ZGC relocate: RS object has unreadable header");
                return entry.raw();
            };
            let candidate = RelocationCandidate {
                handle: h,
                slot,
                ptr,
                size,
            };
            if let Some(relocated) = relocate_candidate(
                ctx,
                pages,
                &mut self.allocator,
                &mut self.scratch,
                candidate,
            ) {
                self.cycle_result.add(relocated);
                return read_entry(ctx, h)
                    .map(|(_, moved)| moved.raw())
                    .unwrap_or(0);
            }
            debug_assert!(
                false,
                "ZGC relocate: failed to allocate relocation target page"
            );
            let repaired = entry.repair_relocate_non_rs();
            if repaired.raw() != entry.raw() {
                write_entry(ctx, slot, repaired);
                ctx.increment_gc_epoch();
            }
            return repaired.raw();
        }

        let repaired = entry.repair_relocate_non_rs();
        if repaired.raw() != entry.raw() {
            write_entry(ctx, slot, repaired);
            ctx.increment_gc_epoch();
        }
        repaired.raw()
    }

    fn finish(&mut self, ctx: &mut GcContext<'_>, pages: &mut ZPageSpace) -> RelocateStep {
        self.active = false;
        self.allocator.reset();
        self.candidates.clear();
        self.cursor = 0;
        pages.clear_relocation_set();
        let stats = stats_for_result(ctx, pages, self.started_at.take(), &self.cycle_result);
        self.cycle_result = RelocateResult::default();
        RelocateStep::Complete { stats }
    }
}

impl RelocationAllocator {
    fn reset(&mut self) {
        self.current = None;
    }

    fn allocate(
        &mut self,
        ctx: &mut GcContext<'_>,
        pages: &mut ZPageSpace,
        size: usize,
    ) -> Option<usize> {
        let aligned = align_up(size, constants::HEAP_ALLOCATION_ALIGNMENT as usize)?;
        if aligned > ZPAGE_SIZE {
            self.current = None;
            return self.allocate_large(ctx, pages, aligned);
        }
        if let Some(page) = &mut self.current
            && let Some(ptr) = bump(page, aligned)
        {
            return Some(ptr);
        }
        let start = reserve_pages(ctx, pages, 1)?;
        self.current = Some(BumpPage {
            cursor: start,
            end: start + ZPAGE_SIZE,
        });
        bump(self.current.as_mut()?, aligned)
    }

    fn allocate_large(
        &mut self,
        ctx: &mut GcContext<'_>,
        pages: &mut ZPageSpace,
        aligned: usize,
    ) -> Option<usize> {
        let page_count = aligned.div_ceil(ZPAGE_SIZE);
        reserve_pages(ctx, pages, page_count)
    }
}

fn remap_remaining_rs_entries(ctx: &mut GcContext<'_>, pages: &ZPageSpace) {
    let obj_table_ptr = ctx.obj_table_ptr();
    let obj_table_count = ctx.obj_table_count();
    let repairs = ctx.with_memory(|data| {
        let mut repairs = Vec::new();
        for h in 0..obj_table_count as Handle {
            let slot = obj_table_ptr + h as usize * constants::HANDLE_TABLE_ENTRY_SIZE as usize;
            let Some(entry) = read_entry_from_memory(data, obj_table_ptr, h) else {
                break;
            };
            if entry.is_empty() || !pages.addr_in_relocation_set(entry.ptr() as usize) {
                continue;
            }
            let repaired = entry.repair_relocate_non_rs();
            if repaired.raw() != entry.raw() {
                repairs.push((slot, repaired));
            }
        }
        repairs
    });
    if repairs.is_empty() {
        return;
    }
    for (slot, entry) in repairs {
        write_entry(ctx, slot, entry);
    }
    ctx.increment_gc_epoch();
}

fn collect_relocation_candidates(
    ctx: &mut GcContext<'_>,
    pages: &ZPageSpace,
) -> Vec<RelocationCandidate> {
    let obj_table_ptr = ctx.obj_table_ptr();
    let obj_table_count = ctx.obj_table_count();
    ctx.with_memory(|data| {
        let mut candidates = Vec::new();
        for h in 0..obj_table_count as Handle {
            let slot = obj_table_ptr + h as usize * constants::HANDLE_TABLE_ENTRY_SIZE as usize;
            let Some(entry) = read_entry_from_memory(data, obj_table_ptr, h) else {
                break;
            };
            if entry.is_empty() {
                continue;
            }
            let ptr = entry.ptr() as usize;
            if !pages.addr_in_relocation_set(ptr) {
                continue;
            }
            let Some(size) = object_size_from_memory(data, ptr) else {
                debug_assert!(
                    false,
                    "ZGC relocate: live obj_table entry has unreadable header"
                );
                continue;
            };
            candidates.push(RelocationCandidate {
                handle: h,
                slot,
                ptr,
                size,
            });
        }
        candidates
    })
}

fn relocate_candidate(
    ctx: &mut GcContext<'_>,
    pages: &mut ZPageSpace,
    allocator: &mut RelocationAllocator,
    scratch: &mut Vec<u8>,
    candidate: RelocationCandidate,
) -> Option<RelocateResult> {
    let dest = allocator.allocate(ctx, pages, candidate.size)?;
    let copied = ctx.with_memory_mut(|data| {
        copy_raw_object(data, candidate.ptr, dest, candidate.size, scratch)
    });
    if !copied {
        return None;
    }
    write_entry(
        ctx,
        candidate.slot,
        ZEntry::new(dest as u32, ZColor::Remapped),
    );
    pages.add_live_bytes_range(dest, candidate.size);
    ctx.increment_gc_epoch();
    let released_pages = release_empty_source_pages(ctx, pages, candidate.ptr, candidate.size);
    Some(RelocateResult {
        relocated_objects: 1,
        relocated_bytes: candidate.size,
        released_pages,
    })
}

fn reserve_pages(ctx: &mut GcContext<'_>, pages: &mut ZPageSpace, count: usize) -> Option<usize> {
    if let Some(start) = pages.take_contiguous_free_pages(count) {
        return Some(start);
    }
    reserve_new_pages(ctx, pages, count)
}

fn reserve_new_pages(
    ctx: &mut GcContext<'_>,
    pages: &mut ZPageSpace,
    count: usize,
) -> Option<usize> {
    let heap_ptr = ctx.heap_ptr().max(pages.dynamic_start());
    let start = align_up(heap_ptr, ZPAGE_SIZE)?;
    let bytes = count.checked_mul(ZPAGE_SIZE)?;
    let end = start.checked_add(bytes)?;
    if end > ctx.heap_limit() {
        return None;
    }
    if end > ctx.env.memory.data_size(&ctx.store) {
        let grow_bytes = end.checked_sub(heap_ptr)?;
        if !matches!(ctx.grow_to_fit_heap_allocation(grow_bytes), Ok(true)) {
            return None;
        }
    }
    ctx.set_heap_ptr(end);
    let alloc_end = ctx.env.memory.data_size(&ctx.store).min(ctx.heap_limit());
    ctx.alloc_window_set(end, alloc_end);
    pages.extend_for_committed_end(alloc_end);
    let start_idx = pages.page_index(start)?;
    pages.activate_page_range(start_idx, count).then_some(start)
}

fn bump(page: &mut BumpPage, size: usize) -> Option<usize> {
    let end = page.cursor.checked_add(size)?;
    if end > page.end {
        return None;
    }
    let ptr = page.cursor;
    page.cursor = end;
    Some(ptr)
}

fn copy_raw_object(
    data: &mut [u8],
    src: usize,
    dest: usize,
    size: usize,
    scratch: &mut Vec<u8>,
) -> bool {
    scratch.clear();
    let Some(bytes) = data.get(src..src.saturating_add(size)) else {
        debug_assert!(false, "ZGC relocate: source object disappeared during copy");
        return false;
    };
    scratch.extend_from_slice(bytes);
    if scratch.len() != size {
        debug_assert!(false, "ZGC relocate: source object copy was truncated");
        return false;
    }
    let Some(out) = data.get_mut(dest..dest.saturating_add(size)) else {
        debug_assert!(
            false,
            "ZGC relocate: destination object range is outside memory"
        );
        return false;
    };
    out.copy_from_slice(scratch);
    true
}

fn release_empty_source_pages(
    ctx: &mut GcContext<'_>,
    pages: &mut ZPageSpace,
    ptr: usize,
    size: usize,
) -> usize {
    let Some(first) = pages.page_index(ptr) else {
        return 0;
    };
    let last_addr = ptr.saturating_add(size.saturating_sub(1));
    let last = pages.page_index(last_addr).unwrap_or(first);
    let mut released = 0usize;
    for idx in first..=last {
        if pages.is_relocation_set(idx)
            && !page_has_live_object(ctx, pages, idx)
            && pages.release(idx)
        {
            released += 1;
        }
    }
    released
}

fn page_has_live_object(ctx: &mut GcContext<'_>, pages: &ZPageSpace, page_idx: usize) -> bool {
    let Some(page_start) = pages.page_start(page_idx) else {
        return false;
    };
    let page_end = page_start.saturating_add(ZPAGE_SIZE);
    let obj_table_ptr = ctx.obj_table_ptr();
    let obj_table_count = ctx.obj_table_count();
    ctx.with_memory(|data| {
        for h in 0..obj_table_count as Handle {
            let Some(entry) = read_entry_from_memory(data, obj_table_ptr, h) else {
                break;
            };
            if entry.is_empty() {
                continue;
            }
            let ptr = entry.ptr() as usize;
            let Some(size) = object_size_from_memory(data, ptr) else {
                continue;
            };
            if ranges_overlap(ptr, ptr.saturating_add(size), page_start, page_end) {
                return true;
            }
        }
        false
    })
}

fn read_entry_from_memory(data: &[u8], obj_table_ptr: usize, h: Handle) -> Option<ZEntry> {
    let slot =
        obj_table_ptr.checked_add(h as usize * constants::HANDLE_TABLE_ENTRY_SIZE as usize)?;
    let bytes: [u8; 4] = data.get(slot..slot + 4)?.try_into().ok()?;
    Some(ZEntry::from(u32::from_le_bytes(bytes)))
}

fn ranges_overlap(a_start: usize, a_end: usize, b_start: usize, b_end: usize) -> bool {
    a_start < b_end && b_start < a_end
}

fn align_up(value: usize, align: usize) -> Option<usize> {
    debug_assert!(align.is_power_of_two());
    value.checked_add(align - 1).map(|v| v & !(align - 1))
}

fn stats_for_result(
    ctx: &mut GcContext<'_>,
    pages: &ZPageSpace,
    started_at: Option<Instant>,
    result: &RelocateResult,
) -> GcStats {
    let metrics = heap_governance::compute_metrics(&pages.free_page_intervals());
    let stats = GcStats {
        marked: result.relocated_objects,
        swept: result.released_pages,
        freed_bytes: result.released_pages.saturating_mul(ZPAGE_SIZE),
        elapsed: started_at.unwrap_or_else(Instant::now).elapsed(),
        free_block_count: metrics.free_block_count,
        total_free_bytes: metrics.total_free_bytes,
        largest_free_block: metrics.largest_free_block,
        external_fragmentation: metrics.external_fragmentation,
        heap_used_bytes: ctx.heap_used(),
        cycle_kind: CycleKind::ZgcCycle,
        relocated_bytes: result.relocated_bytes,
        relocated_objects: result.relocated_objects,
        committed_pages: ctx.committed_pages(),
        free_bytes_reusable: metrics.total_free_bytes,
        ..GcStats::default()
    }
    .with_elapsed_pause();
    ctx.stats = stats.clone();
    stats
}

#[cfg(test)]
#[path = "relocate_tests.rs"]
mod tests;
