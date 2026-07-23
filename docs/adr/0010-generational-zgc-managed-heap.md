# ADR 0010: Generational ZGC Managed Heap

**Status**: Accepted. Active runtime cutover is complete: unified ManagedHeap on shared memory64 is the sole dynamic object-heap path for mark-sweep, G1, and ZGC. JDK-normalized performance matrices (Task 24) and isolated 4/16 GiB / named-capability nightly (Task 25) remain **`needs-verification`** until instrumented JDK 25 probe evidence and hard-isolation runners are available. This ADR supersedes [ADR 0005](0005-pluggable-gc-v2.md) for ownership, concurrency, generation, and entry decisions.

**Date**: 2026-07-20  
**Amended**: 2026-07-23 (Task 27 closure)

**Supersedes**: [ADR 0005: Pluggable GC v2 Boundary](0005-pluggable-gc-v2.md)

**Related**: [ADR 0003](0003-startup-snapshot-boundary.md) (snapshot content/restore), [ADR 0004](0004-build-time-embedded-runtime.md) (support cwasm / engine fingerprint), design `docs/aegis/specs/2026-07-16-zgc-performance-design.md`, plan `docs/aegis/plans/2026-07-16-zgc-performance.md`.

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
- Active tree has **one** dynamic heap owner. Private `managed-heap-v2` feature, memory32 4-byte main path, and dual support `_v2` filenames are deleted.

### 2. Handle / entry ABI (replaces ADR 0005 4-byte obj_table)

- Handle table entries are **8-byte SeqCst atomics**: high48 address + low16 generation/metadata (`HANDLE_TABLE_ENTRY_SIZE = 8`).
- Color bits live in the **reference NaN-box** (bits 38–43 helpers), not as the sole authority inside raw payload f64.
- Mutable-in-place headers (prototype, length, property count, flags, backing refs) are classified and relocated under explicit owners.
- Static main memory remains memory32 for code/data/strings; dynamic objects live only in imported shared memory64.

### 3. Concurrency (replaces ADR 0005 “true threads deferred”)

- Fixed-capacity `GcWorkerPool` with packet slab, local/injector/peer-steal queues, inflight drain/park/join.
- Shared heap atomics for published object data are **SeqCst**; Rust-private metadata may use weaker orderings only when proven.
- Loom models packet/termination without linking production queues; Miri uses NativeHeap; TSan covers std atomics + production queues (`-Zbuild-std` + target triple).
- Host roots, barriers, and side tables publish into concurrent mark/relocation owners — not a second GC context mutex wrapping whole algorithms.

### 4. Generational ZGC policy (replaces ADR 0005 non-generational incremental ZGC)

| Owner | Responsibility |
|---|---|
| `zgc::color` / barrier emit | load/store barriers, good color, SATB/buffer flush |
| young controller | concurrent young mark, promotion, remset edges |
| old mark | concurrent old marking |
| concurrent relocate | relocation set, copy/heal, source reclaim |
| remset | precise remembered sets / promotion destinations |
| director / pacing | prediction, debt, stall bounds, uncommit |
| host_roots | strong/weak/finalizer roots integrated with concurrent cycles |
| `active_zgc::collect_dispatch` | public `--gc zgc` full collect entry on ManagedHeap |

Public GC modes remain `mark-sweep`, `g1`, `zgc` via `RuntimeOptions` / CLI `--gc` / `WJSM_GC`; `WJSM_TEST_GC` is test-matrix only. All three run on the same ManagedHeap.

### 5. G1 / mark-sweep under ManagedHeap

- G1 reuses ManagedHeap pages, dual mark bitmaps, worker pool, RSet, telemetry; young/mixed/full policies attach as algorithms, not separate heaps.
- Mark-sweep is the simple policy on the same heap (mark/retire/sweep), not a parallel dynamic-heap implementation.

### 6. Snapshot / support ABI boundary

- Managed-heap snapshot wire format encodes page metadata, 8-byte handle entries, and generation; artifact manifest binds snapshot ABI + engine fingerprint + support ABI.
- ADR 0003 remains authority for **what** may be captured (primordial / no user objects / no scheduler state); ManagedHeap layout extends the binary format.
- ADR 0004 remains authority for build-time embedded snapshot/support cwasm families; `wjsm-engine-config` is the sole Wasmtime `Config` owner (threads, memory64, multi-memory, Cranelift fingerprint).
- Support module imports shared memory64 + ManagedHeap host helpers; three GC flavors still produce distinct support cwasm for barrier/layout differences, selected at startup by active algorithm.
- Startup snapshot default-on policy of ADR 0003/0004 is unchanged; cold bootstrap remains the only ABI-mismatch fallback (no runtime dual-heap fallback).

### 7. Performance & platform evidence

