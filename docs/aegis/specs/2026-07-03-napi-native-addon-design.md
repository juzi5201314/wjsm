# N-API 原生模块兼容层设计（真 .node，C ABI 先行）

- 日期：2026-07-03
- 状态：待用户审阅
- 输入：`.claude/deep-research-report-napi.md`（外部深度调研报告）、issue #313/#312/#310/#308、ADR 0003/0004、2026-06-14 GC 框架 spec、2026-06-07 side-table lifecycle spec
- 关联 issue：#313（本 spec 修正其 N-API 评估）、#310/#312/#308（垂直切片借用其最小子集）

## 1. 背景与上游输入修正

外部调研报告建议 wjsm 实现 Node 原生模块兼容，推荐"完整 N-API 支持"（其方案 B）。但报告开篇承认无法获取源码，全文基于「wjsm 嵌入 V8/QuickJS」的推测。与真实架构对照，报告的架构假设全部不成立：

| 维度 | 报告假设 | 真实架构 |
|---|---|---|
| JS 引擎 | 嵌入 V8/QuickJS | 无引擎：AOT 编译 JS→WASM，wasmtime 执行 |
| 值表示 | V8 Handle | NaN-boxed i64 + WASM 线性内存堆 + host side table |
| 模块系统 | 运行时 require | 编译期 bundler（`cjs_transform.rs`）；动态 require 未实现（#312） |
| process/Buffer | 注入即可 | 不存在（#310 未实现） |
| 事件循环 | libuv 类似物 | tokio + scheduler（`AsyncHostCompletion` 通道，统一异步模型） |

**同时修正 issue #313 的评估**：#313 将 N-API 判为 "Tier S 基本无解"，理由是 ".node 链接 V8 的 C++ ABI"。该理由对 NAN/直接 V8 API 插件成立，但对纯 N-API 插件不成立——napi_* 是引擎无关的纯 C ABI，符号由宿主进程提供；Bun（JavaScriptCore）与 Deno 均无 Node 的 C++ 运行时而实现了兼容层。wjsm 的真实落差是 "JS 堆在 WASM 实例内 + 值为 NaN-boxed i64"，本 spec 论证该落差可桥接。

Baseline Role Alignment 判定：**Design Defect（scope: both）**——两个上游输入（报告的架构假设、#313 的 Tier S 结论）都有缺陷，本 spec 即修正后的基线；批准后在 #313 上回帖修正评估并挂新实现 issue。

**与报告的关系**：本 spec 是报告方案 B 的架构修正版。报告方案 A（最小 .node 加载 + 全局对象注入）被否定——wjsm 没有 V8，dlopen 之后插件没有任何可调用的 API，"最小加载"不构成可用切片。报告的测试计划（node-addon-api 测试套、真实包集、CI 矩阵）被吸收进第 9 节验收；报告的人月工时表不采用，计划按可验收里程碑组织。

## 2. 目标与非目标

### 目标

在 wjsm 中实现 Node-API（NAPI_VERSION 8）兼容层，使 npm 上以 N-API 编译的预编译原生模块（napi-rs 平台包、prebuilds、node-gyp 产物）无需重编译即可在 wjsm 中加载运行。ABI 路线为**共享语义层，先 C ABI**：napi 语义层设计为 ABI 无关（值访问经语义层原语，不直接绑定裸指针边界），首期交付进程符号导出 + dlopen 真 .node；WASM ABI 前端（wasm32-wasip1 插件 + napi_* 作为 wasm import）为后续独立计划，本 spec 只保证语义层不为其设障。

### 永久非目标

- NAN / 直接 V8 API 插件（依赖 `v8::*`、`node::*` C++ 符号）：物理不可能。dlopen 后找不到 `napi_register_module_v1` 且探测到 V8 符号依赖时，报可诊断错误（指明"仅支持 N-API 插件"）。
- `process.binding`、reach 进引擎内部的包。
- 伪造 `node.exe` / 完整 libuv 移植。

### 本计划非目标（归属其他 issue）

- worker_threads 内加载插件（#313 另一项；本设计的重入 executor 与 TSFN 通道保持 agent 线程兼容，不为其设障）。
- sharp（stretch goal，牵涉 stream/fs 面超界）。
- #310/#312/#308 中未被第 7 节切片清单引用的部分。

## 3. 架构总览

