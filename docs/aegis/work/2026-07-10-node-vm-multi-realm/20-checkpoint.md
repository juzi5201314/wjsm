# TodoCheckpointDraft — node:vm multi-realm

## Status

**Decision: complete**（2026-07-11 closeout）

## Completed

- Phase 0–5：架构 / createContext / Script / timeout / GC / compileFunction（既有提交 + ADR 0008）
- Task 6.1：`codeGeneration.strings` / `contextCodeGeneration` + `microtaskMode: afterEvaluate` + fixtures
- Task 6.2：zero-warning workspace build；`cargo nextest run --workspace` 1601 passed；GC 矩阵 mark-sweep/g1/zgc 对 vm fixtures 11/11
- Function 构造器 + eval/Function codegen 门控
- context free-var / sandbox 内建 / eval IR completion 修复

## Active

无

## Evidence refs

- `docs/aegis/work/2026-07-10-node-vm-multi-realm/90-evidence.md`
- ADR：`docs/adr/0008-node-vm-multi-realm.md`
- Plan：`docs/aegis/plans/2026-07-10-node-vm-multi-realm.md`

## Blocked-on

无

## Next

- 可选：向 #313 回写 vm multi-realm 子范围完成说明（不 close #313）
- finishing branch：用户选择 merge/PR/keep
