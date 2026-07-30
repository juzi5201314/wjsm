# 启动快照与嵌入工件

启动快照把运行时初始化（内置对象、原型链、全局环境）的结果固化下来，让每次启动跳过这段重复工作。它默认开启，无需配置。

快照和 support 模块都在构建 `wjsm` 时嵌入到二进制里，不依赖用户机器上的缓存目录，也不存在「首次运行先生成」的冷启动阶段。

## 关闭快照

只在排查启动期行为差异时才需要关闭：

```bash
WJSM_STARTUP_SNAPSHOT=0 wjsm run app.js
```

`0`、`false`、`off` 三个值关闭快照，其它任何值（包括不设置）都保持开启。

## 观察快照装载

```bash
WJSM_STARTUP_SNAPSHOT_DEBUG=1 wjsm run app.js
```

这个变量只认 `1`、`true`、`on`。

> <details><summary>启动快照到底省了什么时间？</summary>
>
> 量化地说：冷启动 bootstrap 要把内置 JS bundle 编译一次（~500-1500ms），再执行一遍创建所有原生对象、原型链、构造器。快照恢复跳过这两步，直接从内存里把已经建好的对象图恢复回来，耗时 ~50-200ms。
>
> 看起来差异是「一次冷启动 vs 之后每次都快」，但实际影响更微妙：
>
> - **CI 流水线**：每次 CI 启动 wjsm 都是冷启动，跳过 bootstrap 累积能省下几秒到几十秒。
> - **短脚本**：「执行只花 100ms，启动花 1.5s」——关闭快照会让短脚本的执行时间变成 16 倍。生产中跑几百次短任务的场景差异会很明显。
> - **开发调试**：开发时频繁重启 watch 模式，快照让重启延迟保持稳定。
>
> 实际项目里「什么情况下关掉快照」只有一种：你在调试 bootstrap 本身、怀疑初始化过程有 bug。这种场景关掉后所有启动路径都走 cold bootstrap，行为最可预测。
>
> </details>

## 与编译缓存的区别

启动快照和编译缓存是两件事：

- 启动快照固化的是**运行时初始状态**，随二进制分发。
- 编译缓存存放的是**你的程序编译后的机器码**，落在磁盘上，由 `WJSM_CACHE_DIR` 控制，用 `wjsm cache` 查看和清理。

两者互不影响。程序启动慢先看是哪一层：反复重新编译是缓存没命中，初始化慢才和快照有关。

## 版本失配

快照带有 ABI 指纹。用不同版本的 `wjsm` 生成的 `.wasm` 与当前二进制不匹配时，运行时会拒绝加载而不是带着错误的布局继续执行。重新构建 `.wasm` 即可。

## 深入了解

- [快照可以固化哪些内容，边界在哪](../../internals/startup/startup-snapshot.md)
- [构建期嵌入工件的生成流程](../../internals/startup/embedded-artifacts.md)
- [快照二进制格式与重定位规则](../../internals/startup/snapshot-format.md)
- [ABI Hash 如何构成兼容性指纹](../../internals/startup/abi-hash.md)
- [冷启动路径与失配处理](../../internals/startup/cold-start-and-mismatch.md)
