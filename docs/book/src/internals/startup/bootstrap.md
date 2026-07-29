# Bootstrap 阶段

这一章说明冷启动时 builtin JS bundle 的执行流程。

## 冷启动触发条件

冷启动发生在以下情况：

- embedded startup snapshot 不存在（`embedded` feature 未启用）。
- ABI 哈希失配（builtin JS 或 engine 配置变化）。
- `WJSM_STARTUP_SNAPSHOT=0` 显式禁用快照。

## Bootstrap 过程

1. `concat_builtin_js_sources()` 把 `builtin_js::manifest::BUILTIN_JS_FILES` 的源码拼接成一个 seed JS。
2. `compile_source(&seed)` 编译 seed 为 WASM。
3. `instantiate_execute_bundle` 实例化 seed WASM 和 support module。
4. `run_bootstrap_only` 执行 seed 的入口函数，构造 immortal 对象、原型链、内置函数属性。

bootstrap 完成后，对象堆包含所有 primordial 对象（Object.prototype、Array.prototype、全局构造器等），user module 可以直接使用它们。

## warm bootstrap vs cold bootstrap

**cold bootstrap**：从空堆开始，执行全部 builtin JS。

**warm bootstrap**：快照恢复后，user module 直接使用已恢复的 primordial 对象，不需要执行 builtin JS。这是快照加速的核心。

## immortal 对象

immortal 对象是 bootstrap 创建的永生对象，GC 不回收。它们的句柄在快照里固定，恢复时直接写入对象堆的 immortal 区。`immortal_objects_end_rel` 记录 immortal 区的结束偏移。

## 深入了解

- [启动路径概览](startup-path.md)
- [快照捕获与恢复](capture-and-restore.md)
- [ABI 哈希与失效策略](abi-hash-and-invalidation.md)
