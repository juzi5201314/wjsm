# 对象、值与标签索引

这一章汇总 NaN-box 值表示和对象布局的关键常量。权威定义在 `wjsm-ir` 的 `value.rs` / `constants.rs`。

## NaN-box 布局

值是 `i64`，基于 `BOX_BASE` 的 NaN-boxing。标签在 bits 32-37。

| 标签 | 值 | 含义 |
| --- | --- | --- |
| `TAG_EXCEPTION` | `0x5` | 异常完成 |
| `TAG_OBJECT_HANDLE` | 见 `wjsm-ir` | 对象句柄 |
| `TAG_CONTINUATION` | 见 `wjsm-ir` | async 续延 |
| `TAG_PROXY` | 见 `wjsm-ir` | Proxy 对象 |

句柄（`Handle = u32`）放在 NaN-box 值的低 32 位，是对象表的下标。

## 函数入口

当前没有 WASM Type 12 / Type 7。JS 函数慢路径是 `NativeSlowEntry`：

```text
(ctx: *mut NativeVmContext, env: i64, this_value: i64, args_base: u32, args_count: u32) -> i64
```

## 对象堆

生产堆是 `NativeHeapMemory`：逻辑 memory64 字节偏移 + mmap 后备。没有 `HEAP_MEMORY_MIN_PAGES` / `HEAP_MEMORY_MAX_PAGES`。容量由 `ManagedHeapLayout` 与 `--max-heap-size` 决定；提交窗口按 64 KiB granule 增长。

## 快照格式

| 常量 | 值 | 含义 |
| --- | --- | --- |
| `SNAPSHOT_MAGIC` | `WJSMNSP\0` | native startup snapshot 魔数 |
| `SNAPSHOT_FORMAT_VERSION` | `1` | `wjsm-snapshot-format` 容器版本 |
| `HOST_STATE_MAGIC` | `WJSMHST\0` | host 侧字符串 / callable 表 |
| `HOST_STATE_VERSION` | `1` | host-state 版本 |

校验还包含 `bootstrap_hash`、`NATIVE_CODEGEN_HASH`、semantic ABI hash、native ABI hash、target 与 endian。

## GC 统计

`GcStats` 字段：`marked`、`swept`、`freed_bytes`、`elapsed`、`free_block_count`、`total_free_bytes`、`largest_free_block`、`external_fragmentation`。

`CycleKind` 变体：`Full`、`Young`、`Mixed`、`ZgcCycle`、`Step`。

## 深入了解

- [NaN-boxed 值表示](../backend/value-representation.md)
- [对象布局与分配](../gc/object-layout-and-allocation.md)
- [Native ABI 索引](abi-index.md)
