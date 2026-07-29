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
- [ ] codegen 实现了新指令的 WASM 生成。

## 后端改动

- [ ] `wjsm-backend-wasm` 的 codegen 改动有 `dump-wat` / `disasm` 验证。
- [ ] ABI 常量在 `wjsm-ir` 定义，不在后端硬编码。
- [ ] support module 改动后 build.rs 重新生成 cwasm。

## Host / 运行时改动

- [ ] host import 函数是薄包装，语义逻辑在 `wjsm-builtins`。
- [ ] wasmtime 依赖只在 `wjsm-host-wasm`。
- [ ] 新的 env global 在 `WasmEnv` 和 `extract_wasm_env` 更新。

## GC 改动

- [ ] INV-C1 和 INV-C2 不变量保持。
- [ ] 三种回收器（如果改动是通用的）都测试。
- [ ] `GcAlgorithmKind` 注册表更新（如果新增算法）。
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
