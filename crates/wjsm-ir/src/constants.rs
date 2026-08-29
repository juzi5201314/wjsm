//! 数据段布局常量和属性槽相关常量

use crate::string_hash;

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
// `24 + value_capacity * 8` 遍历的前提：扫描期无需查 ShapeTable，
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
pub const HEAP_OBJECT_HEADER_SIZE: u32 = 24;
pub const HEAP_OBJECT_PROTO_OFFSET: u32 = 0;
pub const HEAP_OBJECT_TYPE_OFFSET: u32 = 4;
pub const HEAP_OBJECT_HEADER_PAD_START: u32 = 5;
pub const HEAP_OBJECT_HEADER_PAD_LEN: u32 = 3;
pub const HEAP_OBJECT_HEADER_PAD_END: u32 =
    HEAP_OBJECT_HEADER_PAD_START + HEAP_OBJECT_HEADER_PAD_LEN;
/// 值槽容量（不是属性数）；与数组的 `HEAP_ARRAY_CAPACITY_OFFSET` 靠 heap_type 区分。
pub const HEAP_OBJECT_VALUE_CAPACITY_OFFSET: u32 = 8;
/// 指向宿主 `ShapeTable` 的隐藏类 id；与数组的 length 字段位置别名。
/// Native 与宿主对象首次扩容统一采用四个值槽，覆盖常见构造器字段写入。
pub const HEAP_OBJECT_INITIAL_VALUE_CAPACITY: u32 = 4;
pub const HEAP_OBJECT_SHAPE_ID_OFFSET: u32 = 12;
/// GC word：低 32 位稳定 handle，高 8 位 age；其余位保留为零。
pub const HEAP_OBJECT_GC_WORD_OFFSET: u32 = 16;
pub const HEAP_GC_HANDLE_MASK: u64 = 0xffff_ffff;
pub const HEAP_GC_AGE_SHIFT: u32 = 32;
pub const HEAP_GC_AGE_MASK: u64 = 0xff_u64 << HEAP_GC_AGE_SHIFT;
pub const HEAP_OBJECT_VALUE_SLOT_SIZE: u32 = 8;

// ── Inline Cache 槽布局（主 memory0，紧接 data segment）─────────────────────
// 每个「常量键的属性访问点」在编译期分配一个 32 字节 IC 槽：
//
// +0  u32 shape_id      命中要求与对象头 `+12` 的 shape_id 精确相等
// +4  u32 value_index   值槽下标（×8 即字节偏移；accessor 时为 getter 槽下标）
// +8  u32 kind          0=Empty 1=OwnData 2=ProtoData 3=Megamorphic 4=Accessor
//                       5=OwnDataTrio（单态 name/value/length 共用槽）
// +12 u32 proto_generation  kind=ProtoData/Accessor 时填充时的原型世代
// +16 u32 holder_handle  kind=ProtoData/Accessor 时属性所在对象的句柄；
//                       kind=OwnDataTrio 时为 `value` 的值槽下标
// +20 u32 expected_proto kind=ProtoData/Accessor 时 receiver 的直接原型句柄；
//                       kind=OwnDataTrio 时为 `length` 的值槽下标
// +24 u32 trio_site      OwnDataTrio 规划槽在预填失败时写 1，供 miss 回填识别
// +28 reserved           保留清零
//
// 空槽判定用 `kind == 0`（而非 shape_id == 0）——`SHAPE_ID_EMPTY` 是合法 shape。
// IC 区由 data segment 的零填充自动初始化为 Empty，无需运行时初始化。
//
// shape 变化会自动使 IC 失效（命中前提是 shape_id 精确相等）；原型链/accessor
// 命中额外比对 receiver 的直接原型句柄与 `proto_generation`，后者由宿主
// `ShapeTable` 在原型形状变化时 bump。生成代码从对象头和
// `NativeVmContext::proto_generation` 分别读取两项。
pub const IC_SLOT_SIZE: u32 = 32;
pub const IC_SLOT_SHAPE_ID_OFFSET: u32 = 0;
pub const IC_SLOT_VALUE_INDEX_OFFSET: u32 = 4;
pub const IC_SLOT_KIND_OFFSET: u32 = 8;
pub const IC_SLOT_PROTO_GENERATION_OFFSET: u32 = 12;
pub const IC_SLOT_HOLDER_HANDLE_OFFSET: u32 = 16;
pub const IC_SLOT_EXPECTED_PROTO_OFFSET: u32 = 20;
pub const IC_SLOT_TRIO_VALUE_INDEX_OFFSET: u32 = IC_SLOT_HOLDER_HANDLE_OFFSET;
pub const IC_SLOT_TRIO_LENGTH_INDEX_OFFSET: u32 = IC_SLOT_EXPECTED_PROTO_OFFSET;
pub const IC_SLOT_RESERVED1_OFFSET: u32 = 24;
pub const IC_SLOT_RESERVED2_OFFSET: u32 = 28;
/// 规划为 trio mega-slot 但尚未填 shape 时写在 reserved1，miss 回填据此一次写三键。
pub const IC_SLOT_TRIO_SITE_MARKER: u32 = 1;
/// 空槽：从未命中过，miss 处理器负责回填。
pub const IC_KIND_EMPTY: u32 = 0;
/// 自有数据属性：值就在接收者的值槽里。
pub const IC_KIND_OWN_DATA: u32 = 1;
/// 原型链数据属性：值在 `holder_handle` 的值槽里（方法调用主路径）。
pub const IC_KIND_PROTO_DATA: u32 = 2;
/// 退化：proxy / 字典 shape / 数组命名属性 / 非 callable accessor → 此后永久落宿主。
pub const IC_KIND_MEGAMORPHIC: u32 = 3;
/// accessor 属性：getter 在 `holder_handle` 的值槽 `value_index` 里；
/// shape + 直接原型 + 世代命中后直接 `invoke_callable(getter, receiver)`。
pub const IC_KIND_ACCESSOR: u32 = 4;
/// 单态模板对象的 `name`/`value`/`length` 共用一槽：`+4/+16/+20` 分别是三键值槽下标。
pub const IC_KIND_OWN_DATA_TRIO: u32 = 5;

