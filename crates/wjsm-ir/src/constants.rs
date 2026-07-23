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

// ── Promise 相关字符串区域 ──────────────────────────────────────────────────
pub const PROMISE_STATE_PENDING_OFFSET: u32 = 113; // "pending\0" (8 bytes)
pub const PROMISE_STATE_FULFILLED_OFFSET: u32 = 121; // "fulfilled\0" (10 bytes)
pub const PROMISE_STATE_REJECTED_OFFSET: u32 = 131; // "rejected\0" (9 bytes)
pub const PROMISE_THEN_OFFSET: u32 = 140; // "then\0" (5 bytes)
pub const PROMISE_CATCH_OFFSET: u32 = 145; // "catch\0" (6 bytes)
pub const PROMISE_FINALLY_OFFSET: u32 = 151; // "finally\0" (8 bytes)
pub const PROMISE_RESOLVE_OFFSET: u32 = 159; // "resolve\0" (8 bytes)
pub const PROMISE_REJECT_OFFSET: u32 = 167; // "reject\0" (7 bytes)
pub const PROMISE_ALL_OFFSET: u32 = 174; // "all\0" (4 bytes)
pub const PROMISE_RACE_OFFSET: u32 = 178; // "race\0" (5 bytes)
pub const PROMISE_ALLSETTLED_OFFSET: u32 = 183; // "allSettled\0" (11 bytes)
pub const PROMISE_ANY_OFFSET: u32 = 194; // "any\0" (4 bytes)
pub const PROMISE_CONSTRUCTOR_OFFSET: u32 = 198; // "constructor\0" (12 bytes)
pub const ASYNC_ITERATOR_OFFSET: u32 = 210; // "asyncIterator\0" (14 bytes)
pub const PROMISE_STRINGS_END: u32 = 224;

