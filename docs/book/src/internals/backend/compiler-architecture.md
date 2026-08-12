# 编译器内部结构

`wjsm-backend-native` 把 verified semantic IR 降为 Cranelift IR（CLIF），再生成 relocatable native object。这一章说明编译器内部的组织方式。

## 阶段划分

```text
PortableArtifact
  -> NativeCompiler::compile
       1. verify program（CFG/Phi/ValueId/跨引用）
       2. 逐函数 IR → CLIF 降级
       3. may-GC 点 spill boxed roots，发布 NativeRootFrame
       4. object emission
       5. strict loader 校验 section/symbol/relocation/alignment
       6. 发布 RX mapping → CompiledImage
```

## IR → CLIF 降级

每个 IR `Function` 降为一个 CLIF function。降级规则：

| IR 概念 | CLIF 映射 |
| --- | --- |
| `BasicBlock` | CLIF block；Phi 变为 block parameters |
| `Instruction::Const` | `iconst` / `f64const` |
| `Instruction::Binary` / `Unary` / `Compare` | 对应 CLIF 运算指令 |
| `Instruction::CallBuiltin` | host operation call，通过 vmctx |
| `Instruction::StoreVar` / `LoadVar` | 栈槽读写 |
| `Terminator` | `jump` / `brif` / `return` / `trap` |

production code 不允许 Cranelift trap。异常、栈预算耗尽和终止走显式 return/status 协议，不依赖 trap 机制。

## may-GC 与 root frame

generated code 在 may-GC edge 发布 live boxed roots。`NativeRootFrame` 是编译器生成的 root frame 布局，runtime collector 在 safepoint 读取它来扫描活跃句柄。

root frame 的布局由 `wjsm-native-abi` 定义，`NATIVE_ABI_HASH` 覆盖这一布局。任何 root frame 格式变化都必须递增 hash，使旧 cache miss。

## object emission 与 strict loader

Cranelift 产出 relocatable object 后，strict loader 校验：

- section 范围与对齐；
- symbol 表完整；
- relocation 指向合法 symbol；
- 无未解析引用。

校验通过后发布 W^X mapping。mapping 不可写不可直接修改；function table 不缓存裸 code pointer。

## 深入了解

- [Portable artifact 边界](../../../../backend-implementation-guide.md)
- [IR Program、Module 与 Function](../ir/program-module-function.md)
- [Direct Cranelift 后端概览](README.md)
