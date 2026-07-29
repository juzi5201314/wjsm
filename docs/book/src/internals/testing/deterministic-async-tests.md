# 异步与流式测试的确定性

这一章说明如何让异步和流式测试不依赖 wall-clock 时序。

## 问题

异步测试常见问题：用 `thread::sleep` 或 `start.elapsed()` 断言时序，在负载下随机失败。例如：

- 断言「A 在 B 之前完成」用 `start.elapsed() < 50ms`。
- chunk 之间用 `thread::sleep(10ms)` 分隔。

这些测试在 CI 或高负载下不稳定——wall-clock 时序受系统调度影响。

## 确定性 channel gate

wjsm 用确定性 channel gate 替代 wall-clock 断言：

- 产者通过 channel 发送事件，消费者按顺序接收。
- 断言「A 在 B 之前」变成「A 的事件在 B 的事件之前被接收」——这由 channel 的 FIFO 保证。
- 不需要 `sleep` 或 `elapsed()` 断言。

## 适用场景

| 场景 | 应用 |
| --- | --- |
| HTTP chunked server | chunk 的顺序通过 channel 验证 |
| reader/stream ordering | 数据块的顺序通过 channel 验证 |
| 「X 在 Y 之前」断言 | 用 channel 的事件顺序替代 wall-clock |

## nextest 配置

`.config/nextest.toml` 的 3s 硬超时帮助捕获死锁——如果异步测试因为 channel 死锁挂住，3s 后被判定失败。资源隔离组（`async-hooks-load` 等）确保异步测试不相互干扰。

## 深入了解

- [Promise、微任务与异步调度器](../runtime-features/async-scheduler.md)
- [Fixture 测试框架](fixtures.md)
- [分层调试流程](debugging-workflow.md)
