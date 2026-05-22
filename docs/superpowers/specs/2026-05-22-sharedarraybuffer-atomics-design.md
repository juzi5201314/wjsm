# SharedArrayBuffer + Atomics 设计文档

日期: 2026-05-22
状态: 待实现

## 1. 概述

实现 ECMAScript `SharedArrayBuffer` 和 `Atomics`，包含 test262 agent harness（`$262.agent`）。

核心决策：
- SAB 数据存在 **Rust 侧** `Arc<RwLock<Vec<u8>>>`，不经 WASM 线性内存
- Atomics 操作通过 **Rust host functions** 实现，不依赖 WASM atomic 指令
- 每个 agent = **独立 OS 线程** + 独立 wasmtime Engine/Store/Instance
- 共享状态（SAB 表、agent 通道）通过 `Arc` 跨线程传递

## 2. 架构

### 2.1 整体数据流

```
┌─ 主线程 ─────────────────────┐
│  wasmtime Engine              │
│    Store → Instance           │
│    RuntimeState               │
│      shared_state ────────────┼──────┐
└──────────────────────────────┘      │
                                      │
┌─ Agent 线程 0 ──────────────┐      │
│  wasmtime Engine            │      │
│    Store → Instance         │      │
│    RuntimeState             │      │
│      shared_state ──────────┼──────┤
└─────────────────────────────┘      │
                                     │
         SharedRuntimeState ◄────────┘
         ├── sab_table: Arc<Mutex<Vec<SABEntry>>>
         └── agent_state: Arc<AgentState>
```

### 2.2 关键类型关系

```
SharedArrayBuffer (TAG_OBJECT, handle → sab_table)
  └── sab_table[handle].data: Arc<RwLock<Vec<u8>>>

TypedArray (TAG_OBJECT, handle → typedarray_table)
  └── typedarray_table[handle].buffer_handle
       ├── arraybuffer_table[buffer_handle]  → ArrayBuffer (Vec<u8>)
       └── sab_table[buffer_handle]          → SharedArrayBuffer (Arc<RwLock<Vec<u8>>>)
```

TA 构造时检查 buffer handle 在哪个表中，标记 TypedArrayEntry 是否指向 SAB。所有 TA 访问方法（`set`, `slice`, `at` 等）根据此标记选择 `Vec<u8>` 还是 `Arc<RwLock<Vec<u8>>>`。

## 3. IR 层 (`wjsm-ir`)

### 3.1 Builtin 枚举新增

```rust
// SharedArrayBuffer（4 个）
SharedArrayBufferConstructor,
SharedArrayBufferProtoByteLength,
SharedArrayBufferProtoSlice,
SharedArrayBufferProtoSpecies,

// Atomics 静态方法（13 个）
AtomicsLoad,
AtomicsStore,
AtomicsAdd,
AtomicsSub,
AtomicsAnd,
AtomicsOr,
AtomicsXor,
AtomicsExchange,
AtomicsCompareExchange,
AtomicsIsLockFree,
AtomicsWait,
AtomicsNotify,
AtomicsWaitAsync,
```

### 3.2 不新增

- IR Instruction variants
- HEAP_TYPE tags
- Value tag bits（SAB 复用 `TAG_OBJECT`）

## 4. 语义层 (`wjsm-semantic`)

### 4.1 builtins.rs

`builtin_from_global_ident` 新增映射：

```rust
"SharedArrayBuffer" => Builtin::SharedArrayBufferConstructor
```

`Atomics` 通过成员表达式访问（`Atomics.load(...)`），不在 `builtin_from_global_ident` 中映射。改为在 runtime 创建 `Atomics` 全局对象，其 13 个方法通过属性查找分发。

## 5. 后端 (`wjsm-backend-wasm`)

零变更。所有新 Builtin 走已有 `call` 指令路径。

## 6. Runtime (`wjsm-runtime`)

### 6.1 数据结构

```rust
#[derive(Clone, Debug)]
struct SharedArrayBufferEntry {
    data: Arc<RwLock<Vec<u8>>>,
    byte_length: u64,
}

struct SharedRuntimeState {
    sab_table: Arc<Mutex<Vec<SharedArrayBufferEntry>>>,
    agent_state: Arc<AgentState>,
}

struct AgentState {
    reports: Arc<Mutex<Vec<String>>>,
    waiters: Arc<Mutex<HashMap<(u32, u32), Vec<Waiter>>>>,
}

struct Waiter {
    condvar: Arc<Condvar>,
    notified: Arc<AtomicBool>,
    thread: Thread,    // 用于 park/unpark 作为回退
}
```

