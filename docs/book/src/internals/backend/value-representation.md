# NaN-boxed 值表示

JavaScript 值在 wjsm 里是 `i64`，使用 NaN-boxing 编码。这一章说明值布局和标签分配。

## NaN-boxing 原理

IEEE 754 双精度浮点数的 NaN 空间是 `0x7FF8_0000_0000_0000` 到 `0x7FFF_FFFF_FFFF_FFFF`（正 NaN）和对应的负 NaN。wjsm 利用这段空间编码非浮点值。

基于 `BOX_BASE`，标签放在 bits 32–37：

| 标签 | 值 | 含义 |
| --- | --- | --- |
| `TAG_EXCEPTION` | `0x5` | 异常值 |
| `TAG_OBJECT_HANDLE` | `0x6` | 对象句柄 |
| `TAG_CONTINUATION` | — | async 续延 |
| `TAG_PROXY` | — | Proxy 对象 |

浮点数值直接以 `f64` 表示，不经过标签——只要它不是 NaN-box 编码范围内的 NaN。

## 句柄编码

对象句柄（`Handle = u32`）放在 NaN-box 值的低 32 位。`obj_table[handle]` 是对象的唯一堆指针真相。GC 移动对象时只更新表项，不需要扫描所有 NaN-box 值。

```text
┌─────────────────────────────────┐
│  tag (bits 32-37) │ handle (low 32) │
└─────────────────────────────────┘
```

## 值类型推断

后端在 `wjsm-backend-native` 的 `f64_analysis` 与特化路径中做值类型推断，用于省掉部分 NaN-box 解包。如果编译器能静态确定某个值是 number（例如 `Binary::Add` 的两个操作数都是 `Const(Number)`），生成的代码可以直接用 `f64` 运算，不需要在运行时检查标签。

推断是保守的：不确定时退回到运行时标签检查。

## 相关常量

| 常量 | 定义位置 | 含义 |
| --- | --- | --- |
| `BOX_BASE` | `wjsm-ir/src/value.rs` | NaN-box 基址 |
| `TAG_*` | `wjsm-ir/src/value.rs` | 值标签 |
| `NULL_HANDLE_REL` | `wjsm-ir` | `u32::MAX`，空句柄哨兵 |

## 深入了解

- [Value、变量与类型信息](../ir/values-and-types.md)
- [Handle Table 的结构](../gc/handle-table.md)
- [对象、值与标签索引](../reference/layout-and-tags.md)
- [GC 不变量](../reference/invariants.md)
