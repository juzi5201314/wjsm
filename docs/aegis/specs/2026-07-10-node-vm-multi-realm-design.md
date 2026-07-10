# node:vm 多 Realm 沙箱设计

- 日期：2026-07-10
- 状态：已按用户本轮“无需审批 spec、直接进入 writing-plans”授权进入实现
- 关联 issue：#313（`vm.runInNewContext` 原为非目标，本轮用户明确要求完整落地，取代该非目标条目）
- 输入：issue #313 正文、`AGENTS.md` 运行时/snapshot/handle/GC 约束、`docs/adr/0003-startup-snapshot-boundary.md`、`docs/adr/0004-build-time-embedded-runtime.md`、`docs/adr/0002-runtimestate-stays-flat.md`、`crates/wjsm-runtime/src/runtime_eval.rs`、`crates/wjsm-runtime/src/startup_snapshot.rs` / `startup_snapshot_remap.rs`、`crates/wjsm-runtime/src/runtime_gc/roots.rs`、`crates/wjsm-runtime/src/host_imports/collections_buffers.rs`、`crates/wjsm-snapshot-format/src/lib.rs`、`crates/wjsm-module/src/builtin_modules.rs`、`crates/wjsm-module/src/resolver.rs`、`crates/wjsm-runtime/src/scheduler.rs` 以及三份子系统探底报告

## 1. 背景与当前事实

Node.js `node:vm` 提供在**独立全局上下文（realm）**中编译并执行 JS 代码的能力：脚本里的未声明标识符落到 contextify 后的 sandbox 对象上，且每个 context 拥有**独立的 intrinsics**（`Object`/`Array`/`Function.prototype` 等），因此有 `vm.runInNewContext('[]') instanceof Array === false` 这一标志性语义。关键约束是：**对象跨 context 按引用自由流动**（`vm.runInNewContext('({a:1})')` 返回的是活对象引用，sandbox 上的属性写入立即对宿主可见），而 intrinsics 相互独立。

当前事实：

| 领域 | 当前实现 | 缺口 |
|---|---|---|
| `vm` 模块 | 完全不存在 | `builtin_modules.rs` 无 `vm` 条目，`builtin_js/` 无 `node_vm.js`，`require('node:vm')` 返回 `UnknownNodeBuiltin("vm")` |
| Realm | **单例**：`create_global_object` 全局单例（`collections_buffers.rs`），一次 `__wjsm_bootstrap_once` 引导，snapshot restore 恢复同一套 primordial | 无第二套 intrinsics，无 per-realm global 概念 |
| Eval 执行 | 双路径已完整：`try_compiled_eval_from_caller_async`（编译成 WASM）+ AST 解释器（`eval_stmt`/`eval_expr`/…），均以 `scope_env: Option<i64>` 贯穿绑定查找 | 两条路径的 intrinsic 解析都硬绑主 realm；`eval_cache` 按代码字符串键，realm 无关 |
| Snapshot | embedded startup snapshot = 一份引导完的 primordial 对象图；`handle_rel_offsets` 捕获/恢复 + `remap_array_proto_function_indices` 全堆遍历重写 | restore 是 1:1 原位恢复，不分配新 handle 区，不做全量指针重映射 |
| Handle 分配 | 单例 bump 分配器（`obj_table_count`）+ free-list 复用（`gc_take_freed_handle`） | — |
| GC roots | `RuntimeRoots` 枚举 shadow stack + immortal region + host table（primordial 原型 + `js_global_object` 单例） | 无 per-realm root 集合 |
| Scheduler / microtask | 单 Store 单 scheduler，`enqueue_async_result` 走 completion channel | realm 无关，可直接共享 |

**Baseline Role Alignment：aligned（scope: both）。**
- 需求层（Product/Requirement Baseline）：提供忠实的 `node:vm` 核心稳定全集，含 realm 隔离与 timeout。
- 架构层（Architecture/Runtime Boundary Baseline）：realm 必须活在**同一 Store / 同一线性内存 / 同一 obj_table / 同一 GC**中（否则对象无法跨 context 按引用流动），只是堆里同时存在多套 primordial + 多个 global。这与 V8 的 context 模型一致，也是 wjsm snapshot 架构能优雅承接的形态。

