# ADR 0022: 投机 typed 区、deopt 与 OSR

## Status

Accepted（2026-08-27）

Amends ADR 0014 中「overlay 只做入口 tag 守卫、guard miss 不回滚、无 deopt/OSR」的运行时特化合同。不改变 ADR 0014 的唯一生产编译链、portable `.wjsm` 边界，以及禁止解释器 / Wasm / 第二执行后端的约束。

## Context

AOT generic native 把动态 JS 语义编进同一份 IR。热循环在 SROA / mem2reg 之后仍保留语句级 `is_exception` 菱形与 `AbstractCompare` 装箱仪式，因为 IR 长期不携带值类。运行时 overlay（Issue #390）只给参数 Number 种子并在 CLIF 里走 `fadd`，不重建 CFG。

V8 在反馈稳定后按证明类型**重建**热循环，类型 miss 则 deopt；循环可 OSR。wjsm 没有解释器，不能把 deopt 目标设成 bytecode。需要在「仍只有 Direct Cranelift native」的前提下给出与 V8 同构的控制，而不是再引入 JIT fallback 执行语义。

## Decision

### 1. 值类与 CFG 重建的归属

Number / Int32 值类、可抛性与「证明 Number 的关系比较 → `CompareOp`」是 `wjsm-ir` 的责任。`wjsm-semantic` 与 `wjsm-backend-native` overlay 编译调用同一组函数。`wjsm-backend-native` 不得依赖 `wjsm-semantic`。

portable `Program` 在 AOT 管线末尾已经过静态证明折叠。overlay 编译**克隆**目标函数所属 `Program`，用参数 tag 与稳定 binary 反馈作种子再跑同一套折叠，不修改已验证的分发 IR。

### 2. 允许 generic ↔ overlay 的显式 deopt / OSR

- 仍禁止解释器、Wasm、`cranelift-jit` 作为执行语义。
- 允许同一 verified `Program` 上的第二份 native（overlay）。
- Deopt 目标是 **generic native** 的循环头 landing pad，粒度是 IR `(function_id, block_id)` 加上有序 live boxed `i64`。
- OSR：generic 循环头发现 overlay 已发布时，把 live 写入 resume 槽并进入 overlay 函数体的同一 block。
- Guard 必须出现在可能产生错误副作用的指令之前。第一版在循环头检查投机 Number/Int32 φ；已证明的纯 `fadd`/`iadd` 不再插 `is_exception`。中段 deopt 先跳到最近循环头（可能重做当前迭代的纯计算）。
- 投机 Int32 下标仅在 overlay 中使用，溢出走 deopt；generic AOT 不得静默截断。

### 3. 产品叙述

「无 JIT」指：没有解释器预热，也没有把执行权交给第二编译器/VM。稳定反馈后的 overlay 编译、deopt、OSR 是同一 native owner 上的派生优化，可能引入短暂编译与控制转移，不再承诺「完全没有 deopt 尖刺」。

## Consequences

- `NATIVE_ABI_VERSION` 增加 resume 槽、`NativeFunctionEntry.osr_entry` 与 `DeoptToGeneric`。
- `CompareOp` 扩展关系比较，semantic ABI hash 随 wire 变化。
- 用户文档与 `docs/backend-implementation-guide.md` 必须与本 ADR 一致。

## Verification

- 证明 Number 的标量循环：IR 循环体无 `is_exception`、无 `abstract_compare`。
- `7n + 1` 仍 TypeError。
- overlay 诊断含重建后的 `fcmp`/`fadd`；类型 miss 后可观察结果与 generic 一致。
- 默认测试保持确定性、进程内、无真实网络/子进程。

## References

- ADR 0014 — Direct Cranelift 与 portable `.wjsm` 终态
- `docs/backend-implementation-guide.md`
- `crates/wjsm-ir/src/value_class.rs`、`typed_cfg.rs`