### 6.2 RuntimeState 变更

新增字段：
```rust
shared_state: Option<Arc<SharedRuntimeState>>,
```

`sab_table` 不从 RuntimeState 直接访问。host function 通过 `shared_state.as_ref().unwrap().sab_table` 获取 SAB 表引用（若 shared_state 为 None，说明不涉及 SAB/Atomics 操作，调用即 bug）。

`scope_records: HashMap<u32, ScopeRecord>` 改为 `HashMap<u32, ScopeRecord>`（不变；agent 有自己独立的 RuntimeState，不共享 scope_records）。

`scope_record_next_handle: u32` 不变。

### 6.3 execute_with_writer 签名变更

```rust
pub fn execute_with_writer_shared<W: Write>(
    wasm_bytes: &[u8],
    writer: W,
    shared_state: Option<Arc<SharedRuntimeState>>,
) -> Result<W>
```

原 `execute_with_writer` 内部调用 `execute_with_writer_shared` 传 `None`。

### 6.4 NativeCallable 新增

```rust
enum NativeCallable {
    // ... existing variants ...
    SharedArrayBufferConstructor,
    AtomicsLoad,
    AtomicsStore,
    AtomicsAdd,
    AtomicsSub,
    AtomicsAnd,
    AtomicsOr,
    AtomicsXor,
    AtomicsExchange,
    AtomicsCompareExchange,
    AtomicsIsLockFree,
    AtomicsWait,
    AtomicsNotify,
    AtomicsWaitAsync,
    AtomicsGlobal,
    // Agent harness
    AgentStart,
    AgentBroadcast,
    AgentReceiveBroadcast,
    AgentGetReport,
    AgentSleep,
    AgentMonotonicNow,
}
```

### 6.5 Atomics 操作实现

**对齐访问**（offset % element_size == 0）：
- unsafe 将 `&[u8]` 的切片指针转换为 `&AtomicI32`/`&AtomicI64`/`&AtomicU32`/`&AtomicU64`
- 调用 `.load(Ordering::SeqCst)` / `.store(val, Ordering::SeqCst)` / `.fetch_add(val, Ordering::SeqCst)` 等

**非对齐访问**：
- 获取一个全局 `Mutex<()>` 保护（粒度：每个 SAB handle 一个 Mutex）
- 加锁 → 读写 → 解锁
- 非对齐原子操作合法但性能低；实现保证正确性即可

**Atomics.wait(ta, index, value, timeout)**：
1. 验证 ta 是 Int32Array 或 BigInt64Array（规范限制）
2. 检查不能是从 SAB detached 的 TA（抛 TypeError）
3. 原子读取 `ta[index]`，比较 `value`
4. 不等 → 返回 `"not-equal"`
5. 等 → 注册 `Waiter { condvar, notified, thread }` 到 waiters map
6. `condvar.wait_timeout(timeout)` 或 `thread::park_timeout(timeout)`
7. 超时 → 从 waiters map 移除，返回 `"timed-out"`
8. 被 notify → 从 waiters map 移除，返回 `"ok"`

**Atomics.notify(ta, index, count)**：
1. 验证同 wait
2. 查找 waiters map 中对应 (sab_handle, byte_offset) 的等待者列表
3. 取前 `count` 个（FIFO），设置 `notified = true`，`condvar.notify_one()` / `thread.unpark()`
4. 返回唤醒数量

**Atomics.waitAsync(ta, index, value)**：
1. 验证同 wait
2. 检查 `value` 是否等于 `ta[index]`，不等 → 返回 `{ async: false, value: "not-equal" }`
3. 等 → (简化实现) 返回 `{ async: false, value: "timed-out" }`（当前不支持真 async wait）

### 6.6 SAB 构造器

```rust
fn alloc_shared_arraybuffer(byte_length: u64) -> u32 {
    let entry = SharedArrayBufferEntry {
        data: Arc::new(RwLock::new(vec![0u8; byte_length as usize])),
        byte_length,
    };
    sab_table.lock().unwrap().push(entry);
    (sab_table.lock().unwrap().len() - 1) as u32
}
```

