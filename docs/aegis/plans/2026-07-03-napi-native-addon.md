**执行状态**: 未开始。M0（T0.1–T0.8）→ M1（T1.0–T1.12）→ M2（T2.1–T2.7）。

# N-API 原生模块兼容层实施计划

**Goal**: 实现 spec `2026-07-03-napi-native-addon-design.md` 定义的 N-API（NAPI_VERSION 8）兼容层：wjsm 进程导出 napi_* C 符号，dlopen 加载 npm 预编译 `.node` 插件，napi 语义桥接到 NaN-boxed i64 + WASM 堆。M0 地基（process/Buffer 最小面 + 加载管道）→ M1 全函数面（含 async work/TSFN/重入 executor）→ M2 生态验收（node-addon-api 测试套、napi-rs、better-sqlite3）。

**Architecture**: 见 spec 第 3-4 节。napi 语义层 owner = `crates/wjsm-runtime/src/runtime_napi/`（新模块组）；加载管道复用 bundler 合成模块 + `get_builtin_global` + `NativeCallable` 调用分派，**不新增 WASM import 面**（对 spec 4.7 "`__wjsm_load_native` host import" 的实现精化：`__wjsmLoadNative` 走 `get_builtin_global` 查表 → `NativeCallable::WjsmLoadNative` 调用分派，语义等价、复用全部既有管线）。

**Tech Stack**: Rust 2024、wasmtime（async + epoch）、libloading（dlopen）、tokio multi-thread（CLI）/current-thread（agent）、cc（测试插件编译）、vendored Node N-API 头文件（MIT）。测试 `cargo nextest`（per-test 9s）。

**Baseline/Authority Refs**:
- `docs/aegis/specs/2026-07-03-napi-native-addon-design.md`（唯一设计权威；§4 六项核心决策、§5 M0 清单、§10 风险对策为硬约束）
- issue #313（本计划完成后回写修正）、#310/#308（切片归属勾选）、#312（动态 require 边界引用）
- ADR 0004（快照/embedded ABI hash 纪律）、`docs/aegis/specs/2026-06-14-pluggable-gc-framework-design.md`（roots 纪律）
- 记忆基线：wjsm 两条异常通道——napi pending exception 必须走可捕获 TAG_EXCEPTION 通道（spec §4.9）

**BaselineUsageDraft**: Required refs 全部已读并在任务中按行号引用；Missing: 无；Decision: continue。

**Compatibility Boundary**（必须保持）:
- 现有 fixtures 全绿；WASM 契约（imports/exports/globals）零变更
- 快照 ABI hash **不变**：process/Buffer/加载器全部惰性构造（首访问经 `get_builtin_global`，晚于快照捕获点）+ 全部用 runtime strings（`store_runtime_string_in_state`），不加 primordial 字符串、不加 `SnapshotNativeCallable` 变体；T0.8 加防御测试
- `NapiCallback` 等含裸指针变体**禁止入快照**（`wjsm-snapshot-format/src/lib.rs:74-75` 既有纪律），capture 遇到即放弃捕获
- 两条异常通道边界：插件异常走 TAG_EXCEPTION；仅加载器致命错误（dlopen 失败/符号缺失）走 `set_runtime_error`
- napi_* 除 TSFN 白名单外仅 JS 线程调用（debug 断言线程 id）

**Verification**（全局）:
- 每任务后 `cargo nextest run --workspace` 全绿 + `cargo build` 零 warning
- M0 收口：`tests/napi_addon.rs` 端到端（cc 编译 C 插件 → run_file_in_process）
- M1 收口：`tools/napi/run_node_addon_api_tests.sh` 点亮清单全绿（含 async/TSFN 目录）
- M2 收口：`tools/napi/ecosystem/*.sh`（napi-rs examples、@napi-rs/bcrypt、better-sqlite3）

**ADR Signal**（保留自 spec §11，完成后补 `docs/adr/0005-napi-boundary.md`）: 进程符号导出面、napi_env 生命周期 = store 生命周期、沙箱让渡、`napi_get_uv_event_loop` 拒绝决定、WASM ABI 前端预留不变量。baseline-sync：CLAUDE.md 需增 N-API 段落；#313 Tier S 评估修正。

---

## Plan Pressure Test

```text
- Owner / contract / retirement:
    Owner: runtime_napi/（napi 语义唯一 owner）+ wjsm-napi-symbols（符号清单单一来源）。
    Contract: napi_* C ABI（NAPI_VERSION 8，以 vendored 头文件为准）；NativeCallable 新变体 4 个；
    AsyncHostCompletion 新臂 1 个（RunOnOwner）。
    Retirement: 无退役项（纯新增面）；#313 的"基本无解"结论文本由回写修正。
- Architecture integrity / higher-level path: 加载管道复用 bundler 合成模块 + get_builtin_global
    查表 + NativeCallable 分派，放弃"新 WASM import"路径——更高层复用，WASM 契约零变更。
- Verification scope: 每任务独立验证命令；三收口线（M0 端到端 / M1 测试套 / M2 生态）；
    GC 交互用例并入 #338 矩阵。
- Task executability: 每任务给出确切文件、完整数据结构、锚点实现、确切命令；
    M1 函数族任务附 NAPI_VERSION 8 完整函数清单（符号全集由 wjsm-napi-symbols 宏机械保证）。
- Pressure result: proceed
```

## Plan-Time Complexity Check

```text
- Target files:
    新建 crate: crates/wjsm-napi-symbols（zero-dep：符号清单 + 定义宏）
    新建: crates/wjsm-runtime/src/runtime_napi/{mod,env,handle_scope,reentry,abi_values,abi_object,
      abi_function,abi_class_wrap,abi_error,abi_ref_external,abi_buffer_typedarray,abi_promise_misc,
      abi_lifecycle,async_work,tsfn,module_registry,node_globals,node_fs}.rs（每文件 ≤500 行）
    新建: crates/wjsm-runtime/napi-headers/*.h（vendored）、fixtures/napi/、tools/napi/
    修改: wjsm-runtime/src/{types.rs(NativeCallable 变体),scheduler.rs(RunOnOwner 臂),
      host_imports/get_builtin_global_entry.rs(4 个查表臂),runtime_gc/…/roots 接线,lib.rs(mod 声明)}
    修改: wjsm-module/src/{resolver.rs(.node 候选+合成模块),cjs_transform.rs(__dirname 注入+动态 require 降低)}
    修改: wjsm-cli/build.rs(平台链接参数)
- Existing size / shape signals: types.rs 的 NativeCallable 已 160+ 变体（owner 即在此，加 4 变体合规）；
    get_builtin_global_entry.rs 105 行（加 4 臂无压力）。
- Owner fit: napi 全部逻辑进 runtime_napi/，get_builtin_global/分派点只做一行转发。
- Add-in-place risk: 无大文件增长点；abi_* 按族分文件防单文件膨胀。
- Recommendation: add owner file（runtime_napi/ 模块组 + wjsm-napi-symbols crate）+ split task（三阶段独立提交）。
```

## Tasks 总览

