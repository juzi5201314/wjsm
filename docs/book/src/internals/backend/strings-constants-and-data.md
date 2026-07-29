# 字符串、常量与数据段

这一章说明字符串、数字和其他常量如何编码进 WASM 数据段，以及运行时如何从中读取。

## 数据段布局

`Compiler` 的 `data_base` 是起始偏移，`data_offset` 是当前写入位置。字符串以 null 结尾写入 `string_data: Vec<u8>`，最终作为 WASM `data` 段输出。

```rust
let ptr = self.data_base + self.data_offset;
let mut bytes = value.as_bytes().to_vec();
bytes.push(0);
let len = bytes.len() as u32;
self.string_data.extend(bytes);
self.data_offset += len;
self.string_ptr_cache.insert(value.clone(), ptr);
```

`string_ptr_cache` 去重同一字符串，第二次引用直接返回已有偏移。这是 IR 常量池去重之后的第二层去重。

Normal 模式 `data_base = 0`。Eval 模式 `data_base` 由调用方传入，让多次 eval 的数据段不重叠。

## 字符串值编码

字符串值是 `TAG_STRING` 标签加 32 位指针负载：`encode_string_ptr(ptr)`。运行时通过 `decode_string_ptr` 取出指针，从线性内存读取 null 结尾字节序列。

字符串比较走 `string_eq` helper（support module），按指针比较 + 内容比较两步。相同字符串字面量因为 `string_ptr_cache` 共享指针，直接 `i64.eq` 即可判定相等。

## 数字常量

`f64` 直接 `to_bits()` 编码，不经过数据段。`int32` 走 `TAG_INT32` 标签加 32 位负载。这两种值在编译期就内联为 `i64.const` 指令，不需要数据段访问。

## BigInt 与 RegExp

BigInt 和 RegExp 不在常量池编码时处理，而是在指令编译阶段特殊处理（`encode_constant` 对它们 `bail!` 提示走专用路径）。BigInt 存十进制字符串，RegExp 存 pattern + flags，两者在运行时由宿主解析。

## ModuleId

`Constant::ModuleId(ModuleId)` 直接编码为 `i64` 整数，供动态 `import()` 在运行时查找目标模块。

## 深入了解

- [IR Constant 枚举的完整定义](../ir/instructions-and-constants.md)
- [Eval 模式的数据段基址管理](normal-and-eval-modes.md)
- [用户视角的产物体积分布](../../user/output/wasm-artifacts.md)