属性和方法：
- `sab.byteLength` → `Builtin::SharedArrayBufferProtoByteLength`
- `sab.slice(begin, end)` → `Builtin::SharedArrayBufferProtoSlice`（返回新 SAB，共享同一数据？规范规定 slice 返回新的 SharedArrayBuffer 其数据为原数据的副本——实现 copy）
- `SharedArrayBuffer[Symbol.species]` → `Builtin::SharedArrayBufferProtoSpecies`

### 6.7 TypedArray 集成

`TypedArrayEntry` 新增字段：
```rust
struct TypedArrayEntry {
    buffer_handle: u32,
    byte_offset: u32,
    length: u32,
    element_size: u8,
    element_kind: u8,
    is_shared: bool,  // 新增
}
```

所有 TypedArray 访问方法（`ta_read`, `ta_write`, `set`, `slice`, `at`, 迭代器等）：
- 检查 `entry.is_shared`
- 若 `false` → 走现有 `arraybuffer_table` 路径
- 若 `true` → 走 `sab_table` 路径（通过 `data.read().unwrap()` 或 `data.write().unwrap()` 访问）
  - 非 Atomics 方法不需要原子性，RwLock 的普通 read/write 足够

TA 构造器 (`%TypedArray%(...)`) 检测参数类型：
- 若传入 SAB 对象 → 设置 `is_shared = true`，`buffer_handle` 指向 `sab_table`

### 6.8 globalThis 挂载

在 `create_global_object` 中：
- 创建 `SharedArrayBuffer` 构造器对象
- 创建 `Atomics` 全局对象（非函数，含 load/store/add/.../wait/notify/waitAsync 属性）
- 挂载到全局

Agent harness 挂载到 `$262` 对象上（若 `$262` 不存在则创建）。

## 7. Agent 机制

### 7.1 API

| 方法 | 签名 | 说明 |
|------|------|------|
| `$262.agent.start(script)` | `(script: string) → void` | 在新线程编译执行 script |
| `$262.agent.broadcast(sab, data)` | `(sab: SAB, data: Int32Array) → void` | 向 SAB 尾部写入消息 |
| `$262.agent.receiveBroadcast(sab)` | `(sab: SAB) → Int32Array` | 阻塞读取 SAB 尾部的消息 |
| `$262.agent.getReport()` | `() → string` | 返回 agent 报告 |
| `$262.agent.sleep(ms)` | `(ms: number) → void` | 睡眠 ms 毫秒 |
| `$262.agent.monotonicNow()` | `() → number` | 单调时钟（毫秒） |

### 7.2 broadcast/receiveBroadcast 协议

test262 agent 通信使用 SAB 尾部字节作为数据通道，具体偏移量取决于 SAB 的 `byteLength`：

- **lock 字节**: `byteLength - 1`（0 = 空闲，1 = 已占用）
- **长度字段**: `byteLength - 8` 到 `byteLength - 5`（Int32，小端序，单位：元素个数）
- **数据区域**: 从 `byteLength - 4` 开始向后写入（长度由长度字段指定）

精确约定在实现时对照 test262 `harness/agent.js` 确认。

- `broadcast(sab, data)`: 忙等待 lock==0 → 设 lock=1 → 写数据长度和数据 → 设 lock=0
- `receiveBroadcast(sab)`: 忙等待 lock==1 → 读数据长度和数据 → 设 lock=0 → 返回数据

忙等待用 `thread::yield_now()` + 超时（默认 60s）防止死循环。

### 7.3 start 实现

```rust
fn agent_start(script: String, shared_state: Arc<SharedRuntimeState>) {
    std::thread::spawn(move || {
        // 编译
        let module = wjsm_parser::parse_module(&script).unwrap();
        let program = wjsm_semantic::lower_module(module).unwrap();
        let wasm_bytes = wjsm_backend_wasm::compile(&program).unwrap();

        // 执行（捕获 stdout/stderr 到 report）
        let mut report = Vec::new();
        let result = execute_with_writer_shared(
            &wasm_bytes,
            &mut report,
            Some(shared_state.clone()),
        );

        // 存报告
        let report_str = String::from_utf8_lossy(&report).to_string();
        shared_state.agent_state.reports.lock().unwrap().push(report_str);
    });
}
```

### 7.4 线程安全分析

- `SharedRuntimeState`：全字段 `Arc<Mutex<...>>`，天然 `Send + Sync`
- `RuntimeState`：不跨线程共享。每个 agent 构建自己的 `RuntimeState`：
  - `shared_state` 字段从外层克隆 Arc
  - `scope_records` 是 agent 私有的