| 阶段 | 任务 | 交付 |
|------|------|------|
| M0 | T0.1 process / T0.2 Buffer 构造+静态 / T0.3 Buffer 实例方法 / T0.4 __dirname+resolver .node / T0.5 符号 crate+dlopen+env 骨架 / T0.6 最小 napi 面+scope+GC roots / T0.7 端到端 harness / T0.8 快照防御 | 手写 C 插件端到端 |
| M1 | T1.0 重入 executor / T1.1–T1.8 八个函数族 / T1.9 async work / T1.10 TSFN / T1.11 node-addon-api 套 / T1.12 mac+win 符号导出 | NAPI_VERSION 8 全量 |
| M2 | T2.1 动态 .node require / T2.2 fs 三函数 / T2.3 node-gyp-build 链路 / T2.4 napi-rs 冒烟 / T2.5 better-sqlite3 / T2.6 CI 矩阵 / T2.7 ADR+回写 | 生态验收 |

每任务提交格式：`feat: <内容> (#<新实现 issue>)`（T0.1 前先开实现 issue 挂 #313 子项，见 T0.0）。

---

# 阶段 M0：地基

## T0.0 开实现 issue

**Steps**:
- [ ] `gh issue create --title "实现 N-API 原生模块兼容层（M0 地基）" --body "$(cat <<'EOF'
按 docs/aegis/specs/2026-07-03-napi-native-addon-design.md 实施。父任务 #313。
本 issue 覆盖 M0：process/Buffer 最小面（#310 切片）、.node 加载管道、napi_env/handle scope 骨架、最小 napi 面、端到端 harness。
EOF
)" --label enhancement` → 记录编号 `#N0`，后续提交消息引用。

## T0.1 process 全局对象（#310 切片）

**Files**:
- create: `crates/wjsm-runtime/src/runtime_napi/mod.rs`、`crates/wjsm-runtime/src/runtime_napi/node_globals.rs`
- modify: `crates/wjsm-runtime/src/types.rs`（RuntimeState + NativeCallable）、`crates/wjsm-runtime/src/lib.rs`（`mod runtime_napi;`）、`crates/wjsm-runtime/src/host_imports/get_builtin_global_entry.rs`、GC roots 扫描文件（见步骤）
- test: `fixtures/happy/napi_process_global.js` + `.expected`

**Why**: node-gyp-build/bindings 探测依赖 `process.platform/arch/versions`；spec §5 清单。

**Impact/Compatibility**: 惰性构造（首次 `get_builtin_global("process")` 时），快照 ABI 不变；`process_object` 是新 GC root。

**Verification**: `cargo nextest run -E 'test(happy__napi_process_global)'`；`cargo nextest run --workspace` 全绿。

**Steps**:
- [ ] **写 fixture（RED）** `fixtures/happy/napi_process_global.js`：
```js
console.log(typeof process === "object");
console.log(typeof process.platform === "string" && process.platform.length > 0);
console.log(typeof process.arch === "string");
console.log(process.version.startsWith("v"));
console.log(typeof process.versions.node === "string");
console.log(Number(process.versions.napi) >= 8);
console.log(typeof process.versions.modules === "string");
console.log(process.release.name === "node");
console.log(typeof process.env === "object");
console.log(typeof process.execPath === "string");
console.log(typeof process.cwd() === "string" && process.cwd().length > 0);
```
`.expected`：11 行 `true`，exit 0。`cargo nextest run -E 'test(happy__napi_process_global)'` 确认 RED（当前 `process` 为 undefined 报 TypeError）。
- [ ] **RuntimeState 字段 + NativeCallable 变体**。`types.rs` RuntimeState 加：
```rust
/// 惰性构造的 process 全局对象缓存（GC root；undefined 编码表示未构造）。
pub(crate) process_object: Mutex<i64>,
```
（`new` 中初始化为 `Mutex::new(value::encode_undefined())`）。NativeCallable 加：
```rust
/// process.cwd() 等 process 方法。kind: 0=cwd
ProcessMethod { method: u8 },
```
- [ ] **实现 `node_globals.rs`**：`ensure_process_object(caller) -> i64`——若缓存已是 object 直接返回；否则 `alloc_host_object` + `define_host_data_property_with_env` 逐项写入：`platform`（`std::env::consts::OS` 映射 `linux|darwin|win32`）、`arch`（`std::env::consts::ARCH` 映射 `x64|arm64|...`）、`version`/`versions`（子对象：`node`/`modules`/`napi`——取值硬约束见 spec §5：`node="22.11.0"`,`modules="127"` 为互相一致的真实 LTS 组合；`napi="8"`——napi 字段语义是本运行时保证的 Node-API 能力面，M1 实现 v8 全量故声明 8，禁止虚报更高版本诱导插件调用未实现面）、`env`（子对象，`std::env::vars()` 全量）、`execPath`（`std::env::current_exe()`）、`release` 子对象（`name:"node"`）、`cwd`（`create_native_callable(ProcessMethod{method:0})`）。写入缓存。`ProcessMethod` 调用分派：在 NativeCallable 调用 match 处（搜 `NativeCallable::GcCollect =>` 的分派点照模式）加臂，cwd 返回 `store_runtime_string_in_state(std::env::current_dir())`。
- [ ] **接线**：`get_builtin_global_entry.rs:83` 的 `_ =>` 前加 `"process" => { drop(native_callables); return runtime_napi::node_globals::ensure_process_object(&mut caller); }`（注意先 drop 锁）。GC roots：找 `regexp_prototype` 被扫描为 root 的位置（`rg "regexp_prototype" crates/wjsm-runtime/src/runtime_gc/`），同位置加 `process_object` 锁读值。
- [ ] **GREEN + 提交**：fixture 绿、workspace 绿、零 warning 后 `git commit -m "feat: process 全局对象最小面（napi M0，#N0，#310 切片）"`。

## T0.2 Buffer 构造器 + 静态方法（#310 切片）

**Files**:
- create: `crates/wjsm-runtime/src/runtime_napi/node_buffer.rs`
- modify: `types.rs`（NativeCallable 加 `BufferConstructor`、`BufferStaticMethod { method: u8 }`、`BufferProtoMethod { method: u8 }`；RuntimeState 加 `buffer_prototype: Mutex<i64>` root）、`get_builtin_global_entry.rs`（`"Buffer"` 臂）、GC roots
- test: `fixtures/happy/napi_buffer_static.js` + `.expected`

**Why**: napi_create_buffer/napi_is_buffer 及插件 JS 胶水的数据交换基础；spec §5。

**Impact/Compatibility**: Buffer 实例 = 现有 Uint8Array 基础设施（typedarray side table）+ proto 指向 `Buffer.prototype`（其 proto 链回 Uint8Array.prototype，保证 `buf instanceof Uint8Array === true`，napi_is_typedarray/napi_is_buffer 双真）。

**Verification**: `cargo nextest run -E 'test(happy__napi_buffer_static)'`。

