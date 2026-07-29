# 术语表

这一章是 wjsm 内部手册使用的术语速查表。

| 术语 | 含义 |
| --- | --- |
| AOT | Ahead-of-Time，编译在执行前完成 |
| NaN-boxing | 用 NaN 的低位编码非浮点值的 64 位表示 |
| ManagedHeap | 统一托管堆，三种 GC 回收器共用 |
| Handle | 对象表下标，`u32`，是 JS 值引用对象的间接层 |
| Realm | JavaScript 执行上下文，有独立的全局对象和 intrinsics |
| Support module | 预编译的 WASM 模块，提供 GC 和分配辅助函数 |
| Startup snapshot | bootstrap 后的堆状态快照，加速启动 |
| ABI hash | 判断快照与当前 engine 兼容性的哈希 |
| EngineConfig | wasmtime Engine 的配置，唯一 owner 在 host-wasm |
| ExecContext | 后端无关的执行上下文 trait |
| Builtin | 语义层识别的内置调用，发射 `CallBuiltin` 指令 |
| NativeCallable | `Builtin` enum 到运行时函数的映射 |
| Host import | user wasm 调用宿主函数的通道，通过 `env.*` |
| cwasm | 预编译的 WASM（wasmtime 的编译产物） |
| SAF | store-allocated fragment（ZGC 的着色指针标记） |
| Remset | remembered set，记录跨代/跨分区引用 |
| Safepoint | WASM 执行可以安全暂停的点 |
| Epoch interruption | wasmtime 的中断机制，通过 `increment_epoch` 触发 |
| Two-phase lowering | 预声明 pass + lower pass，保证 TDZ 和 hoisting |
| TDZ | Temporal Dead Zone，let/const 声明前的访问禁区 |
| Shadow stack | 影子栈，GC safepoint spill 的值存储 |
| Cold bootstrap | 从空堆执行 builtin JS，构造 primordial 对象 |
| Warm bootstrap | 快照恢复，跳过 builtin JS 执行 |
| Primordial | bootstrap 创建的永生对象（Object.prototype 等） |
| Immortal | GC 不回收的对象，在快照的 immortal 区 |

## 深入了解

- [用户侧的术语表](../../user/reference/glossary.md)
- [Crate 与公共 API 索引](crate-api-index.md)
- [核心不变量](invariants.md)
