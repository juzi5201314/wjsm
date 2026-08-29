# 用户手册

wjsm 是不依赖 V8 的 JavaScript/TypeScript runtime。源码先被解析并降为 verified semantic IR；`build` 生成跨支持平台携带的 portable `.wjsm`（IR，不是机器码），`run` 在当前宿主把 IR 直接编译为 generic native image，再由 `NativeRuntime` 执行。语言语义仍是动态的：`eval`、动态加载和热路径 overlay 会在运行时再次编译。

当前版本是 `0.1.0`，ECMAScript、Web API 与 Node.js 兼容层仍是子集。真实支持范围以仓库 fixture、Test262 和命令行 `--help` 为准。

## 从这里开始

- 想运行代码：[`run`](cli/run.md)。
- 想构建可携带制品：[`build`](cli/build.md) 与 [Portable `.wjsm` 制品](output/portable-artifacts.md)。
- 想定位阶段错误：`dump-ast` → `dump-ir` → `dump-clif` → `disasm`。
- 想嵌入 Rust 进程： [作为 Rust 库嵌入](workflows/embedding.md)。
- 想确认隔离承诺： [安全与资源边界](output/security-and-resources.md)。

命令示例统一写成 `wjsm ...`。未安装到 `PATH` 时，可替换为 `cargo run -- ...` 或 `target/debug/wjsm ...`。
