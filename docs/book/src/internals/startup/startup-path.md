# 启动路径概览

这一章说明 wjsm 进程从启动到执行用户代码经过的几个阶段。

## 阶段

1. **Engine 构造**：根据 GC、compiler、debug 等配置构造 wasmtime `Engine`。
2. **Support module 加载**：按当前 GC flavor 加载对应的 support cwasm，实例化到对象堆。
3. **用户模块编译/加载**：用户 WASM 经 `compile_or_load_cached` 编译或从缓存反序列化。
4. **Linker 注册**：注册 host import、common bridges、complex bridges 到 wasmtime Linker。
5. **Snapshot 恢复或 Bootstrap**：若 embedded snapshot 的 ABI 哈希匹配，恢复快照；否则冷启动 bootstrap。
6. **执行用户代码**：调用 user module 的入口函数。

## 快速路径 vs 慢速路径

**快速路径**：embedded startup snapshot 存在且 ABI 哈希匹配。快照恢复把 immortal 对象、原型链、内置函数属性直接写入对象堆，跳过 bootstrap JS 的执行。

**慢速路径**：快照不存在或失配。执行 `builtin_js` bundle 的 bootstrap 代码，构造所有 immortal 对象，然后（如果可能）捕获新快照供下次使用。

## ABI 哈希的作用

ABI 哈希判断快照是否与当前 engine 兼容。它由三部分组合：

- support ABI union hash（三种 GC flavor 的 support ABI）
- builtin JS bundle hash（`.js` 文件内容）
- engine compatibility fingerprint（wasmtime 配置）

任一项变化，快照失配，走慢速路径。详见[ABI Hash 与兼容性指纹](abi-hash.md)。

## 深入了解

- [启动快照格式与重定位](snapshot-format.md)
- [Bootstrap 生命周期](bootstrap.md)
- [冷启动与失配处理](cold-start-and-mismatch.md)