## 2. 决定性架构判断

### 2.1 忠实 node:vm 是「单堆多 realm」，不是「多实例隔离」

`vm` 契约要求对象跨 context 按引用流动（返回活对象、sandbox 属性双向可见）。独立 wasmtime Store = 独立线性内存 = 对象只能序列化传递，**直接违反契约**。因此 realm 必须共享单 Store / 单 `obj_table` / 单 GC；一个 realm 仅是「一组带标签的 intrinsic handle + 一个 global 对象」，它运行时 `new` 出来的对象照旧走共享 bump 分配器。

**推论（化解探底报告的悲观假设）**：realm **不需要**独立 handle 空间，bump 分配器 / scheduler / microtask 队列 / handle free-list **全部零改动共享**。探底报告最初担心的「改 bump 分配器 / per-realm obj_table」是基于错误的隔离模型，不成立。

### 2.2 Realm 诞生 = 克隆 pristine snapshot 对象图（用户选定：Snapshot 克隆）

embedded startup snapshot 的内容正是「一套引导完的 primordial 对象图」——它就是**一个 realm 的出厂模板**。`vm.createContext` 的实现：把这份 pristine 对象图克隆到一批**新分配的 handle 槽位**（走现有 bump 分配器），并对克隆出的每个对象做**全量内部 handle 重映射**（旧 handle → 新分配 handle）。这是把现有 `remap_array_proto_function_indices` 从「只重映射 Array.prototype 方法表函数索引」**泛化**为「全量对象图 handle 重映射」。

优势：确定性、快（复用 snapshot 已验证的字节级重定位机制）、无需重跑 `__wjsm_bootstrap_once`，与 V8 context snapshot 造 realm 同源，且 wjsm 的 AOT+snapshot 架构使其比 V8 更自然。

### 2.3 双路径执行引擎都必须 realm 感知（用户选定：编译 eval 为主 + 解释器兜底）

context 内代码在做字面量（`[]`、`{}`、`/re/`）、`new`、以及查 `Object`/`Array` 等全局 intrinsic 时，必须解析到**目标 realm 的原型/构造器**，而非主 realm。

- **编译 eval 路径**：`compile_eval` 生成的 wasm 通过 parent import 拿 intrinsic/global。改为注入「当前 realm 的 intrinsic handle 表 + global」而非硬编码主 realm。`eval_cache` 键从 `code` 改为 `(code, realm-agnostic-shape)`——代码字节码本身 realm 无关，intrinsic 通过运行时参数注入，故**缓存仍可跨 realm 共享**（关键：不引入 per-realm 编译缓存膨胀）。
- **解释器兜底路径**：`eval_stmt`/`eval_expr` 已贯穿 `scope_env`，再补一个 `realm` 上下文，令字面量构造与全局查找解析到 realm 的 intrinsic。

## 3. 目标与非目标

### 目标（核心稳定全集 + timeout）

1. `node:vm` / `require('vm')` 可加载（ESM namespace + CJS default）。
2. `vm.createContext([contextObject][, options])`：contextify 一个对象为 realm 的全局，返回该对象；`vm.isContext(obj)`。
3. `vm.Script` 类：`new vm.Script(code[, options])`（含 `filename`/`lineOffset`/`columnOffset`），实例方法 `runInThisContext([options])`、`runInContext(contextifiedObject[, options])`、`runInNewContext([contextObject][, options])`。
4. `vm.runInThisContext(code[, options])`、`vm.runInContext(code, contextifiedObject[, options])`、`vm.runInNewContext(code[, contextObject][, options])`。
5. `vm.compileFunction(code[, params][, options])`：返回绑定到指定 context 的函数。
6. **Realm 隔离语义**：每个 context 拥有独立 intrinsics（`instanceof` 跨 realm 为 false，`constructor` 指向各自 realm，独立 `Object.prototype` 等）。
7. **对象跨 realm 按引用流动**：返回值、sandbox 属性双向可见，无序列化。
8. **timeout 选项**（用户选定纳入）：`runIn*` / `Script.run*` 的 `timeout`（毫秒）到期抛出，实现见 §5.5。
9. `vm.constants`（`vm.constants.DONT_CONTEXTIFY` 等核心项）与 `contextName`/`microtaskMode: 'afterEvaluate'` 的忠实语义（见 §4 次级决策）。
10. fixtures 覆盖：realm 隔离、跨 realm 对象引用、sandbox 双向可见、timeout、`vm.Script` 复用、错误跨 realm 传播。