```
┌─ wjsm 进程 ──────────────────────────────────────────────┐
│ wasmtime(async, epoch) ── WASM 实例（用户 JS，NaN-boxed） │
│    ▲ func_table + call_async         ▲ host imports      │
│    │                                 │                    │
│ ┌──┴─────────────────────────────────┴───────────────┐   │
│ │ napi 语义层：wjsm-runtime 内 runtime_napi/          │   │
│ │  napi_env │ handle scope 栈 │ napi_ref 表 │ 重入     │   │
│ │  executor │ TSFN 队列 │ async work │ 模块注册表      │   │
│ │  ⇄ 复用：alloc_host_object / store_runtime_string / │   │
│ │    define_host_data_property / HostSideTable roots / │   │
│ │    weakref_finalization / scheduler completion 通道  │   │
│ └──▲──────────────────────────────────────────────────┘   │
│    │ napi_* C 符号（#[no_mangle]，进程导出表）             │
│ libloading::dlopen("addon.node")                          │
│    ▲                                                      │
│ require('./addon.node') ──编译期识别──▶ __wjsm_load_native │
└───────────────────────────────────────────────────────────┘
```

**所有权**：napi 语义层的 canonical owner 是 `crates/wjsm-runtime/src/runtime_napi/`（按函数族分文件，遵守 ≤500 行纪律）+ `host_imports/napi_load.rs`（加载管道）。它是 store 内 JS realm 与原生代码之间的唯一桥接所有者；所有 napi_* 经语义层进入现有堆操作原语，禁止旁路直写线性内存。

**为什么放 runtime 内部而非独立 crate**：napi 实现必须访问 `pub(crate)` 的堆原语、side table、GC roots；独立 crate 会迫使 runtime 开放内部 API，违反"单函数公共 API"约定。C 符号导出直接在 runtime crate 内 `#[no_mangle] pub extern "C"`（rlib 符号传递至 bin），`wjsm-cli/build.rs` 配置平台链接参数。

## 4. 核心设计

### 4.1 napi_env：表示与生命周期

- 一个 store 一个 JS realm；**napi_env 按 addon 实例化**（Node 语义：env per addon per realm）。env 结构包含：所属 addon 标识、handle scope 栈、napi_ref 表、instance data、pending exception 槽、cleanup hook 链、`current_call: *mut CallContext`（见 4.4）。
- env 生命周期 = store 生命周期。store teardown 前逆序执行 `napi_add_env_cleanup_hook` 注册的钩子；`napi_add_async_cleanup_hook` 支持异步完成后再继续 teardown。
- 线程约束与 Node 相同：除 TSFN 明确列出的函数外，napi_* 仅允许 JS 线程调用；debug 构建断言线程 id，release 构建行为与 Node 等价（UB 边界一致）。

### 4.2 napi_value / handle scope / GC 挂接

- `napi_value = *mut i64`：指向当前 handle scope arena 中的 slot，slot 存 NaN-boxed i64。选择 slot 指针而非直接把 i64 当 napi_value，是为了满足 N-API "handle 在 scope 内有效" 的语义并给 GC 一个可枚举的 root 集。
- handle scope 栈 per env：`napi_open_handle_scope`/`close` 压弹 arena 段；`napi_open_escapable_handle_scope` + `napi_escape_handle` 将值复制进父 scope 的 slot。进入插件回调前由桥接层自动开 scope，返回后自动关（Node 等价）。
- **GC 挂接**：所有活 scope 的 slot 作为新 root 源接入 `runtime_gc` 的 roots 扫描（与 `HostSideTable::pinned` 同级）。GC 可在 G1/ZGC 中移动对象，但对外引用只保存 NaN-boxed handle；INV-C1 保证 handle 恒定，slot 内 i64 无需改写，只需保活。此 root 源纳入 GC 回归矩阵（#338）。

### 4.3 napi_ref / external / finalizer

- `napi_ref`：refcount > 0 时进 pinned root 集（复用 `HostSideTable` pin 语义的独立 ref 表）；refcount == 0 转弱引用，复用 `weakref_finalization.rs` 的 sweep 后通知机制判活，`napi_get_reference_value` 对已回收目标返回 NULL 值（Node 等价）。
- `napi_wrap` / `napi_create_external` / `napi_create_external_arraybuffer` / `napi_create_external_buffer` 的 finalize 回调：挂 finalization 队列，sweep 后在 JS 线程调度执行（Node 语义 finalizer 同样不在 GC 中间执行）。`napi_remove_wrap` 解除并跳过 finalize。
- `napi_adjust_external_memory`：真实接入 #337 的 JS 堆预算——外部内存计数参与 GC 触发启发式，不做空操作。

