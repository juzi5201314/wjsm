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
// ── 隐藏类（Shape）与紧凑值数组 ────────────────────────────────────────────
// 属性元数据（name_id / flags / 值槽下标）由宿主侧 `wjsm-gc::ShapeTable` 持有，
// 堆内只留紧凑值数组：每槽 8 字节 boxed i64，与数组元素完全同构。
// accessor 属性占两个相邻值槽（index = getter，index + 1 = setter），无侧表。
//
// 这条同构性是 GC / handle remap / ZGC 重定位 / 快照恢复能统一按
// `16 + value_capacity * 8` 遍历的前提：扫描期无需查 ShapeTable，
// 未使用的值槽恒为 0（即 +0.0，不是句柄），扫到也是惰性的。

/// 空对象 shape；`ShapeTable` 的 0 号记录恒为它。
pub const SHAPE_ID_EMPTY: u32 = 0;
/// 属性数达到该阈值时，shape 内建 name_id → 下标哈希表；之下线性扫。
pub const SHAPE_MAP_THRESHOLD: u32 = 8;
/// 属性数超过该阈值的对象退化为字典 shape（独占、不共享、IC 不回填）。
pub const DICTIONARY_THRESHOLD: u32 = 64;
/// ShapeTable 的全局 shape 数预算；超出后新 transition 一律退化字典。
///
/// 这里刻意**不**用「单个 shape 的 transition 出边上限」：空 shape 是整棵
/// transition 树的根，它的出边数等于全程序中「作为首个属性出现过的名字」个数
/// （bootstrap 一轮就有数百个）。按出边设限会让根 shape 迅速触顶，此后每个新对象
/// 都退化成字典、inline cache 永久失效——正是要避免的病态。
///
/// 全局预算直接约束真正关心的东西（shape 表内存），且正常程序远不会触及：
/// shape 数正比于源码中不同的「属性名序列」条数，实测 bootstrap + 用户代码
/// 通常在数百量级。
pub const SHAPE_TABLE_BUDGET: u32 = 1 << 16;

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
/// 值槽容量（不是属性数）；与数组的 `HEAP_ARRAY_CAPACITY_OFFSET` 靠 heap_type 区分。
pub const HEAP_OBJECT_VALUE_CAPACITY_OFFSET: u32 = 8;
/// 指向宿主 `ShapeTable` 的隐藏类 id；与数组的 length 字段位置别名。
pub const HEAP_OBJECT_SHAPE_ID_OFFSET: u32 = 12;
pub const HEAP_OBJECT_VALUE_SLOT_SIZE: u32 = 8;

// ── Inline Cache 槽布局（主 memory0，紧接 data segment）─────────────────────
// 每个「常量键的属性访问点」在编译期分配一个 16 字节 IC 槽：
//
// +0  u32 shape_id      命中要求与对象头 `+12` 的 shape_id 精确相等
// +4  u32 value_index   值槽下标（×8 即字节偏移）
// +8  u32 kind          0=Empty 1=OwnData 2=ProtoData 3=Megamorphic
// +12 u32 proto_generation  kind=ProtoData 时填充时的原型世代
//
// 空槽判定用 `kind == 0`（而非 shape_id == 0）——`SHAPE_ID_EMPTY` 是合法 shape。
// IC 区由 data segment 的零填充自动初始化为 Empty，无需运行时初始化。
//
// shape 变化会自动使 IC 失效（命中前提是 shape_id 精确相等）；原型链命中
// 额外比对 `proto_generation`，由宿主 `ShapeTable` 在原型形状变化时 bump。
pub const IC_SLOT_SIZE: u32 = 16;
pub const IC_SLOT_SHAPE_ID_OFFSET: u32 = 0;
pub const IC_SLOT_VALUE_INDEX_OFFSET: u32 = 4;
pub const IC_SLOT_KIND_OFFSET: u32 = 8;
pub const IC_SLOT_PROTO_GENERATION_OFFSET: u32 = 12;
/// 空槽：从未命中过，miss 处理器负责回填。
pub const IC_KIND_EMPTY: u32 = 0;
/// 自有数据属性：值就在接收者的值槽里。
pub const IC_KIND_OWN_DATA: u32 = 1;
/// 原型链数据属性：值在 `proto_holder` 的值槽里（方法调用主路径）。
pub const IC_KIND_PROTO_DATA: u32 = 2;
/// 退化：accessor / proxy / 字典 shape / 数组命名属性 → 此后永久落宿主。
pub const IC_KIND_MEGAMORPHIC: u32 = 3;

