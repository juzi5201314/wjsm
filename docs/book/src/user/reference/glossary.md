# 术语表

用户手册中出现的术语速查表。用最短的话解释，不展开实现细节。

| 术语 | 含义 |
| --- | --- |
| **wjsm** | 不依赖 V8 的 AOT JavaScript/TypeScript 运行时。源码先编译为 verified semantic IR，再由 Cranelift 生成当前宿主的机器码执行。 |
| **AOT** | Ahead-of-Time，编译在执行前完成。与 JIT（运行时即时编译）相对。wjsm 在执行前完成全部编译，运行时不做解释或即时编译。 |
| **NaN-boxing** | 用 IEEE 754 浮点 NaN 的低位编码非浮点值的 64 位表示。wjsm 用固定宽度 64 位值表示所有 JS 值，指针、整数和 double 打包在同一个位宽里。 |
| **ManagedHeap** | wjsm 的统一托管堆。对象、数组、字符串、Promise 等都存在这里，由 GC 自动回收。每个 agent 拥有独立 heap，不跨 agent 共享。 |
| **Handle** | JS 值引用对象的间接层，本质是对象表的下标（`u32`）。代码持有 handle 而非 raw pointer，GC 移动对象后 handle 仍然有效。 |
| **Realm** | JavaScript 执行上下文，拥有独立的全局对象和内置对象。`node:vm` 创建的每个上下文是一个新 Realm。 |
| **.wjsm artifact** | `wjsm build` 生成的 portable 制品。包含 verified semantic IR 和 module manifest，不含机器码。同一 `.wjsm` 可在支持平台间携带，运行时再编译为当前宿主的 native image。 |
| **Native cache** | 当前宿主从 IR 编译出的机器码缓存。cache key 绑定 artifact digest、ABI、target、Cranelift 版本等。cache miss 时重新编译，是可重建的派生数据。 |
| **CLIF** | Cranelift IR 的文本格式。`wjsm dump-clif` 输出 CLIF，用于定位 native codegen 问题。 |
| **TDZ** | Temporal Dead Zone，`let` / `const` 声明前的访问禁区。wjsm 混合判定：同函数内前向引用在编译期拒绝，跨函数前向引用在运行时抛 ReferenceError。 |
| **Root frame** | `NativeRootFrame`。GC safepoint 时 generated code 发布的活跃句柄视图；collector 只扫描 bitmap 置位的槽。 |
| **Safepoint** | 程序执行中可以安全暂停做 GC 的点。wjsm 在编译时插入 safepoint 检查，通过 side-channel 触发中断。 |
| **Startup snapshot** | bootstrap 后的堆状态快照，加速启动。恢复快照跳过 builtin JS 执行，直接从已构造好的 primordial 对象开始。 |
| **Bootstrap** | 构造 primordial 对象的过程。Cold bootstrap 从空堆执行 builtin JS；warm bootstrap 从快照恢复。 |
| **Primordial** | bootstrap 创建的永生对象，如 `Object.prototype`、`Array.prototype`。它们不会被 GC 回收。 |
| **Semantic IR** | 源码经解析和 lowering 后的中间表示，是 `.wjsm` 制品中保存的内容。与目标平台无关。 |
| **Artifact verifier** | 加载 `.wjsm` 时的完整性检查层，验证容器格式、哈希、manifest、IR 不变量等。 |
| **native-executable** | `wjsm build --format native-executable` 产出的同宿主 ELF/PE。预链 `wjsm-exec` stub 加上 `.wjsm`、预编译 object 与制品内源码快照，不能跨平台携带。 |

## 深入了解

- [内部手册术语表（开发者向）](../../internals/reference/glossary.md)
- [架构与执行模型](../overview/architecture.md)
