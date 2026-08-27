# 项目定位与适用场景

## wjsm 是什么

wjsm 是一个 AOT 编译的 JavaScript/TypeScript 运行时，整条执行链不依赖 V8。源码经 SWC 解析后降为自有 verified semantic IR，`build` 把 IR 封装成跨平台携带的 portable `.wjsm` 制品，`run` 在当前宿主上把 IR 直接编译为 CLIF/native image，再由 `NativeRuntime` 执行。

关键特征：

- **编译在前，执行在后**。解析、作用域分析与 early error 在任何副作用产生之前完成，写错的程序不会先打印一半再崩溃。
- **制品与宿主解耦**。`.wjsm` 只保存 target-independent semantic IR 与 module/source metadata，不保存机器码；native image、relocation 与 cache 都是当前宿主私有派生数据。
- **无解释器、无第二执行引擎**。入口从 generic native 开始执行；热路径在类型反馈稳定后可以派生 overlay，类型 miss 则 deopt 回 generic，循环可以 OSR。没有 V8 式解释器预热，也没有 Wasm/JIT fallback。

## AOT 的价值

| 属性 | 含义 |
| --- | --- |
| 启动快 | 无解释预热，入口函数从 generic native 第一条指令开始执行 |
| 稳态可特化 | 反馈稳定后按证明类型重建热循环；miss 时 deopt 到 generic native，而不是改语义 |
| 制品可携带 | 一次 `build` 产出的 `.wjsm` 可在任意受支持宿主上 `run`，无需随包携带编译器 |
| 可验证加载 | artifact verifier、checked lowering、strict relocation、symbol allowlist 与 W^X 构成受信加载边界 |

## 适用场景

- **CLI 工具**。需要快速启动、稳定退出码、低内存占用的命令行程序。
- **嵌入式 JS 引擎**。把 `.wjsm` 作为制品分发，宿主加载后由 NativeRuntime 执行，不引入 V8 体积。
- **批量测试**。`wjsm test` 逐文件编译执行，启动开销低，适合 CI 里跑大量小用例。
- **CI 流水线**。portable 制品可跨构建节点复用；设置 `WJSM_CACHE_DIR` 后 native cache 能加速重复执行，`--time` / `--stats` 给出各阶段耗时。
- **运行时开发与验证**。参与 portable AOT / native runtime 方案验证，跑已覆盖的 fixtures 与 Test262 子集。

## 不适用场景

| 场景 | 原因 |
| --- | --- |
| 完整 Node.js API 兼容 | 内置 24 个模块为自有 JS 实现的子集，不是 Node.js 移植，大量 npm 包无法直接运行 |
| 与 V8 比拼一切微架构 | overlay/deopt/OSR 覆盖已证明的 Number/Int32 标量热区，不是完整 Maglev/TurboFan |
| 类型检查 | TypeScript 语法参与解析与 lowering 后擦除，类型不做检查——这是 `tsc` 的职责 |
| 运行不受信任代码 | Direct native code 不提供进程内 sandbox，必须用独立 OS process 与权限隔离 |

## 与同类运行时的区别

一句话：Node.js / Deno / Bun 都在 V8 或 JavaScriptCore 之上做 JIT 运行时与生态兼容，wjsm 走的是 AOT + portable 制品 + Direct Cranelift native backend 路线，不追求生态完整性，换取启动速度与制品可携带性。

## 深入了解

- [面向使用者的架构概览](architecture.md)
- [端到端架构与 crate 边界](../../internals/foundations/architecture.md)