// ── Primordial 字符串区域 ────────────────────────────────────────────────────
// 启动 bootstrap / 函数属性 / host post-bootstrap 中引用的所有属性名字符串。
// 固定在 data section 的固定偏移，使不同用户源码编译产物的 name_id 一致，
// 作为 startup snapshot ABI hash 输入。
pub const PRIMORDIAL_LENGTH_OFFSET: u32 = 224; // "length\0" (7 bytes)
pub const PRIMORDIAL_NAME_OFFSET: u32 = 231; // "name\0" (5 bytes)
pub const PRIMORDIAL_PROTOTYPE_OFFSET: u32 = 236; // "prototype\0" (10 bytes)
pub const PRIMORDIAL_PUSH_OFFSET: u32 = 246; // "push\0" (5 bytes)
pub const PRIMORDIAL_POP_OFFSET: u32 = 251; // "pop\0" (4 bytes)
pub const PRIMORDIAL_INCLUDES_OFFSET: u32 = 255; // "includes\0" (9 bytes)
pub const PRIMORDIAL_INDEXOF_OFFSET: u32 = 264; // "indexOf\0" (8 bytes)
pub const PRIMORDIAL_JOIN_OFFSET: u32 = 272; // "join\0" (5 bytes)
pub const PRIMORDIAL_CONCAT_OFFSET: u32 = 277; // "concat\0" (7 bytes)
pub const PRIMORDIAL_SLICE_OFFSET: u32 = 284; // "slice\0" (6 bytes)
pub const PRIMORDIAL_FILL_OFFSET: u32 = 290; // "fill\0" (5 bytes)
pub const PRIMORDIAL_REVERSE_OFFSET: u32 = 295; // "reverse\0" (8 bytes)
pub const PRIMORDIAL_FLAT_OFFSET: u32 = 303; // "flat\0" (5 bytes)
pub const PRIMORDIAL_SHIFT_OFFSET: u32 = 308; // "shift\0" (6 bytes)
pub const PRIMORDIAL_UNSHIFT_OFFSET: u32 = 314; // "unshift\0" (8 bytes)
pub const PRIMORDIAL_SORT_OFFSET: u32 = 322; // "sort\0" (5 bytes)
pub const PRIMORDIAL_AT_OFFSET: u32 = 327; // "at\0" (3 bytes)
pub const PRIMORDIAL_COPYWITHIN_OFFSET: u32 = 330; // "copyWithin\0" (11 bytes)
pub const PRIMORDIAL_FOREACH_OFFSET: u32 = 341; // "forEach\0" (8 bytes)
pub const PRIMORDIAL_MAP_OFFSET: u32 = 349; // "map\0" (4 bytes)
pub const PRIMORDIAL_FILTER_OFFSET: u32 = 353; // "filter\0" (7 bytes)
pub const PRIMORDIAL_REDUCE_OFFSET: u32 = 360; // "reduce\0" (7 bytes)
pub const PRIMORDIAL_REDUCERIGHT_OFFSET: u32 = 367; // "reduceRight\0" (12 bytes)
pub const PRIMORDIAL_FIND_OFFSET: u32 = 379; // "find\0" (5 bytes)
pub const PRIMORDIAL_FINDINDEX_OFFSET: u32 = 384; // "findIndex\0" (10 bytes)
pub const PRIMORDIAL_SOME_OFFSET: u32 = 394; // "some\0" (5 bytes)
pub const PRIMORDIAL_EVERY_OFFSET: u32 = 399; // "every\0" (6 bytes)
pub const PRIMORDIAL_FLATMAP_OFFSET: u32 = 405; // "flatMap\0" (8 bytes)
pub const PRIMORDIAL_SPLICE_OFFSET: u32 = 413; // "splice\0" (7 bytes)
pub const PRIMORDIAL_ISARRAY_OFFSET: u32 = 420; // "isArray\0" (8 bytes)
pub const PRIMORDIAL_TOSTRING_OFFSET: u32 = 428; // "toString\0" (9 bytes)
pub const PRIMORDIAL_VALUEOF_OFFSET: u32 = 437; // "valueOf\0" (8 bytes)
pub const PRIMORDIAL_SYMBOL_TOSTRINGTAG_OFFSET: u32 = 445; // "Symbol.toStringTag\0" (19 bytes)
pub const PRIMORDIAL_ASYNCITERATOR_OFFSET: u32 = 464; // "AsyncIterator\0" (14 bytes)
pub const PRIMORDIAL_ASYNCGENERATOR_OFFSET: u32 = 478; // "AsyncGenerator\0" (15 bytes)
// ── ES2023/ES2024 新增数组原型方法名（含 keys/values/entries 迭代器方法） ──
pub const PRIMORDIAL_FINDLAST_OFFSET: u32 = 493; // "findLast\0" (9 bytes)
pub const PRIMORDIAL_FINDLASTINDEX_OFFSET: u32 = 502; // "findLastIndex\0" (14 bytes)
pub const PRIMORDIAL_LASTINDEXOF_OFFSET: u32 = 516; // "lastIndexOf\0" (12 bytes)
pub const PRIMORDIAL_TOSORTED_OFFSET: u32 = 528; // "toSorted\0" (9 bytes)
pub const PRIMORDIAL_TOREVERSED_OFFSET: u32 = 537; // "toReversed\0" (11 bytes)
pub const PRIMORDIAL_TOSPLICED_OFFSET: u32 = 548; // "toSpliced\0" (10 bytes)
pub const PRIMORDIAL_WITH_OFFSET: u32 = 558; // "with\0" (5 bytes)
pub const PRIMORDIAL_KEYS_OFFSET: u32 = 563; // "keys\0" (5 bytes)
pub const PRIMORDIAL_VALUES_OFFSET: u32 = 568; // "values\0" (7 bytes)
pub const PRIMORDIAL_ENTRIES_OFFSET: u32 = 575; // "entries\0" (8 bytes)
pub const PRIMORDIAL_STRINGS_END: u32 = 583;

// ── 用户字符串起始位置 ──────────────────────────────────────────────────────
pub const USER_STRING_START: u32 = PRIMORDIAL_STRINGS_END;

