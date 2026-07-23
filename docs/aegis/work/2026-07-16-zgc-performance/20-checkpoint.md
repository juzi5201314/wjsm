# TodoCheckpointDraft

## Current todo

**Task 27 GREEN（local 闭环）已完成。** 计划主路径 Tasks 0–27 local 文档与验证闭环结束。

仍开放的外部门（**不得**记为通过）：

- Task 24 JDK 25 归一 30-sample PR 矩阵 → `needs-verification`
- Task 25 4/16 GiB hard isolation + 具名 capability runners → `needs-verification`
- `.github/workflows/zgc-*.yml` 已按用户要求删除；gate 合同仍在 `wjsm-gc-bench`

## Completed (this session, 2026-07-23 Task 27)

### RED / 阻断修复
- Closure checklist：`27-closure-checklist.md`
- clippy `-D warnings` 阻断：dead_code helper、empty_line_after_doc_comments、
  duplicated `#![allow(dead_code)]` on mark/relocate、unused braces、needless_return、
  `clone_on_copy` for `GcAlgorithmKind`
- fmt 偏差清理

### GREEN 命令（实测）
```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo nextest run --workspace
  → 1795 passed, 17 skipped
WJSM_TEST_GC=mark-sweep|g1|zgc cargo nextest run -E 'test(happy__)'
  → 各 666 passed
cargo +nightly miri test -p wjsm-runtime --test gc_protocol_miri
  → 2 passed
RUSTFLAGS="-Zsanitizer=thread" cargo +nightly test -Zbuild-std \
  --target x86_64-unknown-linux-gnu -p wjsm-runtime --test gc_concurrency_model
  → 2 passed
cargo run -- run --gc zgc -e '…1e6 churn + gc()…'
  → stdout 769
```

### Docs / ADR
- ADR 0010 改写：cutover complete；Task 24/25 诚实 needs-verification
- ADR 0003 / 0004 status 同步 ManagedHeap wire 与 support/engine fingerprint
- AGENTS.md WASM contract 写明 memory32 主存 + memory64 对象堆；GC 段落对齐退役事实
- INDEX 去掉已删除 workflow 伪条目；登记 Task 27 checklist
- 计划 Task 27 checkbox 勾选

## Next step

计划 local 执行完成。后续仅在具备 instrumented JDK 25 + 具名 hard-isolation/capability
runner 时重开 Task 24/25 证据，不得在本机估计通过。

可选：`finishing-a-development-branch` 做 PR/merge 选择。

## ResumeStateHint

读本文件 + ADR 0010 + `27-closure-checklist.md` + 提交
`docs: record generational ZGC architecture`。

## DriftCheckDraft

- 范围：Task 27 全量验证 + ADR/AGENTS/INDEX 闭环；仅修验证阻断，不扩 runtime 语义。
- 兼容：公开 `--gc` / snapshot 内容边界 / support 三 flavor 不变。
- 退役：文档不再声称 private feature 未切完；workflow 文件不伪 GREEN。
- 决策：`continue` 结束本计划 local 执行；外部 perf/platform 门 `needs-verification`。
