# TaskIntentDraft

## Requested outcome

执行 `docs/aegis/plans/2026-07-03-pluggable-gc-v2.md`，落地可插拔 GC v2：默认 mark-sweep、后续 G1/ZGC、生命周期完整算法接口、增量调度、barrier 通道、统一 host 读写层、启动时算法选择与定量 pause 验收。

## Scope

- 父计划：`docs/aegis/plans/2026-07-03-pluggable-gc-v2.md`
- 父规格：`docs/aegis/specs/2026-07-03-pluggable-gc-v2-design.md`
- 当前会话已经完成 P0、P1 T1.1、P1 T1.2；正在执行 P1 T1.3。
- 修改范围覆盖 runtime GC、backend wasm emitter、runtime support ABI、snapshot format 与相关测试。

## Non-goals

- 不创建新的替代计划或改变父计划验收口径。
- 不跳过 fixture/snapshot/ABI/hash 验证。
- 不保留 v1 fallback 或双 owner；完成切换后清理旧路径。

## Risk hints

- env global 顺序是 backend、support module、runtime linker、WasmEnv 共同 ABI，任何变动必须同步。
- snapshot header/wire format 改动必须升级版本并触发 ABI hash 失效。
- INV-C1/C2 是硬约束：值层持 handle，raw ptr 不跨潜在 collect/move 点。
- 分配 fast-path 必须以内联 alloc window 为主，slow-path 只负责 GC/grow/OOM。

## BaselineReadSetHint

必须参考：

- `docs/aegis/plans/2026-07-03-pluggable-gc-v2.md`
- `docs/aegis/specs/2026-07-03-pluggable-gc-v2-design.md`
- `docs/aegis/specs/2026-06-14-pluggable-gc-framework-design.md`
- `docs/adr/0003-startup-snapshot-boundary.md`
- `docs/adr/0004-build-time-embedded-runtime.md`
- `docs/aegis/specs/2026-07-03-napi-native-addon-design.md`

## BaselineUsageDraft

- required refs：父计划、v2 design spec、startup snapshot ADR、embedded runtime ADR、v1 GC framework baseline、N-API design。
- acknowledged refs：父计划、v2 design spec、startup snapshot/embedded runtime 边界（通过代码与计划上下文）。
- cited refs：P0/P1 子任务按父计划 T0.1–T1.3 执行；snapshot 与 support ABI 改动遵守 ADR 0003/0004 边界。
- missing refs：当前无阻塞缺失；进入 N-API 文档同步前需重新读取 N-API spec 对应小节。
- decision：continue。

## ImpactStatementDraft

本任务修改 load-bearing runtime ABI、linear memory layout、GC 分配/扫描边界与 snapshot wire format。影响面包括：

- user wasm 与 support module 的 env globals ABI；
- runtime startup cold/hot path 的 heap boundary 与 GC attach；
- mark-sweep sweep/marker 对 dynamic heap 与 runtime heap tags 的解释；
- backend allocation helpers 与 support module helper body；
- snapshot ABI/hash 与测试基线。
