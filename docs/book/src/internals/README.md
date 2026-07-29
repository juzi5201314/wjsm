# 内部手册

面向 wjsm 开发者：解释每一层由谁拥有、数据如何流动、哪些不变量不能破坏。

阅读顺序建议：

1. [目标与架构基础](foundations/index.html)：项目边界、crate 地图、依赖方向。
2. [编译与执行流水线](pipeline/index.html)：从源码到执行的完整链路。
3. 之后按需进入前端、IR、模块、后端、Host/Runtime、GC、启动等分区。
4. 动手改代码前先读 [开发与扩展](development/index.html) 对应主题，再看 [内部参考](reference/index.html) 的 owner 与不变量清单。

用法层面的问题（命令、配置、环境变量、故障排查）在[用户手册](../user/index.html)，本手册不重复。
