# 项目定位与适用场景

## wjsm 是什么

wjsm 是一个 JavaScript/TypeScript 运行时，整条执行链不依赖 V8。源码经 SWC 解析后降为自有 verified semantic IR，`build` 把 IR 封装成跨平台携带的 portable `.wjsm` 制品，`run` 在当前宿主上把 IR 直接编译为 CLIF/native image，再由 `NativeRuntime` 执行。

文档里的「AOT」只描述**编译时机**：静态可见的模块在第一次执行前编成 **generic native**。它不表示语言变成静态的，也不表示运行时不再编译、峰值一定快过 V8。

关键特征：

- **静态模块先编后跑**。解析、作用域分析与 early error 在任何用户副作用产生之前完成，写错的程序不会先打印一半再崩溃。入口从 generic native 开始，没有字节码解释器当主路径。
- **制品与宿主解耦**。`.wjsm` 只保存 target-independent semantic IR 与 module/source metadata，不保存机器码；native image、relocation 与 cache 都是当前宿主私有派生数据。
- **运行时仍是动态 JS**。对象模型、GC、Shape/IC 都在。`eval`、动态加载在运行时走同一条 `NativeCompiler`。热路径在类型反馈稳定后可以派生 overlay，类型 miss 则 deopt 回 generic，循环可以 OSR——这是同一 native owner 上的运行时特化，按编译器术语属于 JIT，不是第二套 VM。

## generic native 提供什么、不提供什么

| 属性 | 含义 |
| --- | --- |
| 入口无解释器 | 静态模块从 generic native 第一条指令开始执行，没有 V8 Ignition 那种解释预热 |
| 语义仍动态 | generic native 把装箱、类型检查、属性访问和异常路径编进同一份代码 |
| 稳态可特化 | 反馈稳定后按证明类型重建热循环；miss 时 deopt 到 generic native，而不是改语义 |
| 制品可携带 | 一次 `build` 产出的 `.wjsm` 可在任意受支持宿主上 `run`；机器码由该宿主再编 |
| 可验证加载 | artifact verifier、checked lowering、strict relocation、symbol allowlist 与 W^X 构成受信加载边界 |
| 不是闭世界 AOT | 不是 Graal Native Image / .NET Native AOT 那种静态子集；也不是「编完就不再编译」 |
| 不是 V8 替代峰值 | overlay 覆盖已证明的 Number/Int32 标量热区，不是完整 Maglev/TurboFan；AOT 本身不保证更快 |

## 适用场景

- **CLI 工具**。需要可预测的启动路径、稳定退出码、低内存占用的命令行程序。磁盘缓存命中后可跳过 parse/lower 与 Cranelift。
- **嵌入式 JS 引擎**。把 `.wjsm` 作为 IR 制品分发，宿主加载后由 NativeRuntime 执行，不引入 V8 体积。
- **批量测试**。`wjsm test` 逐文件编译执行，适合 CI 里跑已覆盖的小用例。
- **CI 流水线**。portable 制品可跨构建节点复用；磁盘缓存默认可用（`WJSM_CACHE_DIR` 可覆盖目录），重复执行跳过 parse/lower 与 native 编译，`--time` / `--stats` 给出各阶段耗时。
- **运行时开发与验证**。参与 portable IR / native runtime 方案验证，跑已覆盖的 fixtures 与 Test262 子集。

## 不适用场景

| 场景 | 原因 |
| --- | --- |
| 完整 Node.js API 兼容 | 内置 24 个模块为自有 JS 实现的子集，不是 Node.js 移植，大量 npm 包无法直接运行 |
| 与 V8 比拼一切微架构 | overlay/deopt/OSR 覆盖已证明的 Number/Int32 标量热区，不是完整 Maglev/TurboFan |
| 类型检查 | TypeScript 语法参与解析与 lowering 后擦除，类型不做检查——这是 `tsc` 的职责 |
| 运行不受信任代码 | Direct native code 不提供进程内 sandbox，必须用独立 OS process 与权限隔离 |
| 期望「编成 native 就变成静态语言」 | 运行时对象模型、GC、动态代码和 overlay 都还在 |

## 与同类运行时的区别

Node.js / Deno / Bun 在 V8 或 JavaScriptCore 之上做解释器预热加多层 JIT，并追求生态兼容。wjsm 用 portable IR 制品加 Direct Cranelift：静态模块先编成 generic native，没有第二套执行引擎，但语言仍是动态的，热路径也可以再特化。项目不追求生态完整性；启动快慢取决于是否命中 native cache、以及 generic/overlay 相对 V8 各层的实际代码质量，不要从「AOT」二字推断更快。

## 深入了解

- [面向使用者的架构概览](architecture.md)
- [端到端架构与 crate 边界](../../internals/foundations/architecture.md)
