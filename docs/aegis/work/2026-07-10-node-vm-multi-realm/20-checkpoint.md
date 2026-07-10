# TodoCheckpointDraft

- current todo: Task 6.2 full workspace regression (partial: targeted done)
- active slice: completion candidate
- completed todos:
  - Phase 0–5 full
  - 6.1 codegen/non-goals/microtask afterEvaluate
  - 6.3 ADR 0008 + INDEX + AGENTS
- evidence refs:
  - happy__vm_* 6 passed
  - errors__vm_unsupported passed
  - modules__node_builtin_vm passed
  - reclaim_dead_realms unit + realm_clone probes
- blocked-on: optional full `cargo nextest run --workspace` (long)
- next step: optional full workspace GC matrix; finishing branch

# ResumeStateHint

- branch: feat/node-vm-multi-realm
- tip: 6abaef6b feat(vm): codegen flags, non-goal errors, ADR 0008

# DriftCheckDraft

- scope: plan essentially complete; compileFunction full wiring deferred
- compatibility: single-realm path unchanged
- retirement: remap walker dual-policy
- decision: continue (completion candidate)
