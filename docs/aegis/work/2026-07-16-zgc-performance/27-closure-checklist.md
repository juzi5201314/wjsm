# Task 27 Closure Checklist

> Parent: `docs/aegis/plans/2026-07-16-zgc-performance.md` Task 27  
> Spec: `docs/aegis/specs/2026-07-16-zgc-performance-design.md`  
> Date: 2026-07-23

## Parent hard gates

| Gate | Owner / evidence | Status |
|------|------------------|--------|
| Unified ManagedHeap on shared memory64 | ADR 0010 §1; runtime heap/*; Task 15 cutover | verified-in-tree |
| 8-byte atomic handle entries | ADR 0010 §2; handle table | verified-in-tree |
| No private `managed-heap-v2` / dual dynamic heap | Task 15/26 residual purge; negative audit | verified-in-tree |
| Generational ZGC (young/old/remset/relocate/director) | Tasks 16–23 + active wiring | verified-in-tree |
| Colored barriers | Task 16 | verified-in-tree |
| Host roots / WeakRef / finalization concurrent cycle | Task 21 | verified-in-tree |
| Legacy GC paths/benchmarks retired | Task 26 | verified-in-tree |
| Three public collectors on same heap | `--gc mark-sweep\|g1\|zgc` + happy matrix | verified (Task 27 GREEN) |
| `wjsm-gc-bench` sole perf entry | Task 1 + Task 26 | verified-in-tree |
| JDK 25 normalized PR matrix (Task 24) | instrumented probe + 30-sample compare/gate | **needs-verification** |
| 4/16 GiB nightly hard isolation (Task 25) | named resource runners | **needs-verification** |
| Capability matrix (AVX-512 / AArch64 / Win / macOS / NUMA) | named capability runners | **needs-verification** (local portable/ISA only) |

## Platform / resource gates

| Gate | Status |
|------|--------|
| Fail-closed preflight (exit 78, no auto-shrink) | verified-in-tree (Task 1/23) |
| Local host only admits portable/actual ISA | verified-in-tree |
| Missing capability/resource never auto-pass | **kept open as needs-*** |
| `.github/workflows/zgc-*.yml` | **retired by user request** — gates remain open until runners reattached |

## Retirement

| Item | Status |
|------|--------|
| memory32 4-byte obj_table main path | deleted |
| `managed-heap-v2` feature/cfg | deleted |
| V1 collector dual path / dyn GcAlgorithm mutex | deleted |
| `gc_stress` / `zgc_autoresearch` / `zgc_barrier_pressure` | deleted |
| dual support `_v2` filenames / forever-true flags | deleted |
| ADR 0005 ownership/concurrency/generation/entry | superseded by ADR 0010 |

## Docs closure targets

- [x] ADR 0010 status reflects cutover complete; Task 24/25 honest `needs-verification`
- [x] ADR 0003 ManagedHeap wire/restore boundary synced
- [x] ADR 0004 support cwasm / engine fingerprint boundary synced
- [x] AGENTS.md WASM/GC/perf/debug only verified facts
- [x] INDEX / checkpoint / evidence / reflection updated
- [x] No dangling refs to deleted workflows as if GREEN

## Full verification commands (Task 27 GREEN)

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo nextest run --workspace
WJSM_TEST_GC=mark-sweep cargo nextest run -E 'test(happy__)'
WJSM_TEST_GC=g1 cargo nextest run -E 'test(happy__)'
WJSM_TEST_GC=zgc cargo nextest run -E 'test(happy__)'
cargo +nightly miri test -p wjsm-runtime --test gc_protocol_miri
RUSTFLAGS="-Zsanitizer=thread" cargo +nightly test -Zbuild-std \
  --target x86_64-unknown-linux-gnu -p wjsm-runtime --test gc_concurrency_model
cargo run -- run --gc zgc -e 'let roots=[]; for(let i=0;i<1e6;i++){let o={i,next:roots[i&1023]}; if((i&255)===0)roots[i&1023]=o;} gc(); console.log(roots.length)'
```

Local results 2026-07-23: all pass (1795 workspace; 666×3 happy; Miri 2; TSan 2; smoke `769`).

## Residual negative audit

```bash
rg -n 'managed-heap-v2|alloc_from_bump|HANDLE_TABLE_ENTRY_SIZE\s*=\s*4|gc_stress|zgc_autoresearch|zgc_barrier_pressure' crates
```

Missing Task 24/25 runner evidence must remain `needs-verification` — never estimated pass.