/// 返回所有固定偏移的 primordial 字符串及其偏移量列表。
/// 顺序必须与 pre-write 顺序一致，供 ABI hash 与测试使用。
pub fn primordial_string_offsets() -> &'static [(u32, &'static str)] {
    &[
        (PRIMORDIAL_LENGTH_OFFSET, "length"),
        (PRIMORDIAL_NAME_OFFSET, "name"),
        (PRIMORDIAL_PROTOTYPE_OFFSET, "prototype"),
        (PRIMORDIAL_PUSH_OFFSET, "push"),
        (PRIMORDIAL_POP_OFFSET, "pop"),
        (PRIMORDIAL_INCLUDES_OFFSET, "includes"),
        (PRIMORDIAL_INDEXOF_OFFSET, "indexOf"),
        (PRIMORDIAL_JOIN_OFFSET, "join"),
        (PRIMORDIAL_CONCAT_OFFSET, "concat"),
        (PRIMORDIAL_SLICE_OFFSET, "slice"),
        (PRIMORDIAL_FILL_OFFSET, "fill"),
        (PRIMORDIAL_REVERSE_OFFSET, "reverse"),
        (PRIMORDIAL_FLAT_OFFSET, "flat"),
        (PRIMORDIAL_SHIFT_OFFSET, "shift"),
        (PRIMORDIAL_UNSHIFT_OFFSET, "unshift"),
        (PRIMORDIAL_SORT_OFFSET, "sort"),
        (PRIMORDIAL_AT_OFFSET, "at"),
        (PRIMORDIAL_COPYWITHIN_OFFSET, "copyWithin"),
        (PRIMORDIAL_FOREACH_OFFSET, "forEach"),
        (PRIMORDIAL_MAP_OFFSET, "map"),
        (PRIMORDIAL_FILTER_OFFSET, "filter"),
        (PRIMORDIAL_REDUCE_OFFSET, "reduce"),
        (PRIMORDIAL_REDUCERIGHT_OFFSET, "reduceRight"),
        (PRIMORDIAL_FIND_OFFSET, "find"),
        (PRIMORDIAL_FINDINDEX_OFFSET, "findIndex"),
        (PRIMORDIAL_SOME_OFFSET, "some"),
        (PRIMORDIAL_EVERY_OFFSET, "every"),
        (PRIMORDIAL_FLATMAP_OFFSET, "flatMap"),
        (PRIMORDIAL_SPLICE_OFFSET, "splice"),
        (PRIMORDIAL_ISARRAY_OFFSET, "isArray"),
        (PRIMORDIAL_TOSTRING_OFFSET, "toString"),
        (PRIMORDIAL_VALUEOF_OFFSET, "valueOf"),
        (PRIMORDIAL_SYMBOL_TOSTRINGTAG_OFFSET, "Symbol.toStringTag"),
        (PRIMORDIAL_ASYNCITERATOR_OFFSET, "AsyncIterator"),
        (PRIMORDIAL_ASYNCGENERATOR_OFFSET, "AsyncGenerator"),
        (PRIMORDIAL_FINDLAST_OFFSET, "findLast"),
        (PRIMORDIAL_FINDLASTINDEX_OFFSET, "findLastIndex"),
        (PRIMORDIAL_LASTINDEXOF_OFFSET, "lastIndexOf"),
        (PRIMORDIAL_TOSORTED_OFFSET, "toSorted"),
        (PRIMORDIAL_TOREVERSED_OFFSET, "toReversed"),
        (PRIMORDIAL_TOSPLICED_OFFSET, "toSpliced"),
        (PRIMORDIAL_WITH_OFFSET, "with"),
        (PRIMORDIAL_KEYS_OFFSET, "keys"),
        (PRIMORDIAL_VALUES_OFFSET, "values"),
        (PRIMORDIAL_ENTRIES_OFFSET, "entries"),
    ]
}

// ── 属性键编码 ──────────────────────────────────────────────────────────────
// name_id 的高位区分 memory string、runtime string 和 Symbol；低位是对应表下标。
pub const NAME_ID_RUNTIME_STRING_FLAG: u32 = 0x4000_0000;
pub const NAME_ID_SYMBOL_FLAG: u32 = 0x8000_0000;
pub const NAME_ID_KIND_MASK: u32 = NAME_ID_RUNTIME_STRING_FLAG | NAME_ID_SYMBOL_FLAG;
pub const NAME_ID_INDEX_MASK: u32 = !NAME_ID_KIND_MASK;
// ── 属性槽相关常量 ──────────────────────────────────────────────────────────
// 属性槽格式（32 字节）：
// Offset 0:  name_id (4 bytes)  - 字符串或 Symbol 属性键编码
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

// ── 启动快照相关堆布局常量 ──────────────────────────────────────────────────
// 这些值决定 object heap 与 handle table 的二进制布局；任何变更都必须进入
// `wjsm-snapshot-format::abi_hash()`，否则旧启动快照会按新布局静默恢复。
pub const HEAP_OBJECT_HEADER_SIZE: u32 = 16;
pub const HEAP_OBJECT_PROTO_OFFSET: u32 = 0;
pub const HEAP_OBJECT_TYPE_OFFSET: u32 = 4;
pub const HEAP_OBJECT_HEADER_PAD_START: u32 = 5;
pub const HEAP_OBJECT_HEADER_PAD_LEN: u32 = 3;
pub const HEAP_OBJECT_HEADER_PAD_END: u32 =
    HEAP_OBJECT_HEADER_PAD_START + HEAP_OBJECT_HEADER_PAD_LEN;
