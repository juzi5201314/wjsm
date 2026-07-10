# TaskIntentDraft

- Goal: 完整实现 `node:vm` 多 realm 沙箱（单堆多 realm、pristine 克隆、execution_realm、timeout、条件 GC root），覆盖 issue #313 原非目标。
- Scope: `wjsm-runtime` realm 基础设施 + 克隆 + eval 执行帧 + node:vm builtin/API + GC + timeout + ADR 0008。
- Non-goals: SourceTextModule/SyntheticModule、measureMemory 完整语义、importModuleDynamically 完整语义、跨线程 realm、安全沙箱语义。
- Risk hints: remap 语义分叉（函数表 idx vs handle map）；epoch 已被 async-yield 占用；eval_cache 键含 data_base；构造器路径须读 execution_realm。
- Parent plan: `docs/aegis/plans/2026-07-10-node-vm-multi-realm.md`
- Parent design: `docs/aegis/specs/2026-07-10-node-vm-multi-realm-design.md`
- Branch: `feat/node-vm-multi-realm`

# BaselineReadSetHint

- Required refs:
  - docs/aegis/plans/2026-07-10-node-vm-multi-realm.md
  - issue #313 / ADR 0002/0003/0004/0005
  - crates/wjsm-runtime/src/lib.rs (RuntimeState)
  - crates/wjsm-runtime/src/runtime_gc/roots.rs
  - crates/wjsm-runtime/src/startup_snapshot_remap.rs
  - crates/wjsm-runtime/src/runtime_eval.rs
  - crates/wjsm-runtime/src/runtime_startup.rs

# BaselineUsageDraft

- Required baseline refs: plan、ADR 0002–0005、RuntimeState/roots/snapshot remap/eval/startup
- Acknowledged before plan refs: plan 审查修正 1–12；RuntimeState 扁平字段布局；roots 显式 primordial 列表；ErrorPrototypes + TypedArray COUNT=11
- Cited in plan refs: 全部 Phase 0–6 任务
- Missing refs: 无阻塞
- Decision: continue

# ImpactStatementDraft

- 新增 `realm.rs` / 后续 `handle_remap.rs` / `realm_clone.rs` / `runtime_node_vm.rs` / `node_vm.js`
- RuntimeState 新增 active_realms / next_realm_id / execution_realm（及后续 contextified side table）
- 兼容边界：execution_realm=0 时单 realm 行为不变