### 非目标

- `vm.SourceTextModule` / `vm.SyntheticModule`（ESM `--experimental-vm-modules` 实验 API）：Node 自身标注 experimental，本轮不做，`require('vm').SourceTextModule` 访问时抛明确错误，不留静默 no-op。
- `vm.measureMemory()`：依赖 V8 堆度量，wjsm GC 模型不同，明确抛错。
- `importModuleDynamically` 回调完整语义：`vm.Script`/`compileFunction` 的动态 import 钩子暂抛「未实现」明确错误（不伪造）。
- 把 `vm` 当作**安全沙箱**：与 Node/V8 一致，`vm` 不是不可信代码的安全边界；文档需显式声明。真隔离由 `worker_threads`（#313 独立 Store）承接。
- 跨线程 realm：realm 绑定创建它的 Store/线程，不跨 Worker 共享。

## 4. 次级决策（按 Node 语义与仓库约定拍板，不再打扰用户）

| 决策点 | 取值 | 依据 |
|---|---|---|
| `microtaskMode: 'afterEvaluate'` | 忠实实现：该 context 的 microtask 在脚本求值后同步排空 | Node 语义；复用现有单 scheduler 的 drain 能力 |
| `contextCodeGeneration.{strings,wasm}` | 支持 `strings:false` → 该 realm 内 `eval`/`Function` 抛 `EvalError` | Node 语义，realm 级开关，落到 realm 上下文标志 |
| Realm 数量上限 | 软上限（默认 1024），可经 `WJSM_VM_MAX_REALMS` 调整，超限抛错 | 防句柄/内存失控；与 net/worker 的资源上限风格一致 |
| GC per-realm roots | `RuntimeState` 增 `active_realms: Vec<RealmRoots>`，`for_each_host_table_root` 遍历每个活跃 realm 的 intrinsic 原型 + global | 探底报告 §2.4 指出的唯一 GC 改动点 |
| Realm 回收 | contextified sandbox 不可达时，其 realm intrinsic 随之成为 GC 垃圾（弱持有）；`active_realms` 存弱引用语义句柄，GC 后清理死 realm | 避免 realm 泄漏；sandbox 是 realm 生命周期锚点 |
| `filename`/`lineOffset`/`columnOffset` | 注入现有 `wjsm_sourcemap` backtrace 机制 | 复用现有源映射，错误栈忠实 |
| `displayErrors` | 默认 true，编译错误带源码帧 | Node 默认 |
| `breakOnSigint` | 非目标降级：无 SIGINT 集成时忽略该选项（不抛错，Node 也允许缺省） | 无对应 substrate；标注 |

## 5. 架构设计

### 5.1 Owner 分层（沿用 JS builtin + Rust host bridge 模式）

```
crates/wjsm-module/src/builtin_modules.rs        # 注册 canonical "vm" builtin
crates/wjsm-module/builtin_js/node_vm.js         # vm API 外形：Script / runIn* / compileFunction / constants
crates/wjsm-runtime/src/runtime_node_vm.rs       # __wjsm_node_vm host bridge：createContext / runInRealm / isContext
crates/wjsm-runtime/src/realm.rs                 # NEW: Realm 结构、克隆、intrinsic 表、realm 上下文
crates/wjsm-runtime/src/realm_clone.rs           # NEW: pristine 对象图克隆 + 全量 handle 重映射（泛化自 startup_snapshot_remap）
crates/wjsm-runtime/src/runtime_eval.rs          # 改造：eval 双路径注入 realm 上下文
crates/wjsm-runtime/src/runtime_gc/roots.rs      # 改造：per-realm root 枚举
crates/wjsm-runtime/src/lib.rs                   # RuntimeState 增 active_realms 字段（保持扁平，遵 ADR 0002）
```

- **Node API 外形**由 `node_vm.js` 拥有；Rust runtime 只暴露 realm 创建/执行/身份判定 host method。
- `NativeCallable` 新增 `VmMethod` 无状态分派项，同步 `SnapshotNativeCallable` discriminant 与 `abi_hash()`。

