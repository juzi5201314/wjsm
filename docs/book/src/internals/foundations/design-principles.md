# 设计原则与规范来源

这一章说明改动 wjsm 时依据什么判断对错，以及查证顺序。

## 判断顺序

遇到语义问题按这个顺序查，不要跳步：

1. **当前源码**。owner 层的实现是行为的事实，注释和文档都可能过期。
2. **测试证据**。`fixtures/happy`、`fixtures/errors`、`fixtures/modules`、`fixtures/semantic` 与 crate 测试锁定的是已验证行为。
3. **生效的 ADR**。`docs/adr/` 记录架构决策，被取代的部分只用于理解历史（如 ADR 0005 已被 0010 取代，0011/0013 已被 0014 取代）。
4. **官方规范**。ECMAScript、WHATWG、Cranelift 文档。

## 核心原则

**单一 owner。** 每个事实只有一个权威定义点。GC 算法解析在 `NativeRuntimeConfig::from_environment`，root 帧布局在 `wjsm-native-abi::NativeRootFrame`，缓存目录由调用方传入的 `cache_dir` / `WJSM_CACHE_DIR` 决定（默认关闭）。其他位置引用，不复制。

**后端边界不可越。** Cranelift 依赖只允许出现在 `wjsm-backend-native` 和 `wjsm-host-native`。`wjsm-builtins`、`wjsm-host`、`wjsm-gc`、`wjsm-module` 保持后端无关（ADR 0014）。

**泛型单态化而非 dyn。** `ExecContext` 的实现通过 `<E: ExecContext>` 泛型传播，编译期内联，无 vtable 开销。`HeapAccessV2<M>` 同理。

**在 owner 层修复。** 症状出现在下游时不打补丁。不通过放宽 fixture、削弱快照或加特例来掩盖失败。

**彻底切换。** 替换实现时迁移所有调用方并删除旧路径。不保留兜底分支和「暂时保留」的兼容层。

> <details><summary>「彻底切换」为什么是原则，不是规则？</summary>
>
> 「保留兼容层」听起来更稳妥——万一新实现有 bug，可以回退。但实践里它造成的伤害远大于收益：
>
> - **双倍维护成本**：旧路径和新路径都要修 bug、改 feature。半年后没人记得「为什么这里有两个实现」。
> - **接口僵化**：为了兼容旧路径，公共 API 不能演进。久而久之整套 API 都受「被废弃实现」约束。
> - **测试盲区**：新旧路径只有一条被充分测试，另一条逐渐腐烂。bug 复现时不知道是哪条出的问题。
>
> 「彻底切换」的做法是：迁移期间用 `git log` 保留旧实现（可回滚），但代码里不保留。新实现稳定后，旧代码可以 git 抛弃。
>
> wjsm 的几个例子：ManagedHeap 取代 memory32 对象堆时直接删了旧堆；JavaScript Builtins 拆出后语义层不再持有后端专有状态；Direct Cranelift 切换时删除整个 Wasmtime/Wasm 生产路径。这些决定都让代码更简单，代价是回滚成本稍高——可接受。
>
> </details>

## 代码约定

- Rust 2024，默认 rustfmt，零编译警告。
- 源码注释用中文，API/类型/函数名保留原文。
- 文件按职责收敛（目标 ≤500 行），函数保持内聚（目标 ≤30 行）。拆分按语义/后端/宿主域切，不新增平行约定。
- 生成物写 `/tmp`。临时 JS/TS 用 `-e`，不建临时源文件。

## 深入了解

- [ADR 导航与决策状态](../reference/adr-index.md)
- [核心不变量清单](../reference/invariants.md)
- [开发工作流与提交前检查](../development/workflow-and-conventions.md)
