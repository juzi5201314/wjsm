# TodoCheckpointDraft

- current todo: Task 2.0 with_execution_realm + WASM global swap
- active slice: Phase 2
- completed todos:
  - Task 0.1 Realm registry → e0a87ca8
  - Task 0.2 handle_remap → 6a2f71df
  - Task 1.1 + 1.2 pristine clone + closure → (this commit)
- evidence refs:
  - binary(realm_registry) + realm unit → 7 passed
  - binary(handle_remap_kernel)|startup_snapshot_gc_fixes → 7 passed
  - binary(realm_clone) → 3 passed
- blocked-on: none
- next step: Task 2.0 execution_realm frame with array/object proto global swap

# ResumeStateHint

- branch: feat/node-vm-multi-realm
- plan: docs/aegis/plans/2026-07-10-node-vm-multi-realm.md
- clone owner: crates/wjsm-runtime/src/realm_clone.rs
- probe API: wjsm_runtime::probe_clone_pristine_realm

# DriftCheckDraft

- scope: on-plan Phase 0–1 complete
- compatibility: non-vm path unchanged; clone only on explicit probe/vm path
- retirement: none new
- decision: continue
