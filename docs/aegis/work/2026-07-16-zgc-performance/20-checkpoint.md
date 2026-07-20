# TodoCheckpointDraft

## Current todo

Task 24–27 verification/retirement slice landed by PrettyAntlion with honest open gates. Task 15 full V2 activation remains the primary runtime blocker owned by activation peers.

## Completed (this slice)

- Task 24 infrastructure: `wjsm-gc-bench compare` accepts `--heap` / `--duration`; PR preflight admits 32/256/1024 MiB on this host; stock JDK 25 path records `needs-verification` without diagnostic numerators.
- Task 25: `.github/workflows/zgc-nightly.yml` (build/run separation, 4/16 GiB named runners `if: false`, fail-closed local exit 78 smoke); nightly `gate --profile nightly` checks hard isolation / 3600s / child ceiling evidence.
- Task 26: deleted `gc_stress` bench, `zgc_autoresearch` / `zgc_barrier_pressure` examples; `autoresearch.sh` points at `wjsm-gc-bench`.
- Task 27 docs: ADR 0010 supersedes 0005; ADR 0003/0004 status cross-links; AGENTS ManagedHeap section; INDEX entries.

## Active slice

Task 15 activation (other agents): complete single V2 runtime owner cutover, then re-run Task 24/25/27 full GREEN commands.

## Evidence refs

- `docs/aegis/work/2026-07-16-zgc-performance/90-evidence.md` (Task 24–27 sections)
- `docs/adr/0010-generational-zgc-managed-heap.md`
- `.github/workflows/zgc-nightly.yml`

## Blocked on

- Instrumented JDK 25 GA with `0001-zgc-benchmark-counters.patch` applied (Task 24 GREEN).
- Named large runners with delegated cgroup v2 / Job isolation + exclusive lock (Task 25 GREEN).
- Task 15 cutover GREEN before workspace full nextest / feature retirement audit can close.

## Next step

Activation owners finish Task 15; then re-run compare 30-sample PR matrix on dedicated runner and enable nightly large jobs when runners register.

## ResumeStateHint

Read this file + Task 24–27 evidence sections + ADR 0010. Do not treat local preflight-only evidence as performance GREEN.

## DriftCheckDraft

- Scope: Tasks 24–27 do not claim JDK normalized GREEN or large-heap GREEN without named runner evidence.
- Compatibility: no dual-heap fallback; private feature retirement still Task 15.
- Retirement: legacy bench/examples deleted; residual `managed-heap-v2` cfg remains until cutover.
