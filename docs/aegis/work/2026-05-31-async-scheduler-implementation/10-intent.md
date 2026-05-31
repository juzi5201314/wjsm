# Task Intent: Async Scheduler Redesign Implementation (2026-05-31 Plan)

**Date**: 2026-05-31
**Worktree**: .worktrees/feat-async-scheduler-2026-05-31 (branch feat/async-scheduler-2026-05-31)
**Requested Outcome**: Execute the reviewed implementation plan in docs/aegis/plans/2026-05-31-async-scheduler-implementation-plan.md to completion, following subagent-driven-development (fresh subagent + 2-stage review per task) and rust-style-guide.

**Goal** (from plan): Convert wjsm-runtime to a Wasmtime async-capable execution path (Tokio + async Engine/Store) that can drive main, microtasks, timers, and future async host completions without blocking or corrupting Store<RuntimeState>. Preserve all existing observable behavior for sync wrappers and fixtures.

**Scope / Success Evidence**:
- All 14 Success Criteria in the plan (clean checks, async APIs, preserved ordering/semantics, epoch RAII, completion materialization, CLI fixtures still pass, docs updated).
- No Wasmtime sync call APIs remain on any async Store path.
- RuntimeState remains Send.
- Baseline fetch compile breakage restored as prerequisite hygiene (data: only; no new fetch behavior).

**Stop Condition**: All phases complete, final code reviewer approves, finishing-a-development-branch executed (or explicit user stop).

**Non-Goals** (strict per plan):
- No CLI-native async migration (`#[tokio::main]`, cmd handlers, watch mode).
- No async fetch implementation (only the scheduler + materialization channel shape).
- No changes to IR/semantic/backend.
- No behavior changes to microtask/timer ordering, main error precedence, or fixture outputs.

**Baseline Refs** (must be re-read by every subagent before editing):
- Plan: docs/aegis/plans/2026-05-31-async-scheduler-implementation-plan.md (full text)
- Spec: docs/aegis/specs/2026-05-31-async-scheduler-redesign-design.md
- Current runtime entry: crates/wjsm-runtime/src/lib.rs (execute_with_writer and friends)
- Key callback sites: runtime_microtask.rs, runtime_async_fn.rs, runtime_host_helpers.rs, runtime_values.rs, runtime_eval.rs, host_imports/*
- Wasmtime 43 async contract (instantiate_async, call_async, Func::new_async, Data: Send)
- AGENTS.md (spec compliance, Chinese comments, no PoC compromises)

**Known Facts from Baseline Audit (in worktree)**:
- Pre-existing fetch compile breakage (ureq + incomplete NativeCallable match) was present on branch creation. Restored to compilable state (data: URLs only + wired existing helpers) as Phase 0 prerequisite. See edits in host_imports/fetch.rs and runtime_builtins.rs + mod.rs re-exports. This is not part of the scheduler feature.

**Risks / Unsafe Assumptions**:
- Wasmtime async + epoch yielding will not change main-to-post-main interleaving (plan narrows to run main to completion first).
- Existing timer/microtask code can be lifted to _async variants without semantic drift.
- Tokio block_on in sync wrappers is safe for CLI (documented limitation: not callable from within existing Tokio runtime).

**Verification Commands** (must pass after relevant slices):
- cargo check -p wjsm-runtime
- cargo nextest run -p wjsm-runtime
- Targeted E2E: happy__promise_microtask_order, timer*, eval*, errors__*, modules__*
- Manual: sync wrapper vs async wrapper output identity test (new in Phase 7/9)

**Next**: Create TodoCheckpointDraft, then dispatch Phase 0 implementer (minimal Tokio + wasmtime async dep + Send assertion).