**Steps**:
- [ ] **写 fixture（RED）**：
```js
const b = Buffer.alloc(4);
console.log(b.length === 4 && b[0] === 0);
const c = Buffer.from([1, 2, 3]);
console.log(c[2] === 3);
const d = Buffer.from("hi");
console.log(d.length === 2 && d[0] === 0x68);
const e = Buffer.from(new ArrayBuffer(8));
console.log(e.length === 8);
console.log(Buffer.isBuffer(c) === true && Buffer.isBuffer([1]) === false);
console.log(Buffer.byteLength("héllo", "utf8") === 6);
const f = Buffer.concat([c, d]);
console.log(f.length === 5 && f[3] === 0x68);
console.log(c instanceof Uint8Array);
const g = Buffer.allocUnsafe(3);
console.log(g.length === 3);
```
`.expected`：9 行 `true`。
- [ ] **实现**：`ensure_buffer_constructor(caller) -> i64`——构造 `Buffer.prototype`（`alloc_host_object`，proto 设为 Uint8Array.prototype——经现有 typedarray 原型获取路径，`rg "Uint8Array" crates/wjsm-runtime/src/runtime_typedarray.rs` 找原型 handle 入口）+ `BufferConstructor` native callable 上挂静态方法属性（`alloc`=0/`allocUnsafe`=1/`from`=2/`isBuffer`=3/`byteLength`=4/`concat`=5 的 `BufferStaticMethod`）。实例创建走现有 Uint8Array 分配路径后改写 proto header + side-table 标记 `is_buffer: bool`（`TypedArrayEntry` 加字段；napi_is_buffer 据此判定）。`Buffer(n)` 直调等价 `alloc`。分派臂照 `ProcessMethod` 模式。
- [ ] **GREEN + 提交** `feat: Buffer 构造器与静态方法（napi M0，#N0，#310 切片）`。

## T0.3 Buffer 实例方法面（#310 切片）

**Files**: modify `node_buffer.rs`；test `fixtures/happy/napi_buffer_proto.js`

**Why/Impact**: spec §5 实例清单；全部挂 `Buffer.prototype`（`BufferProtoMethod`），索引读写/length 由 Uint8Array 基础设施天然提供。

**Verification**: 新 fixture 绿 + workspace 绿。

**Steps**:
- [ ] **写 fixture（RED）** 覆盖：`toString("utf8"|"hex"|"base64"|"latin1")`、`slice/subarray`（共享底层验证：写子视图读父）、`copy`（返回字节数+偏移语义）、`write`（返回写入字节数）、`equals/compare`、`readUInt8/writeUInt8/readUInt32LE/writeUInt32LE/readUInt32BE/writeUInt32BE`（spec §5 "最小读写器"落地为 U8/U32 两宽度 × LE/BE + 越界 RangeError）。每断言一行 `true`。
- [ ] **实现** `BufferProtoMethod` 方法枚举（0=toString,1=slice,2=subarray,3=copy,4=write,5=equals,6=compare,7=readUInt8,8=writeUInt8,9=readUInt32LE,10=writeUInt32LE,11=readUInt32BE,12=writeUInt32BE）；hex/base64 编解码手写（无新依赖，base64 标准字母表 + padding）。
- [ ] **GREEN + 提交** `feat: Buffer 实例方法面（napi M0，#N0，#310 切片）`。

## T0.4 __dirname/__filename 注入 + resolver .node 合成模块

**Files**:
- modify: `crates/wjsm-module/src/cjs_transform.rs`（使用检测 + 顶部 const 注入）、`crates/wjsm-module/src/resolver.rs`（`.node` 候选 + 合成模块）、`crates/wjsm-module/src/bundler.rs`（路径传递，如需）
- test: `crates/wjsm-module/tests/`（现有测试模式旁加单元测试）+ `fixtures/happy/napi_dirname.js`

**Why**: node-gyp-build 用 `__dirname` 定位 prebuilds；`.node` require 必须降低为运行时加载调用（spec §4.7 第 1 步）。

**Impact/Compatibility**: 仅 CJS 变换模块注入 `__dirname/__filename`（Node 语义：ESM 无）；`.node` 在扩展名候选**末位**（Node 顺序 .js/.json 之后）。合成模块源：
```js
export default __wjsmLoadNative("<canonical-abs-path>");
```
`__wjsmLoadNative` 为未声明全局 → 现有 lowering 自动走 `get_builtin_global` 查表（T0.5 接臂），**零管线改动**。

**Verification**: `cargo nextest run -p wjsm-module`；`fixtures/happy/napi_dirname.js`（打印 `typeof __dirname === "string"` 与文件名后缀断言）。

**Steps**:
- [ ] **写 wjsm-module 单元测试（RED）**：(a) CJS 模块体含 `__dirname` 标识符 → 变换输出首部有 `const __dirname = "<模块目录>";`；不含则不注入。(b) `resolve_path` 对 `./addon.node`（测试临时目录放置空文件）返回该路径；(c) resolver 对 `.node` 产出合成 source（断言含 `__wjsmLoadNative` 与 canonical 路径字符串）。
- [ ] **实现**：`file_candidates` 增补 `node` 扩展（置于现有 `MODULE_EXTENSIONS` 之后，不改 JS/TS 顺序）；`ModuleResolver` 载入模块处（读文件 source 的位置）对 `.node` 扩展生成合成 source 而非读二进制；`CjsTransformer` 记录标识符使用（walk 时置位）+ `transform_module` 注入 const（路径由 bundler 传入构造器）。
- [ ] **GREEN + 提交** `feat: __dirname 注入与 .node 合成模块降低（napi M0，#N0）`。

## T0.5 wjsm-napi-symbols crate + dlopen 管道 + napi_env 骨架 + NapiCallback

**Files**:
- create: `crates/wjsm-napi-symbols/{Cargo.toml,src/lib.rs}`（zero-dep）
- create: `crates/wjsm-runtime/src/runtime_napi/{env.rs,module_registry.rs}`
- create: `crates/wjsm-runtime/napi-headers/{js_native_api.h,js_native_api_types.h,node_api.h,node_api_types.h}`（从 Node v22.x include/node/ 原样拷贝，文件头注明来源与 MIT 许可）
- modify: `types.rs`（NativeCallable 加 `WjsmLoadNative`、`NapiCallback { env_id: u32, cb: usize, data: usize }`）、`get_builtin_global_entry.rs`（`"__wjsmLoadNative"` 臂）、`crates/wjsm-runtime/Cargo.toml`（`libloading = "0.8"`、`wjsm-napi-symbols` 依赖）、`crates/wjsm-cli/build.rs`（Linux `-Wl,--export-dynamic`）
- test: `crates/wjsm-runtime/tests/napi_symbols_exported.rs`

**Why**: spec §4.7 加载管道 + §4.8 符号单一来源。

**Impact/Compatibility**: `NapiCallback` 含裸指针，禁入快照（T0.8 防御）；dlopen 的库不 dlclose（spec §4.7 第 4 点）。

**Verification**: `cargo nextest run -p wjsm-runtime -E 'test(napi_symbols)'`；`nm -D target/debug/wjsm | grep napi_create_object` 非空。