- wasmtime `Engine::default()`：是 `Send`，每个线程可独立创建
- GC：主线程的 GC 不 trace agent 的 object handle（agent 独立运行）

## 8. 测试

### 8.1 test262 配置

`crates/wjsm-test262/src/config.rs` 新增 feature：
```rust
"SharedArrayBuffer",
"Atomics",
"Atomics.waitAsync",
```

`$262.agent` 测试不需 feature 标记——harness 检测到 `$262.agent` 不存在时会跳过。

### 8.2 E2E Fixtures

| 文件 | 覆盖 |
|------|------|
| `fixtures/happy/sharedarraybuffer.js` | SAB 构造器、byteLength、slice、SpeciesConstructor |
| `fixtures/happy/atomics.js` | Atomics.load/store/add/sub/and/or/xor/exchange/compareExchange/isLockFree |
| `fixtures/happy/atomics_wait_notify.js` | Atomics.wait/notify 单 agent 场景 |
| `fixtures/errors/sab_detached.js` | 对 detached SAB 调用 Atomics 方法的 TypeError |
| `fixtures/errors/atomics_bad_ta.js` | 对非 Int32Array/BigInt64Array 调 wait 的 TypeError |

### 8.3 语义快照

`fixtures/semantic/sharedarraybuffer.ir` — 验证 SAB 构造器 + Atomics.store 调用生成的 IR。

### 8.4 test262 套件

- `built-ins/SharedArrayBuffer/` — ~200 测试
- `built-ins/Atomics/` — ~330 测试
- `features/SharedArrayBuffer/` — agent 集成测试
- `features/Atomics/` — agent 集成测试

预期：非 agent 测试通过率 >90%。Agent 测试需要端到端验证 agent harness。

## 9. 变更文件清单

| 文件 | 操作 | 说明 |
|------|------|------|
| `crates/wjsm-ir/src/builtin.rs` | 修改 | 新增 ~19 个 Builtin variants + Display impl |
| `crates/wjsm-semantic/src/builtins.rs` | 修改 | builtin_from_global_ident 加 SAB 映射 |
| `crates/wjsm-backend-wasm/src/compiler_core.rs` | 可能修改 | 新 Builtin 可能需加 match 臂 |
| `crates/wjsm-runtime/src/lib.rs` | 修改 | RuntimeState 加 shared_state，NativeCallable 加 variants，execute_with_writer 加参数，TypedArrayEntry 加 is_shared |
| `crates/wjsm-runtime/src/host_imports/collections_buffers.rs` | 修改 | SAB 构造器，Atomics 全局对象创建 |
| `crates/wjsm-runtime/src/host_imports/get_builtin_global_entry.rs` | 修改 | SAB + Atomics 不再走 StubGlobal |
| `crates/wjsm-runtime/src/host_imports/typedarray_new_methods.rs` | 修改 | TA 方法支持 is_shared 分派 |
| `crates/wjsm-runtime/src/host_imports/` | 新增 | `atomics.rs` — 所有 Atomics host functions |
| `crates/wjsm-runtime/src/host_imports/` | 新增 | `agent.rs` — agent harness 实现 |
| `crates/wjsm-test262/src/config.rs` | 修改 | SUPPORTED_FEATURES 加 SharedArrayBuffer/Atomics |
| `fixtures/happy/` | 新增 | 3-4 个 JS fixture |
| `fixtures/errors/` | 新增 | 2 个 JS fixture |
| `fixtures/semantic/` | 新增 | 1 个 .ir snapshot |

## 10. 风险和缓解

| 风险 | 影响 | 缓解 |
|------|------|------|
| wasmtime 多线程创建 Engine 冲突 | agent 无法启动 | wasmtime Engine::default() 已验证是 Send，每个线程独立创建安全。若遇问题，改为全局 Engine 单例 |
| Atomics 非对齐访问性能 | 符合规范但慢 | 实际 JS 代码总是对齐访问；非对齐路径仅保底 |
| Agent 线程 panic | 主线程无法感知 | catch_unwind 包裹 agent 线程，panic 转为 report 字符串 |
| TypedArray 方法分派遗漏 | SAB 上的 TA 方法走错路径 | 审计全部 `ta_read`/`ta_write` 调用点，确保 is_shared 检查全覆盖 |
| `format` 规范 | AtomFormat 枚举 (Int8/Uint8/...) 对应关系 | 严格按规范 25.4 节 `ValidateIntegerTypedArray` + `AtomicReadModifyWrite` 实现 |