// ── 类型反馈槽布局（Issue #390 运行时特化）──────────────────────────────────
// 每个「可观察动态语义」的调用点在编译期分配一个 80 字节反馈槽，由 image loader
// 分配零初始化缓冲；宿主 dispatcher 与生成代码的守卫快路径都原地写它，owner
// thread 在 CooperativePoll / dispatcher drain 读取并驱动 overlay 编译。
//
// +0  u64 last_target_image_id  PrepareCall 系列解析出的目标 image
// +8  u64 last_tag_signature    参数 tag 签名（低 4 位 tag 数，随后每 6 位一个 tag）
// +16 u32 consecutive_count     目标与签名均未变化的连续命中数
// +20 u32 total_count           总命中数
// +24 u32 caller_function       分配该槽的函数下标（首次更新时写入）
// +28 u32 site_index            全局反馈槽下标（首次更新时写入）
// +32 u32 last_target_function  目标函数在其 image 内的下标
// +36 u32 operation             观察到的 dispatcher operation / builtin id
// +40 u32 state                 0=Empty 1=Recording 2=Disabled
// +44 u32 shape_id              单态对象 shape（GetProp/SetProp）
// +48 u32 slot_or_kind          值槽下标或 elements kind
// +52 u32 proto_generation      回填时的原型世代
// +56 u32 poly_len              0–4 多态；5 = megamorphic
// +60 u32 poly_key[4]           额外 shape / callee
// +76 u32 flags                 bit0 = own_data
pub const FEEDBACK_SLOT_SIZE: u32 = 80;
pub const FEEDBACK_SLOT_TARGET_IMAGE_OFFSET: u32 = 0;
pub const FEEDBACK_SLOT_TAG_SIGNATURE_OFFSET: u32 = 8;
pub const FEEDBACK_SLOT_CONSECUTIVE_OFFSET: u32 = 16;
pub const FEEDBACK_SLOT_TOTAL_OFFSET: u32 = 20;
pub const FEEDBACK_SLOT_CALLER_FUNCTION_OFFSET: u32 = 24;
pub const FEEDBACK_SLOT_SITE_INDEX_OFFSET: u32 = 28;
pub const FEEDBACK_SLOT_TARGET_FUNCTION_OFFSET: u32 = 32;
pub const FEEDBACK_SLOT_OPERATION_OFFSET: u32 = 36;
pub const FEEDBACK_SLOT_STATE_OFFSET: u32 = 40;
pub const FEEDBACK_SLOT_SHAPE_ID_OFFSET: u32 = 44;
pub const FEEDBACK_SLOT_SLOT_OR_KIND_OFFSET: u32 = 48;
pub const FEEDBACK_SLOT_PROTO_GENERATION_OFFSET: u32 = 52;
pub const FEEDBACK_SLOT_POLY_LEN_OFFSET: u32 = 56;
pub const FEEDBACK_SLOT_POLY_KEY_OFFSET: u32 = 60;
pub const FEEDBACK_SLOT_FLAGS_OFFSET: u32 = 76;
pub const FEEDBACK_POLY_MEGAMORPHIC: u32 = 5;
pub const FEEDBACK_FLAG_OWN_DATA: u32 = 1;
/// GetElem/SetElem 接收者是 TypedArray 视图。
pub const FEEDBACK_FLAG_TYPED_ARRAY: u32 = 1 << 1;
/// 目标与 tag 签名连续相同达到该次数后，宿主把该调用点列为特化编译候选。
pub const FEEDBACK_STABLE_THRESHOLD: u32 = 100;
/// 单个调用点最多观察的实际参数 tag 数；超出部分不进入签名。
pub const FEEDBACK_MAX_TAGS: u32 = 10;
/// 单个调用点保留的特化 tag-signature 版本上限。
pub const FEEDBACK_MAX_VARIANTS_PER_SITE: u32 = 2;
pub const FEEDBACK_STATE_EMPTY: u32 = 0;
pub const FEEDBACK_STATE_RECORDING: u32 = 1;
pub const FEEDBACK_STATE_DISABLED: u32 = 2;