// ── 数组 ElementsKind（header pad 首字节）──────────────────────────────────
// kind 存在对象头 `+5`（pad 区首字节），**不借 capacity 的高位**：
// capacity 有十余处读写点（publish/relocate/array_shape/GC 扫描/handle remap…），
// 借位要求每一处都记得掩码，漏一处就静默算错数组尺寸并损坏堆。pad 字节此前
// 恒为 0、无人读写，改动面收敛在本文件与 heap_access 的 kind 存取器里。
//
// kind 让元素读只用一次字节比较就能判定「能否直接读槽」：
// - PACKED：无洞、无索引 accessor → 快链可直接 load
// - HOLEY：可能含 `encode_array_hole()` → 落宿主（洞须按缺失属性继续查原型链）
// - DICTIONARY：索引位置存在 accessor 等异质属性 → 必须走完整 [[Get]]
//
// kind 只单向升级（PACKED → HOLEY → DICTIONARY），永不回退：回退需要全扫描，
// 而三种状态都只决定是否走快链，不影响语义正确性。
pub const HEAP_ARRAY_KIND_OFFSET: u32 = HEAP_OBJECT_HEADER_PAD_START;
/// 无洞、无异质索引属性：元素快链可直接读槽。
pub const ARRAY_KIND_PACKED: u32 = 0;
/// 可能含洞哨兵：洞按缺失属性处理（须查原型链），故落宿主。
pub const ARRAY_KIND_HOLEY: u32 = 1;
/// 索引位置存在 accessor 等异质属性：必须走完整 `[[Get]]`。
pub const ARRAY_KIND_DICTIONARY: u32 = 2;
pub const HEAP_ARRAY_LENGTH_OFFSET: u32 = 8;
pub const HEAP_ARRAY_CAPACITY_OFFSET: u32 = 12;
pub const HEAP_ARRAY_ELEMENT_SIZE: u32 = 8;
pub const HANDLE_TABLE_ENTRY_SIZE: u32 = 8;

// ── handle entry 状态编码（codegen ↔ 宿主堆的 ABI 契约）───────────────────
// entry = (address << 16) | state；下列判别值必须与 `wjsm_gc::HandleState`
// 逐一对应。wasm 侧发布对象时写 `HANDLE_STATE_STABLE_YOUNG`，属性访问快链用
// `state >= HANDLE_STATE_STABLE_MIN` 单比较判定「地址可直接使用」。
//
// 稳定态刻意排在连续高位区间，使快链省下两次相等比较与两个分支。
pub const HANDLE_STATE_FREE: u32 = 0;
pub const HANDLE_STATE_RETIRED: u32 = 1;
pub const HANDLE_STATE_RELOCATING_YOUNG: u32 = 2;
pub const HANDLE_STATE_RELOCATING_OLD: u32 = 3;
pub const HANDLE_STATE_STABLE_YOUNG: u32 = 4;
pub const HANDLE_STATE_STABLE_OLD: u32 = 5;
pub const HANDLE_STATE_PINNED_OLD: u32 = 6;
/// 稳定态下界：`state >= 此值` ⇔ entry 地址可被直接使用。
pub const HANDLE_STATE_STABLE_MIN: u32 = HANDLE_STATE_STABLE_YOUNG;
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
        (
            "heap_object_value_capacity_offset",
            HEAP_OBJECT_VALUE_CAPACITY_OFFSET,
        ),
        ("heap_object_shape_id_offset", HEAP_OBJECT_SHAPE_ID_OFFSET),
        (
            "heap_object_value_slot_size",
            HEAP_OBJECT_VALUE_SLOT_SIZE,
        ),
        ("shape_id_empty", SHAPE_ID_EMPTY),
        ("shape_map_threshold", SHAPE_MAP_THRESHOLD),
        ("dictionary_threshold", DICTIONARY_THRESHOLD),
        ("shape_table_budget", SHAPE_TABLE_BUDGET),
        // 数组 ElementsKind 占 header pad 首字节，改变数组头解释方式 → 进 ABI。
        ("heap_array_kind_offset", HEAP_ARRAY_KIND_OFFSET),
        ("array_kind_holey", ARRAY_KIND_HOLEY),
        ("array_kind_dictionary", ARRAY_KIND_DICTIONARY),
        // IC 区插在 data segment 与 heap_start 之间，改变对象堆基址 → 进 ABI。
        ("ic_slot_size", IC_SLOT_SIZE),
        ("ic_kind_own_data", IC_KIND_OWN_DATA),
        ("ic_kind_proto_data", IC_KIND_PROTO_DATA),
        ("ic_kind_megamorphic", IC_KIND_MEGAMORPHIC),
        // handle entry 状态编码：codegen 发布对象时写入，快链据此判定稳定态。
        ("handle_state_stable_min", HANDLE_STATE_STABLE_MIN),
        ("handle_state_stable_young", HANDLE_STATE_STABLE_YOUNG),
        ("handle_state_retired", HANDLE_STATE_RETIRED),
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
