# Backend 与 Runtime 定向测试

这一章说明 `wjsm-backend-native` 和 `wjsm-host-native` 的定向测试分层。

## 分层

| 层 | 测试范围 | 跑法 |
| --- | --- | --- |
| IR → CLIF | lowering 正确性、CFG/Phi 映射 | `dump-clif` + fixture |
| object emission | relocation、symbol、alignment | `validate` + `size` |
| image lifecycle | W^X、unwind、cache invalidation | `disasm` + `cache` 命令 |
| runtime execution | Promise drain、exit code、diagnostics | `run` + fixture |
| GC | 三种回收器行为一致 | `WJSM_TEST_GC` 切换跑 fixture |

## IR → CLIF 测试

`dump-clif` 输出 Cranelift IR，用于定位 native lowering 问题。诊断顺序：

```text
dump-ast → dump-ir → dump-clif → disasm
```

AST 正确而 IR 错误，问题属于 semantic lowering；IR 正确而 CLIF 错误，问题属于 native lowering；CLIF 正确而机器码/relocation 错误，再看 `disasm` 与 image loader。

## GC 回归测试

`WJSM_TEST_GC` 环境变量在测试中强制指定 GC 算法，优先级高于 `WJSM_GC`。fixture 在三种回收器下都应行为一致——mark-sweep 虽不移动对象，但代码仍遵守 `INV-C1` / `INV-C2` 不变量。

## 平台 fail-closed 测试

不支持的平台在 `NativeCompiler::new()` 时返回 `UnsupportedTargetCapability`。测试验证：不支持的宿主不切换到其他 backend，直接 fail-closed。

缺少真实平台 runner、AVX-512、大内存或 NUMA 能力时，报告 `needs-capability-runner`，不能当作通过。

## 深入了解

- [Fixture 测试框架](fixtures.md)
- [测试与验证](README.md)
- [分层调试流程](debugging-workflow.md)
- [Direct Cranelift 后端概览](../backend/README.md)