### 5.2 Realm 数据结构（RuntimeState 扁平字段）

```rust
// realm.rs
pub(crate) struct Realm {
    pub id: RealmId,
    pub global_object: i64,          // contextify 后的 global（= 用户 sandbox 或 DONT_CONTEXTIFY 对象）
    pub intrinsics: RealmIntrinsics, // 该 realm 的原型/构造器 handle 集合
    pub code_generation: CodeGenFlags, // strings/wasm 开关
    pub scope_records: ...,          // realm 局部 scope（复用现有 scope_record 机制，键空间隔离）
}

pub(crate) struct RealmIntrinsics {
    // 与 snapshot header 中 primordial 字段一一对应的 per-realm 版本：
    object_proto, array_proto, function_proto, error_prototypes,
    iterator_prototype, generator_prototype, async_iterator_prototype,
    async_gen_prototype, symbol_prototype, promise_prototype,
    regexp_prototype, date_prototype, /* … 与 RuntimeState primordial 字段对齐 */
}

// lib.rs RuntimeState 新增（扁平）：
active_realms: Mutex<Vec<Realm>>,   // realm 0 = 主 realm（现有单例，惰性登记）
```

主 realm（realm 0）即现有全局单例，登记进 `active_realms[0]`，使 GC root 枚举与 realm 执行路径统一，无特例分支。

### 5.3 createContext = 克隆 pristine realm（§2.2 展开）

复用 embedded snapshot 的对象图作为 pristine 模板，克隆步骤：

1. 取 pristine primordial 对象图字节范围（来自 embedded snapshot 或主 realm 引导后快照）。
2. 为每个 pristine 对象经 `obj_new`（共享 bump 分配器）分配**新 handle**，建立 `old_handle → new_handle` 映射表。
3. 复制每个对象的堆字节到新地址，写 `obj_table[new_handle] = new_ptr`。
4. **全量内部 handle 重映射**：遍历克隆对象的每个属性槽、proto header、函数索引，按映射表把旧 handle 改写为新 handle（`realm_clone.rs`，泛化自 `remap_array_proto_function_indices` 的遍历+条件重写模式）。
5. 装配 `RealmIntrinsics`：从映射表取出各原型的新 handle。
6. contextify：把用户 `sandbox` 对象设为该 realm 的 `global_object`；未声明标识符读写落到它上（复用 eval `scope_env` 回退，回退目标从主 global 改为 realm global）。
7. 安装 per-realm 内建全局（`Object`/`Array`/`console` 等）到 realm global，指向该 realm intrinsic；host bridge（`__wjsm_node_*`）按 Node 语义**默认不注入** vm context（vm context 是纯 JS 环境，除非用户 sandbox 显式提供）。

### 5.4 执行路径 realm 感知（§2.3 展开）

- `vm.runInContext(code, ctx)` → host `runInRealm(realm_id, code, options)`：
  - 优先编译 eval：`compile_eval` 产物 + 注入 realm intrinsic import + realm global；`eval_cache` 键保持 `code`（intrinsic 运行时注入，缓存跨 realm 共享）。
  - 失败兜底 AST 解释器：`eval_stmt` 携带 realm 上下文，字面量/全局查找解析到 realm intrinsic。
- `runInThisContext`：realm = 主 realm，共享全局 intrinsic，但**不 contextify**（无 sandbox 全局回退，标识符按普通 global 语义）。
- `runInNewContext`：createContext + runInContext 的组合。
- `compileFunction`：把 code 包成函数体，绑定目标 realm，返回可跨 realm 调用的函数值（函数捕获其定义 realm 的 intrinsic）。

### 5.5 timeout 实现（用户选定纳入）

- 复用 GC safepoint 插桩点：编译 eval 路径在 safepoint poll 处检查 realm 执行 deadline（`Instant`），超时抛 `vm` timeout 错误并中止 wasm 执行（经 wasmtime `Store` epoch/fuel 或既有 safepoint host 回调触发 trap）。
- 解释器兜底路径在 `eval_stmt` 循环检查 deadline。
- 优先方案：wasmtime **epoch interruption**（`Engine::set_epoch_deadline` + 后台 timer 线程 bump epoch），比 fuel 计数更低开销且不改 codegen；在 `--inspect` guest_debug 已启用 epoch/safepoint 的前提下天然兼容。计划阶段验证 epoch 与现有 scheduler 协作无冲突。