// ── 数组 ElementsKind（header pad 首字节）──────────────────────────────────
// kind 存在对象头 `+5`（pad 区首字节），**不借 capacity 的高位**：
// capacity 有十余处读写点（publish/relocate/array_shape/GC 扫描/handle remap…），
// 借位要求每一处都记得掩码，漏一处就静默算错数组尺寸并损坏堆。pad 字节此前
// 恒为 0、无人读写，改动面收敛在本文件与 heap_access 的 kind 存取器里。
//
// kind 让元素读只用一次字节比较就能判定「能否直接读槽」：
// - PACKED：无洞、无索引 accessor、槽为 boxed 对象句柄 → 快链可直接 load
// - HOLEY：可能含 `encode_array_hole()` → overlay 须检查哨兵；generic 洞走原型链
// - DICTIONARY：索引位置存在 accessor 等异质属性 → 必须走完整 [[Get]]
// - PACKED_NUMBER / HOLEY_NUMBER：槽存 unboxed f64（holey 用专用 hole 哨兵）；
//   GC 不按句柄扫这些槽
//
// kind 只单向升级（PACKED → HOLEY → DICTIONARY，NUMBER 同类），永不回退。
pub const HEAP_ARRAY_KIND_OFFSET: u32 = HEAP_OBJECT_HEADER_PAD_START;
/// 无洞、无异质索引属性：元素快链可直接读 boxed 槽。
pub const ARRAY_KIND_PACKED: u32 = 0;
/// 可能含洞哨兵：洞按缺失属性处理（须查原型链）。
pub const ARRAY_KIND_HOLEY: u32 = 1;
/// 索引位置存在 accessor 等异质属性：必须走完整 `[[Get]]`。
pub const ARRAY_KIND_DICTIONARY: u32 = 2;
/// packed Number 元素：槽为 unboxed f64。
pub const ARRAY_KIND_PACKED_NUMBER: u32 = 3;
/// holey Number 元素：槽为 unboxed f64 或 hole 哨兵。
pub const ARRAY_KIND_HOLEY_NUMBER: u32 = 4;
pub const HEAP_ARRAY_LENGTH_OFFSET: u32 = 8;
pub const HEAP_ARRAY_CAPACITY_OFFSET: u32 = 12;
pub const HEAP_ARRAY_ELEMENT_SIZE: u32 = 8;

