# 输出与运行环境

这一部分说明 wjsm 产出什么、写到哪里、以什么状态退出。

- [WASM 产物与宿主要求](wasm-artifacts.md)：`build` 生成的模块依赖什么才能运行。
- [标准输出、标准错误与退出码](process-io.md)：哪些内容进 stdout，哪些进 stderr，退出码含义。
- [诊断信息与流水线阶段](diagnostics.md)：`--verbose`、`--time`、`--stats` 各自输出什么。
- [文件系统权限与资源边界](security-and-resources.md)：默认允许访问什么，如何放宽。
