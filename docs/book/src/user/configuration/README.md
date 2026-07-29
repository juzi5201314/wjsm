# 配置

wjsm 的行为由三类输入决定：命令行选项、项目配置文件、环境变量。本部分先说明三者的合并规则，再按主题列出具体可调项。

- [配置来源与优先级](sources-and-precedence.md)：谁覆盖谁。
- [`wjsm.toml` 与 `wjsm.json`](project-files.md)：项目级默认值，以及哪些选项不能写进文件。
- [命令行配置项](cli-options.md)：全局选项的完整取值。
- [环境变量](environment-variables.md)：面向使用者的环境变量。
- [垃圾回收器](gc.md)、[堆、影子栈与内存预留](memory.md)：内存相关调参。
- [启动快照与嵌入工件](startup-snapshot.md)、[Inspector 调试](inspector.md)、[模块解析条件](module-resolution.md)：其余子系统开关。
