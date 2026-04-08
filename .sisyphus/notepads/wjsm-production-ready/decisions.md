# wjsm-production-ready Decisions

## Phase 1 Task 1.2

- 选择用子进程执行已构建的 `wjsm run <fixture>`，而不是在测试中直接链接编译器/运行时内部模块；这样保持当前二进制 crate 结构不变，避免为测试基础设施改动生产代码布局。
- fixture snapshot 统一记录三部分：`exit_code`、`stdout`、`stderr`，并使用 `*.expected` 与源码同目录存放，降低 fixture 搬运和审查成本。