### 4.4 同步重入 executor（最深设计点）

调用链形态：JS 调插件函数 → 该调用是 async host import，运行在 wasmtime fiber 栈（epoch 协作，`epoch_deadline_async_yield_and_update`，`lib.rs:358`）→ 桥接层在 `CallContext` 中登记 `Caller`/`WasmEnv` 裸指针、设置 `env.current_call` → 同线程同步调用插件 C 函数 → 插件调 napi_*，语义层经 `current_call` 直接操作堆（多数 napi_* 无需重入 WASM）→ 返回后清空 `current_call`。

**napi_call_function / napi_new_instance / getter-setter 触发等需要重入 WASM 执行 JS**：实现自研**嵌套重入 executor**——在当前线程循环 poll 内层 `func_table` 取出的 funcref 的 `call_async` future 直至 Ready。

无死锁论证：
1. Pending 唯一稳态来源是 epoch yield（`epoch_deadline_async_yield_and_update(1)` 的 yield future 由自身 waker 立即唤醒），继续 poll 即推进；
2. 内层 JS 的 `await` 不阻塞调用（同步段返回 pending promise，completion 经 scheduler 通道由外层主循环处理，与现有 promise 语义一致）；
3. 内层再调插件、插件再回调 JS 的任意深度嵌套，每层都在同一 OS 线程的 fiber/C 交替栈上，无跨线程等待。

**不采用 `tokio::task::block_in_place + Handle::block_on`**：它要求 multi-thread runtime，而 agent 线程（`agent_cluster.rs:47`）是 current_thread runtime；自研 executor 在两种线程形态下统一可用，为将来 worker_threads 内 N-API 不留死角。

栈深度：嵌套重入消耗宿主线程栈（fiber 栈由 wasmtime 分配）。深递归插件回调链的栈溢出行为与 Node（同样是原生栈递归）等价，wasmtime async 栈大小沿用现有配置，不为此单独加限制。

### 4.5 napi_threadsafe_function

- 数据结构：JS 函数值（napi_ref 强持有）+ context + max_queue_size 有界/无界队列 + acquire/release 线程计数。
- 任意线程 `napi_call_threadsafe_function` → 投递到 scheduler 的 napi 事件队列（与 `AsyncHostCompletion` 通道并轨的新枚举臂，`runtime_startup.rs:126-134` 体系）→ 主事件循环收到后在 JS 线程开 scope、经重入 executor 执行 call_js 回调。语义与 `uv_async_send` 同构。
- `napi_tsfn_blocking` 在队列满时阻塞投递线程（有界 channel 天然背压）；`napi_tsfn_nonblocking` 返回 `napi_queue_full`。
- 存活语义与 Node 一致：ref 状态的 TSFN 使事件循环保持存活（接入 scheduler 的存活计数），`napi_unref_threadsafe_function` 解除；`napi_release_threadsafe_function` 计数归零触发 finalize（JS 线程）。

### 4.6 napi_async_work

- `napi_queue_async_work` → `tokio::task::spawn_blocking` 线程池执行 execute 回调（约束与 Node 相同：execute 内禁止调用需要 env 的 napi_*）→ 完成后经 completion 通道回 JS 线程执行 complete 回调（带 napi_status）。
- `napi_cancel_async_work`：仅在尚未开始执行时可取消（complete 收到 `napi_cancelled`），已开始则返回 `napi_generic_failure`——Node 等价。

### 4.7 加载管道

