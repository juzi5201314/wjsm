# 对象、值与标签索引

这一章汇总 NaN-box 值表示和对象布局的关键常量。

## NaN-box 布局

值是 `i64`，基于 `BOX_BASE` 的 NaN-boxing。标签在 bits 32-37。

| 标签 | 值 | 含义 |
| --- | --- | --- |
| `TAG_EXCEPTION` | 0x5 | 异常 |
| `TAG_OBJECT_HANDLE` | - | 对象句柄 |
| `TAG_CONTINUATION` | - | async 续延 |
| `TAG_PROXY` | - | Proxy 对象 |

句柄（`Handle = u32`）放在 NaN-box 值的低 32 位，是对象表的下标。

## 类型索引

| Type | 签名 | 用途 |
| --- | --- | --- |
| Type 12 | `(i64, i64, i32, i32) -> i64` | 函数调用约定 |
| Type 7 | `(i32) -> i32` | 简单函数 |

`JS_FUNC_TYPE_INDEX = 12` 是函数调用的标准类型索引。

## 对象堆

| 常量 | 值 | 含义 |
| --- | --- | --- |
| `HEAP_MEMORY_MIN_PAGES` | 524288 | 最小页数（32 GiB） |
| `HEAP_MEMORY_MAX_PAGES` | 4294967296 | 最大页数（256 TiB） |

`NULL_HANDLE_REL = u32::MAX` 是 `obj_table[i] == 0` 的哨兵值。

## 快照格式

| 常量 | 值 | 含义 |
| --- | --- | --- |
| `SNAPSHOT_MAGIC` | `WJSMSNP\0` | 快照魔数 |
| `SNAPSHOT_FORMAT_VERSION` | 9 | 格式版本 |
| `HEADER_LEN` | 104 | 头部长度 |

## GC 统计

`GcStats` 字段：`marked`、`swept`、`freed_bytes`、`elapsed`、`free_block_count`、`total_free_bytes`、`largest_free_block`、`external_fragmentation`。

`CycleKind` 变体：`Full`、`Young`、`Mixed`、`ZgcCycle`、`Step`。

## 深入了解

- [NaN-boxed 值表示](../backend/value-representation.md)
- [对象布局与分配](../gc/object-layout-and-allocation.md)
- [WASM 与 Host ABI 索引](abi-index.md)