// ── 字符串对象布局（header 32 字节，payload 从 +32 开始）────────────────────
// 与对象/数组共享统一 header 语义（GC 遍历、handle remap、快照恢复都依赖
// `+0 proto / +4 type / +16 gc_word` 三字段的位置不变）：
//
// +0   u32 proto            与对象/数组同位置；字符串原型（String.prototype）
// +4   u8  heap_type        = HEAP_TYPE_STRING
// +5   u8  repr             Latin1Flat | Utf16Flat | Cons | Slice | Builder
// +6   u8  flags            已内部化 / 已扁平 / 全 ASCII
// +7   u8  pad              保留为零
// +8   u32 length           码元数（UTF-16 码元语义；与数组 length 同位置）
// +12  u32 capacity         payload 字节容量（按 8 对齐）；Cons/Slice 为固定
//                           子引用区字节数（8 / 16），GC 尺寸公式统一为
//                           `HEAP_STRING_HEADER_SIZE + capacity`
// +16  u64 gc_word          handle + age（与对象/数组同位置同编码）
// +24  u32 hash             惰性内容哈希；0 = 尚未计算（与宿主 RuntimeString
//                           `HASH_UNCOMPUTED` 语义一致，真实哈希归一化到非 0）
// +28  u32 pad              保留为零
// +32  payload…             Flat/Builder 为原始字节（Latin1 每码元 1 字节，
//                           Utf16 每码元 2 字节）；Cons 为 left/right 两个
//                           独立 8 字节引用槽（与数组元素同构，写入必须走
//                           store_reference）；Slice 为 base 引用槽 +
//                           start/end 打包 word
//
// Cons/Slice 的子引用是句柄，写入必须走 store_reference（写屏障 + remset），
// 与数组元素同规则；payload 其余字节是原始数据，GC 扫描不解释。
pub const HEAP_STRING_HEADER_SIZE: u32 = 32;
pub const HEAP_STRING_REPR_OFFSET: u32 = HEAP_OBJECT_HEADER_PAD_START;
pub const HEAP_STRING_FLAGS_OFFSET: u32 = HEAP_OBJECT_HEADER_PAD_START + 1;
pub const HEAP_STRING_LENGTH_OFFSET: u32 = HEAP_ARRAY_LENGTH_OFFSET;
pub const HEAP_STRING_CAPACITY_OFFSET: u32 = HEAP_ARRAY_CAPACITY_OFFSET;
pub const HEAP_STRING_HASH_OFFSET: u32 = 24;
pub const HEAP_STRING_PAYLOAD_OFFSET: u32 = HEAP_STRING_HEADER_SIZE;
/// Cons 子引用区：left/right 各占一个独立 8 字节槽（与数组元素同构，引用值
/// 可以独立走 store_reference 与颜色处理）。
pub const HEAP_STRING_CONS_PAYLOAD_SIZE: u32 = 16;
/// Slice 子引用区：base 引用槽 + start/end 打包 word，共 16 字节。
pub const HEAP_STRING_SLICE_PAYLOAD_SIZE: u32 = 16;
/// Cons 载荷中子引用槽偏移（相对 payload，均为 8 对齐）。
pub const HEAP_STRING_CONS_LEFT_OFFSET: u32 = 0;
pub const HEAP_STRING_CONS_RIGHT_OFFSET: u32 = 8;
/// Slice 载荷中 base 引用槽偏移（相对 payload，8 对齐）。
pub const HEAP_STRING_SLICE_BASE_OFFSET: u32 = 0;
/// Slice 载荷中 start/end 打包 word 偏移（相对 payload，8 对齐；
/// 低 32 位 start、高 32 位 end——两者都非引用，可共享 word）。
pub const HEAP_STRING_SLICE_RANGE_OFFSET: u32 = 8;

// ── 字符串 repr 编码（header +5）────────────────────────────────────────────
/// 单字节载荷：每个码元 1 字节，仅当内容可编码为 Latin-1（0..=0xFF）。
pub const STRING_REPR_LATIN1_FLAT: u8 = 0;
/// 双字节载荷：每个码元 2 字节（UTF-16），ECMAScript 语义的规范表示。
pub const STRING_REPR_UTF16_FLAT: u8 = 1;
/// 拼接节点：left + right 两个子句柄，O(1) 不扁平。
pub const STRING_REPR_CONS: u8 = 2;
/// 切片节点：base 句柄 + start/end 码元区间。
pub const STRING_REPR_SLICE: u8 = 3;
/// 原地增长的累加器缓冲（非逃逸字符串拼接的宿主侧缓冲）。
pub const STRING_REPR_BUILDER: u8 = 4;