1. **编译期**（`wjsm-module`）：resolver 将解析到 `.node` 文件的 require/import 降低为运行时调用 `__wjsm_load_native(abs_path)`（新 host import），不进 JS 编译管线。`.node` 的路径解析遵循 Node 规则（含目录 `index.node`、package.json main 指向 .node）。
2. **运行时**（`host_imports/napi_load.rs`）：原生模块注册表（path → exports 缓存，重复 require 返回缓存）→ `libloading` dlopen → 符号协商：优先 `node_api_module_get_api_version_v1`（无则按 version 8），查 `napi_register_module_v1`；缺失时探测 V8/NAN 符号依赖并报第 2 节的可诊断错误 → 构造 napi_env + exports 对象（`alloc_host_object`）→ 调用注册函数，插件填充 exports → 返回 NaN-boxed exports。
3. **插件导出函数的可调用表示**：新增 `NativeCallable` 变体 `NapiCallback { env_id, cb: napi_callback, data }`（实体存 side table）。JS 调用它 → 分派进 4.4 的桥接层。**快照 ABI 纪律**：新增变体改变 `SnapshotNativeCallable` discriminants，必须同步更新 `wjsm-snapshot-format::abi_hash()`；`NapiCallback` 含进程内裸指针，**不可快照**——快照捕获阶段遇到已加载原生模块时放弃捕获（embedded snapshot 只覆盖 primordial 启动堆，用户代码 require 的插件本就发生在快照恢复之后，正常路径无交集）。
4. dlopen 的库**不主动 dlclose**（Node 同样常驻；插件生命周期以 env cleanup hooks 收尾）。

### 4.8 符号导出（跨平台）

- 符号清单从语义层函数表**单一来源生成**（build 时代码生成 no_mangle 转发 + 各平台链接清单，防实现/导出漂移）。
- Linux：`-Wl,--export-dynamic`（cli build.rs 注入）。
- macOS：node-gyp 产物默认 `-undefined dynamic_lookup`，宿主可执行文件符号默认可见，无额外动作。
- Windows：node-gyp 产物 delay-load `node.exe` 并携带 `win_delay_load_hook`（将 "node.exe" 模块查找重定向到宿主进程句柄）；wjsm.exe 经 `.def`/`/EXPORT` 链接参数将 napi_* 放入 PE 导出表即可，不伪造 node.exe。
- 三平台均为 CI 矩阵验收项（第 9 节）。

### 4.9 错误与异常模型

- napi_status 全集实现；所有 napi_* 在 env 有 pending exception 时立即返回 `napi_pending_exception`（Node 等价短路语义）。
- `napi_throw*` 写入 env pending exception 槽；插件调用返回桥接层时，pending exception 转入 **TAG_EXCEPTION 可捕获通道**（JS `try/catch` 可捕获插件抛出的异常）。禁止走 `set_runtime_error` 不可捕获通道——该通道仅保留给加载器自身的致命错误（如 dlopen 失败、符号缺失）。
- `napi_fatal_error` → 进程 abort（Node 等价）；`napi_fatal_exception` → 触发 uncaughtException 语义（当前映射为 runtime error 出口，exit 2）。

### 4.10 数据视图与指针稳定性

- `napi_get_arraybuffer_info` / `napi_get_typedarray_info` / `napi_get_dataview_info` / `napi_get_buffer_info` 返回 host 侧 `ArrayBufferEntry.data`（`types.rs:109`，`Vec<u8>`）的裸指针：该 backing store 不位于 JS 对象堆，GC 仅移动/重映射持有它的 JS wrapper handle；INV-C1 保证 handle 恒定，Vec 定长期间不重分配，指针在 Node 等价的有效窗口内稳定；resizable ArrayBuffer resize 后旧指针失效，与 V8 backing store 行为一致。
- `ArrayBufferEntry` 扩展 backing 表示：`Owned(Vec<u8>)` | `External { ptr, len, finalize_ctx }`，支撑 `napi_create_external_arraybuffer`/`napi_create_external_buffer`（零拷贝，finalize 归还所有权）。
- `napi_detach_arraybuffer` / `napi_is_detached_arraybuffer` 完整实现。
- 字符串：`napi_create_string_utf8/utf16/latin1` 与 `napi_get_value_string_*` 按现有 runtime 字符串语义转换（编码边界沿用 wjsm 全局字符串行为，不在本 spec 扩大或收窄）。

### 4.11 明确降级面（诚实的不支持，非 stub）

- `napi_get_uv_event_loop`：wjsm 无 libuv，返回 `napi_generic_failure` 并置可诊断的 extended error info（message 指明 wjsm 不提供 uv loop）。这是文档化的语义决定，等价于 Node `--no-addons` 类明确拒绝，而非假装成功的 stub。依赖它的插件在此边界得到确定性失败。

## 5. M0 地基清单（垂直切片借用面）

切片纪律：清单内 spec-complete，清单外 not-present（不留 stub）；各项归属注明，完成后在对应 roadmap issue 勾选。