pub const HEAP_OBJECT_CAPACITY_OFFSET: u32 = 8;
pub const HEAP_OBJECT_PROPERTY_COUNT_OFFSET: u32 = 12;
pub const HEAP_OBJECT_PROPERTY_SLOT_SIZE: u32 = PROP_SLOT_SIZE;
pub const HEAP_ARRAY_LENGTH_OFFSET: u32 = 8;
pub const HEAP_ARRAY_CAPACITY_OFFSET: u32 = 12;
pub const HEAP_ARRAY_ELEMENT_SIZE: u32 = 8;
pub const HANDLE_TABLE_ENTRY_SIZE: u32 = 8;
pub const GC_INITIAL_TRIGGER_BYTES: u32 = 256 * 1024;
pub const HANDLE_TABLE_GC_WINDOWS: u32 = 2;
pub const HANDLE_TABLE_MIN_ENTRIES: u32 =
    HANDLE_TABLE_GC_WINDOWS * GC_INITIAL_TRIGGER_BYTES / HEAP_OBJECT_HEADER_SIZE;
pub const HANDLE_TABLE_FUNCTION_ENTRY_FACTOR: u32 = 4;
pub const HEAP_ALLOCATION_ALIGNMENT: u32 = 8;
pub const GC_REGION_SIZE: u32 = 64 * 1024;
pub const GC_CARD_SIZE: u32 = 512;
pub const GC_BARRIER_EVENT_SIZE: u32 = 24;
pub const GC_BARRIER_EVENT_BUFFER_SIZE: u32 = 24 * 1024;

/// 返回所有会影响启动快照 object heap / handle table 兼容性的布局输入。
/// 名称也参与 hash，避免两个常量值交换时 hash 不变。
pub fn heap_layout_abi_inputs() -> &'static [(&'static str, u32)] {
    &[
        ("heap_object_header_size", HEAP_OBJECT_HEADER_SIZE),
        ("heap_object_proto_offset", HEAP_OBJECT_PROTO_OFFSET),
        ("heap_object_type_offset", HEAP_OBJECT_TYPE_OFFSET),
        ("heap_object_header_pad_start", HEAP_OBJECT_HEADER_PAD_START),
        ("heap_object_header_pad_len", HEAP_OBJECT_HEADER_PAD_LEN),
        ("heap_object_capacity_offset", HEAP_OBJECT_CAPACITY_OFFSET),
        (
            "heap_object_property_count_offset",
            HEAP_OBJECT_PROPERTY_COUNT_OFFSET,
        ),
        (
            "heap_object_property_slot_size",
            HEAP_OBJECT_PROPERTY_SLOT_SIZE,
        ),
        ("heap_array_length_offset", HEAP_ARRAY_LENGTH_OFFSET),
        ("heap_array_capacity_offset", HEAP_ARRAY_CAPACITY_OFFSET),
        ("heap_array_element_size", HEAP_ARRAY_ELEMENT_SIZE),
        ("handle_table_entry_size", HANDLE_TABLE_ENTRY_SIZE),
        ("handle_table_min_entries", HANDLE_TABLE_MIN_ENTRIES),
        ("gc_initial_trigger_bytes", GC_INITIAL_TRIGGER_BYTES),
        ("handle_table_gc_windows", HANDLE_TABLE_GC_WINDOWS),
        (
            "handle_table_function_entry_factor",
            HANDLE_TABLE_FUNCTION_ENTRY_FACTOR,
        ),
        ("name_id_runtime_string_flag", NAME_ID_RUNTIME_STRING_FLAG),
        ("name_id_symbol_flag", NAME_ID_SYMBOL_FLAG),
        ("name_id_kind_mask", NAME_ID_KIND_MASK),
        ("name_id_index_mask", NAME_ID_INDEX_MASK),
        ("heap_allocation_alignment", HEAP_ALLOCATION_ALIGNMENT),
        ("gc_region_size", GC_REGION_SIZE),
        ("gc_card_size", GC_CARD_SIZE),
        ("gc_barrier_event_size", GC_BARRIER_EVENT_SIZE),
        ("gc_barrier_event_buffer_size", GC_BARRIER_EVENT_BUFFER_SIZE),
    ]
}

// ── 属性标志位定义 ──────────────────────────────────────────────────────────
// flags 字段的位定义
pub const FLAG_CONFIGURABLE: i32 = 1 << 0; // bit 0: 可配置
pub const FLAG_ENUMERABLE: i32 = 1 << 1; // bit 1: 可枚举
pub const FLAG_WRITABLE: i32 = 1 << 2; // bit 2: 可写（数据属性专用）
pub const FLAG_IS_ACCESSOR: i32 = 1 << 3; // bit 3: 是否为访问器属性
pub const FLAG_PRIVATE: i32 = 1 << 4; // bit 4: 类私有成员槽（不参与普通属性访问）
