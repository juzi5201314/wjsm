# 开发工作流与代码约定

这一章说明 wjsm 的开发约定和代码风格。

## 基本约定

- Rust 2024 edition，default rustfmt。
- 源码注释中文。
- 零编译器 warning——新代码不能引入 warning。
- 文件职责聚焦，目标 ≤500 行；函数内聚，目标 ≤30 行。
- 按语义/后端/宿主域拆分文件，不引入平行约定。

## 工作流

1. **诊断阶段**：parse → lower → module graph → codegen → host/runtime，用 `dump-ast`、`dump-ir`、`dump-clif`、`disasm` 比较相邻阶段。
2. **修改**：在 owning 层修改，删除旧路径，不保留兼容层。
3. **测试**：lowering 改动跑 IR 快照；行为改动用 fixture；模块行为用 `fixtures/modules`。
4. **审查**：审查 fixture/snapshot 变更，不要通过修改测试来避开正确逻辑。

## 命令

```bash
cargo build
cargo run -- run -e 'console.log(1 + 2)'
cargo run -- build -e 'console.log(1)' -o /tmp/out.wjsm
cargo nextest run --workspace
WJSM_UPDATE_FIXTURES=1 cargo nextest run -E 'test(happy__<name>)'
WJSM_UPDATE_SNAPSHOTS=1 cargo nextest run -p wjsm-semantic -- lowering_snapshots
```

## 临时文件

生成产物放 `/tmp`，不要在仓库内创建临时文件。ad-hoc JS/TS 用 `-e`，不创建临时源码文件。

## 错误处理

遇到非预期错误，即使不是你导致的，如果影响任务就修复——不遗留后续问题。不通过削弱 fixture 或 snapshot 来隐藏失败。

## 深入了解

- [新增语言功能](adding-language-features.md)
- [跨层变更检查清单](cross-layer-checklist.md)
- [用户侧的命令](../../user/cli/README.md)