- **process 最小面**（归属 #310 提前切片）：`platform`、`arch`、`version`、`versions`（含 `node`/`napi`/`modules` 键）、`env`、`execPath`、`cwd()`、`release.name === "node"`（node-gyp-build 探测路径依赖）。`versions` 取值硬约束：`napi` ≥ `"8"` 且必须使 node-gyp-build 优先命中 `node.napi.node`（napi 构建）路径；宣称的 `node`/`modules` 具体数字在实现计划中锁定为一个真实 Node LTS 的组合（两者必须互相一致，不得虚构组合）。
- **Buffer 核心**（归属 #310 提前切片；所有者与未来全量 Buffer 同一，扩展而非并行实现）：`Buffer.alloc/allocUnsafe/from(string|array|arraybuffer|buffer)/isBuffer/byteLength/concat`；实例：`length`、索引读写、`toString(utf8|hex|base64|latin1)`、`slice/subarray`、`copy`、`write`、`equals/compare`、`readUInt8/writeUInt8` 系最小读写器；`Buffer.prototype instanceof Uint8Array === true`（napi_is_buffer/napi_is_typedarray 双真，Node 等价）。
- **`__dirname` / `__filename`**（归属 #310）：cjs_transform 编译期常量注入（node-gyp-build 定位 prebuilds 必需）。
- **`.node` require 管道 + dlopen + 注册表**（本 spec 4.7）。

## 6. M1：NAPI_VERSION 8 全函数面

按函数族组织实现文件（`runtime_napi/` 下每族一文件，预计 12–15 个）：

值创建与读取 / 类型判定与强制转换 / 对象与属性（含 `napi_define_properties`、property attributes 全集）/ 数组与 TypedArray/DataView / 函数调用与构造（`napi_create_function`、`napi_call_function`、`napi_new_instance`、`napi_get_cb_info`、`napi_get_new_target`）/ class（`napi_define_class` + `napi_wrap` 家族）/ 错误异常（4.9）/ handle scope（4.2）/ napi_ref（4.3）/ external 与 finalizer（4.3、4.10）/ Promise（`napi_create_promise`/`resolve`/`reject`/`is_promise`，接入现有 promise 表）/ BigInt、Date、Symbol（wjsm 已有对应 NaN-box tag）/ env 元数据（`napi_get_version`、`napi_get_node_version`——返回宣称的 Node 兼容版本，与 `process.versions` 同源）/ instance data / cleanup hooks（4.1）/ **async work（4.6）+ TSFN（4.5）+ buffer（4.10）**。

M1 含异步是用户明确决策：重入 executor（4.4）与 TSFN 调度（4.5）在 M1 内完成并以 node-addon-api 官方测试套的 async 目录验收，不设 spike 缓冲。

## 7. M2：生态验收所需的受限动态加载

- **动态 require 的 `.node` 子集**：`__wjsm_load_native` 接受运行时计算路径，但仅当解析结果为 `.node` 文件；解析到 JS/JSON 的动态 require 报确定性错误并指向 #312（不悄悄扩大为通用动态 require）。
- **fs 只读三函数**（归属 #308 提前切片）：`existsSync`、`readdirSync`、`statSync`（node-gyp-build 枚举 prebuilds 所需的最小面，同步、只读）。
- node-gyp-build / bindings 两个定位器包经上述面原生跑通（不做包 shim、不改写包内容）。

## 8. 测试策略

1. **单元/集成**（wjsm 仓内）：handle scope 与 GC 交互（分配风暴中 scope slot 保活）、napi_ref 强弱转换与 finalizer 时序、重入 executor 嵌套深度与 epoch yield 交叠、TSFN 多线程投递压力、快照 ABI hash 变更防回归。GC 侧用例并入 #338 回归矩阵。
2. **fixtures**：`fixtures/napi/` 新目录——C 测试插件源码入库、构建产物进 `/tmp`（构建脚本用系统 cc，测试 harness 编译后加载），`.expected` 快照对齐现有 E2E 纪律。
3. **node-addon-api 官方测试套**：作为 submodule/外部拉取，按目录分批点亮，M1 验收线 = 同步 + async work + TSFN 目录全绿；点亮清单入 CI。
4. **真实包冒烟**（M2）：napi-rs 官方 examples 全套、`@napi-rs/bcrypt`、`better-sqlite3`（open/exec/query prepared statements）。
5. **CI**：Linux/macOS/Windows 三平台矩阵跑 1–4（符号导出与 delay-load hook 唯一的真实验证场）。