**Steps**:
- [ ] **建符号 crate**。`wjsm-napi-symbols/src/lib.rs`：
```rust
//! N-API 导出符号清单（单一来源）。runtime 用宏定义 extern fn，cli build.rs 用清单生成链接参数。
//! 清单必须与 vendored napi-headers 的 NAPI_VERSION 8 声明一致（T0.6/T1.x 逐族补全实现时同步增补）。
#[macro_export]
macro_rules! for_each_napi_symbol {
    ($m:ident) => {
        // M0 最小面（T0.6 实现）；M1 各族任务在此追加，追加后 cli 链接参数自动跟进。
        $m!(napi_get_undefined); $m!(napi_get_null); $m!(napi_get_boolean); $m!(napi_get_global);
        $m!(napi_create_object); $m!(napi_create_double); $m!(napi_create_int32); $m!(napi_create_uint32);
        $m!(napi_create_int64); $m!(napi_create_string_utf8); $m!(napi_get_value_string_utf8);
        $m!(napi_get_value_double); $m!(napi_get_value_int32); $m!(napi_get_value_int64);
        $m!(napi_get_value_uint32); $m!(napi_get_value_bool); $m!(napi_typeof);
        $m!(napi_set_named_property); $m!(napi_get_named_property); $m!(napi_has_named_property);
        $m!(napi_create_function); $m!(napi_get_cb_info);
        $m!(napi_throw_error); $m!(napi_throw); $m!(napi_is_exception_pending);
        $m!(napi_get_and_clear_last_exception); $m!(napi_get_last_error_info);
        $m!(napi_open_handle_scope); $m!(napi_close_handle_scope);
        $m!(napi_get_version); $m!(napi_module_register);
        $m!(node_api_get_module_file_name);
    };
}
pub fn symbol_names() -> Vec<&'static str> {
    let mut v = Vec::new();
    macro_rules! push { ($n:ident) => { v.push(stringify!($n)); }; }
    for_each_napi_symbol!(push);
    v
}
```
- [ ] **vendor 头文件**：`curl -sL https://raw.githubusercontent.com/nodejs/node/v22.11.0/src/js_native_api.h -o crates/wjsm-runtime/napi-headers/js_native_api.h`（同法拉 `js_native_api_types.h`、`node_api.h`、`node_api_types.h`），每文件头部加注释块（来源 URL、commit 标签、MIT）。
- [ ] **napi_env 骨架** `env.rs`：
```rust
/// napi_env 实体。Box 固定地址，env_id 索引 RuntimeState.napi_envs。
#[repr(C)]
pub(crate) struct NapiEnvData {
    pub(crate) env_id: u32,
    /// 每 env 的 handle scope 栈（chunk 链，slot 地址稳定，T0.6 实现）。
    pub(crate) scopes: HandleScopeStack,
    pub(crate) last_error: NapiExtendedError,
    pub(crate) pending_exception: i64, // encode_undefined = 无
    pub(crate) instance_data: Option<(usize /*data*/, usize /*finalize_cb*/, usize /*hint*/)>,
    pub(crate) cleanup_hooks: Vec<(usize /*hook*/, usize /*arg*/)>,
    pub(crate) module_path: String, // node_api_get_module_file_name
    pub(crate) js_thread: std::thread::ThreadId,
}
pub type RawNapiEnv = *mut NapiEnvData;
/// 当前调用上下文：进入插件前压栈，返回后弹栈。仅 JS 线程 TLS。
pub(crate) struct NapiCallCtx {
    pub(crate) caller: *mut wasmtime::Caller<'static, crate::RuntimeState>,
    pub(crate) wasm_env: *const crate::wasm_env::WasmEnv,
    pub(crate) env: RawNapiEnv,
}
thread_local! {
    pub(crate) static NAPI_CALL_STACK: std::cell::RefCell<Vec<NapiCallCtx>> = const { std::cell::RefCell::new(Vec::new()) };
}
```
`RuntimeState` 加 `napi_envs: Mutex<Vec<Box<NapiEnvData>>>`、`napi_module_registry: Mutex<HashMap<PathBuf, i64>>`、`napi_libs: Mutex<Vec<libloading::Library>>`（registry 的 exports 值是新 GC root 集：roots 接线同 T0.1）。
- [ ] **dlopen 管道** `module_registry.rs`：`load_native_module(caller, env_ref, path) -> Result<i64>`——canonical 路径查 registry 缓存 → `unsafe { libloading::Library::new(&path) }`（失败→`set_runtime_error` 加载器致命通道）→ `lib.get::<unsafe extern "C" fn() -> i32>(b"node_api_module_get_api_version_v1")` 可选协商（>8 也接受，函数面按 8 报告）→ `lib.get(b"napi_register_module_v1")`；缺失时依次探测 `_register_`/`v8`/`node` 前缀符号存在性，报 spec §2 的可诊断错误（"仅支持 N-API 插件"）→ 建 env（Box 入表）+ `alloc_host_object` exports → 压 `NapiCallCtx` + 开 root scope → 调注册函数 → 弹栈；返回值非 NULL 用返回值否则用 exports；写 registry。`WjsmLoadNative` 分派臂：参数字符串 → `load_native_module`。
- [ ] **cli build.rs**：
```rust
if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
    println!("cargo:rustc-link-arg-bins=-Wl,--export-dynamic");
}
```
- [ ] **写导出自检测试（RED→GREEN）** `napi_symbols_exported.rs`：`wjsm_napi_symbols::symbol_names()` 非空且含 `"napi_create_object"`；（符号真实进导出表由 T0.7 端到端验证——dlopen 的插件解析符号失败即 RED）。
- [ ] **提交** `feat: napi 符号清单 crate、dlopen 管道与 napi_env 骨架（napi M0，#N0）`。

## T0.6 最小 napi 面 + handle scope + GC roots + status 模型

**Files**:
- create: `runtime_napi/{handle_scope.rs,abi_core.rs}`
- modify: `env.rs`（补 HandleScopeStack）、GC roots 接线、`lib.rs`
- test: `crates/wjsm-runtime/tests/napi_handle_scope_gc.rs`

**Why**: T0.5 的符号清单需要实现体；handle scope 是 GC 正确性的根基（spec §4.2）。

**Impact/Compatibility**: 新 GC root 源（所有 env 的活 scope slots + pending_exception + registry exports），进 #338 矩阵。

**Verification**: `cargo nextest run -p wjsm-runtime -E 'test(napi_handle_scope)'`。

