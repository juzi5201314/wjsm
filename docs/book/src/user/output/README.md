# 输出与运行环境

这一部分说明 wjsm 产出什么、写到哪里、以什么状态退出。

- [Portable `.wjsm` 制品](portable-artifacts.md)：`build` 默认写出的跨平台 semantic-IR 容器。
- [制品与宿主要求](wasm-artifacts.md)：哪些命令需要受支持的宿主，以及同宿主 `native-executable` 的约束。
- [标准输出、标准错误与退出码](process-io.md)：哪些内容进 stdout，哪些进 stderr，退出码含义。
- [诊断信息与流水线阶段](diagnostics.md)：`--verbose`、`--time`、`--stats` 各自输出什么。
- [安全与资源边界](security-and-resources.md)：默认允许访问什么，如何放宽。