## 9. 里程碑与验收线

| 里程碑 | 内容 | 验收 |
|---|---|---|
| M0 地基 | 第 5 节全部 | 手写最小 C 插件（值往返 + 回调 + Buffer 读写）三平台端到端；现有 fixtures 全绿零回归 |
| M1 语义层全量 | 第 4、6 节全部（含 async work/TSFN/cleanup hooks/重入 executor） | node-addon-api 测试套同步 + 异步目录全绿；napi-rs hello world + async task example |
| M2 生态验收 | 第 7 节 + 修复面 | napi-rs examples 全套 + @napi-rs/bcrypt + better-sqlite3 冒烟；node-addon-api 套持续扩绿 |

## 10. 风险与既定对策

| 风险 | 对策（设计内，非遗留问题） |
|---|---|
| 重入 executor 遇到未预期的 Pending 源（epoch 之外） | 4.4 的无死锁论证覆盖现有三类源；executor 加 debug 计数器，poll 超阈值 panic with 诊断（fail-fast 暴露新 Pending 源而非死等） |
| 插件在非 JS 线程误用 env | debug 断言线程 id；release 与 Node 同为 UB 边界（文档声明） |
| 新 root 源导致 GC 漏标/多标 | 单元用例入 #338 矩阵；handle scope root 扫描走与 HostSideTable 相同的审计路径 |
| Windows delay-load 兼容差异（不同 node-gyp 版本 hook 行为） | CI Windows 矩阵用真实 node-gyp/prebuildify 产物验收，不用手工模拟 |
| 快照 ABI 漂移 | 4.7 的 abi_hash 同步是变更清单硬项；防回归测试固化 |

## 11. 兼容边界与 ADR 信号

- 现有 WASM 契约（imports/exports/globals）不变——napi 全在 host 侧，不新增 WASM 模块导入面（仅新增 host import `__wjsm_load_native`，属常规 host import 演进）。
- 快照边界：见 4.7；embedded snapshot 与原生模块无交集（插件加载必然晚于快照恢复点）。
- 沙箱边界让渡：加载原生插件的进程放弃插件代码的 WASM 沙箱隔离（与 Node 等价）。这是产品级决定，记入 ADR。
- **ADR 信号**：实现期补 `docs/adr/0005-napi-boundary.md`——进程符号导出面、napi_env 生命周期 = store 生命周期、沙箱让渡声明、`napi_get_uv_event_loop` 拒绝决定、WASM ABI 前端预留的语义层不变量。
- 回写上游：批准后在 #313 修正 Tier S 评估并开实现 issue 挂其子项；M0/M2 完成项在 #310/#308 对应勾选。

## 附录：工作制品

**TaskIntentDraft** — 结果：本 spec 获批 → writing-plans；成功证据：三里程碑验收线全绿；停止条件：M2 验收完成；非目标见第 2 节；风险见第 10 节。

**BaselineReadSetHint / BaselineUsageDraft** — Required refs：调研报告、#313/#312/#310/#308、ADR 0003/0004、GC 框架 spec、side-table lifecycle spec、CLAUDE.md WASM 契约/快照纪律——全部已读并引用（第 1、4 节）；Missing：无；Decision：continue。

**ImpactStatementDraft** — 受影响：`wjsm-runtime`（runtime_napi/、host_imports/napi_load.rs、NativeCallable、scheduler 队列、GC roots、Buffer/process builtins）、`wjsm-module`（.node resolver 降低）、`wjsm-semantic`（__dirname/__filename 注入点若在 lower 层）、`wjsm-cli`（build.rs 链接参数）、`wjsm-snapshot-format`（abi_hash）；不变量：WASM 契约、快照 ABI 纪律、GC 可达性纪律、两条异常通道边界。

**Product Risk Lens** — Value：npm 原生模块生态开箱可用（wjsm 从"玩具级 Node 兼容"跨入"生态级"）；Non-goals：NAN/V8、worker 内插件；Trade-offs：沙箱让渡（ADR 记录）、维护 ~150 C ABI 函数面的长期成本；Decision needed：已由用户四项拍板（C ABI 先行/垂直切片/M1 含异步/三验收基准）。