**Steps**:
- [ ] **HandleScopeStack**（`handle_scope.rs`）：
```rust
/// slot 地址稳定的 scope 栈：chunk 固定 256 slot，chunk 满开新 chunk，永不 realloc 旧 chunk。
pub(crate) struct HandleScopeStack {
    chunks: Vec<Box<[i64; 256]>>,
    len: usize,                    // 全局已用 slot 数
    scopes: Vec<ScopeMark>,        // 每 scope 的起始 len + escapable 预留槽
}
pub(crate) struct ScopeMark { start: usize, escape_slot: Option<*mut i64>, escaped: bool }
impl HandleScopeStack {
    pub(crate) fn alloc_slot(&mut self, v: i64) -> *mut i64 { /* len→chunk 定位，写值返回槽指针 */ }
    pub(crate) fn open(&mut self, escapable: bool) { /* escapable 时先在父 scope 预留 escape_slot */ }
    pub(crate) fn close(&mut self) { /* len 回退到 start */ }
    pub(crate) fn escape(&mut self, v: i64) -> Option<*mut i64> { /* 写入预留槽，一次性 */ }
    pub(crate) fn live_values(&self) -> impl Iterator<Item = i64> + '_ { /* roots 扫描 */ }
}
```
- [ ] **abi 宏与 status 模型**（`abi_core.rs`）：每个 napi 函数体统一模式——TLS 取 `NapiCallCtx`（空 → `napi_generic_failure`）、debug 断言线程 id、pending exception 非空且函数不在豁免名单（`napi_is_exception_pending`/`napi_get_and_clear_last_exception`/`napi_get_last_error_info`/`napi_throw*`）→ 返回 `napi_pending_exception`。定义宏 `napi_abi_fn!` 封装该前奏。用 `for_each_napi_symbol!` + 编译期断言（zero-sized 数组技巧或单测比对 `symbol_names()` 与实现注册表长度）保证清单=实现。
- [ ] **实现 T0.5 清单全部符号**（`for_each_napi_symbol!` 当前全集，实现数=清单数由编译期断言保证）。锚点（其余同构）：
```rust
#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_create_string_utf8(
    env: RawNapiEnv, s: *const c_char, len: usize, result: *mut napi_value,
) -> napi_status {
    napi_abi_fn!(env, ctx, {
        if result.is_null() || s.is_null() { return invalid_arg(env); }
        let bytes = if len == usize::MAX { CStr::from_ptr(s).to_bytes() }
                    else { std::slice::from_raw_parts(s.cast::<u8>(), len) };
        let owned = String::from_utf8_lossy(bytes).into_owned();
        let caller = &mut *ctx.caller;
        let v = crate::store_runtime_string_in_state(caller.data(), owned);
        *result = (*ctx.env).scopes.alloc_slot(v).cast();
        napi_ok(env)
    })
}
```
（`napi_value = *mut i64` 经 `.cast()`；读值统一 `fn value_of(v: napi_value) -> i64 { unsafe { *(v as *mut i64) } }`。）`napi_create_function` → `create_native_callable(NapiCallback{env_id, cb, data})` + 函数名属性；`napi_get_cb_info` 从 `NapiCallCtx` 补充的调用帧（argc/argv slots/this/data——`NapiCallCtx` 加 `frame: Option<NapiCbFrame>`）读出；`NapiCallback` 分派臂（call 分派 match）：压 ctx + frame + 开 scope → 调 cb → 关 scope → 若 pending_exception 非 undefined → 取出并按现有 TAG_EXCEPTION 抛出路径返回（`rg "TAG_EXCEPTION" crates/wjsm-runtime/src/` 找编码助手），否则返回 cb 返回槽的值（NULL → undefined）。`napi_module_register`（老式 NAPI_MODULE 宏路径）：记录 pending 注册供 dlopen 后消费。`node_api_get_module_file_name` 返回 `env.module_path`。
- [ ] **GC roots 接线 + 写 GC 交互测试（RED→GREEN）** `napi_handle_scope_gc.rs`：进程内构造 store + env，scope 内 alloc 大量字符串/对象 slot，手动触发 GC（照 `GcCollect` 测试模式），断言 slot 值仍可读（对象属性完好）；close scope 后再 GC，断言堆量回落（经 `heap_used_bytes` 或对象计数）。
- [ ] **提交** `feat: 最小 napi 面 + handle scope + GC roots（napi M0，#N0）`。

## T0.7 端到端 harness（C 插件）

**Files**:
- create: `fixtures/napi/addons/hello/hello.c`、`fixtures/napi/hello.js`、`tools/napi/build_addon.sh`
- create: `tests/napi_addon.rs`（顶层，参照 `tests/fixture_runner.rs` 用 `wjsm_cli::run_file_in_process`）

**Why**: M0 收口验收（spec §9 M0 行）。

**Impact/Compatibility**: 插件编译产物进 `/tmp/wjsm-napi-fixtures/`（项目目录零污染）；测试 `cfg(target_os = "linux")` 先行（mac/win 在 T1.12 解锁）。

**Verification**: `cargo nextest run -E 'test(napi_addon)'` 输出匹配。

**Steps**:
- [ ] **写 C 插件**（值往返 + 回调 + 属性，M0 面内）：
```c
// fixtures/napi/addons/hello/hello.c — M0 端到端：字符串/数值往返、napi_create_function、异常。
#include <node_api.h>
static napi_value Add(napi_env env, napi_callback_info info) {
  size_t argc = 2; napi_value argv[2];
  napi_get_cb_info(env, info, &argc, argv, NULL, NULL);
  double a, b;
  if (napi_get_value_double(env, argv[0], &a) != napi_ok ||
      napi_get_value_double(env, argv[1], &b) != napi_ok) {
    napi_throw_error(env, NULL, "expected numbers"); return NULL;
  }
  napi_value out; napi_create_double(env, a + b, &out); return out;
}
static napi_value Greet(napi_env env, napi_callback_info info) {
  size_t argc = 1; napi_value argv[1]; char buf[64]; size_t n;
  napi_get_cb_info(env, info, &argc, argv, NULL, NULL);
  napi_get_value_string_utf8(env, argv[0], buf, sizeof buf, &n);
  char out[80]; snprintf(out, sizeof out, "hello %s", buf);
  napi_value s; napi_create_string_utf8(env, out, NAPI_AUTO_LENGTH, &s); return s;
}
NAPI_MODULE_INIT() {
  napi_value fn;
  napi_create_function(env, "add", NAPI_AUTO_LENGTH, Add, NULL, &fn);
  napi_set_named_property(env, exports, "add", fn);
  napi_create_function(env, "greet", NAPI_AUTO_LENGTH, Greet, NULL, &fn);
  napi_set_named_property(env, exports, "greet", fn);
  return exports;
}
```
- [ ] **build 脚本** `tools/napi/build_addon.sh`：`cc -shared -fPIC -I crates/wjsm-runtime/napi-headers -o "$OUT" "$SRC"`（mac 加 `-undefined dynamic_lookup`）。
- [ ] **JS 侧** `fixtures/napi/hello.js`：
```js
const addon = require("./hello.node");
console.log(addon.add(2, 40) === 42);
console.log(addon.greet("wjsm") === "hello wjsm");
try { addon.add("x"); } catch (e) { console.log(e.message === "expected numbers"); }
```
- [ ] **集成测试（RED→GREEN）** `tests/napi_addon.rs`：`#[cfg(target_os="linux")]`——tempdir 组装（拷 hello.js、编译 hello.node 到同目录）→ `run_file_in_process` → 断言 stdout 三行 `true`、exit 0。RED 先跑（T0.5/T0.6 有缺即在此暴露：符号未导出 → dlopen 报 undefined symbol）。
- [ ] **提交** `feat: napi 端到端 harness（C 插件 add/greet/throw，napi M0，#N0）`。

