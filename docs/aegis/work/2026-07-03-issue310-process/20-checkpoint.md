# TodoCheckpointDraft
- Current todo: 接入 process builtin global；扩展 RuntimeOptions 与 process 状态；接入 process 全局安装入口。
- Completed todos: 建立 issue310 检查点；阅读关键代码入口；完成首轮复杂度检查。
- Active slice: Runtime Core 第一段接线。
- Next step: 先改 semantic builtin global 与 runtime owner 接线，再实现 `runtime_process.rs` 基础状态与安装 helper。

# ResumeStateHint
- 已确认 `create_global_object_fn` 是唯一全局挂接点，`call_native_callable_with_args_from_caller{,_async}` 是新增 `NativeCallable` 的唯一分发入口。
- `run_file_in_process` 当前只有 fixture runner 调用，后续可安全扩签并保留默认 wrapper。

# DriftCheckDraft
- Scope: 仍严格在 issue #310 `process` slice 内。
- Compatibility: 计划中的新增 owner 为 `runtime_process.rs`；不向 `collections_buffers.rs` 与 `types.rs` 继续堆实现细节。
- Retirement: 暂无旧路径要退役，但 CLI 运行时选项构造函数会从单一 `runtime_options(cli)` 拆成按场景 helper。
- Evidence growth: 已读 plan、builtin global、RuntimeState、global object hook、NativeCallable 分发、Proxy trap owner、CLI Run/fixture/integration 入口。
- Decision: continue

Complexity Budget:
- Artifact class: Source Complexity
- Target files / artifacts: `crates/wjsm-semantic/src/builtins.rs`, `crates/wjsm-runtime/src/lib.rs`, `crates/wjsm-runtime/src/runtime_process.rs`, `crates/wjsm-runtime/src/runtime_microtask.rs`, `crates/wjsm-runtime/src/types.rs`, `crates/wjsm-runtime/src/runtime_builtins.rs`, `crates/wjsm-runtime/src/host_imports/collections_buffers.rs`, `crates/wjsm-cli/src/{cli_args.rs,lib.rs}`, `tests/integration/fixtures.rs`, `tests/fixture_runner.rs`, `fixtures/happy/process_basic.*`
- Current pressure: `collections_buffers.rs` 2668 行、`types.rs` 1279 行、`lib.rs` 1585 行；直接加塞会继续恶化 owner 边界。
- Projected post-change pressure: 若把 `process` 逻辑塞进现有大文件将 over-budget；拆出 `runtime_process.rs` 后主压力落在接线与分发，整体为 at-risk 但可治理。
- Budget result: at-risk
- Planned governance: 新建 `runtime_process.rs` 作为唯一 process owner；大文件只保留字段接线、enum 变体、调用入口。

Pre-Edit Complexity Check:
- Safer edit boundary: semantic 仅改 builtin 名单；runtime 新建 `runtime_process.rs` 承载状态/组装/trap/write/exit；CLI 仅做参数和 options 接线。
- Decision: add owner file