### 5.6 GC roots（§4 展开）

`RuntimeRoots::for_each_host_table_root` 在扫描完主 realm primordial 后，遍历 `active_realms`：每个 realm 贡献 `global_object` + `RealmIntrinsics` 全部原型 handle 作为 root。realm 内用户对象经共享 obj_table 由 shadow stack / 常规 root 覆盖，无需额外枚举。死 realm（global 不可达）在 GC 后从 `active_realms` 清理。

## 6. Architecture Integrity Lens

- **Invariant**：realm 共享单 Store/单 obj_table/单 GC；Node API 外形归 `node_vm.js`；realm 克隆/执行/root 归 runtime。
- **Canonical owner / contract**：`realm.rs`（Realm 结构）、`realm_clone.rs`（克隆+重映射）、`runtime_node_vm.rs`（host bridge）、`node_vm.js`（API 外形）各单一 owner。
- **Responsibility overlap**：`realm_clone.rs` 与 `startup_snapshot_remap.rs` 共享「全堆遍历+handle 重写」内核——把重写核心抽成共享 helper（`handle_remap` 模块），snapshot restore 与 realm clone 都调用，避免两份重写逻辑漂移。
- **Higher-level simplification**：主 realm 登记为 `active_realms[0]`，消除「单例 global vs realm global」的双路径特例。
- **Dependency direction**：`node_vm.js` → host bridge → realm → 共享 handle/GC 基础设施，依赖向稳定层收敛。
- **Retirement / falsifier**：`vm.runInNewContext('[]') instanceof Array === false`、跨 realm 对象引用可见、sandbox 双向可见、timeout 触发、`vm.Script` 复用——均由 fixtures 证明；experimental/measureMemory 明确抛错，不留 no-op。
- **Verdict**：proceed。realm 模型与 snapshot 架构同源，无架构冲突。

## 7. ADR 信号

本设计触及多项 load-bearing 约定，实现完成后需回填 ADR（暂定 ADR 0008: node:vm multi-realm）：
- **新增运行时能力**：单堆多 realm，`RuntimeState.active_realms`（ADR 0002 扁平约束仍守，仅加字段）。
- **snapshot 机制复用/泛化**：pristine 对象图作为 realm 模板；`handle_remap` 共享内核（ADR 0003 重定位规则的直接延伸）。
- **ABI hash 输入**：新增 `NativeCallable::VmMethod` discriminant → 更新 `abi_hash()`（ADR 0003/0004 硬约束）。
- **GC roots 契约**：per-realm root 枚举（ADR 0005 pluggable GC 的 root provider 扩展）。
- 真备选方案：多 Store 隔离（拒绝，违反对象跨 realm 引用契约）、重跑 bootstrap 造 realm（拒绝，慢且不确定，snapshot 克隆更优）。

## 8. 兼容边界

- `node:vm` 与裸 `vm` specifier 解析到同一 canonical builtin。
- CJS `require('vm')` 返回 default export，ESM `import ... from 'node:vm'` 返回 namespace。
- 现有主 realm 行为**完全不变**：单 realm 程序不进入 realm 克隆路径（主 realm 即 `active_realms[0]`，惰性登记，零额外开销）。
- 现有所有 fixture `.expected` 输出不变；snapshot ABI 因新增 NativeCallable 而 rebake（构建期自动，`abi_hash` 同步）。
- `WJSM_STARTUP_SNAPSHOT=0` 关闭 embedded snapshot 时，realm 模板改用主 realm 引导后的运行时快照，功能不降级。
- realm 绑定创建它的 Store/线程；不跨 Worker。

## 9. 验证策略