## T0.8 快照防御 + M0 收口

**Files**: modify snapshot capture 变体转换处；test `crates/wjsm-runtime/tests/napi_snapshot_guard.rs`

**Steps**:
- [ ] **定位 capture 的 NativeCallable→SnapshotNativeCallable 转换**（`rg "SnapshotNativeCallable" crates/wjsm-runtime/src/`），确认/加上：遇 `NapiCallback|WjsmLoadNative|ProcessMethod|BufferConstructor|BufferStaticMethod|BufferProtoMethod` → 返回 None 且整体放弃捕获（带 debug 日志），**不得 panic**。
- [ ] **写防御测试**：构造含 `NapiCallback` 的 callable 表 → capture 返回放弃；同时断言 `startup_snapshot_format::abi_hash()` 与 M0 之前的已知值一致（把当前 hash 固化为常量断言，防止本阶段无意变更 ABI）。
- [ ] **收口**：`cargo nextest run --workspace` 全绿 + `cargo build` 零 warning + `cargo run -- run fixtures/napi/hello.js` 手册核验（组装目录）。提交 `feat: 快照防御与 M0 收口（napi M0，#N0）`，issue #N0 关闭说明 + #310 勾选 process/Buffer/__dirname 切片项。

---

# 阶段 M1：NAPI_VERSION 8 全函数面（开新 issue #N1，模式同 T0.0）

## T1.0 重入 executor（spec §4.4）

**Files**: create `runtime_napi/reentry.rs`；test `crates/wjsm-runtime/tests/napi_reentry.rs`

**Why**: `napi_call_function`/`napi_new_instance`/getter-setter/TSFN call_js 全依赖它；M1 最深风险最先落地。

**Impact/Compatibility**: 仅在 napi 桥内使用；epoch yield 自唤醒假设由 fail-fast 计数器守护（spec §10 第 1 行）。

**Verification**: `cargo nextest run -E 'test(napi_reentry)'`（含插件回调 JS 再回插件的两层嵌套 + 长循环触发 epoch yield 用例）。

**Steps**:
- [ ] **实现**：
```rust
/// 嵌套重入 executor：当前线程同步驱动进入 WASM 的 call_async future。
/// Pending 仅预期来自 epoch yield（自唤醒）；连续 Pending 超阈值 → panic 暴露新 Pending 源（fail-fast，spec §10）。
pub(crate) fn block_on_reentrant<F: Future>(mut fut: Pin<&mut F>) -> F::Output {
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    let mut consecutive_pending = 0u64;
    loop {
        match fut.as_mut().poll(&mut cx) {
            Poll::Ready(v) => return v,
            Poll::Pending => {
                consecutive_pending += 1;
                assert!(consecutive_pending < 1_000_000,
                    "napi reentry: unexpected persistent Pending (non-epoch source?)");
                std::hint::spin_loop();
            }
        }
    }
}
/// 从 napi C 栈调用 JS 函数值：TAG_FUNCTION/CLOSURE 走 func_table.get + call_async（模式照
/// runtime_async_fn.rs:344-349），NativeCallable 走现有分派，BOUND 解包。返回 (result, threw)。
pub(crate) fn call_js_value_reentrant(ctx: &NapiCallCtx, func: i64, this: i64, args: &[i64]) -> (i64, bool)
```
- [ ] **写测试（RED→GREEN）**：C 测试插件 `fixtures/napi/addons/reentry/`——导出 `callTwice(fn)`：napi_call_function 调 fn 两次；JS 侧传入含长循环（触发 epoch yield）的函数与再次调用插件的函数（两层嵌套）。断言结果正确、无 panic。
- [ ] **提交** `feat: napi 嵌套重入 executor（napi M1，#N1）`。

## T1.1–T1.8 函数族任务（统一结构）

每族任务结构相同：**(a)** 在 `wjsm-napi-symbols` 的 `for_each_napi_symbol!` 追加本族符号；**(b)** 新建 `runtime_napi/abi_<族>.rs` 实现（`napi_abi_fn!` 前奏 + 现有 runtime 原语，禁旁路）；**(c)** 扩展 C 测试插件目录 `fixtures/napi/addons/<族>/` + `tests/napi_addon.rs` 用例（先 RED）；**(d)** workspace 绿零 warning 后提交 `feat: napi <族>（napi M1，#N1）`。族清单（NAPI_VERSION 8 全集，vendored 头文件为符号权威）：

