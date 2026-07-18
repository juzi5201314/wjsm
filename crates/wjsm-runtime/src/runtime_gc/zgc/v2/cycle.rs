use std::collections::VecDeque;
use std::time::Duration;

use crate::heap::{AllocatorError, HandleId, HeapAddress};
use crate::runtime_gc::api::{CycleKind, GcStats};

use super::{ZgcV2, ZgcV2Error, ZgcV2Phase, ZgcV2Report, ZgcV2StepOutcome};

impl ZgcV2 {
    pub(super) fn mark_step(
        &self,
        work_budget: usize,
        mut cleanup: impl FnMut(HandleId),
    ) -> Result<ZgcV2StepOutcome, ZgcV2Error> {
        let budget = work_budget.max(1);
        let mut worked = 0;
        while worked < budget {
            let Some(handle) = self.state.lock().pending_mark.pop_front() else {
                break;
            };
            let object = match self.objects.lock().get(&handle).cloned() {
                Some(object) => object,
                None => continue,
            };
            if self.state.lock().marked.contains(&handle) {
                continue;
            }
            self.heap
                .allocator()
                .mark_current(object.allocation.object())?;
            let mut state = self.state.lock();
            if !state.marked.insert(handle) {
                continue;
            }
            for reference in object.references.into_iter().flatten() {
                if !state.marked.contains(&reference) {
                    state.pending_mark.push_back(reference);
                }
            }
            worked += usize::try_from(object.allocation.bytes())
                .expect("48-bit managed heap object size fits usize");
        }
        if !self.state.lock().pending_mark.is_empty() {
            return Ok(self.progress(ZgcV2Phase::Mark));
        }
        self.finish_mark(&mut cleanup)
    }

    fn finish_mark(
        &self,
        cleanup: &mut impl FnMut(HandleId),
    ) -> Result<ZgcV2StepOutcome, ZgcV2Error> {
        let marked = {
            let mut state = self.state.lock();
            state.phase = ZgcV2Phase::Relocate;
            state.marked.clone()
        };
        let dead = self
            .objects
            .lock()
            .keys()
            .filter(|handle| !marked.contains(handle))
            .copied()
            .collect::<Vec<_>>();
        let mut report = self.state.lock().report;
        report.marked = marked.len();
        self.retire_dead(&dead, &mut report, cleanup)?;
        let live = {
            let objects = self.objects.lock();
            marked
                .into_iter()
                .filter(|handle| objects.contains_key(handle))
                .collect::<VecDeque<_>>()
        };
        let remaining = live.len();
        let mut state = self.state.lock();
        state.pending_relocation = live;
        state.report = report;
        drop(state);
        if remaining == 0 {
            self.finish_cycle()
        } else {
            Ok(self.progress(ZgcV2Phase::Relocate))
        }
    }

    pub(super) fn relocate_step(
        &self,
        work_budget: usize,
        _cleanup: impl FnMut(HandleId),
    ) -> Result<ZgcV2StepOutcome, ZgcV2Error> {
        let budget = work_budget.max(1);
        let mut worked = 0;
        while worked < budget {
            let Some(handle) = self.state.lock().pending_relocation.pop_front() else {
                return self.finish_cycle();
            };
            let mut report = self.state.lock().report;
            if let Some(bytes) = self.relocate_one(handle, &mut report)? {
                worked +=
                    usize::try_from(bytes).expect("48-bit managed heap object size fits usize");
            }
            self.state.lock().report = report;
        }
        if self.state.lock().pending_relocation.is_empty() {
            self.finish_cycle()
        } else {
            Ok(self.progress(ZgcV2Phase::Relocate))
        }
    }

