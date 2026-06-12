# Intent: Fix 9 Failing Fixtures

- **Outcome:** Execute `docs/aegis/plans/2026-06-10-fix-9-failing-fixtures.md` so the listed ECMAScript compliance fixture failures are fixed and the workspace suite returns to green.
- **Scope:** Internal semantic lowering, wasm backend codegen, and runtime fixes for labeled control flow, for-of iterator cleanup/re-entry, eval exception/TDZ behavior, Proxy invariants, timer callback validation, and class private method access if feasible.
- **Non-goals:** Public API changes, NaN-box format changes, IR instruction set changes, WASM import/export contract changes, or broad refactors outside the proven failing paths.
- **BaselineReadSetHint:** `AGENTS.md` lines 214-222; plan file `docs/aegis/plans/2026-06-10-fix-9-failing-fixtures.md`; relevant code and IR/WAT per task before editing.
- **ImpactStatementDraft:** Expected impact is internal spec compliance fixes. Highest-risk areas are structured control flow, exception propagation, iterator close semantics, and direct eval scope behavior.
