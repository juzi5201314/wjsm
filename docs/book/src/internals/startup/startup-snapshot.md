# 启动快照边界

这一章说明 startup snapshot 在启动路径中的位置和边界条件。

## 启用条件

`startup_snapshot_enabled()` 默认返回 `true`。`WJSM_STARTUP_SNAPSHOT` 设为 `0` / `false` / `off` 时返回 `false`，禁用快照——此时不构造 engine 也不散列任何 snapshot ABI 状态，直接走 cold bootstrap。

## 快速路径（warm bootstrap）

embedded startup snapshot 存在且 ABI 哈希匹配时：

1. `embedded_startup_snapshot_view(engine)` 解码并校验 ABI hash，返回快照字节。
2. 快照恢复把 immortal 对象、原型链、内置函数属性写入对象堆。
3. 跳过 builtin JS bundle 的执行。

恢复后堆状态等价于 bootstrap 完成，但省去了 JS 执行的时间。

## 慢速路径（cold bootstrap）

快照不存在或 ABI 哈希失配时：

1. `embedded_startup_snapshot_view` 返回 `None`。
2. `concat_builtin_js_sources()` 拼接 builtin JS bundle。
3. `compile_source` 编译为 WASM。
4. `run_bootstrap_only` 执行 bootstrap，构造所有 immortal 对象。
5. 如果 `startup_snapshot_enabled()` 为 `true`，`capture_startup_snapshot` 捕获新快照并 `install_embedded_startup_snapshot`。

## 调试

`WJSM_STARTUP_SNAPSHOT_DEBUG=1` 启用诊断输出。快照失配时会在 stderr 打印 `embedded snapshot abi hash mismatch; falling back to cold startup`，帮助定位 ABI 变化的原因。

## 深入了解

- [启动路径概览](startup-path.md)
- [ABI Hash 与兼容性指纹](abi-hash.md)
- [快照格式与重定位](snapshot-format.md)