// ── 字符串 flags 位（header +6）─────────────────────────────────────────────
/// 已进入内容去重表（字面量 / 属性名）；删除去重表前必须先清该位。
pub const STRING_FLAG_INTERNED: u8 = 1 << 0;
/// Cons/Slice 已惰性扁平化（repr 改为 Flat 后由编译器置位，供调试断言）。
pub const STRING_FLAG_FLATTENED: u8 = 1 << 1;
/// 内容全部为 ASCII（0..=0x7F）；Latin-1 载荷的快速判定位，供 2.5 追平 V8。
pub const STRING_FLAG_ALL_ASCII: u8 = 1 << 2;
pub const HANDLE_TABLE_ENTRY_SIZE: u32 = 8;

// ── handle entry 状态编码（codegen ↔ 宿主堆的 ABI 契约）───────────────────
// entry = (address << 16) | state；下列判别值必须与 `wjsm_gc::HandleState`
// generated code 发布对象时写 `HANDLE_STATE_STABLE_YOUNG`，属性访问快链用
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
        ("heap_object_gc_word_offset", HEAP_OBJECT_GC_WORD_OFFSET),
        ("heap_gc_handle_mask", HEAP_GC_HANDLE_MASK as u32),
        ("heap_gc_age_shift", HEAP_GC_AGE_SHIFT),
        ("heap_gc_age_mask_upper", (HEAP_GC_AGE_MASK >> 32) as u32),
        ("heap_object_value_slot_size", HEAP_OBJECT_VALUE_SLOT_SIZE),
        ("shape_id_empty", SHAPE_ID_EMPTY),
        ("shape_map_threshold", SHAPE_MAP_THRESHOLD),
        ("dictionary_threshold", DICTIONARY_THRESHOLD),
        ("shape_table_budget", SHAPE_TABLE_BUDGET),
        // 数组 ElementsKind 占 header pad 首字节，改变数组头解释方式 → 进 ABI。
        ("heap_array_kind_offset", HEAP_ARRAY_KIND_OFFSET),
        ("array_kind_holey", ARRAY_KIND_HOLEY),
        ("array_kind_dictionary", ARRAY_KIND_DICTIONARY),
        ("array_kind_packed_number", ARRAY_KIND_PACKED_NUMBER),
        ("array_kind_holey_number", ARRAY_KIND_HOLEY_NUMBER),
        // IC 区插在 data segment 与 heap_start 之间，改变对象堆基址 → 进 ABI。
        ("ic_slot_size", IC_SLOT_SIZE),
        ("ic_slot_holder_handle_offset", IC_SLOT_HOLDER_HANDLE_OFFSET),
        (
            "ic_slot_expected_proto_offset",
            IC_SLOT_EXPECTED_PROTO_OFFSET,
        ),
        ("ic_kind_own_data", IC_KIND_OWN_DATA),
        ("ic_kind_proto_data", IC_KIND_PROTO_DATA),
        ("ic_kind_megamorphic", IC_KIND_MEGAMORPHIC),
        ("ic_kind_accessor", IC_KIND_ACCESSOR),
        ("ic_kind_own_data_trio", IC_KIND_OWN_DATA_TRIO),
        ("ic_slot_trio_site_marker", IC_SLOT_TRIO_SITE_MARKER),
        // 反馈槽布局：dispatcher 与生成代码共享写协议，改变字段解释方式 → 进 ABI。
        ("feedback_slot_size", FEEDBACK_SLOT_SIZE),
        (
            "feedback_slot_tag_signature_offset",
            FEEDBACK_SLOT_TAG_SIGNATURE_OFFSET,
        ),
        (
            "feedback_slot_consecutive_offset",
            FEEDBACK_SLOT_CONSECUTIVE_OFFSET,
        ),
        ("feedback_stable_threshold", FEEDBACK_STABLE_THRESHOLD),
        ("feedback_max_tags", FEEDBACK_MAX_TAGS),
        (
            "feedback_max_variants_per_site",
            FEEDBACK_MAX_VARIANTS_PER_SITE,
        ),
        // handle entry 状态编码：codegen 发布对象时写入，快链据此判定稳定态。
        ("handle_state_stable_min", HANDLE_STATE_STABLE_MIN),
        ("handle_state_stable_young", HANDLE_STATE_STABLE_YOUNG),
        ("handle_state_retired", HANDLE_STATE_RETIRED),
        ("heap_array_length_offset", HEAP_ARRAY_LENGTH_OFFSET),
        ("heap_array_capacity_offset", HEAP_ARRAY_CAPACITY_OFFSET),
        ("heap_array_element_size", HEAP_ARRAY_ELEMENT_SIZE),
        // 字符串对象头布局：快照恢复按 header+capacity 重建 metadata → 进 ABI。
        ("heap_string_header_size", HEAP_STRING_HEADER_SIZE),
        ("heap_string_repr_offset", HEAP_STRING_REPR_OFFSET),
        ("heap_string_flags_offset", HEAP_STRING_FLAGS_OFFSET),
        ("heap_string_hash_offset", HEAP_STRING_HASH_OFFSET),
        ("heap_string_payload_offset", HEAP_STRING_PAYLOAD_OFFSET),
        (
            "heap_string_cons_payload_size",
            HEAP_STRING_CONS_PAYLOAD_SIZE,
        ),
        (
            "heap_string_slice_payload_size",
            HEAP_STRING_SLICE_PAYLOAD_SIZE,
        ),
        // 字符串内容哈希种子：堆快照内烘焙的哈希值随种子变化 → 进 ABI。
        ("string_content_hash_seed", string_hash::STRING_HASH_SEED),
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

