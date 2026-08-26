# ADR 0021: Latin-1 inline string（双 marker SSO）

## Status

Accepted（2026-08-26）

## Context

ADR 与既有实现已落地 **ASCII SSO**：≤6 个 7-bit ASCII 码元直接编码在 NaN-box payload，零堆、零句柄，用于 install 期字符串常量、运行时 `intern`、以及 `PropertyKey::inline_string` 属性键。

该编码每个码元占 7 bit，payload 限在 bits 0–41，无法容纳 6 个完整 Latin-1 单字节码元（需 48 bit）。常见属性名与短标识符在 Latin-1 范围内仍频繁出现，继续走 heap `RuntimeString` 与句柄管理会保留 ~80ns 分配与 GC 压力。

Native ABI 当前为 v18；本 ADR 引入 **第二种 inline string marker**，将 `NATIVE_ABI_VERSION` 升至 **19**，旧 native image 必须重建。

## Decision

### 1. 双 marker 布局

| 种类 | Marker (bits 48–50) | Payload | 最大码元数 |
|------|---------------------|---------|------------|
| ASCII SSO | `101` | bits 0–41，每码元 7 bit | 6 |
| Latin-1 SSO | `110` | bits 0–41（最多 5×8 bit 落在 bits 0–39；bits 42–44 保持为零） | 5 |

公共字段（两种 SSO 共享）：

- bits 45–47：`length`（0–6）
- bits 42–44：ASCII 路径必须为 0（`INLINE_STRING_RESERVED_MASK`）；Latin-1 路径作为 payload 延续位，**不得**与 GC color 语义混用——inline string 永不携带 GC color
- `BOX_BASE` 与 quiet-NaN 判别不变

API：

- `encode_inline_ascii` / `is_inline_ascii` / `decode_inline_ascii`（现有，语义不变）
- `encode_inline_latin1` / `is_inline_latin1` / `decode_inline_latin1`（新增）
- `is_inline_string`：两种 marker 的析取；`inline_string_len` 对两者通用

### 2. PropertyKey

`PropertyKey::inline_string(encoded)` 接受规范 ASCII 或 Latin-1 SSO 的完整 `i64` 值，经 `INLINE_NAMESPACE`（bit 62）存入 shape / IC；比较仍在完整 64 bit 上进行，禁止截断 hash。

### 3. JIT 与 GC color

生成代码在 strip GC color 时，对 **任一** inline string（ASCII 或 Latin-1）保留完整 payload；Latin-1 不使用 bits 51–63（与 `BOX_BASE` 重叠）。

`emit_inline_string_predicate` 在 CLIF 层对 marker `101` 与 `110` 做快路径分派。

### 4. 宿主与 install

- `publish_baked_string`：Latin-1 烘焙常量在 ASCII 编码失败时尝试 `encode_inline_latin1`
- `intern_text` / `intern_property_string`：flat Latin-1 载荷优先 inline，再落 heap
- install 期 inline 字符串不进入 `install_string_roots`

### 5. 明确不做

- UTF-16 非 Latin-1 码元：仍走 heap string 或现有 Latin-1 单字节堆表示
- 超过 6 码元：仍走 heap
- `create_closure`：side-table 索引，不在 TLAB 范围（与 SSO 无关）

## Consequences

- 短 Latin-1 字符串与属性键获得与 ASCII SSO 相同的零分配路径
- ABI v19 与 artifact 版本 bump；混用旧 image 在 `abi_version` 校验处失败
- 测试：`gc_color`、`property_key`、happy fixture `latin1_sso_property_key.js`、ZGC 下 `string_ids` 不增长
