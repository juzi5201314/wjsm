# 启动快照格式

这一章说明 `wjsm-snapshot-format` crate 的二进制布局。

## 设计目标

快照是自描述的小端字节序二进制，header + sections。热路径可以 bounds-check + slice-copy 直接加载，不做堆分配或 JSON 解析。

## 文件头

`StartupSnapshotHeader`（104 字节）：

| 字段 | 类型 | 含义 |
| --- | --- | --- |
| `magic` | `[u8; 8]` | `WJSMSNP\0` |
| `format_version` | `u32` | 格式版本，当前 v9 |
| `abi_hash` | `u64` | ABI 哈希，用于快照匹配校验 |
| `heap_used` | `u32` | 堆已用字节 |
| `obj_table_count` | `u32` | 对象表条目数 |
| `function_props_base` | `u32` | 函数属性基址 |
| `immortal_objects_end_rel` | `u32` | immortal 对象结束相对偏移 |
| `object_proto_handle` | `u32` | Object.prototype 句柄 |
| `array_proto_handle` | `u32` | Array.prototype 句柄 |
| `arr_proto_table_base` | `u32` | 数组原型表基址 |
| `arr_proto_table_len` | `u32` | 数组原型表长度 |
| `arr_proto_table_hash` | `u64` | 数组原型表哈希 |
| `iterator_prototype` | `i64` | Iterator prototype |
| `generator_prototype` | `i64` | Generator prototype |
| `async_iterator_prototype` | `i64` | Async Iterator prototype |
| `async_gen_prototype` | `i64` | Async Generator prototype |
| `array_proto_values` | `i64` | Array.prototype.values |

## Sections

| Section | 内容 |
| --- | --- |
| `object_bytes` | 对象堆的 immortal 区字节 |
| `handle_rel_offsets` | 句柄到堆相对偏移的映射表 |
| `runtime_strings` | 运行时字符串表（UTF-16） |
| `native_callables` | NativeCallable 注册表 |
| `native_callable_methods` | NativeCallable 方法位图 |

## 版本演化

`SNAPSHOT_FORMAT_VERSION` 当前是 9。v9 新增函数属性对象的 `prototype` + `constructor` 属性。任何 wire 改动必须递增版本号。

## NULL_HANDLE_REL

`NULL_HANDLE_REL = u32::MAX` 是 `obj_table[i] == 0` 的哨兵值。实际堆偏移远小于 `u32::MAX`，不会碰撞。它区分「rel == 0（堆起点）」与「null 句柄」。

## ManagedHeapV2ArtifactAbi

`ManagedHeapV2ArtifactAbi` 记录 engine fingerprint 和 support ABI hash，是 support cwasm 工件的 ABI 锚点。

## 深入了解

- [ABI Hash 与兼容性指纹](abi-hash.md)
- [快照捕获与恢复的流程](capture-and-restore.md)
- [启动路径概览](startup-path.md)
