# Reflection — 2026-07-16 ZGC Performance Plan

## What shipped

- 统一 ManagedHeap + shared memory64 + 8-byte handle 成为唯一动态对象堆路径。
- Generational ZGC（barrier / young / remset / old / relocate / director）与三 collector 同底座。
- `wjsm-gc-bench` 成为唯一性能证据入口；旧 Criterion/example 退役。
- ADR 0010 取代 ADR 0005 的所有权、并发、分代与 entry 决策；ADR 0003/0004 与 AGENTS 同步。

## What remains open (honest)

- Task 24/25 性能与大堆/平台门：缺 instrumented JDK 与 hard-isolation/capability runners → `needs-verification`。
- CI workflow 文件已按操作者要求删除；合同仍在 bench CLI，不可因无 YAML 而记通过。

## Process notes

- Task 15 单点 cutover 正确；中间双轨会破坏公开 `--gc` 一致性。
- 全量验证门（clippy `-D warnings`）暴露的是收尾卫生债，应在 Task 27 一并清掉，不留“docs only”虚假闭环。
- TSan 必须 `-Zbuild-std` + 显式 target，否则 ABI mismatch 被误读为 race。

## Stop condition

Local plan Tasks 0–27 documentation and verification closure is done.
External perf/platform evidence is explicitly not claimed.
