# ADR 0003: Startup Snapshot Boundary

**Status**: Accepted (default on; opt-out env; Array.prototype table ABI-checked restore remap implemented)

**Date**: 2026-06-18

## Context

wjsm 每次冷启动都重复执行 primordial bootstrap：分配 Array.prototype/Object.prototype、注册方法、创建 %AsyncIteratorPrototype% / AsyncGenerator.prototype 等。对短命 CLI / 小 fixture / future realm 创建，bootstrap 开销占比可观。

Deno 和 V8 使用 custom startup snapshot 预初始化 JS heap，启动时直接反序列化，跳过重复初始化。

## Decision

wjsm 实现 **relocatable primordial heap snapshot**：

1. **不捕获 Wasmtime Instance/Store**。Snapshot 仅保存 wasm 线性内存中对象堆片段 + handle 表相对偏移 + runtime strings + 无状态 NativeCallable 表项。
2. 恢复时按当前模块的 `__object_heap_start` 重定位，随后执行当前模块专属的 `__wjsm_init_function_props`（幂等），再进入用户 `main`。
3. Snapshot 格式为手写 little-endian 二进制，不走 JSON/serde 热路径。
4. 进程内 cache 按 cache 目录 + ABI hash 共享 primordial snapshot；磁盘 cache 使用同一 ABI 级文件。
5. 默认开启；显式设 `WJSM_STARTUP_SNAPSHOT=0`/`false`/`off` 关闭。可恢复 cache miss / 损坏 / ABI mismatch 走 cold bootstrap rebuild，不污染默认 stderr；`WJSM_STARTUP_SNAPSHOT_DEBUG=1` 输出诊断。

### Snapshot 内容

- **header**: magic `WJSMSNP\0`, format version, ABI hash, heap range, handle count, prototype handles, `arr_proto_table_base`/length/table ABI hash, 三个 `i64` 原型字段
- **object_bytes**: `memory[object_heap_start..heap_ptr]` 的原始拷贝
- **handle_rel_offsets**: `obj_table[0..count]` 中每个 entry → `entry - object_heap_start`；`obj_table[i]==0` 的 null 槽编码为 `NULL_HANDLE_REL`（`u32::MAX`），与 `rel == 0`（句柄在 heap 起点）区分
- **runtime_strings**: 计数 + 长度前缀字符串列表
- **native_callables**: 58 个无状态 `SnapshotNativeCallable` 变体的判别式表（`abi_hash` 对 discriminant `0..=57` 哈希）

### Snapshot 排除项

不在其中的表（capture 时断言为空/初始化态）：timer、microtask、promise、continuation、async generator、error、map、set、weakmap、weakset、weakref、finalization registry、pending cleanup、proxy、arraybuffer、dataview、typedarray、headers、fetch response/request、abort signal、http response、readable stream、reader、controller、byob request、writable stream、writer、transform stream、eval_cache、combinator contexts、async from sync iterators。

### ABI hash 输入

- format version
- NaN-box 常量 (`BOX_BASE`, tag 位掩码, 各类型 tag)
- heap type tags (`HEAP_TYPE_OBJECT` 等)
- 35 个 primordial 字符串的固定偏移 **与字符串内容**
- 58 个 `SnapshotNativeCallable` discriminants
- Property slot 常量 (`PROP_SLOT_SIZE`, `FLAG_*`)

### 函数属性 handle 布局变更

从 `0..num_ir_functions` 改为 `function_props_base..function_props_base+num_ir_functions`，由导出 global `__function_props_base` 决定起点。GC roots 规则同步更新。

### Wasm 导出契约变更

新增导出：

- `__wjsm_bootstrap_once: () -> i64` — 幂等 bootstrap（设置 `__bootstrap_done=1`）
- `__wjsm_init_function_props: () -> i64` — 幂等函数属性初始化（设置 `__function_props_done=1`）
- `__bootstrap_done: mutable i32`
- `__function_props_done: mutable i32`
- `__function_props_base: mutable i32`
- `__arr_proto_table_base: immutable i32`
- `__arr_proto_table_len: immutable i32`
- `__arr_proto_table_hash: immutable i64`

### Data section 新增

- 35 个固定偏移的 primordial 字符串：Array.prototype 方法名、`length`、`name`、`prototype`、`Symbol.toStringTag`、`AsyncIterator`、`AsyncGenerator`。位于 `constants::PRIMORDIAL_STRINGS_END = 493`。
- `USER_STRING_START` 变为 493（原 224）。

### RuntimeState 兼容性

`RuntimeState` 保持扁平结构，遵守 ADR 0002。Snapshot restore 直接替换 `runtime_strings`/`native_callables`/`async_iterator_prototype`/`async_gen_prototype`/`array_proto_values`，其他 side table 保持新实例的零值。

### Async scheduler 兼容性

Snapshot 不保存 scheduler、worker、async host completion channel/counter。Restore 仅在 scheduler owner 上执行。

## Consequences

### Positive

- 固定 primordial 字符串表使不同用户源码编译产物的 name_id 一致，为 snapshot ABI 提供确定性
- Bootstrap 拆分使 restore 后 main 可以直接跳过 `__wjsm_bootstrap_once`
- format/capture/restore 均为独立 owner 模块，不增长 lib.rs 热点块
- 默认开启前的 P7 release bench 显示 warm snapshot 端到端快于关闭路径（`full execute off` 3.92–4.47ms/each，`full execute on warm` 3.75–3.94ms/each），且 cache miss 不再重复 instantiate。

### Negative / Risks

- **函数表索引是模块局部值**：snapshot header 记录 seed 模块 `arr_proto_table_base`、表长度和表 ABI hash；restore 先校验当前模块 `__arr_proto_table_len`/`__arr_proto_table_hash`，再把 Array.prototype 方法函数值重定位到当前模块 `__arr_proto_table_base`。
- 新增 builtin/NativeCallable/primordial string 时必须更新 ABI hash 输入表，否则 snapshot 会静默不匹配；Array.prototype 方法顺序/集合由 backend host import registry 的 `ArrayPrototypeMethod` 分组和导出的表 ABI hash 负责。

## Alternatives Considered

- **Wasmtime Instance/Store snapshot**：绑定了特定编译产物的 linear memory 布局，不可跨模块重定位，且 wasmtime snapshot API 不稳定。
- **Per-module snapshot (单模块缓存)**：曾可绕开 `indirect call type mismatch`，但会失去跨模块共享收益；现由 `arr_proto_table_base` 导出 + restore 重定位替代。
- **JSON/serde 序列化**：较简单的格式，但 restore 热路径的解析和分配开销大。

## References

- V8 custom startup snapshots: https://v8.dev/blog/custom-startup-snapshots
- Deno deno_core snapshot: https://github.com/denoland/deno_core
- wjsm ADR 0002: RuntimeState stays flat
