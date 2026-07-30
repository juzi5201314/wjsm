# NaN-boxed 值表示

wjsm 用 64 位整数承载所有 JavaScript 值。这一章说明位布局、标签编码以及为什么「严格相等」可以直接用 `i64.eq`。

## 位布局

`crates/wjsm-ir/src/value.rs` 定义常量与编解码函数。布局以 IEEE 754 NaN 为基础：

- `BOX_BASE = 0x7FF8_0000_0000_0000`：高 12 位是 quiet NaN 的指数位。
- `TAG_MASK = 0x0000_003F_0000_0000`：bit 32–37 是类型标签，6 位可容纳 64 种类型。
- `PAYLOAD_MASK = 0x0000_0000_FFFF_FFFF`：低 32 位是负载。

所有非 double 值都写成 `BOX_BASE | (tag << 32) | payload`。double 值原样保留，因为它们不会与 `BOX_BASE` 冲突（canonical NaN 在 wjsm 中不出现）。

## 标签清单

| 常量 | 值 | 类型 |
| --- | --- | --- |
| `TAG_UNDEFINED` | 0 | undefined |
| `TAG_NULL` | 1 | null |
| `TAG_TRUE` / `TAG_FALSE` | 2 / 3 | boolean |
| `TAG_INT32` | 4 | 32 位整数（`i32`） |
| `TAG_EXCEPTION` | 5 | 异常值（传播中的抛出） |
| `TAG_OBJECT_HANDLE` | 6 | 对象句柄 |
| `TAG_STRING` | 7 | 字符串句柄 |
| `TAG_BIGINT` | 8 | BigInt 句柄 |
| `TAG_SYMBOL` | 9 | Symbol 句柄 |
| `TAG_FUNCTION` | 10 | 函数引用 |
| `TAG_PROXY` | 11 | Proxy 句柄 |
| `TAG_ASYNC_GENERATOR` | 12 | async generator |
| `TAG_CONTINUATION` | 13 | async continuation |
| `TAG_DATE` / `TAG_REGEXP` / `TAG_ERROR` / … | 14+ | 各专用类型 |

`is_exception` 是一个 `value & TAG_MASK == TAG_EXCEPTION << 32` 检查，这就是控制流异常传播的全部机制。

> <details><summary>为什么用 64 位整数而不是结构体？</summary>
>
> 把 JS 值塞进单个 64 位整数有几个好处：
>
> 1. **WASM 原生支持**：WASM 局部变量和栈操作都是针对 `i32`/`i64`，单值类型最高效。
> 2. **寄存器友好**：值放寄存器比放堆快得多。`i64` 是 X86_64 的原生类型，没有额外开销。
> 3. **比较省事**：值是整数，`i64.eq` 是机器指令；如果是结构体，要先读内存字段。
> 4. **GC 移动对象时不需更新值**：值里是 handle index（对象表下标），不是裸指针。GC 移动对象只更新表，值不动。
>
> 代价是 NaN-boxing 的「拆箱」操作——`i64` 转换回 `f64`、取出 handle 等等，每次跨类型操作都要做。
>
> 这个代价通过后端的值类型推断缓解：纯数值函数识别出「`%0` 是 int32，`%1` 是 double」之后，可以跳过拆箱直接做 `i32.add`。
>
> </details>

## 编码策略

`encode_handle(tag, index)` 把索引放进低 32 位负载。`index` 是 `HandleIndex`（对象表下标），而非裸堆指针——GC 可以移动对象而不需要更新值里的指针。

`encode_i32(n)` 是直接 `(tag << 32) | (n as u32)`，无额外间接层。小整数算术直接 decode→运算→encode，不走宿主调用。

`encode_f64(x)` 是 `x.to_bits()`：double 就是 double，没有 box。`decode_f64` 是 `f64::from_bits`。

## 严格相等为什么可以直接 `i64.eq`

NaN-box 值的位表示是规范化的：

- `undefined`、`null`、`true`、`false` 各自只有一个位模式。
- int32 与句柄类值的编码是 injective 的：不同值不同位模式。
- double 的位模式由 IEEE 754 保证，`NaN !== NaN` 的语义可以通过额外检查 `is_nan` 实现。

因此后端把 `CompareOp::StrictEq` 编为 `i64.eq`，不调用宿主函数。`==` 不行——抽象相等需要完整类型转换，走 `CallBuiltin`。

## Type 12 调用约定

`dump-wat` 里大量出现的 `(param i64 i64 i32 i32) (result i64)` 是原型方法调用约定：

- `i64` env 对象
- `i64` this 值
- `i32` 影子栈参数基址
- `i32` 参数个数
- `i64` 返回值

变长参数不走 WASM 参数列表，而是写入影子栈，函数从 `read_shadow_arg` 读取。这让任意元数的调用共用一个 type index，WAT 里看到 `type $#type12` 大量出现就是这个原因。

## 深入了解

- [用户视角的产物体积分布](../../user/output/wasm-artifacts.md)
- [影子栈槽位活跃性与 GC Spill 规则](liveness-slots-and-spills.md)
- [对象布局与分配](../gc/object-layout-and-allocation.md)
