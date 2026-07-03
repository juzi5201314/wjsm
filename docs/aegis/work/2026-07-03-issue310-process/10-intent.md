# TaskIntentDraft
- Goal: 按已批准方案完整实现 issue #310 的 `process` 基础能力，仅覆盖 `process` 全局对象及其计划列出的成员与行为。
- Scope: semantic builtin global、runtime 宿主快照与 `process` 组装、只读 `process.env` Proxy、`process.nextTick`、`stdout/stderr.write`、`process.exit`、CLI 参数与环境接线、fixture/runtime/integration 验证。
- Non-goals: `Buffer`、`global` alias、`__dirname`/`__filename`、`import.meta.*`、其他 Web Platform globals。
- Risk hints: 需要跨 semantic/runtime/CLI 三层接线；Proxy trap 与退出信号会触及共享 owner；microtask 调度顺序必须保持现有行为不回归。

# BaselineReadSetHint
- Required refs:
  - local://issue310-process-plan.md
  - crates/wjsm-semantic/src/builtins.rs
  - crates/wjsm-runtime/src/lib.rs
  - crates/wjsm-runtime/src/runtime_microtask.rs
  - crates/wjsm-runtime/src/host_imports/collections_buffers.rs
  - crates/wjsm-cli/src/lib.rs

# BaselineUsageDraft
- Acknowledged refs: plan file, builtins.rs, lib.rs, runtime_microtask.rs, collections_buffers.rs, cli lib.rs
- Missing refs: proxy dispatch owner, NativeCallable 枚举与 host object helper 的具体定义位置
- Decision: continue

# ImpactStatementDraft
- Semantic 层新增 builtin global 名字 `process`，复用现有 `$0.$global` 属性读取路径。
- Runtime 层新增 process 宿主快照 owner 与 next-tick 队列，修改全局对象安装和微任务 drain 顺序。
- CLI 层新增 `run ... -- <args>` 参数透传、cwd/env/pid/version 注入与正常退出码传播。