- **T1.1 值创建/读取补全**（`abi_values.rs`）：napi_create_array, napi_create_array_with_length, napi_create_string_latin1, napi_create_string_utf16, napi_create_symbol, napi_create_bigint_int64, napi_create_bigint_uint64, napi_create_bigint_words, napi_create_date, napi_get_value_string_latin1, napi_get_value_string_utf16, napi_get_value_bigint_int64, napi_get_value_bigint_uint64, napi_get_value_bigint_words, napi_get_date_value, napi_get_array_length, napi_get_prototype。锚点：bigint_words ↔ 现有 BIGINT side table 的肢体表示互转。
- **T1.2 类型判定/转换**（`abi_types.rs`）：napi_is_array, napi_is_arraybuffer, napi_is_typedarray, napi_is_dataview, napi_is_date, napi_is_error, napi_is_promise, napi_instanceof, napi_strict_equals, napi_coerce_to_bool, napi_coerce_to_number, napi_coerce_to_object, napi_coerce_to_string（coerce 走现有 ToPrimitive/ToNumber 助手；instanceof 走现有 instanceof 分派，Proxy/Symbol.hasInstance 语义免费获得）。
- **T1.3 对象属性全集**（`abi_object.rs`）：napi_get_property_names, napi_get_all_property_names, napi_set_property, napi_get_property, napi_has_property, napi_delete_property, napi_has_own_property, napi_set_element, napi_get_element, napi_has_element, napi_delete_element, napi_define_properties, napi_object_freeze, napi_object_seal（key 支持 string/symbol/number；getter/setter 描述符 → `NapiCallback`；属性读写走现有 get/set 分派使 Proxy trap 语义正确——**getter 触发即重入**，依赖 T1.0）。
- **T1.4 ArrayBuffer/TypedArray/DataView/detach**（`abi_buffer_typedarray.rs`）：napi_create_arraybuffer, napi_create_external_arraybuffer, napi_get_arraybuffer_info, napi_create_typedarray, napi_get_typedarray_info, napi_create_dataview, napi_get_dataview_info, napi_detach_arraybuffer, napi_is_detached_arraybuffer + node_api.h 的 napi_create_buffer, napi_create_buffer_copy, napi_create_external_buffer, napi_is_buffer, napi_get_buffer_info。前置：`ArrayBufferEntry` 改为
```rust
pub(crate) enum ArrayBufferBacking { Owned(Vec<u8>), External { ptr: *mut u8, len: usize, finalize: Option<NapiFinalizer> } }
pub(crate) struct ArrayBufferEntry { pub(crate) backing: ArrayBufferBacking }
```
（全仓 `entry.data` 使用点机械迁移为 `backing.as_slice()/as_mut_slice()` 助手；`Send` 安全性：External 指针仅 JS 线程解引用，类型上用 usize 存储。）info 族返回 `as_mut_ptr()`（spec §4.10 指针窗口语义）。
- **T1.5 函数/构造/class/wrap**（`abi_function.rs` + `abi_class_wrap.rs`）：napi_call_function, napi_new_instance, napi_get_new_target, napi_define_class, napi_wrap, napi_unwrap, napi_remove_wrap, napi_type_tag_object, napi_check_object_type_tag, napi_add_finalizer + node_api.h 的 napi_make_callback, napi_async_init, napi_async_destroy, napi_open_callback_scope, napi_close_callback_scope（make_callback = call_function + 微任务 drain 检查点，照现有 host 回调后 drain 模式；async_context 无 async_hooks 语义，保存/透传不额外行为——**这是语义决定非 stub**：wjsm 无 async_hooks（#313 非目标），context 参数按 Node 无 hook 监听时的可观察行为处理）。napi_define_class：构造器 = `NapiCallback` + `napi_wrap` 槽（对象 hidden 属性存 external side handle，照 `HostSideTable` wrapper 绑定模式）；static/instance 属性经 T1.3 define。new_target：构造调用帧记录。
- **T1.6 错误/异常补全**（`abi_error.rs`）：napi_throw_type_error, napi_throw_range_error, napi_create_error, napi_create_type_error, napi_create_range_error, napi_fatal_exception, napi_fatal_error（abort）。error 对象构造走现有 Error 构造器路径（含 stack 属性行为一致）。
- **T1.7 napi_ref/external/instance data**（`abi_ref_external.rs`）：napi_create_reference, napi_delete_reference, napi_reference_ref, napi_reference_unref, napi_get_reference_value, napi_create_external, napi_get_value_external, napi_set_instance_data, napi_get_instance_data。ref 表：`RuntimeState.napi_refs: Mutex<HostSideTable<NapiRefEntry>>`——refcount>0 pin（roots 既有路径），==0 unpin + sweep 后失效通知（接 `weakref_finalization.rs` 的 sweep 回调点，`rg "fn.*sweep" crates/wjsm-runtime/src/runtime_gc/weak_refs*`）。finalizer 统一队列：sweep 后 JS 线程执行（`NapiFinalizer { cb: usize, data: usize, hint: usize, env_id: u32 }`）。
- **T1.8 Promise/RunScript/元数据/cleanup hooks**（`abi_promise_misc.rs` + `abi_lifecycle.rs`）：napi_create_promise, napi_resolve_deferred, napi_reject_deferred（接 `alloc_promise` + `settle_promise`，`runtime_promises.rs:314`）、napi_run_script（接现有 eval 管线 `runtime_eval.rs`）、napi_get_node_version（与 process.versions 同源常量）、napi_adjust_external_memory（接 #337 heap budget 计数）、napi_get_uv_event_loop（返回 napi_generic_failure + extended error msg，spec §4.11）、napi_add_env_cleanup_hook, napi_remove_env_cleanup_hook, napi_add_async_cleanup_hook, napi_remove_async_cleanup_hook（store teardown 前逆序执行——挂 `execute_with_writer_shared_inner` 的 main 完成后清理段）。

## T1.9 napi_async_work（spec §4.6）

