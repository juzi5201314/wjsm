# ADR 0015: Builtin 段 native 镜像复用

## Status

Accepted

## Context

`happy__node_builtin_perf_hooks_*` 每个 fixture 都要把约 236 个 builtin 函数整包 Cranelift codegen，因此 `.config/nextest.toml` 用 `threads-required = 'num-cpus'` 独占执行槽。#344 已经把 builtin 闭包 IR 缓存到 `${WJSM_CACHE_DIR}/builtin_ir/<key>.bin`，但合并后的 `PortableArtifact` 仍按整包 digest 编一份 native image，用户源码一变就 miss。

用户可分发的 `.wjsm` 必须继续是合并 Program：builtin 函数在前、用户函数在后，不新增 section。运行时需要把同一份合并 Program 切成两段独立 image，让 builtin 机器码按 frontier IR digest 跨 fixture 复用，同时两份 image 共享一份按名字分区编号的 `variables` 表。

## Decision

`.wjsm` 仍是合并 Program（builtin 函数在前、用户函数在后），不新增 section、不改 `PortableArtifact::from_input` / `decode`。

运行时按 `$builtin_main` 把合并 Program 切成两段，分别 codegen 成两份 `CompiledImage`。builtin image 的 cache key 用 frontier IR digest，不绑用户 artifact digest。

两份 image 共存于同一 `NativeAgentState`；`activate_image` 只换函数表 / 常量 / function metadata，**不**换 `variables`。`NativeAgentState.variables` 是 agent 级单表。槽号按段分区：builtin 名字占据 `0..B`，用户独有名字从 `B` 起，避免用户名字按字典序插入后打乱旧 builtin image 的槽号。

模块顶层执行：用户 `$module_main` 入口块第一条 Call 进入 `$builtin_main`；`execute` 只调用户 `$module_main`。不再把 `$builtin_main` 函数体 inline 进 `$module_main`。

TLA / `WJSM_NO_BUILTIN_CACHE` / 无 frontier 仍走整包单 image 路径（与 #344 回退条件一致）。找不到 `$builtin_main`、或其不是 builtin 段末函数时，`split_builtin_segment` 返回 `None`，runtime 回退整包单 image。

用户段里指向用户函数的 `Constant::FunctionRef` 改写成段内下标（从 0 起）；指向 builtin 的引用改写成 `user_count + 合并下标`。runtime 在用户 image 下看到 `function_index >= user_function_count` 时映射到 builtin image。

`function_slots` 不再把两段共用的模块槽列入 callee-save，避免 Call `$builtin_main` 返回时把共享模块变量恢复成调用前的 `undefined`。用户嵌套作用域局部量仍 callee-save。eval / vm / 动态 `import` 的整包 image 使用隔离 `variables` 表，不覆盖 agent 共享表。

生产 `lower_artifact_input` 走 `lower_bundle_cached`，使 in-process fixture 真走到分段。`execute_module_bundle` 保持整包编译。

## Consequences

同一 frontier 的 builtin 机器码可跨用户源码复用，perf_hooks fixture 不再需要独占 nextest 执行槽。`.wjsm` 编码与可分发语义不变。整包回退路径继续覆盖 TLA、禁用缓存和坏 IR 布局。共享 `variables` 表让 `$builtin_main` 写入的模块槽在切回用户 image 后仍然可见。
