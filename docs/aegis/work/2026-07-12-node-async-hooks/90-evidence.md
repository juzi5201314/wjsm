# EvidenceBundleDraft — node:async_hooks final（2026-07-13）

## Commands

```bash
cargo build --workspace
# Finished dev profile；零 warning

cargo nextest run --workspace
# Summary [12.531s] 1651 passed, 2 skipped

cargo nextest run -E 'test(modules__node_builtin_async_hooks) | test(happy__async_hooks) | test(happy__async_local) | test(happy__async_resource) | test(happy__set_immediate) | test(errors__async_hooks)'
# 45 passed

cargo nextest run -p wjsm-runtime --test async_scheduler --test async_reentry_audit --test timer_timing
# 15 passed

WJSM_TEST_GC=mark-sweep cargo nextest run -E 'test(happy__async_hooks_destroy) | test(happy__async_resource_gc) | test(happy__async_hooks_load_100k) | test(happy__async_resource_handle_reuse)'
WJSM_TEST_GC=g1 cargo nextest run -E 'test(happy__async_hooks_destroy) | test(happy__async_resource_gc) | test(happy__async_hooks_load_100k) | test(happy__async_resource_handle_reuse)'
WJSM_TEST_GC=zgc cargo nextest run -E 'test(happy__async_hooks_destroy) | test(happy__async_resource_gc) | test(happy__async_hooks_load_100k) | test(happy__async_resource_handle_reuse)'
# 每种算法 3 passed

WJSM_STARTUP_SNAPSHOT=0 cargo nextest run -E 'test(happy__async_hooks_phase_order)'
WJSM_STARTUP_SNAPSHOT=1 cargo nextest run -E 'test(happy__async_hooks_phase_order)'
# cold/snapshot 各 1 passed

cargo nextest run -p wjsm --test cluster_ipc
# 8 passed
```

## Observable contracts verified

- ids/providers、createHook mutation snapshot、init/before/after/destroy/promiseResolve、fatal bypass。
- Timeout/TickObject/Immediate/PROMISE phase order、parent trigger、balanced Promise combinator/finally 尾声。
- AsyncResource constructor/options/runInAsyncScope/bind/emitDestroy/manual+auto destroy/execution resource identity。
- ALS run/enterWith/exit/disable/defaultValue/name/bind/snapshot、`new`/`instanceof`、100k hot path。
- fetch/net/TLS/dgram/fs.promises/worker/child_process 调度时 context 捕获。
- VM realm 共用 AsyncHooksState；startup snapshot capture 排除 runtime-only async state。
- mark-sweep/G1/ZGC roots、host handle 复用、Promise side-table/未处理 rejection owner 生命周期。
- Module Record 同名 var 隔离；top-level await 后 import/模块绑定恢复。

## Architecture evidence

- 单一 owner：`RuntimeState.async_hooks: Arc<Mutex<AsyncHooksState>>`。
- `CapturedScope = frame_id + async_id + trigger_async_id + resource`；所有 user callback owner 统一 enter/emit/exit。
- handle 表容量、GC trigger/window 纳入 snapshot ABI；越界 guard 以 immutable barrier-buffer 基址计算。
- ADR：`docs/adr/0009-async-hooks-host-core.md`。

## Residual risk

- 计划内无未闭合项；明确非目标仍为 Node 新增 `withScope` 与 `AsyncResource.domain`。
