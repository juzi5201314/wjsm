# 冷启动与失配处理

这一章说明快照失配时 cold bootstrap 的执行流程和后续行为。

## 失配检测

`embedded_startup_snapshot_view(engine)` 在解码快照后比较 `view.header.abi_hash` 和 `current_abi`。不匹配时：

- 如果 `WJSM_STARTUP_SNAPSHOT_DEBUG=1`，在 stderr 打印 `embedded snapshot abi hash mismatch; falling back to cold startup`。
- 返回 `None`，调用方走 cold bootstrap。

## cold bootstrap 流程

1. `concat_builtin_js_sources()` 把 `builtin_js::manifest::BUILTIN_JS_FILES` 的源码拼接成 seed JS。
2. `compile_source(&seed)` 编译 seed 为 WASM。
3. `instantiate_execute_bundle` 实例化 seed WASM 和 support module。
4. `run_bootstrap_only` 执行 seed 的入口函数，构造 immortal 对象、原型链、内置函数属性。

bootstrap 完成后，对象堆包含所有 primordial 对象（Object.prototype、Array.prototype、全局构造器等），user module 可以直接使用。

## 失配后的快照捕获

如果 `startup_snapshot_enabled()` 为 `true`（默认），cold bootstrap 完成后：

1. `capture_startup_snapshot` 从运行中的堆捕获快照。
2. 比较 `snap.header.abi_hash` 和 `current_abi`——不匹配则 bail，防止嵌入无效快照。
3. `install_embedded_startup_snapshot` 把新快照注入进程，后续启动可以直接恢复。

这条路径只在进程内生效——新快照不写回磁盘。下次进程启动仍使用构建时嵌入的快照（如果存在）。

## 禁用快照

`WJSM_STARTUP_SNAPSHOT=0` / `false` / `off` 完全禁用快照：

- `startup_snapshot_enabled()` 返回 `false`。
- 不构造 engine，不散列任何 snapshot ABI 状态。
- 每次启动都走 cold bootstrap，不捕获也不恢复快照。

这在调试 GC 或 bootstrap 问题时有用，确保行为可预测。

## 深入了解

- [启动快照边界](startup-snapshot.md)
- [Bootstrap 生命周期](bootstrap.md)
- [ABI Hash 与兼容性指纹](abi-hash.md)