- Sole performance entry: `wjsm-gc-bench` (`capabilities`, `preflight`, `prepare-jdk`, `baseline`/`run`, `micro`, `compare`, `replay`, `gate`).
- Normalized gates: five metrics `WJSM <= JDK * 1.10`, at least two `<= JDK * 0.85`, p99.9 ≤ JDK, max pause < 1 ms; missing numerator/resource/patch hash ⇒ `needs-verification` (never estimated pass).
- PR matrix: 32/256/1024 MiB. Nightly: 4/16 GiB with delegated cgroup v2 or Windows Job hard isolation, exclusive sequential WJSM/JDK, child ceiling `2*heap+2 GiB`, 3600s duration scenarios.
- Platform capabilities (ISA/NUMA/decommit) report `needs-capability-runner` when local host cannot close a named capability; auto-skip-as-pass is forbidden.
- Legacy entries **retired**: `gc_stress` Criterion bench, `zgc_autoresearch`, `zgc_barrier_pressure` examples.
- GitHub workflow YAML for capability/nightly matrices was removed from the tree by operator request; gate contracts and fail-closed preflight remain in `wjsm-gc-bench` and are still required when runners reattach.

## Consequences

### Positive

- One heap/entry/barrier model for all collectors; Generational ZGC can be algorithmically compared to JDK 25 ZGC.
- Fail-closed resource admission prevents host OOM and false GREEN on under-provisioned machines.
- Snapshot/support/engine fingerprint stay auditable after cutover without dual ABI.

### Negative / Risks

- Instrumented JDK 25 diagnostic counters require applying `crates/wjsm-gc-bench/jdk-probe/0001-zgc-benchmark-counters.patch` on a JDK 25 GA tree; stock JDK alone cannot GREEN normalized gates.
- Named large/ISA/NUMA runners are mandatory for Task 24/25 GREEN; local WSL evidence is limited to preflight fail-closed, protocol tests, and small admitted heaps.
- Historical V1 mark/relocate protocol modules may remain for unit tests; they are not the active collect path and must not reintroduce memory32 object-heap ownership.

## ADR 0005 mapping (what is superseded)

| ADR 0005 decision | Status under ADR 0010 |
|---|---|
| INV-C1 handle identity | **Kept**, strengthened to 8-byte atomic entries + shared memory64 |
| INV-C2 explicit GC points | **Kept** |
| Policy hooks attach/alloc/safepoint/barrier | **Kept** as policy layer on ManagedHeap |
| host `heap_access` owner | **Kept** as ManagedHeap `HeapAccess` |
| three support cwasm flavors | **Kept** (ADR 0004); ABI tracks ManagedHeap |
| non-moving mark-sweep assumption for raw ptrs | **Superseded** — no long-lived raw object ptrs |
| “true concurrent threads deferred” | **Superseded** — fixed worker pool + shared heap |
| non-generational incremental ZGC | **Superseded** — young/old concurrent + remset + relocate |
| Criterion / ad-hoc ZGC examples as evidence | **Superseded** — `wjsm-gc-bench` only |
| private feature dual track | **Superseded** — feature deleted; single active ABI |

## Compatibility

- Public CLI/env GC selection remains (`--gc` / `WJSM_GC` / `WJSM_TEST_GC`).
- User-visible JS semantics unchanged; GC is not a language feature.
- Snapshot restore still excludes side tables listed in ADR 0003.
- No runtime fallback to memory32 dynamic heap or 4-byte entries.

## Verification status (honest)

| Gate | Status |
|---|---|
| ManagedHeap / workers / barriers / young-old / remset / relocate / director | Implemented and active (Tasks 0–23) |
| Task 15 full activation + delete private feature / dual path residual purge | GREEN (cutover + Task 26 residual purge) |
| Task 24 JDK 25 normalized 30-sample PR matrix | **`needs-verification`** (instrumented probe / dedicated runner window) |
| Task 25 4/16 GiB nightly hard isolation + capability runners | **`needs-verification`** (named runners not attached; workflows removed by request) |
| Task 26 legacy bench/example / dual-path retirement | GREEN |
| Task 27 docs/ADR/AGENTS + full local verification suite | GREEN for local commands; perf/platform gates still open as above |

### Local Task 27 command evidence (2026-07-23)

```text
cargo fmt --all -- --check                          # pass
cargo clippy --workspace --all-targets --all-features -- -D warnings  # pass
cargo nextest run --workspace                       # 1795 passed, 17 skipped
WJSM_TEST_GC=mark-sweep cargo nextest run -E 'test(happy__)'  # 666 passed
WJSM_TEST_GC=g1 cargo nextest run -E 'test(happy__)'          # 666 passed
WJSM_TEST_GC=zgc cargo nextest run -E 'test(happy__)'         # 666 passed
cargo +nightly miri test -p wjsm-runtime --test gc_protocol_miri  # 2 passed
RUSTFLAGS="-Zsanitizer=thread" cargo +nightly test -Zbuild-std \
  --target x86_64-unknown-linux-gnu -p wjsm-runtime --test gc_concurrency_model  # 2 passed
cargo run -- run --gc zgc -e '…1e6 churn + gc()…'  # stdout: 769
```

## References

- Spec: `docs/aegis/specs/2026-07-16-zgc-performance-design.md`
- Plan: `docs/aegis/plans/2026-07-16-zgc-performance.md`
- Evidence: `docs/aegis/work/2026-07-16-zgc-performance/90-evidence.md`
- Closure checklist: `docs/aegis/work/2026-07-16-zgc-performance/27-closure-checklist.md`
- Bench: `crates/wjsm-gc-bench`
