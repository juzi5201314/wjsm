# 术语表

这一章是 wjsm 内部手册使用的术语速查表。

| 术语 | 含义 |
| --- | --- |
| AOT | Ahead-of-Time，编译在执行前完成 |
| NaN-boxing | 用 NaN 的低位编码非浮点值的 64 位表示 |
| ManagedHeap | 统一托管堆，三种 GC 回收器共用 |
| NativeHeapMemory | 生产对象堆：mmap reservation + 64 位逻辑字节偏移 |
| Handle | 对象表下标，是 JS 值引用对象的间接层 |
| Realm | JavaScript 执行上下文；宿主表，不是第二份 runtime |
| Support module | 历史上的预编译辅助模块；当前不存在。GC/分配辅助是 `NativeHostSymbol` thunk + `wjsm-gc` |
| Startup snapshot | 嵌入的 `startup_snapshot.bin`，`NativeRuntime::new_*` 始终恢复 |
| ABI hash | `native_abi_hash()`，覆盖 vmctx / frame / host symbol；进入 native cache 键 |
| ISA config | Cranelift ISA 的配置，唯一 owner 在 backend-native |
| ExecContext | 后端无关的执行上下文 trait |
| Builtin | 语义层识别的内置调用，发射 `CallBuiltin` 指令 |
| NativeCallable | `Builtin` enum 到运行时函数的映射 |
| Host call | generated code 经 `wjsm_native_host_operation` 进入宿主 |
| SAF | store-allocated fragment（ZGC 的着色指针标记） |
| Remset | remembered set，记录跨代/跨分区引用 |
| Safepoint | native mutator（JS 线程）可以安全暂停、发布 `NativeRootFrame` 的点 |
| Epoch interruption | 合作式回边预算耗尽后的 `CooperativePoll` |
| TDZ | Temporal Dead Zone，let/const 声明前的访问禁区 |
| Root frame | `NativeRootFrame`：safepoint 上的 boxed root 视图 |
| Cold bootstrap | 构建期从空堆执行 builtin JS，写出嵌入快照 |
| Warm bootstrap | 进程启动时恢复嵌入快照，跳过 builtin JS 执行 |
| Primordial | bootstrap 创建的永生对象（Object.prototype 等） |
| Immortal | GC 不回收的对象，在快照的 immortal 区 |

## 深入了解

- [用户侧的术语表](../../user/reference/glossary.md)
- [Crate 与公共 API 索引](crate-api-index.md)
- [核心不变量](invariants.md)