    fn relocate_one(
        &self,
        handle: HandleId,
        report: &mut ZgcV2Report,
    ) -> Result<Option<u64>, ZgcV2Error> {
        let source = self
            .objects
            .lock()
            .get(&handle)
            .map(|object| object.allocation.clone())
            .ok_or(ZgcV2Error::UnknownHandle(handle))?;
        let destination = match self.heap.allocate(&mut self.nlab.lock(), source.bytes()) {
            Ok(allocation) => allocation,
            Err(AllocatorError::OutOfPages { .. }) => {
                report.relocation_deferred += 1;
                return Ok(None);
            }
            Err(error) => return Err(error.into()),
        };
        if let Err(error) = self.grow_for(&destination) {
            self.discard_allocation(&destination);
            return Err(error);
        }
        let payload = match self
            .heap
            .memory()
            .copy_to(HeapAddress::new(source.object().offset()), source.bytes())
        {
            Ok(payload) => payload,
            Err(error) => {
                self.discard_allocation(&destination);
                return Err(ZgcV2Error::Memory(error.to_string()));
            }
        };
        if let Err(error) = self
            .heap
            .memory()
            .copy_from(HeapAddress::new(destination.object().offset()), &payload)
        {
            self.discard_allocation(&destination);
            return Err(ZgcV2Error::Memory(error.to_string()));
        }
        if let Err(error) = self.heap.allocator().mark_current(destination.object()) {
            self.discard_allocation(&destination);
            return Err(error.into());
        }
        if let Err(error) = self.handles.begin_relocation(handle) {
            self.discard_allocation(&destination);
            return Err(error.into());
        }
        self.handles
            .complete_relocation(handle, destination.object().offset())?;
        self.objects
            .lock()
            .get_mut(&handle)
            .ok_or(ZgcV2Error::UnknownHandle(handle))?
            .allocation = destination;
        self.release_allocation(&source, report)?;
        report.relocated += 1;
        report.relocated_bytes = report.relocated_bytes.saturating_add(source.bytes());
        Ok(Some(source.bytes()))
    }

    fn retire_dead(
        &self,
        handles: &[HandleId],
        report: &mut ZgcV2Report,
        cleanup: &mut impl FnMut(HandleId),
    ) -> Result<(), ZgcV2Error> {
        let mut retired = Vec::with_capacity(handles.len());
        {
            let mut objects = self.objects.lock();
            for handle in handles {
                let object = objects
                    .remove(handle)
                    .ok_or(ZgcV2Error::UnknownHandle(*handle))?;
                self.handles.retire(*handle)?;
                report.reclaimed_bytes = report
                    .reclaimed_bytes
                    .saturating_add(object.allocation.bytes());
                self.release_allocation(&object.allocation, report)?;
                report.retired += 1;
                retired.push(*handle);
            }
        }
        for handle in retired {
            cleanup(handle);
        }
        self.handles.advance_epoch();
        self.handles.reclaim_quarantine();
        Ok(())
    }

    fn finish_cycle(&self) -> Result<ZgcV2StepOutcome, ZgcV2Error> {
        let (report, elapsed) = {
            let mut state = self.state.lock();
            let elapsed = state
                .started_at
                .map(|started_at| started_at.elapsed())
                .unwrap_or(Duration::ZERO);
            let report = state.report;
            *state = Default::default();
            (report, elapsed)
        };
        self.record_collection(report, elapsed);
        Ok(ZgcV2StepOutcome::CycleComplete(report))
    }

    fn progress(&self, phase: ZgcV2Phase) -> ZgcV2StepOutcome {
        let state = self.state.lock();
        let remaining = match phase {
            ZgcV2Phase::Idle => 0,
            ZgcV2Phase::Mark => state.pending_mark.len(),
            ZgcV2Phase::Relocate => state.pending_relocation.len(),
        };
        ZgcV2StepOutcome::Progress { phase, remaining }
    }

    fn record_collection(&self, report: ZgcV2Report, elapsed: Duration) {
        let mut stats = GcStats {
            marked: report.marked,
            swept: report.retired,
            freed_bytes: usize::try_from(report.reclaimed_bytes)
                .expect("48-bit managed heap byte count fits usize"),
            elapsed,
            cycle_kind: CycleKind::ZgcCycle,
            relocated_bytes: usize::try_from(report.relocated_bytes)
                .expect("48-bit managed heap byte count fits usize"),
            relocated_objects: report.relocated,
            ..GcStats::default()
        };
        stats.record_pause(elapsed);
        self.telemetry.record_cycle("zgc-v2", &stats);
    }
}
