# 快照捕获与恢复

这一章说明快照如何从运行中的堆捕获，以及如何恢复到新堆。

## 捕获

`startup_snapshot::capture_startup_snapshot` 在 bootstrap 完成后调用：

1. 从 `WasmEnv` 读取堆指针、对象表、原型句柄等状态。
2. 把 immortal 区的堆字节复制到 `object_bytes`。
3. 遍历对象表，记录每个句柄的堆相对偏移到 `handle_rel_offsets`。
4. 序列化运行时字符串表和 NativeCallable 注册表。
5. 写入 header（含 ABI 哈希）。
6. `encode_snapshot` 编码为字节序列。

捕获的快照用 `install_embedded_startup_snapshot` 注入进程，后续启动可以直接恢复。

## 恢复

快照恢复是捕获的逆过程：

1. `decode_snapshot` 解码 header + sections。
2. 校验 ABI 哈希——失配则放弃恢复。
3. 把 `object_bytes` 写入对象堆的 immortal 区。
4. 根据 `handle_rel_offsets` 重建对象表。
5. 恢复运行时字符串表和 NativeCallable 注册表。
6. 设置 `WasmEnv` 的原型句柄、`immortal_objects_end` 等 global。

恢复后堆状态等价于 bootstrap 完成，但跳过了 builtin JS 的执行。

## 为什么捕获时校验 ABI

`build_embedded_startup_snapshot_bytes_async` 在捕获后比较 `snap.header.abi_hash` 和 `current_abi`。不匹配则 bail，防止嵌入无效快照。这捕获 build 环境的配置漂移。

## 深入了解

- [启动快照格式与重定位](snapshot-format.md)
- [Bootstrap 生命周期](bootstrap.md)
- [ABI Hash 与兼容性指纹](abi-hash.md)