**Files**: create `runtime_napi/async_work.rs`；modify `scheduler.rs`
**Steps**:
- [ ] **scheduler 加通用臂**（`scheduler.rs:32` 枚举）：
```rust
/// napi/finalizer 等非 promise 的 JS 线程回调载体。
RunOnOwner { run: Box<dyn FnOnce(&mut Store<RuntimeState>, &WasmEnv) + Send> },
```
`process_one_completion` 加分派臂。
- [ ] **实现** napi_create_async_work/delete/queue/cancel：结构 `{ execute: usize, complete: usize, data: usize, env_id, state: AtomicU8 }`；queue → `AsyncOpGuard` 计数（事件循环存活）+ `tokio::task::spawn_blocking`（execute(env=NULL 语义按 Node：传 env 但禁用 JS 调用——execute 内 TLS 无 ctx，napi_* 自然返回 generic_failure）→ 完成投递 `RunOnOwner`（压 ctx + 开 scope 调 complete(status)）。cancel：CAS state，未开始→complete(napi_cancelled)。
- [ ] **C 插件测试**（`fixtures/napi/addons/asyncwork/`）：异步平方 + cancel 用例；`tests/napi_addon.rs` 断言。提交 `feat: napi_async_work（napi M1，#N1）`。

## T1.10 napi_threadsafe_function（spec §4.5）

**Files**: create `runtime_napi/tsfn.rs`
**Steps**:
- [ ] **实现** 全 8 函数（create/get_context/call/acquire/release/unref/ref + call 的 blocking/nonblocking）：结构 `{ js_func_ref, context, max_queue: Option<usize>, queue: std::sync::mpsc 或 crossbeam 有界/无界, call_js_cb: usize, finalize, thread_count: AtomicUsize, refed: AtomicBool }`；投递路径：TSFN 自持 `host_completion_tx` clone → `RunOnOwner`（内部 drain TSFN 自身队列执行 call_js_cb；js_func 经重入 executor 调用）；blocking 满队列阻塞投递线程；refed=true 持 `AsyncOpGuard`（主循环存活，`run_post_main_scheduler_async` 既有计数语义）；release 归零 → finalize 经 `RunOnOwner` 执行 + 释放 ref。
- [ ] **C 插件测试**（`fixtures/napi/addons/tsfn/`）：起 pthread 投递 N 次 call → JS 收满 N 次回调后 resolve；nonblocking 满队列返回 queue_full 用例。提交 `feat: napi_threadsafe_function（napi M1，#N1）`。

## T1.11 node-addon-api 官方测试套接入

**Files**: create `tools/napi/run_node_addon_api_tests.sh`、`tools/napi/node_addon_api_status.txt`（点亮清单）
**Steps**:
- [ ] 脚本：clone `nodejs/node-addon-api`（固定 tag）到 `/tmp` → `npm ci` + node-gyp 构建其 test addon（用系统 node 构建二进制——产物只依赖 napi 符号，正是"预编译插件"验收形态）→ 逐 `test/*.js` 以 `cargo run --release -- run` 执行 → 与清单比对（在清单=必须绿，不在=记录输出不 fail）。
- [ ] 首轮跑通同步基础目录（objects/functions/strings/error handling），按输出修 bug、逐批点亮 async/tsfn 目录；每批提交 `fix: 点亮 node-addon-api <目录>（napi M1，#N1）`。M1 收口标准：同步 + async work + TSFN 目录全部入清单。

## T1.12 macOS/Windows 符号导出完整化

**Files**: modify `crates/wjsm-cli/build.rs`
**Steps**:
- [ ] Windows：build.rs 用 `wjsm_napi_symbols::symbol_names()`（build-dependency，zero-dep 无负担）生成 `OUT_DIR/wjsm.def` + `cargo:rustc-link-arg-bins=/DEF:<path>`；macOS：默认导出已可见，无参数（注释说明）。`tests/napi_addon.rs` 移除 `cfg(target_os="linux")` 限制（win 的插件编译分支用 `cl /LD` 或跳过本机无编译器时 skip-with-message）。
- [ ] 三平台各跑 `tests/napi_addon.rs`（本地仅当前平台，其余 CI T2.6 验证）。提交 `feat: mac/win napi 符号导出（napi M1，#N1）`。M1 收口：workspace 绿 + T1.11 清单达标，关闭 #N1。

---

# 阶段 M2：生态验收（开 issue #N2）

## T2.1 动态 require 的 .node 子集（spec §7）

**Files**: modify `cjs_transform.rs`、`get_builtin_global_entry.rs`（`__wjsmRequireDynamic` 臂 → `NativeCallable::WjsmRequireDynamic`）、`module_registry.rs`
**Steps**:
- [ ] cjs_transform：非字面量 `require(expr)` 不再编译期报错，降低为 `__wjsmRequireDynamic(expr, __dirname)`（模块目录作解析基准）。
- [ ] 运行时：按 Node 规则解析（复用 `ModuleResolver::resolve_path` 逻辑抽出的纯路径函数）→ `.node` → `load_native_module`；非 `.node` → throw TypeError`("dynamic require of non-native module is not supported yet (see wjsm#312): " + specifier)`（TAG_EXCEPTION 可捕获——node-gyp-build 的 try/catch 探测依赖此行为）。
- [ ] wjsm-module 单测 + fixture（`try { require(name) } catch {}` 探测模式）。提交 `feat: 动态 require .node 子集（napi M2，#N2）`。

## T2.2 fs 只读三函数（#308 切片）

**Files**: create `runtime_napi/node_fs.rs`；modify `resolver.rs`（bare specifier `fs`/`node:fs` → 合成模块 `const m=__wjsmGetBuiltinModule("fs"); export default m; export const existsSync=m.existsSync; export const readdirSync=m.readdirSync; export const statSync=m.statSync;`）、`get_builtin_global_entry.rs`
**Steps**:
- [ ] `NativeCallable::FsMethod { method: u8 }`（0=existsSync,1=readdirSync,2=statSync）：readdirSync 返回字符串数组（现有数组构造助手）；statSync 返回对象（size/mtimeMs 数据属性 + isFile/isDirectory 方法 = `FsStatsMethod` callable）；ENOENT → throw Error（`code` 属性 `"ENOENT"`，可捕获）。
- [ ] fixture：`require('fs').readdirSync` 枚举 tempdir 断言。提交 `feat: fs 只读三函数（napi M2，#N2，#308 切片）`。

## T2.3 node-gyp-build / bindings 链路

**Steps**:
- [ ] `tools/napi/ecosystem/gyp_build_smoke.sh`：tempdir 组装真实 `node_modules/node-gyp-build`（npm pack 固定版本）+ `prebuilds/<platform>-<arch>/node.napi.node`（T0.7 插件重命名放置）+ 入口 `index.js`（`module.exports = require('node-gyp-build')(__dirname)`）→ `cargo run --release -- run` 断言导出可用。RED 驱动修复解析/探测差异（预期触及 `package.json` exports 字段等 #309 面时：仅修 node-gyp-build 实际路径需要的最小解析，越界项记录到 #309 不展开）。
- [ ] 提交 `feat: node-gyp-build 链路冒烟（napi M2，#N2）`。

## T2.4 napi-rs examples + @napi-rs/bcrypt 冒烟

**Steps**:
- [ ] `tools/napi/ecosystem/napi_rs_smoke.sh`：`npm i @napi-rs/bcrypt`（拉平台预编译包）→ 冒烟脚本 hash/verify 往返断言；napi-rs 官方 examples（clone 固定 tag，`napi build` 产 .node）核心用例（class/buffer/async task/tsfn 各一）。
- [ ] 修复暴露差异（预期集中在 class wrap/external/async task），每修一类提交。提交 `feat: napi-rs 生态冒烟（napi M2，#N2）`。

## T2.5 better-sqlite3 冒烟

**Steps**:
- [ ] `tools/napi/ecosystem/better_sqlite3_smoke.sh`：`npm i better-sqlite3`（prebuild 二进制）→ 脚本：open(:memory:) → exec DDL → prepare/run/get/all 断言（含 Buffer BLOB 往返）。
- [ ] 该链路串联 T2.1+T2.2+T2.3+M1 全面（wrap/class/buffer/external），是最终验收；修复后提交 `feat: better-sqlite3 冒烟通过（napi M2，#N2）`。

## T2.6 CI 三平台矩阵

**Files**: modify `.github/workflows/`（现有 CI 文件旁加 `napi.yml`）
**Steps**:
- [ ] matrix `os: [ubuntu-latest, macos-latest, windows-latest]`：build → `cargo nextest run -E 'test(napi)'` → `tools/napi/run_node_addon_api_tests.sh` → `tools/napi/ecosystem/*.sh`（win 首版允许 ecosystem 子集 + 显式清单，缺项列 issue 不静默）。提交 `feat: napi CI 三平台矩阵（napi M2，#N2）`。

## T2.7 ADR 0005 + 基线回写 + 收口

**Steps**:
- [ ] 写 `docs/adr/0005-napi-boundary.md`（spec §11 五要素：符号导出面、env=store 生命周期、沙箱让渡、uv loop 拒绝、WASM ABI 前端预留不变量）+ INDEX Baselines 行。
- [ ] CLAUDE.md 增 N-API 段（加载管道、快照不变量、测试入口）；#313 回帖修正 Tier S 评估（引 spec §1 论证 + 实测结果）；#310/#308 勾选切片项；关闭 #N2 并附三平台验收证据。
- [ ] 提交 `docs: ADR 0005 与 napi 基线回写（napi M2，#N2）`。

---

## Risks（执行期对照 spec §10）

| 风险 | 触发信号 | 既定动作 |
|---|---|---|
| 重入 executor 新 Pending 源 | T1.0 fail-fast panic | 定位源（fuel/其他 yield）→ 在 reentry.rs 加该源的驱动分支，spec §4.4 论证补充 |
| ArrayBufferEntry 迁移波及面大 | T1.4 编译错误数 | 机械迁移（`backing.as_slice()`），一次提交内完成，禁半迁移 |
| node-addon-api 套暴露语义偏差批量 | T1.11 首轮失败率 | 按目录分批，每批 RED→修→GREEN→提交；清单文件记录暂缓项与原因（不静默跳过） |
| Windows delay-load 差异 | T2.6 win job | 用真实 node-gyp 产物调试；必要时补 hook 兼容说明入 ADR 0005 |

## Retirement

无退役项（纯新增面）。唯一"旧真相"清理：#313 的 "Tier S 基本无解" 文本由 T2.7 回写修正（不删 issue，追加修正评论）。
