# 认识 wjsm

这一部分回答四个问题：wjsm 用来做什么、它的组成、代码从源文件到输出经过了什么、以及哪些东西现在还不能用。

四章的顺序是递进的：

1. [项目定位与适用场景](purpose-and-use-cases.md) — 它解决什么问题，什么时候不该选它。
2. [面向使用者的架构概览](architecture.md) — 你会在报错和诊断里看到的那些名字分别是什么。
3. [执行模型](execution-model.md) — 一次 `wjsm run` 实际发生了什么。
4. [兼容性与支持范围](compatibility.md) — 支持程度怎么判断，以及判断依据在哪。

看完这四章，你能读懂手册后面出现的 IR、support module、ManagedHeap、启动快照这些词。