/// 对象模板 install 期元数据每条最多支持的 SSO 属性数。
pub const OBJECT_TEMPLATE_MAX_PROPS: u32 = 16;
/// 对象模板元数据头：`shape_id, slot_count, capacity, prop_count`。
pub const OBJECT_TEMPLATE_META_HEADER_WORDS: u32 = 4;
/// 单条对象模板元数据占用的 u32 字数（头 + 固定槽位索引数组）。
pub const OBJECT_TEMPLATE_META_WORDS: u32 =
    OBJECT_TEMPLATE_META_HEADER_WORDS + OBJECT_TEMPLATE_MAX_PROPS;

// ── 属性标志位定义 ──────────────────────────────────────────────────────────
// flags 字段的位定义
pub const FLAG_CONFIGURABLE: i32 = 1 << 0; // bit 0: 可配置
pub const FLAG_ENUMERABLE: i32 = 1 << 1; // bit 1: 可枚举
pub const FLAG_WRITABLE: i32 = 1 << 2; // bit 2: 可写（数据属性专用）
pub const FLAG_IS_ACCESSOR: i32 = 1 << 3; // bit 3: 是否为访问器属性
pub const FLAG_PRIVATE: i32 = 1 << 4; // bit 4: 类私有成员槽（不参与普通属性访问）

// ── IntrinsicPristine / IntrinsicResolve 的站点家族编码（args[0]）──────────
// 语义层调用降级与宿主共享；args[1] 恒为快路径 builtin 的 wire_id，站点
// 名字由宿主经 `intrinsic_sites` 反查（属性名不进制品常量池）。其余实参：
// - GLOBAL_IDENT / STATIC_MEMBER: 无
// - STRING_PROTO / ARRAY_PROTO:   args[2]=receiver
/// 裸全局标识符调用（`parseInt(...)`）。
pub const INTRINSIC_FAMILY_GLOBAL_IDENT: i64 = 0;
/// 内建容器静态成员调用（`String.raw(...)` / `Math.floor(...)`）。
pub const INTRINSIC_FAMILY_STATIC_MEMBER: i64 = 1;
/// %String.prototype% 方法调用（`"x".slice(...)`）。
pub const INTRINSIC_FAMILY_STRING_PROTO: i64 = 2;
/// %Array.prototype% 方法调用（`[1].map(...)`）。
pub const INTRINSIC_FAMILY_ARRAY_PROTO: i64 = 3;
