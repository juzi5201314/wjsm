# ADR 0010: Generational ZGC Managed Heap

**Status**: Accepted for architecture; active runtime cutover (Task 15) and JDK normalized performance gates (Task 24/25) remain open with `needs-verification` where named runners / instrumented JDK 25 probe evidence is missing. This ADR supersedes [ADR 0005](0005-pluggable-gc-v2.md) for ownership, concurrency, generation, and entry decisions.

**Date**: 2026-07-20

**Supersedes**: [ADR 0005: Pluggable GC v2 Boundary](0005-pluggable-gc-v2.md)

**Related**: [ADR 0003](0003-startup-snapshot-boundary.md) (snapshot content/restore), [ADR 0004](0004-build-time-embedded-runtime.md) (support cwasm / engine fingerprint), design `docs/aegis/specs/2026-07-16-zgc-performance-design.md`.

## Context

ADR 0005 established pluggable mark-sweep / G1 / ZGC under a safepoint-budgeted, primarily single-threaded mutator model with a 4-byte `obj_table` entry and host/support dual barrier owners. The ZGC performance program (2026-07-16) proved that design cannot meet Generational ZGC normalized gates against JDK 25:

1. Object heap must be **shared memory64** with real concurrent workers, not mutator-only incremental steps.
2. All collectors must share one **ManagedHeap** owner (pages, NLAB, handle table, bitmaps) — no per-collector dynamic heap dual track.
3. Reference identity must use **8-byte atomic handle entries** (high48 address + low16 metadata/generation) with **reference-only color** bits in the NaN-box ABI.
4. ZGC must be **generational** (young/old concurrent mark, remset, promotion, concurrent relocation) with a pacing director.
5. Performance evidence must come from `wjsm-gc-bench` (preflight, JDK 25 probe, normalized metrics, hard isolation), not Criterion/`zgc_autoresearch`/`zgc_barrier_pressure`.

## Decision

### 1. Unified ManagedHeap (replaces ADR 0005 dynamic heap ownership)

- Single `ManagedHeap<M>` owns virtual address layout: 32 GiB handle reserve, control region, object heap grow, Wasmtime guards.
- Memory backends: `SharedHeapMemory` (product / Wasmtime shared memory64) and `NativeHeapMemory` (Miri / unit tests only).
- Pages are 64 KiB logical commit units; NLAB is heap-relative; large/humongous are contiguous.
- **No** long-lived host raw pointers across GC points; identity is always handle.

### 2. Handle / entry ABI (replaces ADR 0005 4-byte obj_table)

- Handle table entries are **8-byte SeqCst atomics**: high48 address + low16 generation/metadata.
- Color bits live in the **reference NaN-box** (bits 38–43 helpers), not as the sole authority inside raw payload f64.
- Mutable-in-place headers (prototype, length, property count, flags, backing refs) are classified and relocated under explicit owners.
- Active cutover deletes private `managed-heap-v2` feature and the memory32 4-byte main path once Task 15 GREEN; intermediate dual-track is forbidden as a permanent state.

### 3. Concurrency (replaces ADR 0005 “true threads deferred”)

- Fixed-capacity `GcWorkerPool` with packet slab, local/injector/peer-steal queues, inflight drain/park/join.
- Shared heap atomics for published object data are **SeqCst**; Rust-private metadata may use weaker orderings only when proven.
- Loom models packet/termination without linking production queues; Miri uses NativeHeap; TSan covers std atomics + production queues on named runners.
- Host roots, barriers, and side tables publish into concurrent mark/relocation owners — not a second GC context mutex.

### 4. Generational ZGC policy (replaces ADR 0005 non-generational incremental ZGC)

| Owner | Responsibility |
|---|---|
| `zgc::color` / barrier emit | load/store barriers, good color, SATB/buffer flush |
| young controller | concurrent young mark, promotion, remset edges |
| old mark | concurrent old marking |
| relocate | concurrent relocation set, copy/heal, source reclaim |
| remset | precise remembered sets / promotion destinations |
| director / pacing | prediction, debt, stall bounds, uncommit |
| host_roots | strong/weak/finalizer roots integrated with concurrent cycles |

Public GC modes remain `mark-sweep`, `g1`, `zgc` via `RuntimeOptions` / CLI `--gc` / `WJSM_GC`; `WJSM_TEST_GC` is test-matrix only. All three must run on the same ManagedHeap after cutover.

### 5. G1 / mark-sweep under ManagedHeap

- G1 reuses ManagedHeap pages, dual mark bitmaps, worker pool, RSet, telemetry; young/mixed/full policies attach as algorithms, not separate heaps.
- Mark-sweep is the simple policy on the same heap (mark/retire/sweep), not a parallel dynamic-heap implementation.

### 6. Snapshot / support ABI boundary

