//! 数据段布局常量和属性槽相关常量

// ── TYPEOF 字符串区域 ──────────────────────────────────────────────────────
// 6 个类型字符串（nul 终止）预分配在 data segment 开头
pub const TYPEOF_UNDEFINED_OFFSET: u32 = 0; // "undefined\0" (10 bytes)
pub const TYPEOF_OBJECT_OFFSET: u32 = 10; // "object\0" (7 bytes)
pub const TYPEOF_BOOLEAN_OFFSET: u32 = 17; // "boolean\0" (8 bytes)
pub const TYPEOF_STRING_OFFSET: u32 = 25; // "string\0" (7 bytes)
pub const TYPEOF_FUNCTION_OFFSET: u32 = 32; // "function\0" (9 bytes)
pub const TYPEOF_NUMBER_OFFSET: u32 = 41; // "number\0" (7 bytes)
// offset 48-66 预留给 "symbol\0" (7) 和 "bigint\0" (7)
/// offset 48: "symbol\0" (7 bytes) — 对应 encode_typeof_symbol()
pub const TYPEOF_SYMBOL_OFFSET: u32 = 48;
/// offset 55: "bigint\0" (7 bytes) — 对应 encode_typeof_bigint()
pub const TYPEOF_BIGINT_OFFSET: u32 = 55;
pub const TYPEOF_RESERVED_END: u32 = 66;

// ── 属性描述符字符串区域 ────────────────────────────────────────────────────
// 紧接 TYPEOF 区域之后，用于 Object.getOwnPropertyDescriptor 返回的描述符对象
pub const PROP_DESC_VALUE_OFFSET: u32 = 66; // "value\0" (6 bytes)
pub const PROP_DESC_WRITABLE_OFFSET: u32 = 72; // "writable\0" (9 bytes)
pub const PROP_DESC_ENUMERABLE_OFFSET: u32 = 81; // "enumerable\0" (11 bytes)
pub const PROP_DESC_CONFIGURABLE_OFFSET: u32 = 92; // "configurable\0" (13 bytes)
pub const PROP_DESC_GET_OFFSET: u32 = 105; // "get\0" (4 bytes)
pub const PROP_DESC_SET_OFFSET: u32 = 109; // "set\0" (4 bytes)
pub const PROP_DESC_END: u32 = 113;

// ── 用户字符串起始位置 ──────────────────────────────────────────────────────
pub const USER_STRING_START: u32 = 113;

// ── 属性槽相关常量 ──────────────────────────────────────────────────────────
// 属性槽格式（32 字节）：
// Offset 0:  name_id (4 bytes)  - 属性名在字符串表中的 ID
// Offset 4:  flags (4 bytes)    - 属性标志位
// Offset 8:  value (8 bytes)    - 数据属性的值，访问器属性为 undefined
// Offset 16: getter (8 bytes)   - 访问器属性的 getter，数据属性为 undefined
// Offset 24: setter (8 bytes)   - 访问器属性的 setter，数据属性为 undefined
pub const PROP_SLOT_SIZE: u32 = 32;
pub const PROP_SLOT_NAME_ID_OFFSET: u32 = 0;
pub const PROP_SLOT_FLAGS_OFFSET: u32 = 4;
pub const PROP_SLOT_VALUE_OFFSET: u32 = 8;
pub const PROP_SLOT_GETTER_OFFSET: u32 = 16;
pub const PROP_SLOT_SETTER_OFFSET: u32 = 24;

// ── 属性标志位定义 ──────────────────────────────────────────────────────────
// flags 字段的位定义
pub const FLAG_CONFIGURABLE: i32 = 1 << 0; // bit 0: 可配置
pub const FLAG_ENUMERABLE: i32 = 1 << 1; // bit 1: 可枚举
pub const FLAG_WRITABLE: i32 = 1 << 2; // bit 2: 可写（数据属性专用）
pub const FLAG_IS_ACCESSOR: i32 = 1 << 3; // bit 3: 是否为访问器属性
