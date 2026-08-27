# 跨层变更检查清单

这一章提供一个检查清单，确保跨层改动不遗漏。

## 通用检查

- [ ] 改动在 owning 层完成，旧路径已删除。
- [ ] 不引入兼容层、shim、deprecated 路径。
- [ ] 不引入编译器 warning。
- [ ] 源码注释中文，文件 ≤500 行，函数 ≤30 行。
- [ ] 生成产物放 `/tmp`，不在仓库创建临时文件。

## 语义层改动

- [ ] `wjsm-semantic` 的 lowering 改动有 IR 快照测试。
- [ ] `WJSM_UPDATE_SNAPSHOTS=1` 更新快照，审查了 diff。
- [ ] 早期错误约束有 `fixtures/errors/` 测试。

## IR 改动

- [ ] 新指令有 `Display` 实现，输出格式稳定。
- [ ] IR 验证 pass 添加了类型规则。
- [ ] codegen 实现了新指令的 CLIF 生成。

## 后端改动

- [ ] `wjsm-backend-native` 的 codegen 改动有 `dump-clif` / `disasm` 验证。
- [ ] ABI 常量在 `wjsm-ir` 定义，不在后端硬编码。
- [ ] native image 改动后重新编译验证。

## Host / 运行时改动

- [ ] host import 函数是薄包装，语义逻辑在 `wjsm-builtins`。
- [ ] Cranelift 依赖只在 `wjsm-backend-native` 和 `wjsm-host-native`。
- [ ] 新的 vmctx 布局在 `wjsm-native-abi` 和 `wjsm-host-native` 更新。

## GC 改动

- [ ] INV-C1 和 INV-C2 不变量保持。
- [ ] GC benchmark 跑过，没有性能回归。

## 快照改动

- [ ] `SNAPSHOT_FORMAT_VERSION` 递增（wire 改动时）。
- [ ] `build.rs` 重新生成嵌入工件。
- [ ] ABI 哈希计算更新（如果 ABI 输入变化）。
- [ ] `startup_snapshot.rs` 和 `embedded_startup_snapshot.rs` 测试通过。

## 测试

- [ ] 窄测试先跑（`cargo nextest run -E 'test(...)'`）。
- [ ] 全 workspace 测试通过（`cargo nextest run --workspace`）。
- [ ] fixture 变更是预期的，不是避开正确逻辑。
- [ ] test262 如果受影响，已跑过。

## 深入了解

- [开发工作流与代码约定](workflow-and-conventions.md)
- [分层调试流程](../testing/debugging-workflow.md)
- [核心不变量](../reference/invariants.md)