- Managed-heap snapshot wire format encodes page metadata, 8-byte handle entries, and generation; artifact manifest binds snapshot ABI + engine fingerprint + support ABI.
- ADR 0003 remains authority for **what** may be captured (primordial / no user objects / no scheduler state); managed-heap layout extends the binary format when V2 is active.
- ADR 0004 remains authority for build-time embedded snapshot/support cwasm families; engine fingerprint and support flavor selection must match active ManagedHeap/support ABI.
- Startup snapshot default-on policy of ADR 0003/0004 is unchanged; V2 artifact install is feature/cutover gated until Task 15 completes.

### 7. Performance & platform evidence

- Sole performance entry: `wjsm-gc-bench` (`capabilities`, `preflight`, `prepare-jdk`, `baseline`/`run`, `micro`, `compare`, `replay`, `gate`).
- Normalized gates: five metrics `WJSM <= JDK * 1.10`, at least two `<= JDK * 0.85`, p99.9 ≤ JDK, max pause < 1 ms; missing numerator/resource/patch hash ⇒ `needs-verification` (never estimated pass).
- PR matrix: 32/256/1024 MiB. Nightly: 4/16 GiB with delegated cgroup v2 or Windows Job hard isolation, exclusive sequential WJSM/JDK, child ceiling `2*heap+2 GiB`, 3600s duration scenarios.
- Platform capabilities (ISA/NUMA/decommit) report `needs-capability-runner` when local host cannot close a named capability; auto-skip-as-pass is forbidden.
- Legacy entries **retired**: `gc_stress` Criterion bench, `zgc_autoresearch`, `zgc_barrier_pressure` examples.

## Consequences

### Positive

- One heap/entry/barrier model for all collectors; Generational ZGC can be algorithmically compared to JDK 25 ZGC.
- Fail-closed resource admission prevents host OOM and false GREEN on under-provisioned machines.
- Snapshot/support/engine fingerprint stay auditable across cutover.

### Negative / Risks

- Task 15 single-point cutover is large: incomplete activation leaves workspace `--all-features` RED by design until every V1 producer is migrated.
- Instrumented JDK 25 diagnostic counters require applying `crates/wjsm-gc-bench/jdk-probe/0001-zgc-benchmark-counters.patch` on a JDK 25 GA tree; stock JDK alone cannot GREEN normalized gates.
- Named large/ISA/NUMA runners are mandatory for Task 24/25 GREEN; local WSL evidence is limited to preflight fail-closed and small admitted heaps.

## ADR 0005 mapping (what is superseded)

| ADR 0005 decision | Status under ADR 0010 |
|---|---|
| INV-C1 handle identity | **Kept**, strengthened to 8-byte atomic entries + shared memory64 |
| INV-C2 explicit GC points | **Kept** |
| `GcAlgorithm` attach/alloc/safepoint/barrier hooks | **Kept** as policy layer on ManagedHeap |
| host `heap_access` owner | **Kept** / extended as `HeapAccessV2` during cutover |
| three support cwasm flavors | **Kept** (ADR 0004); ABI must track ManagedHeap |
| non-moving mark-sweep assumption for raw ptrs | **Superseded** — no long-lived raw object ptrs |
| “true concurrent threads deferred” | **Superseded** — fixed worker pool + shared heap |
| non-generational incremental ZGC | **Superseded** — young/old concurrent + remset + relocate |
| Criterion / ad-hoc ZGC examples as evidence | **Superseded** — `wjsm-gc-bench` only |

## Compatibility

- Public CLI/env GC selection remains.
- User-visible JS semantics unchanged; GC is not a language feature.
- Snapshot restore still excludes side tables listed in ADR 0003.
- Until Task 15 cutover GREEN, `managed-heap-v2` may still appear as a private feature in Cargo manifests; retirement of that feature is part of cutover, not a permanent dual ABI.

## Verification status (honest)

| Gate | Status |
|---|---|
| ManagedHeap / workers / barriers / young-old / remset / relocate / director staged | Implemented in tree (Tasks 0–14, 16–23 slices) |
| Task 15 full V2 activation + delete private feature | Open / in progress by activation owners |
| Task 24 JDK 25 normalized 30-sample PR matrix | `needs-verification` (stock JDK lacks diagnostic numerators; full matrix needs dedicated runner window) |
| Task 25 4/16 GiB nightly hard isolation | Workflow + fail-closed preflight in tree; named large runners not registered (`if: false`) |
| Task 26 legacy bench/example retirement | Done (this change set) |
| Task 27 docs/ADR/AGENTS closure | This ADR + AGENTS/evidence update; full fmt/clippy/nextest/Miri/TSan remain blocked on Task 15 |

## References

- Spec: `docs/aegis/specs/2026-07-16-zgc-performance-design.md`
- Plan: `docs/aegis/plans/2026-07-16-zgc-performance.md`
- Evidence: `docs/aegis/work/2026-07-16-zgc-performance/90-evidence.md`
- Bench: `crates/wjsm-gc-bench`
- Workflows: `.github/workflows/zgc-capability-matrix.yml`, `.github/workflows/zgc-nightly.yml`