- `cargo nextest run -E 'test(modules__node_builtin_vm)'`：ESM/CJS 加载。
- `cargo nextest run -E 'test(happy__vm_realm_isolation) | test(happy__vm_cross_realm_ref) | test(happy__vm_sandbox_visible) | test(happy__vm_timeout) | test(happy__vm_script_reuse)'`：核心语义 fixtures。
- `cargo nextest run -p wjsm-runtime -E 'test(realm) | test(vm) | test(snapshot)'`：realm 克隆、handle 重映射、snapshot ABI 单测。
- `cargo nextest run -p wjsm-runtime -E 'test(gc)'`：per-realm GC root、死 realm 回收。
- CLI smoke：
  - `cargo run -- run -e "const vm=require('vm'); console.log(vm.runInNewContext('[]') instanceof Array);"` → `false`
  - `cargo run -- run -e "const vm=require('vm'); const s={}; vm.runInNewContext('x=1',s); console.log(s.x);"` → `1`
  - `cargo run -- run -e "const vm=require('vm'); const o=vm.runInNewContext('({a:2})'); console.log(o.a);"` → `2`
  - `cargo run -- run -e "const vm=require('vm'); try{vm.runInNewContext('while(1){}',{},{timeout:50})}catch(e){console.log('timeout')}"` → `timeout`
- 全工作区回归：`cargo nextest run --workspace` 全绿，零编译警告。

## 10. Working artifacts

**TaskIntentDraft**
- Outcome：`node:vm` 核心稳定全集 + timeout 完整可用，realm 隔离忠实且对象跨 realm 按引用流动。
- Goal：把 wjsm 的 snapshot/handle/eval 双路径优势用到极致，以「单堆多 realm + pristine 克隆」实现忠实 vm。
- Success evidence：§9 全部 fixtures 与单测通过，标志性语义（`[] instanceof Array === false`、跨 realm 引用、sandbox 可见、timeout）成立，全工作区回归零警告。
- Stop condition：核心稳定全集 API 可 import/require 且语义忠实；experimental/measureMemory/importModuleDynamically 明确抛错。
- Non-goals：SourceTextModule/SyntheticModule、measureMemory、安全沙箱、跨线程 realm。
- Scope：`wjsm-module`（builtin 注册 + node_vm.js）、`wjsm-runtime`（realm/clone/host bridge/eval 改造/GC roots/snapshot ABI）、fixtures。
- Risks：全量 handle 重映射正确性、eval 双路径 realm 注入、timeout 与 scheduler/epoch 协作、GC per-realm root 完整性、snapshot ABI 同步。

**BaselineReadSetHint**
- Required：issue #313、AGENTS、ADR 0002/0003/0004/0005、`runtime_eval.rs`、`startup_snapshot*.rs`、`runtime_gc/roots.rs`、`collections_buffers.rs`、`snapshot-format`、`builtin_modules.rs`、`scheduler.rs`。
- Authority gaps：无阻塞；ADR 0008 待实现后回填。

**BaselineUsageDraft**
- Required baseline refs：上列 ADR 与 runtime/eval/snapshot/gc 源码。
- Delivered context refs：AGENTS 注入、issue #313 正文、三份探底报告。
- Acknowledged before plan refs：已读 ADR 0003/0004 全文、eval 解释器结构、handle/GC/snapshot 探底、host builtin 注册探底。
- Cited in design refs：本设计 §1–§9。
- Missing refs：无阻塞。
- Decision：continue。

**ImpactStatementDraft**
- Affected layers：`wjsm-module` builtin registry + builtin_js；`wjsm-runtime` realm/clone/host bridge/eval/gc roots/snapshot ABI；`wjsm-snapshot-format` abi_hash 输入；fixtures。
- Owners：API 外形 = `node_vm.js`；realm 机制 = `realm.rs`/`realm_clone.rs`；host 桥 = `runtime_node_vm.rs`；重写内核 = 共享 `handle_remap`。
- Invariants：单 Store/单 obj_table/单 GC；主 realm = active_realms[0]；RuntimeState 扁平；snapshot ABI 同步。
- Compatibility：单 realm 程序行为与开销不变；现有 fixtures 不变。
- Non-goals：见 §3。

**Product Risk Lens**
- Value：忠实 `node:vm` 打开配置求值、模板、插件脚本、DSL、测试框架 context 等长尾生态。
- Non-goals：不冒充安全沙箱；不做 experimental module API。
- Trade-offs：全量 handle 重映射与 eval 双路径改造是主要工程重量，换取 V8 级忠实度与对象引用语义。
- Decision needed：用户已选定四大分叉并免审批，直接进入 writing-plans。
